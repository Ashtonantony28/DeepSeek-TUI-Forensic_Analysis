//! Generic OpenAI-compatible chat completions adapter (SSE streaming).
//!
//! Used directly as `OpenAiCompatClient` and reused by `OpenAiClient`,
//! `DeepSeekClient`, `GroqClient`, `XaiClient`, `OllamaClient`.

use crate::{
    ChatRequest, ChatStream, ClientConfig, LlmClient, LlmError, StreamEvent, ToolSchema, Usage,
};
use agent_tui_protocol::{ContentBlock, Message, ModelInfo, Provider, Role, ToolCallId};
use async_trait::async_trait;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub struct OpenAiCompatClient {
    pub http: reqwest::Client,
    pub api_key: Option<String>,
    pub base_url: String,
    pub provider: Provider,
    pub extra_headers: Vec<(String, String)>,
}

impl OpenAiCompatClient {
    pub fn new(cfg: ClientConfig, provider: Provider, default_base: &str) -> Self {
        let base_url = cfg.base_url.unwrap_or_else(|| default_base.to_string());
        Self {
            http: reqwest::Client::builder()
                .user_agent("agent-tui/0.1")
                .build()
                .unwrap_or_default(),
            api_key: cfg.api_key,
            base_url,
            provider,
            extra_headers: Vec::new(),
        }
    }

    pub fn from_config(cfg: ClientConfig) -> Self {
        Self::new(cfg, Provider::OpenAiCompat, "http://localhost:8000")
    }
}

#[async_trait]
impl LlmClient for OpenAiCompatClient {
    async fn stream(&self, req: ChatRequest) -> Result<ChatStream, LlmError> {
        let body = build_request_body(&req)?;
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));

        let mut rb = self
            .http
            .post(&url)
            .header("content-type", "application/json");
        if let Some(k) = &self.api_key {
            rb = rb.header("authorization", format!("Bearer {k}"));
        }
        for (k, v) in &self.extra_headers {
            rb = rb.header(k.as_str(), v.as_str());
        }
        let resp = rb.json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(LlmError::Provider { status, body });
        }

        let tool_id_by_index: Arc<Mutex<std::collections::HashMap<u32, (ToolCallId, bool)>>> =
            Arc::new(Mutex::new(std::collections::HashMap::new()));
        let tool_id_for_closure = tool_id_by_index.clone();

        let byte_stream = resp.bytes_stream();
        let mapped = byte_stream
            .map(|chunk_res| chunk_res.map_err(LlmError::Http))
            .scan(Vec::<u8>::new(), move |buf, chunk_res| {
                let tool_id_by_index = tool_id_for_closure.clone();
                let res = chunk_res.map(|bytes| {
                    buf.extend_from_slice(&bytes);
                    let mut out: Vec<Result<StreamEvent, LlmError>> = Vec::new();
                    while let Some(pos) = find_newline(buf) {
                        let raw: Vec<u8> = buf.drain(..pos + 1).collect();
                        let line = String::from_utf8_lossy(&raw).trim().to_string();
                        if line.is_empty() {
                            continue;
                        }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" {
                                continue;
                            }
                            if let Ok(val) = serde_json::from_str::<Value>(data) {
                                out.extend(translate_chunk(val, &tool_id_by_index));
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
        Ok(vec![])
    }

    fn provider(&self) -> Provider {
        self.provider
    }
}

fn find_newline(buf: &[u8]) -> Option<usize> {
    buf.iter().position(|&b| b == b'\n')
}

#[derive(Serialize)]
struct OaiMsg<'a> {
    role: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

fn build_request_body(req: &ChatRequest) -> Result<Value, LlmError> {
    let mut messages: Vec<OaiMsg> = Vec::new();

    if let Some(sys) = &req.system {
        messages.push(OaiMsg {
            role: "system",
            content: Some(Value::String(sys.clone())),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for m in &req.messages {
        let role = match m.role {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        };
        let (content, tool_calls, tool_call_id) = render_oai_content(m);
        messages.push(OaiMsg {
            role,
            content,
            tool_calls,
            tool_call_id,
        });
    }

    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
    });
    if let Some(max) = req.max_output_tokens {
        body["max_tokens"] = json!(max);
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
                        "type":"function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.input_schema,
                        }
                    })
                })
                .collect::<Vec<_>>(),
        )
        .map_err(|e| LlmError::Decode(e.to_string()))?;
    }
    Ok(body)
}

fn render_oai_content(m: &Message) -> (Option<Value>, Option<Value>, Option<String>) {
    let mut text = String::new();
    let mut tool_calls: Vec<Value> = Vec::new();
    let mut tool_result_id: Option<String> = None;
    let mut tool_result_text: Option<String> = None;
    for block in &m.content {
        match block {
            ContentBlock::Text { text: t } | ContentBlock::Thinking { text: t } => {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(t);
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(json!({
                    "id": id.0,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": serde_json::to_string(input).unwrap_or_default(),
                    }
                }));
            }
            ContentBlock::ToolResult { tool_use_id, content, .. } => {
                tool_result_id = Some(tool_use_id.0.clone());
                tool_result_text = Some(content.clone());
            }
            ContentBlock::Image { .. } => {}
        }
    }
    if let Some(tid) = tool_result_id {
        return (
            Some(Value::String(tool_result_text.unwrap_or_default())),
            None,
            Some(tid),
        );
    }
    let content_val = if text.is_empty() { None } else { Some(Value::String(text)) };
    let tc_val = if tool_calls.is_empty() {
        None
    } else {
        Some(Value::Array(tool_calls))
    };
    (content_val, tc_val, None)
}

#[derive(Deserialize)]
struct Chunk {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<UsageRaw>,
}

#[derive(Deserialize)]
struct UsageRaw {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<FunctionDelta>,
}

#[derive(Deserialize, Default)]
struct FunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

fn translate_chunk(
    val: Value,
    tool_id_by_index: &Arc<Mutex<std::collections::HashMap<u32, (ToolCallId, bool)>>>,
) -> Vec<Result<StreamEvent, LlmError>> {
    let chunk: Chunk = match serde_json::from_value(val) {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    let mut out: Vec<Result<StreamEvent, LlmError>> = Vec::new();

    for choice in &chunk.choices {
        if let Some(text) = &choice.delta.content {
            if !text.is_empty() {
                out.push(Ok(StreamEvent::TextDelta(text.clone())));
            }
        }
        // DeepSeek/some providers use `reasoning_content`; others use `reasoning`.
        for rc in [&choice.delta.reasoning_content, &choice.delta.reasoning] {
            if let Some(t) = rc {
                if !t.is_empty() {
                    out.push(Ok(StreamEvent::ThinkingDelta(t.clone())));
                }
            }
        }
        for tc in &choice.delta.tool_calls {
            let mut guard = tool_id_by_index.lock().unwrap();
            let entry = guard.entry(tc.index).or_insert_with(|| {
                (
                    ToolCallId(
                        tc.id.clone().unwrap_or_else(|| ToolCallId::new().0),
                    ),
                    false,
                )
            });
            let started = entry.1;
            let id = entry.0.clone();
            if !started {
                if let Some(name) = tc.function.as_ref().and_then(|f| f.name.clone()) {
                    out.push(Ok(StreamEvent::ToolCallStart { id: id.clone(), name }));
                    entry.1 = true;
                }
            }
            if let Some(args) = tc.function.as_ref().and_then(|f| f.arguments.clone()) {
                if !args.is_empty() {
                    out.push(Ok(StreamEvent::ToolCallDelta {
                        id: id.clone(),
                        json_fragment: args,
                    }));
                }
            }
        }
        if let Some(reason) = &choice.finish_reason {
            // Close any open tool calls
            let mut guard = tool_id_by_index.lock().unwrap();
            for (_, (id, started)) in guard.iter() {
                if *started {
                    out.push(Ok(StreamEvent::ToolCallEnd { id: id.clone() }));
                }
            }
            guard.clear();
            let usage = chunk.usage.as_ref();
            let u = Usage {
                input_tokens: usage.map(|u| u.prompt_tokens).unwrap_or(0),
                output_tokens: usage.map(|u| u.completion_tokens).unwrap_or(0),
                cache_read_tokens: 0,
                cache_creation_tokens: 0,
            };
            out.push(Ok(StreamEvent::MessageEnd {
                stop_reason: reason.clone(),
                usage: u,
            }));
        }
    }
    out
}
