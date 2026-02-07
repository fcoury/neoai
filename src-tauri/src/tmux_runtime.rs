use std::collections::{HashMap, HashSet};
use std::path::Path;

use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use tokio::process::Command;

const DEFAULT_OUTPUT_LIMIT: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TmuxCommandMode {
    Split,
    Window,
    Hidden,
}

impl TmuxCommandMode {
    pub fn from_config_str(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "split" => Some(Self::Split),
            "window" => Some(Self::Window),
            "hidden" => Some(Self::Hidden),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Split => "split",
            Self::Window => "window",
            Self::Hidden => "hidden",
        }
    }
}

#[derive(Debug, Clone)]
struct TerminalTmuxConfig {
    enabled: bool,
    session_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ManagedTmuxCommand {
    pub host_terminal_id: String,
    pub pane_id: String,
    pub output_byte_limit: Option<u64>,
}

#[derive(Debug, Default)]
pub struct TmuxRuntimeState {
    terminals: HashMap<String, TerminalTmuxConfig>,
    commands: HashMap<String, ManagedTmuxCommand>,
    next_command_id: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TmuxStatus {
    pub terminal_id: String,
    pub available: bool,
    pub enabled: bool,
    pub mode: String,
    pub session_name: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartNvimResult {
    pub launch_mode: String,
    pub session_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Copy)]
pub struct TmuxPaneState {
    pub dead: bool,
    pub exit_code: Option<u32>,
}

impl TmuxRuntimeState {
    pub fn new() -> Self {
        Self {
            terminals: HashMap::new(),
            commands: HashMap::new(),
            next_command_id: 1,
        }
    }

    fn ensure_terminal_entry(&mut self, terminal_id: &str) -> &mut TerminalTmuxConfig {
        self.terminals
            .entry(terminal_id.to_string())
            .or_insert_with(|| TerminalTmuxConfig {
                enabled: true,
                session_name: None,
            })
    }

    pub fn snapshot_for_terminal(
        &mut self,
        terminal_id: &str,
        available: bool,
        error: Option<String>,
    ) -> TmuxStatus {
        let entry = self.ensure_terminal_entry(terminal_id);
        let mode = if available && entry.enabled {
            "tmux"
        } else {
            "fallback"
        };
        TmuxStatus {
            terminal_id: terminal_id.to_string(),
            available,
            enabled: entry.enabled,
            mode: mode.to_string(),
            session_name: entry
                .session_name
                .clone()
                .unwrap_or_else(|| "neoai".to_string()),
            error,
        }
    }

    pub fn terminal_enabled(&mut self, terminal_id: &str) -> bool {
        self.ensure_terminal_entry(terminal_id).enabled
    }

    pub fn set_terminal_enabled(&mut self, terminal_id: &str, enabled: bool) {
        self.ensure_terminal_entry(terminal_id).enabled = enabled;
    }

    pub fn session_name(&mut self, terminal_id: &str) -> Option<String> {
        self.ensure_terminal_entry(terminal_id).session_name.clone()
    }

    pub fn set_session_name(&mut self, terminal_id: &str, session_name: String) {
        self.ensure_terminal_entry(terminal_id).session_name = Some(session_name);
    }

    pub fn assigned_session_names(&self) -> HashSet<String> {
        self.terminals
            .values()
            .filter_map(|config| config.session_name.clone())
            .collect()
    }

    pub fn register_command(
        &mut self,
        host_terminal_id: &str,
        pane_id: String,
        output_byte_limit: Option<u64>,
    ) -> String {
        let command_id = format!("tmux-{}", self.next_command_id);
        self.next_command_id += 1;

        self.commands.insert(
            command_id.clone(),
            ManagedTmuxCommand {
                host_terminal_id: host_terminal_id.to_string(),
                pane_id,
                output_byte_limit,
            },
        );

        command_id
    }

    pub fn command(&self, command_id: &str) -> Option<ManagedTmuxCommand> {
        self.commands.get(command_id).cloned()
    }

    pub fn remove_command(&mut self, command_id: &str) -> Option<ManagedTmuxCommand> {
        self.commands.remove(command_id)
    }

    pub fn remove_terminal(&mut self, terminal_id: &str) -> (Option<String>, Vec<String>) {
        let session = self
            .terminals
            .remove(terminal_id)
            .and_then(|config| config.session_name);

        let mut pane_ids = Vec::new();
        self.commands.retain(|_, command| {
            if command.host_terminal_id == terminal_id {
                pane_ids.push(command.pane_id.clone());
                false
            } else {
                true
            }
        });

        (session, pane_ids)
    }
}

pub async fn detect_tmux_available() -> Result<(), String> {
    let output = Command::new("tmux")
        .arg("-V")
        .output()
        .await
        .map_err(|e| format!("Failed to execute tmux: {e}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = if stderr.trim().is_empty() {
            "tmux is installed but returned a non-zero status".to_string()
        } else {
            stderr.trim().to_string()
        };
        Err(msg)
    }
}

pub async fn ensure_session_exists(session_name: &str, cwd: Option<&Path>) -> Result<(), String> {
    if tmux_has_session(session_name).await? {
        return Ok(());
    }

    let mut args = vec![
        "new-session".to_string(),
        "-d".to_string(),
        "-s".to_string(),
        session_name.to_string(),
    ];
    if let Some(cwd) = cwd {
        args.push("-c".to_string());
        args.push(cwd.to_string_lossy().to_string());
    }
    run_tmux_checked(args).await?;
    Ok(())
}

pub async fn prepare_nvim_window(
    session_name: &str,
    socket_path: &str,
    cwd: Option<&Path>,
) -> Result<(), String> {
    let command = format!("nvim --listen {}", shell_quote(socket_path));
    if !tmux_has_session(session_name).await? {
        let mut args = vec![
            "new-session".to_string(),
            "-d".to_string(),
            "-s".to_string(),
            session_name.to_string(),
            "-n".to_string(),
            "neoai-nvim".to_string(),
        ];
        if let Some(cwd) = cwd {
            args.push("-c".to_string());
            args.push(cwd.to_string_lossy().to_string());
        }
        args.push(command);
        run_tmux_checked(args).await?;
    } else {
        run_tmux_checked(vec![
            "kill-window".to_string(),
            "-t".to_string(),
            format!("{session_name}:neoai-nvim"),
        ])
        .await
        .ok();

        let mut args = vec![
            "new-window".to_string(),
            "-d".to_string(),
            "-n".to_string(),
            "neoai-nvim".to_string(),
            "-t".to_string(),
            session_name.to_string(),
        ];
        if let Some(cwd) = cwd {
            args.push("-c".to_string());
            args.push(cwd.to_string_lossy().to_string());
        }
        args.push(command);
        run_tmux_checked(args).await?;
    }

    run_tmux_checked(vec![
        "set-option".to_string(),
        "-t".to_string(),
        format!("{session_name}:neoai-nvim"),
        "remain-on-exit".to_string(),
        "on".to_string(),
    ])
    .await?;

    run_tmux_checked(vec![
        "select-window".to_string(),
        "-t".to_string(),
        format!("{session_name}:neoai-nvim"),
    ])
    .await?;

    // Keep startup deterministic: only Neovim window exists until ACP opens command panes/splits.
    prune_non_nvim_windows(session_name).await?;

    Ok(())
}

pub async fn create_command_pane(
    session_name: &str,
    mode: TmuxCommandMode,
    command: &str,
    args: &[String],
    env: &[acp::EnvVariable],
    cwd: Option<&Path>,
) -> Result<String, String> {
    let pane_id = create_pane_target(session_name, mode, cwd).await?;
    let pane_id = pane_id.trim().to_string();
    if pane_id.is_empty() {
        return Err("tmux did not return a pane id".to_string());
    }

    run_tmux_checked(vec![
        "set-option".to_string(),
        "-t".to_string(),
        pane_id.clone(),
        "remain-on-exit".to_string(),
        "on".to_string(),
    ])
    .await?;

    let shell_command = build_shell_command(command, args, env);
    run_tmux_checked(vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.clone(),
        "-l".to_string(),
        shell_command,
    ])
    .await?;
    run_tmux_checked(vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.clone(),
        "Enter".to_string(),
    ])
    .await?;

    Ok(pane_id)
}

async fn create_pane_target(
    session_name: &str,
    mode: TmuxCommandMode,
    cwd: Option<&Path>,
) -> Result<String, String> {
    match mode {
        TmuxCommandMode::Window => new_window_pane(session_name, "neoai-cmd", cwd).await,
        TmuxCommandMode::Hidden => new_window_pane(session_name, "neoai-cmd-bg", cwd).await,
        TmuxCommandMode::Split => split_window_pane(session_name, cwd).await,
    }
}

async fn new_window_pane(
    session_name: &str,
    window_name: &str,
    cwd: Option<&Path>,
) -> Result<String, String> {
    let mut create_args = vec![
        "new-window".to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{pane_id}".to_string(),
        "-n".to_string(),
        window_name.to_string(),
        "-t".to_string(),
        session_name.to_string(),
    ];
    if let Some(cwd) = cwd {
        create_args.push("-c".to_string());
        create_args.push(cwd.to_string_lossy().to_string());
    }
    run_tmux_checked(create_args).await
}

async fn split_window_pane(session_name: &str, cwd: Option<&Path>) -> Result<String, String> {
    let mut create_args = vec![
        "split-window".to_string(),
        "-d".to_string(),
        "-P".to_string(),
        "-F".to_string(),
        "#{pane_id}".to_string(),
        "-t".to_string(),
        format!("{session_name}:neoai-nvim"),
    ];
    if let Some(cwd) = cwd {
        create_args.push("-c".to_string());
        create_args.push(cwd.to_string_lossy().to_string());
    }

    match run_tmux_checked(create_args).await {
        Ok(out) => Ok(out),
        Err(primary_err) => {
            log::warn!(
                "tmux split target neoai-nvim unavailable, falling back to session root: {}",
                primary_err
            );
            let mut fallback_args = vec![
                "split-window".to_string(),
                "-d".to_string(),
                "-P".to_string(),
                "-F".to_string(),
                "#{pane_id}".to_string(),
                "-t".to_string(),
                session_name.to_string(),
            ];
            if let Some(cwd) = cwd {
                fallback_args.push("-c".to_string());
                fallback_args.push(cwd.to_string_lossy().to_string());
            }
            run_tmux_checked(fallback_args).await
        }
    }
}

pub async fn pane_output(pane_id: &str) -> Result<String, String> {
    run_tmux_checked(vec![
        "capture-pane".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
    ])
    .await
}

pub async fn pane_state(pane_id: &str) -> Result<TmuxPaneState, String> {
    let status = run_tmux_checked(vec![
        "display-message".to_string(),
        "-p".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "#{pane_dead}:#{pane_exit_status}".to_string(),
    ])
    .await?;
    let (dead, exit_code) = parse_pane_state(&status);
    Ok(TmuxPaneState { dead, exit_code })
}

pub async fn interrupt_pane(pane_id: &str) -> Result<(), String> {
    run_tmux_checked(vec![
        "send-keys".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
        "C-c".to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn kill_pane(pane_id: &str) -> Result<(), String> {
    run_tmux_checked(vec![
        "kill-pane".to_string(),
        "-t".to_string(),
        pane_id.to_string(),
    ])
    .await?;
    Ok(())
}

pub async fn kill_session(session_name: &str) -> Result<(), String> {
    run_tmux_checked(vec![
        "kill-session".to_string(),
        "-t".to_string(),
        session_name.to_string(),
    ])
    .await?;
    Ok(())
}

pub fn truncate_output(output: String, output_byte_limit: Option<u64>) -> (String, bool) {
    let limit = output_byte_limit.unwrap_or(DEFAULT_OUTPUT_LIMIT) as usize;
    if output.len() <= limit {
        return (output, false);
    }

    let mut start = output.len().saturating_sub(limit);
    while start < output.len() && !output.is_char_boundary(start) {
        start += 1;
    }

    (output[start..].to_string(), true)
}

fn parse_pane_state(raw: &str) -> (bool, Option<u32>) {
    let value = raw.trim();
    let mut parts = value.splitn(2, ':');
    let dead = parts.next().unwrap_or("0") == "1";
    let exit_code = parts
        .next()
        .and_then(|value| value.parse::<i32>().ok())
        .and_then(|value| u32::try_from(value).ok());
    (dead, exit_code)
}

pub async fn find_available_session_name(
    base: &str,
    reserved: &HashSet<String>,
) -> Result<String, String> {
    let normalized = if base.trim().is_empty() {
        "neoai"
    } else {
        base.trim()
    };

    let mut index = 1_u32;
    loop {
        let candidate = if index == 1 {
            normalized.to_string()
        } else {
            format!("{normalized}-{index}")
        };
        index += 1;

        if reserved.contains(&candidate) {
            continue;
        }
        if !tmux_has_session(&candidate).await? {
            return Ok(candidate);
        }
    }
}

pub fn session_base_name(cwd: Option<&Path>, terminal_id: &str) -> String {
    if let Some(cwd) = cwd {
        if let Some(name) = cwd.file_name().and_then(|value| value.to_str()) {
            let sanitized = sanitize_identifier(name);
            if !sanitized.is_empty() {
                return format!("neoai-{sanitized}");
            }
        }
    }

    let fallback = terminal_id.strip_prefix("terminal-").unwrap_or(terminal_id);
    let sanitized = sanitize_identifier(fallback);
    if sanitized.is_empty() {
        "neoai".to_string()
    } else {
        format!("neoai-{sanitized}")
    }
}

fn sanitize_identifier(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn build_shell_command(command: &str, args: &[String], env: &[acp::EnvVariable]) -> String {
    let mut parts = Vec::new();
    for var in env {
        if valid_env_name(&var.name) {
            parts.push(format!("{}={}", var.name, shell_quote(&var.value)));
        }
    }
    parts.push(shell_quote(command));
    for arg in args {
        parts.push(shell_quote(arg));
    }
    parts.join(" ")
}

fn valid_env_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(ch) if ch.is_ascii_alphabetic() || ch == '_' => {}
        _ => return false,
    }
    chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

fn shell_quote(value: &str) -> String {
    if value.is_empty() {
        "''".to_string()
    } else {
        format!("'{}'", value.replace('\'', "'\"'\"'"))
    }
}

async fn tmux_has_session(session_name: &str) -> Result<bool, String> {
    let output = Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .output()
        .await
        .map_err(|e| format!("Failed to execute tmux has-session: {e}"))?;
    if output.status.success() {
        return Ok(true);
    }
    if output.status.code() == Some(1) {
        return Ok(false);
    }
    Err(format!(
        "tmux has-session failed: {}",
        preferred_error(&output)
    ))
}

async fn run_tmux_checked(args: Vec<String>) -> Result<String, String> {
    let output = Command::new("tmux")
        .args(&args)
        .output()
        .await
        .map_err(|e| format!("Failed to execute tmux {}: {e}", args.join(" ")))?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(format!(
            "tmux {} failed: {}",
            args.join(" "),
            preferred_error(&output)
        ))
    }
}

fn preferred_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stderr.is_empty() {
        return stderr;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stdout.is_empty() {
        return stdout;
    }
    format!("exit status {}", output.status)
}

async fn prune_non_nvim_windows(session_name: &str) -> Result<(), String> {
    let windows_raw = run_tmux_checked(vec![
        "list-windows".to_string(),
        "-t".to_string(),
        session_name.to_string(),
        "-F".to_string(),
        "#{window_id} #{window_name}".to_string(),
    ])
    .await?;

    for line in windows_raw.lines() {
        let mut parts = line.splitn(2, ' ');
        let Some(window_id) = parts.next() else {
            continue;
        };
        let window_name = parts.next().unwrap_or_default();
        if window_name == "neoai-nvim" {
            continue;
        }
        let _ = run_tmux_checked(vec![
            "kill-window".to_string(),
            "-t".to_string(),
            window_id.to_string(),
        ])
        .await;
    }

    Ok(())
}
