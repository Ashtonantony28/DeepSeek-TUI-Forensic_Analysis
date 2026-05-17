//! Auto routing (extension 3.6).
//!
//! A lightweight classifier that maps an incoming user prompt to a model
//! "tier" — `Small`, `Mid`, or `Large` — based on cheap textual signals:
//! prompt length, presence of multi-file edit verbs, fenced code blocks,
//! file-path mentions, and stack-trace markers.
//!
//! The router then resolves the tier to a concrete model name using a
//! per-provider mapping. When `routing == "auto"` is set in `Extensions`,
//! the engine calls `Router::pick_model_for(...)` before each new user
//! turn and swaps the session's `model` if the recommendation differs.
//!
//! The classifier is intentionally deterministic and offline so the
//! prefix-cache stays stable across reruns of the same prompt.

use agent_tui_protocol::Provider;
use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Small,
    Mid,
    Large,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Small => "small",
            Tier::Mid => "mid",
            Tier::Large => "large",
        }
    }
}

/// Default routing policy. Public so tests and the CLI can introspect.
#[derive(Debug, Clone)]
pub struct Router {
    pub provider: Provider,
}

impl Router {
    pub fn new(provider: Provider) -> Self {
        Self { provider }
    }

    /// Map a tier to a concrete model for the configured provider.
    /// Picks reasonable defaults: cheap → mid → flagship.
    pub fn model_for_tier(&self, tier: Tier) -> &'static str {
        match (self.provider, tier) {
            (Provider::Anthropic, Tier::Small) => "claude-haiku-4-5",
            (Provider::Anthropic, Tier::Mid) => "claude-sonnet-4-6",
            (Provider::Anthropic, Tier::Large) => "claude-opus-4-7",

            (Provider::OpenAi, Tier::Small) => "gpt-4o-mini",
            (Provider::OpenAi, Tier::Mid) => "gpt-4o",
            (Provider::OpenAi, Tier::Large) => "gpt-4.1",

            (Provider::DeepSeek, Tier::Small) => "deepseek-chat",
            (Provider::DeepSeek, Tier::Mid) => "deepseek-chat",
            (Provider::DeepSeek, Tier::Large) => "deepseek-reasoner",

            (Provider::Groq, Tier::Small) => "llama-3.1-8b-instant",
            (Provider::Groq, Tier::Mid) => "llama-3.1-70b-versatile",
            (Provider::Groq, Tier::Large) => "llama-3.1-70b-versatile",

            (Provider::Xai, _) => "grok-2",
            (Provider::Ollama, _) => "llama3.1",
            (Provider::OpenAiCompat, _) => "default",
        }
    }

    /// Classify and recommend a model. Returns `(tier, model_name)`.
    pub fn pick(&self, prompt: &str) -> (Tier, &'static str) {
        let tier = classify(prompt);
        (tier, self.model_for_tier(tier))
    }
}

/// Public classifier — exposed so the TUI and CLI can label prompts.
pub fn classify(prompt: &str) -> Tier {
    let score = complexity_score(prompt);
    if score >= 3 {
        Tier::Large
    } else if score >= 1 {
        Tier::Mid
    } else {
        Tier::Small
    }
}

/// Heuristic complexity in [0, 8]. Each signal contributes 1–2 points.
fn complexity_score(prompt: &str) -> u32 {
    let mut score: u32 = 0;
    let len = prompt.len();
    if len >= 4_000 {
        score += 2;
    } else if len >= 800 {
        score += 1;
    }

    let lower = prompt.to_lowercase();
    if HARD_VERB_RE.is_match(&lower) {
        score += 2;
    }
    if EASY_VERB_RE.is_match(&lower) {
        // "list", "show", "what is" — slight downward pressure relative
        // to the length signal. We don't go negative.
        score = score.saturating_sub(1);
    }
    if CODE_FENCE_RE.is_match(prompt) {
        score += 1;
    }
    if PATH_RE.is_match(prompt) {
        score += 1;
    }
    if STACK_TRACE_RE.is_match(prompt) {
        score += 2;
    }
    score
}

static HARD_VERB_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"\b(implement|refactor|design|architect|migrate|debug|optimi[sz]e|fix|port|integrate|prototype|build out)\b")
        .unwrap()
});
static EASY_VERB_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^(\s*)(list|show|what is|what's|where is|where's|how many|name|print|echo)\b")
        .unwrap()
});
static CODE_FENCE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"```").unwrap());
static PATH_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:[\w\-\.]+/){1,}[\w\-\.]+\.[A-Za-z0-9]{1,6}").unwrap());
static STACK_TRACE_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?im)^\s*(at\s+[\w\.\$<>]+\(|thread\s+'\w+'\s+panicked|Traceback \(most recent|goroutine \d+ \[)")
        .unwrap()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trivial_prompt_is_small() {
        assert_eq!(classify("list the files in src"), Tier::Small);
        assert_eq!(classify("what is 2+2"), Tier::Small);
    }

    #[test]
    fn refactor_with_code_is_large() {
        let p = "Please refactor the parser in src/parser.rs to use the new tokenizer.\n```rust\nfn parse() {}\n```";
        let t = classify(p);
        assert_eq!(t, Tier::Large, "got {:?}", t);
    }

    #[test]
    fn stack_trace_pushes_to_large() {
        let p = "Investigate this crash:\n\
                 thread 'main' panicked at 'unreachable', src/lib.rs:42:5\n\
                 stack backtrace:\n\
                    0: rust_begin_unwind";
        assert_eq!(classify(p), Tier::Large);
    }

    #[test]
    fn long_prompt_is_at_least_mid() {
        let p = "a".repeat(900);
        assert!(matches!(classify(&p), Tier::Mid | Tier::Large));
    }

    #[test]
    fn router_maps_to_concrete_models() {
        let r = Router::new(Provider::Anthropic);
        // design (+2) + path (+1) = Large
        let (t, m) = r.pick("design a sharded retry queue and prototype it in src/queue.rs");
        assert_eq!(t, Tier::Large);
        assert!(m.contains("opus") || m.contains("sonnet"));

        let (t2, m2) = r.pick("list files in this directory");
        assert_eq!(t2, Tier::Small);
        assert!(m2.contains("haiku"));
    }

    #[test]
    fn deepseek_routes_reasoner_only_for_hard() {
        let r = Router::new(Provider::DeepSeek);
        let (_, m1) = r.pick("what is the time?");
        let (_, m2) = r.pick("refactor the entire async pipeline in src/runtime.rs");
        assert_eq!(m1, "deepseek-chat");
        assert_eq!(m2, "deepseek-reasoner");
    }
}
