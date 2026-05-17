//! Configuration for agent-tui.
//!
//! Five-layer cascade (highest precedence first):
//!   1. managed (built-in compile-time managed defaults overridden by --managed-config)
//!   2. CLI arguments
//!   3. local project config (`.agent-tui/config.toml`)
//!   4. user config (`~/.agent-tui/config.toml`)
//!   5. built-in defaults
//!
//! Env overrides:
//!   AGENT_TUI_API_KEY_<PROVIDER>, AGENT_TUI_PROFILE, AGENT_TUI_BASE_URL_<PROVIDER>
//!
//! Project overlays MUST NOT supply these keys (security):
//!   api_key, base_url, provider, mcp_config_path

use agent_tui_protocol::Provider;
use camino::{Utf8Path, Utf8PathBuf};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error reading {path}: {source}")]
    Io { path: Utf8PathBuf, source: std::io::Error },
    #[error("toml parse error in {path}: {source}")]
    Toml { path: Utf8PathBuf, source: toml::de::Error },
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("project overlay attempts to set forbidden key `{0}`")]
    ForbiddenProjectKey(String),
    #[error("home directory not available")]
    NoHome,
    #[error("{0}")]
    Other(String),
}

/// Top-level config, fully merged.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub provider: Option<Provider>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub providers: HashMap<String, ProviderConfig>,
    #[serde(default)]
    pub mcp_config_path: Option<Utf8PathBuf>,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub extensions: Extensions,
    #[serde(default)]
    pub yolo: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProviderConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub extra_headers: HashMap<String, String>,
}

/// Extension toggles. Default values per Phase 3 plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Extensions {
    #[serde(default = "Extensions::default_true")]
    pub checkpoint_enabled: bool,
    #[serde(default = "Extensions::default_true")]
    pub lint_on_edit: bool,
    #[serde(default = "Extensions::default_true")]
    pub auto_test: bool,
    #[serde(default = "Extensions::default_retrieval_mode")]
    pub retrieval_mode: String,
    #[serde(default = "Extensions::default_true")]
    pub memory_enabled: bool,
    #[serde(default = "Extensions::default_auto")]
    pub routing: String,
    #[serde(default = "Extensions::default_true")]
    pub plan_blocks: bool,
    #[serde(default = "Extensions::default_true")]
    pub dars_branching: bool,
    #[serde(default = "Extensions::default_verifier_count")]
    pub verifier_count: u32,
    #[serde(default)]
    pub pipeline_enabled: bool,
    #[serde(default)]
    pub repl_tools: bool,
}

impl Default for Extensions {
    fn default() -> Self {
        Self {
            checkpoint_enabled: true,
            lint_on_edit: true,
            auto_test: true,
            retrieval_mode: "hybrid".into(),
            memory_enabled: true,
            routing: "auto".into(),
            plan_blocks: true,
            dars_branching: true,
            verifier_count: 3,
            pipeline_enabled: false,
            repl_tools: false,
        }
    }
}

impl Extensions {
    fn default_true() -> bool { true }
    fn default_retrieval_mode() -> String { "hybrid".into() }
    fn default_auto() -> String { "auto".into() }
    fn default_verifier_count() -> u32 { 3 }
}

/// Forbidden keys for project-level overlays.
pub const FORBIDDEN_PROJECT_KEYS: &[&str] = &[
    "api_key",
    "base_url",
    "provider",
    "mcp_config_path",
];

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub provider: Option<Provider>,
    pub model: Option<String>,
    pub yolo: Option<bool>,
}

/// Load the merged config from the standard cascade.
///
/// Order applied last-wins on top of defaults: user → project → CLI → env.
pub fn load(workspace_root: &Utf8Path, cli: &CliOverrides) -> Result<Config, ConfigError> {
    let mut cfg = Config::default();

    // 5: built-in defaults — already in `cfg`.

    // 4: user config
    if let Some(user_path) = user_config_path() {
        if user_path.exists() {
            let raw = read_toml(&user_path)?;
            apply_overlay(&mut cfg, raw, /*forbid=*/ false)?;
        }
    }

    // 3: project overlay
    let project_path = workspace_root.join(".agent-tui").join("config.toml");
    if project_path.exists() {
        let raw = read_toml(&project_path)?;
        apply_overlay(&mut cfg, raw, /*forbid=*/ true)?;
    }

    // 2: CLI args
    if let Some(p) = cli.provider {
        cfg.provider = Some(p);
    }
    if let Some(m) = &cli.model {
        cfg.model = Some(m.clone());
    }
    if let Some(y) = cli.yolo {
        cfg.yolo = y;
    }

    // Env overrides for api keys / base urls / profile
    apply_env(&mut cfg);

    Ok(cfg)
}

/// Where the user-level config lives.
pub fn user_config_path() -> Option<Utf8PathBuf> {
    let home = dirs::home_dir()?;
    Utf8PathBuf::from_path_buf(home.join(".agent-tui").join("config.toml")).ok()
}

/// Read the path as TOML into a generic value table.
fn read_toml(path: &Utf8Path) -> Result<toml::Value, ConfigError> {
    let raw = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_owned(),
        source: e,
    })?;
    toml::from_str(&raw).map_err(|e| ConfigError::Toml {
        path: path.to_owned(),
        source: e,
    })
}

/// Apply a TOML overlay onto `cfg`. If `forbid` is true, refuse forbidden keys.
fn apply_overlay(cfg: &mut Config, raw: toml::Value, forbid: bool) -> Result<(), ConfigError> {
    let table = match raw {
        toml::Value::Table(t) => t,
        _ => return Err(ConfigError::Other("top-level must be a table".into())),
    };

    if forbid {
        for k in FORBIDDEN_PROJECT_KEYS {
            if table.contains_key(*k) {
                return Err(ConfigError::ForbiddenProjectKey((*k).to_string()));
            }
            if *k == "api_key" || *k == "base_url" {
                if let Some(toml::Value::Table(providers)) = table.get("providers") {
                    for (_, v) in providers {
                        if let toml::Value::Table(pt) = v {
                            if pt.contains_key(*k) {
                                return Err(ConfigError::ForbiddenProjectKey(
                                    format!("providers.*.{}", k),
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    let overlay: Config = toml::Value::Table(table).try_into().map_err(|e| {
        ConfigError::Other(format!("invalid config shape: {e}"))
    })?;

    if overlay.provider.is_some() {
        cfg.provider = overlay.provider;
    }
    if overlay.model.is_some() {
        cfg.model = overlay.model;
    }
    if overlay.mcp_config_path.is_some() {
        cfg.mcp_config_path = overlay.mcp_config_path;
    }
    if overlay.profile.is_some() {
        cfg.profile = overlay.profile;
    }
    if overlay.yolo {
        cfg.yolo = true;
    }
    for (k, v) in overlay.providers {
        let entry = cfg.providers.entry(k).or_default();
        if v.api_key.is_some() {
            entry.api_key = v.api_key;
        }
        if v.base_url.is_some() {
            entry.base_url = v.base_url;
        }
        if v.default_model.is_some() {
            entry.default_model = v.default_model;
        }
        for (h, val) in v.extra_headers {
            entry.extra_headers.insert(h, val);
        }
    }
    cfg.extensions = overlay.extensions;
    Ok(())
}

fn apply_env(cfg: &mut Config) {
    if let Ok(profile) = std::env::var("AGENT_TUI_PROFILE") {
        cfg.profile = Some(profile);
    }
    for provider in [
        Provider::Anthropic,
        Provider::OpenAi,
        Provider::DeepSeek,
        Provider::Groq,
        Provider::Xai,
        Provider::Ollama,
        Provider::OpenAiCompat,
    ] {
        let upper = provider.as_str().to_uppercase();
        let api_key_env = format!("AGENT_TUI_API_KEY_{upper}");
        let base_url_env = format!("AGENT_TUI_BASE_URL_{upper}");
        let entry = cfg.providers.entry(provider.as_str().to_string()).or_default();
        if let Ok(v) = std::env::var(&api_key_env) {
            entry.api_key = Some(v);
        }
        if let Ok(v) = std::env::var(&base_url_env) {
            entry.base_url = Some(v);
        }
    }
}

/// Write the user config to disk. On unix, sets mode 0600.
pub fn save_user(cfg: &Config) -> Result<Utf8PathBuf, ConfigError> {
    let path = user_config_path().ok_or(ConfigError::NoHome)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| ConfigError::Io {
            path: parent.to_owned(),
            source: e,
        })?;
    }
    let body = toml::to_string_pretty(cfg)?;
    std::fs::write(&path, body).map_err(|e| ConfigError::Io {
        path: path.clone(),
        source: e,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(&path, perms).map_err(|e| ConfigError::Io {
            path: path.clone(),
            source: e,
        })?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir() -> Utf8PathBuf {
        let p = std::env::temp_dir().join(format!(
            "agent-tui-cfg-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        Utf8PathBuf::from_path_buf(p).unwrap()
    }

    #[test]
    fn defaults_have_expected_extensions() {
        let cfg = Config::default();
        assert!(cfg.extensions.checkpoint_enabled);
        assert!(cfg.extensions.lint_on_edit);
        assert!(cfg.extensions.auto_test);
        assert_eq!(cfg.extensions.retrieval_mode, "hybrid");
        assert_eq!(cfg.extensions.routing, "auto");
        assert_eq!(cfg.extensions.verifier_count, 3);
    }

    #[test]
    fn project_overlay_rejects_api_key() {
        let dir = tmpdir();
        let p = dir.join(".agent-tui");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("config.toml"), "api_key = \"secret\"\n").unwrap();
        let err = load(&dir, &CliOverrides::default()).unwrap_err();
        assert!(matches!(err, ConfigError::ForbiddenProjectKey(_)));
    }

    #[test]
    fn project_overlay_rejects_nested_provider_api_key() {
        let dir = tmpdir();
        let p = dir.join(".agent-tui");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(
            p.join("config.toml"),
            "[providers.anthropic]\napi_key = \"x\"\n",
        )
        .unwrap();
        let err = load(&dir, &CliOverrides::default()).unwrap_err();
        assert!(matches!(err, ConfigError::ForbiddenProjectKey(_)));
    }

    #[test]
    fn cli_overrides_provider() {
        let dir = tmpdir();
        let cfg = load(
            &dir,
            &CliOverrides {
                provider: Some(Provider::Anthropic),
                model: Some("claude-opus-4-7".into()),
                yolo: Some(true),
            },
        )
        .unwrap();
        assert_eq!(cfg.provider, Some(Provider::Anthropic));
        assert_eq!(cfg.model.as_deref(), Some("claude-opus-4-7"));
        assert!(cfg.yolo);
    }
}
