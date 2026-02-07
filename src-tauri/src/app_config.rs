use std::path::PathBuf;

use serde::Deserialize;
use tauri::Manager;

use crate::tmux_runtime::TmuxCommandMode;

const DEFAULT_CONFIG_TEMPLATE: &str = r#"# NeoAI configuration
# How ACP command terminals are placed in tmux: split | window | hidden
tmux_command_mode = "split"

# Allow the agent to request a specific tmux mode through ACP request metadata.
allow_agent_tmux_override = true

# Accepted values for agent-requested mode overrides.
agent_tmux_override_whitelist = ["split", "window", "hidden"]
"#;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub tmux_command_mode: TmuxCommandMode,
    pub allow_agent_tmux_override: bool,
    pub agent_tmux_override_whitelist: Vec<TmuxCommandMode>,
}

impl AppConfig {
    pub fn runtime_default() -> Self {
        Self {
            tmux_command_mode: TmuxCommandMode::Window,
            allow_agent_tmux_override: true,
            agent_tmux_override_whitelist: vec![
                TmuxCommandMode::Split,
                TmuxCommandMode::Window,
                TmuxCommandMode::Hidden,
            ],
        }
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawAppConfig {
    tmux_command_mode: Option<String>,
    allow_agent_tmux_override: Option<bool>,
    agent_tmux_override_whitelist: Option<Vec<String>>,
}

#[derive(Debug)]
pub struct AppConfigState {
    config: AppConfig,
    config_path: Option<PathBuf>,
}

impl Default for AppConfigState {
    fn default() -> Self {
        Self {
            config: AppConfig::runtime_default(),
            config_path: None,
        }
    }
}

impl AppConfigState {
    pub fn initialize(&mut self, app_handle: &tauri::AppHandle) -> Result<(), String> {
        let root = app_root_dir(app_handle)?;
        std::fs::create_dir_all(&root).map_err(|e| {
            format!(
                "Failed to create app config directory '{}': {e}",
                root.display()
            )
        })?;
        let path = root.join("config.toml");

        if !path.exists() {
            std::fs::write(&path, DEFAULT_CONFIG_TEMPLATE).map_err(|e| {
                format!(
                    "Failed to write initial config file '{}': {e}",
                    path.display()
                )
            })?;
        }

        let contents = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read config file '{}': {e}", path.display()))?;

        let parsed = parse_config_contents(&contents);
        self.config = parsed;
        self.config_path = Some(path);
        Ok(())
    }

    pub fn resolve_tmux_command_mode(
        &self,
        requested: Option<TmuxCommandMode>,
    ) -> (TmuxCommandMode, &'static str) {
        if let Some(requested) = requested {
            if self.config.allow_agent_tmux_override
                && self
                    .config
                    .agent_tmux_override_whitelist
                    .iter()
                    .any(|candidate| *candidate == requested)
            {
                return (requested, "agent");
            }
            return (self.config.tmux_command_mode, "config_fallback");
        }

        (self.config.tmux_command_mode, "config")
    }

    pub fn config_path(&self) -> Option<PathBuf> {
        self.config_path.clone()
    }
}

fn parse_config_contents(contents: &str) -> AppConfig {
    let mut config = AppConfig::runtime_default();

    let raw = match toml::from_str::<RawAppConfig>(contents) {
        Ok(raw) => raw,
        Err(err) => {
            log::warn!("Failed to parse NeoAI config.toml. Falling back to defaults: {err}");
            return config;
        }
    };

    if let Some(mode) = raw
        .tmux_command_mode
        .as_deref()
        .and_then(TmuxCommandMode::from_config_str)
    {
        config.tmux_command_mode = mode;
    }
    if let Some(allow) = raw.allow_agent_tmux_override {
        config.allow_agent_tmux_override = allow;
    }
    if let Some(whitelist) = raw.agent_tmux_override_whitelist {
        let parsed: Vec<TmuxCommandMode> = whitelist
            .iter()
            .filter_map(|value| TmuxCommandMode::from_config_str(value))
            .collect();
        if !parsed.is_empty() {
            config.agent_tmux_override_whitelist = parsed;
        }
    }

    config
}

fn app_root_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    {
        let home = app_handle
            .path()
            .home_dir()
            .map_err(|e| format!("Failed to resolve home directory: {e}"))?;
        return Ok(home.join(".neoai"));
    }

    #[cfg(not(target_os = "macos"))]
    {
        app_handle
            .path()
            .app_local_data_dir()
            .map_err(|e| format!("Failed to resolve app local data directory: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falls_back_to_runtime_default_for_invalid_toml() {
        let config = parse_config_contents("invalid = [");
        assert_eq!(config.tmux_command_mode, TmuxCommandMode::Window);
    }

    #[test]
    fn parses_tmux_mode_and_whitelist() {
        let toml = r#"
tmux_command_mode = "split"
allow_agent_tmux_override = true
agent_tmux_override_whitelist = ["split","hidden"]
"#;
        let config = parse_config_contents(toml);
        assert_eq!(config.tmux_command_mode, TmuxCommandMode::Split);
        assert_eq!(
            config.agent_tmux_override_whitelist,
            vec![TmuxCommandMode::Split, TmuxCommandMode::Hidden]
        );
    }
}
