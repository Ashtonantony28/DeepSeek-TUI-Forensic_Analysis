//! Anthropic Messages API adapter (streaming SSE).
//!
//! Implements the bare minimum needed by Phase 2 smoke tests:
//!   - server-sent events streaming of `text_delta`, `thinking_delta`,
//!     `tool_use` blocks
//!   - `list_models` returns a known-good static list (the public API
//!     does not expose model listing)

use crate::{
    ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError, StreamEvent, ToolSchema, Usage,
};
use agent_tui_protocol::{ContentBlock, Message, ModelInfo, Provider, Role, ToolCallId};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const API_VERSION: &str = "2023-06-01";

pub struct AnthropicClient {
    http: reqwest::Client,
    api_key: Option<String>,
    base_url: String,
}

impl AnthropicClient {
    pub fn new(cfg: ClientConfig) -> Self {
        let base_url = cfg.base_url.unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            http: reqwest::Client::builder()
                .user_agent("agent-tui/0.1")
                .build()
                .unwrap_or_default(),
            api_key: cfg.api_key,
            base_url,
        }
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let api_key = self
            .api_key
            .clone()
            .ok_or_else(|| LlmError::MissingApiKey("anthropic".into()))?;

        let body = build_request_body(&req)?;
        let url = format!("{}/v1/messages", self.base_url);

        let resp = self
            .http
            .post(&url)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider { status, body });
        }

        let byte_stream = resp.bytes_stream();
        let tool_id_by_index: Arc<Mutex<std::collections::HashMap<u32, ToolCallId>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let tool_id_for_closure = tool_id_by_index.clone();

        let mapped = byte_stream
            .map(|chunk_res| chunk_res.map_err(LlmError::Http))
            .scan(Vec::<u8>::new(), move |buf, chunk_res| {
                let tool_id_by_index = tool_id_for_closure.clone();
                let res = chunk_res.map(|bytes| {
                    buf.extend_from_slice(&bytes);
                    let mut out: Vec<Result<StreamEvent, LlmError>> = Vec::new();
                    // Drain complete SSE events (separated by \n\n).
                    while let Some(pos) = find_double_newline(buf) {
                        let raw = buf.drain(..pos + 2).collect::<Vec<u8>>();
                        let s = String::from_utf8_lossy(&raw);
                        for line in s.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    continue;
                                }
                                if let Ok(val) = serde_json::from_str::<Value>(data) {
                                    out.extend(translate_event(val, &tool_id_by_index));
                                }
                            }
                        }
                    }
                    out
                });
                std::future::ready(Some(res))
            })
            .flat_map(|chunk_res| match chunk_res {
                Ok(events) => futures::stream::iter(events).boxed(),
                Err(e) => futures::stream::iter(vec![Err(e)]).boxed(),
            });

        Ok(Box::pin(mapped))
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, LlmError> {
        Ok(vec![
            ModelInfo {
                provider: Provider::Anthropic,
                name: "claude-opus-4-7".into(),
                context_window: 1_000_000,
                max_output_tokens: 8192,
                supports_thinking: true,
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                provider: Provider::Anthropic,
                name: "claude-sonnet-4-6".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_thinking: true,
                supports_tools: true,
                supports_vision: true,
            },
            ModelInfo {
                provider: Provider::Anthropic,
                name: "claude-haiku-4-5-20251001".into(),
                context_window: 200_000,
                max_output_tokens: 8192,
                supports_thinking: false,
                supports_tools: true,
                supports_vision: true,
            },
        ])
    }

    fn provider(&self) -> Provider {
        Provider::Anthropic
    }
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

#[derive(Serialize)]
struct AnthropicMsg<'a> {
    role: &'a str,
    content: Value,
}

fn build_request_body(req: &ChatRequest) -> Result<Value, LlmError> {
    let mut msgs: Vec<AnthropicMsg> = Vec::new();
    let mut system_text: Option<String> = req.system.clone();

    for m in &req.messages {
        if m.role == Role::System {
            // Anthropic puts system as a top-level field; concatenate if present.
            for block in &m.content {
                if let ContentBlock::Text { text } = block {
                    system_text = Some(match system_text {
                        Some(s) => format!("{s}\n\n{text}"),
                        None => text.clone(),
                    });
                }
            }
            continue;
        }
        let role = match m.role {
            Role::User | Role::Tool => "user",
            Role::Assistant => "assistant",
            Role::System => unreachable!(),
        };
        msgs.push(AnthropicMsg {
            role,
            content: serialize_content(m)?,
        });
    }

    let mut body = json!({
        "model": req.model,
        "messages": msgs,
        "stream": true,
        "max_tokens": req.max_output_tokens.unwrap_or(4096),
    });

    if let Some(s) = system_text {
        body["system"] = Value::String(s);
    }
    if let Some(t) = req.temperature {
        body["temperature"] = json!(t);
    }
    if !req.tools.is_empty() {
        body["tools"] = serde_json::to_value(
            req.tools
                .iter()
                .map(|t: &ToolSchema| {
                    json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    Ok(body)
}

fn serialize_content(m: &Message) -> Result<Value, LlmError> {
    let mut out = Vec::new();
    for block in &m.content {
        match block {
            ContentBlock::Text { text } => out.push(json!({"type":"text","text":text})),
            ContentBlock::Thinking { text } => {
                out.push(json!({"type":"thinking","thinking":text}))
            }
            ContentBlock::ToolUse { id, name, input } => out.push(json!({
                "type":"tool_use","id":id.0,"name":name,"input":input
            })),
            ContentBlock::ToolResult { tool_use_id, content, is_error } => out.push(json!({
                "type":"tool_result",
                "tool_use_id": tool_use_id.0,
                "content": content,
                "is_error": is_error,
            })),
            ContentBlock::Image { media_type, data } => out.push(json!({
                "type":"image","source":{"type":"base64","media_type":media_type,"data":data}
            })),
        }
    }
    Ok(Value::Array(out))
}

#[derive(Deserialize)]
struct ContentBlockMeta {
    #[serde(rename = "type")]
    kind: String,
    id: Option<String>,
    name: Option<String>,
}

fn translate_event(
    ev: Value,
    tool_id_by_index: &Arc<Mutex<std::collections::HashMap<u32, ToolCallId>>>,
) -> Vec<Result<StreamEvent, LlmError>> {
    let event_type = ev.get("type").and_then(Value::as_str).unwrap_or("");
    match event_type {
        "content_block_start" => {
            let index = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let block = ev.get("content_block");
            if let Some(b) = block {
                if let Ok(meta) = serde_json::from_value::<ContentBlockMeta>(b.clone()) {
                    if meta.kind == "tool_use" {
                        let id = ToolCallId(meta.id.unwrap_or_else(|| ToolCallId::new().0));
                        tool_id_by_index
                            .lock()
                            .unwrap()
                            .insert(index, id.clone());
                        return vec![Ok(StreamEvent::ToolCallStart {
                            id,
                            name: meta.name.unwrap_or_default(),
                        })];
                    }
                }
            }
            vec![]
        }
        "content_block_delta" => {
            let index = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let delta = ev.get("delta").cloned().unwrap_or(Value::Null);
            let dtype = delta.get("type").and_then(Value::as_str).unwrap_or("");
            match dtype {
                "text_delta" => {
                    let t = delta.get("text").and_then(Value::as_str).unwrap_or("");
                    vec![Ok(StreamEvent::TextDelta(t.to_string()))]
                }
                "thinking_delta" => {
                    let t = delta.get("thinking").and_then(Value::as_str).unwrap_or("");
                    vec![Ok(StreamEvent::ThinkingDelta(t.to_string()))]
                }
                "input_json_delta" => {
                    let p = delta.get("partial_json").and_then(Value::as_str).unwrap_or("");
                    let id = tool_id_by_index.lock().unwrap().get(&index).cloned();
                    if let Some(id) = id {
                        vec![Ok(StreamEvent::ToolCallDelta {
                            id,
                            json_fragment: p.to_string(),
                        })]
                    } else {
                        vec![]
                    }
                }
                _ => vec![],
            }
        }
        "content_block_stop" => {
            let index = ev.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
            let id = tool_id_by_index.lock().unwrap().get(&index).cloned();
            if let Some(id) = id {
                vec![Ok(StreamEvent::ToolCallEnd { id })]
            } else {
                vec![]
            }
        }
        "message_delta" => {
            let stop_reason = ev
                .get("delta")
                .and_then(|d| d.get("stop_reason"))
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_string();
            let usage = ev.get("usage").cloned().unwrap_or(Value::Null);
            let u = Usage {
                input_tokens: usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
                output_tokens: usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0) as u32,
                cache_read_tokens: usage
                    .get("cache_read_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
                cache_creation_tokens: usage
                    .get("cache_creation_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32,
            };
            vec![Ok(StreamEvent::MessageEnd {
                stop_reason,
                usage: u,
            })]
        }
        _ => vec![],
    }
}
