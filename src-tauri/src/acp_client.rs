use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use acp::Agent as _;
use agent_client_protocol as acp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};
use tokio::sync::{mpsc, oneshot, Mutex};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

use crate::app_config;
use crate::nvim_bridge::{nvim_read_file_for_terminal, nvim_write_file_for_terminal};
use crate::tmux_runtime;

const CODEX_ACP_VERSION: &str = "0.9.2";
const CODEX_RELEASES_URL: &str = "https://github.com/zed-industries/codex-acp/releases";
const DEFAULT_AGENT_PATH: &str = "codex-acp";
const DEFAULT_AGENT_PATH_WINDOWS: &str = "codex-acp.exe";

static CODEX_INSTALL_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

#[derive(Debug, Clone, Copy)]
struct CodexAsset {
    target: &'static str,
    binary_name: &'static str,
    archive: ArchiveFormat,
    url: &'static str,
    sha256: &'static str,
}

// -- Serializable types for IPC --

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum AgentStatus {
    Stopped,
    Starting,
    Running,
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", content = "data")]
#[serde(rename_all = "camelCase")]
pub enum AcpEvent {
    ContentChunk(String),
    ThoughtChunk(String),
    ToolCallStarted {
        id: String,
        title: String,
        kind: String,
    },
    ToolCallUpdated {
        id: String,
        status: String,
    },
    Done {
        stop_reason: String,
    },
    Error(String),
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AcpInstallStatusEvent {
    pub phase: String,
    pub message: String,
    pub version: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AcpPermissionRequestEvent {
    pub request_id: String,
    pub session_id: String,
    pub terminal_id: Option<String>,
    pub tool_call_id: String,
    pub title: Option<String>,
    pub kind: Option<String>,
    pub options: Vec<AcpPermissionOption>,
}

type PendingPermissionRequests =
    Arc<Mutex<std::collections::HashMap<String, oneshot::Sender<acp::RequestPermissionOutcome>>>>;
type SessionTerminalBindings = Arc<Mutex<std::collections::HashMap<String, String>>>;

// -- Channel-based communication with the !Send ACP connection --

enum AcpCommand {
    CreateSession {
        working_dir: PathBuf,
        terminal_id: String,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Prompt {
        session_id: String,
        messages: Vec<String>,
        context: Option<String>,
        reply: oneshot::Sender<Result<String, String>>,
    },
    Shutdown,
}

struct AcpClientHandler {
    app_handle: tauri::AppHandle,
    pending_permission_requests: PendingPermissionRequests,
    permission_request_counter: Arc<AtomicU64>,
    session_terminal_bindings: SessionTerminalBindings,
}

#[async_trait::async_trait(?Send)]
impl acp::Client for AcpClientHandler {
    async fn request_permission(
        &self,
        args: acp::RequestPermissionRequest,
    ) -> acp::Result<acp::RequestPermissionResponse> {
        let request_number = self
            .permission_request_counter
            .fetch_add(1, Ordering::Relaxed);
        let request_id = format!("perm-{}", request_number);
        let session_id = args.session_id.to_string();
        let terminal_id = {
            let bindings = self.session_terminal_bindings.lock().await;
            bindings.get(&session_id).cloned()
        };

        let permission_event = AcpPermissionRequestEvent {
            request_id: request_id.clone(),
            session_id,
            terminal_id,
            tool_call_id: args.tool_call.tool_call_id.to_string(),
            title: args.tool_call.fields.title.clone(),
            kind: args.tool_call.fields.kind.map(|kind| format!("{:?}", kind)),
            options: args
                .options
                .iter()
                .map(|option| AcpPermissionOption {
                    option_id: option.option_id.to_string(),
                    name: option.name.clone(),
                    kind: format!("{:?}", option.kind),
                })
                .collect(),
        };

        let (decision_tx, decision_rx) = oneshot::channel::<acp::RequestPermissionOutcome>();
        self.pending_permission_requests
            .lock()
            .await
            .insert(request_id.clone(), decision_tx);

        if let Err(err) = self
            .app_handle
            .emit("acp-permission-request", &permission_event)
        {
            self.pending_permission_requests
                .lock()
                .await
                .remove(&request_id);
            return Err(acp::Error::internal_error().data(err.to_string()));
        }

        let outcome = match tokio::time::timeout(Duration::from_secs(300), decision_rx).await {
            Ok(Ok(outcome)) => outcome,
            _ => {
                self.pending_permission_requests
                    .lock()
                    .await
                    .remove(&request_id);
                acp::RequestPermissionOutcome::Cancelled
            }
        };

        Ok(acp::RequestPermissionResponse::new(outcome))
    }

    async fn session_notification(&self, args: acp::SessionNotification) -> acp::Result<()> {
        let event = match args.update {
            acp::SessionUpdate::AgentMessageChunk(chunk) => {
                if let acp::ContentBlock::Text(text) = chunk.content {
                    AcpEvent::ContentChunk(text.text)
                } else {
                    return Ok(());
                }
            }
            acp::SessionUpdate::AgentThoughtChunk(chunk) => {
                if let acp::ContentBlock::Text(text) = chunk.content {
                    AcpEvent::ThoughtChunk(text.text)
                } else {
                    return Ok(());
                }
            }
            acp::SessionUpdate::ToolCall(tool_call) => AcpEvent::ToolCallStarted {
                id: tool_call.tool_call_id.to_string(),
                title: tool_call.title,
                kind: format!("{:?}", tool_call.kind),
            },
            acp::SessionUpdate::ToolCallUpdate(update) => AcpEvent::ToolCallUpdated {
                id: update.tool_call_id.to_string(),
                status: "updated".to_string(),
            },
            _ => return Ok(()),
        };

        let _ = self.app_handle.emit("acp-event", &event);
        Ok(())
    }

    async fn read_text_file(
        &self,
        args: acp::ReadTextFileRequest,
    ) -> acp::Result<acp::ReadTextFileResponse> {
        if !args.path.is_absolute() {
            return Err(acp::Error::invalid_params().data(
                serde_json::json!({ "reason": "path must be absolute", "path": args.path }),
            ));
        }

        let session_id = args.session_id.to_string();
        let terminal_id = {
            let bindings = self.session_terminal_bindings.lock().await;
            bindings.get(&session_id).cloned()
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "session is not bound to a terminal",
                "sessionId": session_id
            }))
        })?;

        let content = nvim_read_file_for_terminal(
            &self.app_handle,
            &terminal_id,
            &args.path,
            args.line,
            args.limit,
        )
        .await
        .map_err(|e| acp::Error::internal_error().data(e))?;

        Ok(acp::ReadTextFileResponse::new(content))
    }

    async fn write_text_file(
        &self,
        args: acp::WriteTextFileRequest,
    ) -> acp::Result<acp::WriteTextFileResponse> {
        if !args.path.is_absolute() {
            return Err(acp::Error::invalid_params().data(
                serde_json::json!({ "reason": "path must be absolute", "path": args.path }),
            ));
        }

        let session_id = args.session_id.to_string();
        let terminal_id = {
            let bindings = self.session_terminal_bindings.lock().await;
            bindings.get(&session_id).cloned()
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "session is not bound to a terminal",
                "sessionId": session_id
            }))
        })?;

        nvim_write_file_for_terminal(&self.app_handle, &terminal_id, &args.path, &args.content)
            .await
            .map_err(|e| acp::Error::internal_error().data(e))?;

        Ok(acp::WriteTextFileResponse::new())
    }

    async fn create_terminal(
        &self,
        args: acp::CreateTerminalRequest,
    ) -> acp::Result<acp::CreateTerminalResponse> {
        let session_id = args.session_id.to_string();
        let host_terminal_id = {
            let bindings = self.session_terminal_bindings.lock().await;
            bindings.get(&session_id).cloned()
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "session is not bound to a terminal",
                "sessionId": session_id
            }))
        })?;

        let acp::CreateTerminalRequest {
            session_id: _,
            command,
            args: command_args,
            env,
            cwd,
            output_byte_limit,
            meta,
            ..
        } = args;

        tmux_runtime::detect_tmux_available().await.map_err(|err| {
            acp::Error::method_not_found().data(serde_json::json!({
                "reason": "tmux unavailable",
                "detail": err
            }))
        })?;

        let tmux_state = self
            .app_handle
            .state::<Mutex<tmux_runtime::TmuxRuntimeState>>();
        let (tmux_enabled, assigned_session_name, assigned_names) = {
            let mut state = tmux_state.lock().await;
            (
                state.terminal_enabled(&host_terminal_id),
                state.session_name(&host_terminal_id),
                state.assigned_session_names(),
            )
        };
        if !tmux_enabled {
            return Err(acp::Error::method_not_found().data(serde_json::json!({
                "reason": "tmux disabled for this terminal",
                "terminalId": host_terminal_id
            })));
        }

        let requested_mode = requested_tmux_mode(meta.as_ref());
        let (command_mode, command_mode_source) = {
            let config_state = self
                .app_handle
                .state::<std::sync::Mutex<app_config::AppConfigState>>();
            let state = config_state
                .lock()
                .map_err(|_| acp::Error::internal_error().data("App config lock poisoned"))?;
            state.resolve_tmux_command_mode(requested_mode)
        };
        log::info!(
            "ACP tmux mode resolved: terminal='{}' requested='{}' applied='{}' source='{}'",
            host_terminal_id,
            requested_mode
                .map(|mode| mode.as_str().to_string())
                .unwrap_or_else(|| "none".to_string()),
            command_mode.as_str(),
            command_mode_source
        );

        let session_name = if let Some(name) = assigned_session_name {
            name
        } else {
            let base_name = tmux_runtime::session_base_name(cwd.as_deref(), &host_terminal_id);
            let chosen = tmux_runtime::find_available_session_name(&base_name, &assigned_names)
                .await
                .map_err(|e| acp::Error::internal_error().data(e))?;
            let mut state = tmux_state.lock().await;
            state.set_session_name(&host_terminal_id, chosen.clone());
            chosen
        };

        let cwd_ref = cwd.as_deref();
        tmux_runtime::ensure_session_exists(&session_name, cwd_ref)
            .await
            .map_err(|e| acp::Error::internal_error().data(e))?;

        let pane_id = tmux_runtime::create_command_pane(
            &session_name,
            command_mode,
            &command,
            &command_args,
            &env,
            cwd_ref,
        )
        .await
        .map_err(|e| acp::Error::internal_error().data(e))?;

        let terminal_handle = {
            let mut state = tmux_state.lock().await;
            state.register_command(&host_terminal_id, pane_id, output_byte_limit)
        };

        Ok(acp::CreateTerminalResponse::new(terminal_handle))
    }

    async fn terminal_output(
        &self,
        args: acp::TerminalOutputRequest,
    ) -> acp::Result<acp::TerminalOutputResponse> {
        let command_id = args.terminal_id.to_string();
        let tmux_state = self
            .app_handle
            .state::<Mutex<tmux_runtime::TmuxRuntimeState>>();
        let command = {
            let state = tmux_state.lock().await;
            state.command(&command_id)
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "unknown tmux terminal id",
                "terminalId": command_id
            }))
        })?;

        let output = tmux_runtime::pane_output(&command.pane_id)
            .await
            .map_err(|e| acp::Error::internal_error().data(e))?;
        let pane_state = tmux_runtime::pane_state(&command.pane_id)
            .await
            .map_err(|e| acp::Error::internal_error().data(e))?;

        let (output, truncated) = tmux_runtime::truncate_output(output, command.output_byte_limit);
        let mut response = acp::TerminalOutputResponse::new(output, truncated);
        if pane_state.dead {
            response = response
                .exit_status(acp::TerminalExitStatus::new().exit_code(pane_state.exit_code));
        }
        Ok(response)
    }

    async fn wait_for_terminal_exit(
        &self,
        args: acp::WaitForTerminalExitRequest,
    ) -> acp::Result<acp::WaitForTerminalExitResponse> {
        let command_id = args.terminal_id.to_string();
        let tmux_state = self
            .app_handle
            .state::<Mutex<tmux_runtime::TmuxRuntimeState>>();
        let command = {
            let state = tmux_state.lock().await;
            state.command(&command_id)
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "unknown tmux terminal id",
                "terminalId": command_id
            }))
        })?;

        loop {
            let pane_state = tmux_runtime::pane_state(&command.pane_id)
                .await
                .map_err(|e| acp::Error::internal_error().data(e))?;

            if pane_state.dead {
                let exit_status = acp::TerminalExitStatus::new().exit_code(pane_state.exit_code);
                return Ok(acp::WaitForTerminalExitResponse::new(exit_status));
            }

            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    async fn kill_terminal_command(
        &self,
        args: acp::KillTerminalCommandRequest,
    ) -> acp::Result<acp::KillTerminalCommandResponse> {
        let command_id = args.terminal_id.to_string();
        let tmux_state = self
            .app_handle
            .state::<Mutex<tmux_runtime::TmuxRuntimeState>>();
        let command = {
            let state = tmux_state.lock().await;
            state.command(&command_id)
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "unknown tmux terminal id",
                "terminalId": command_id
            }))
        })?;

        tmux_runtime::interrupt_pane(&command.pane_id)
            .await
            .map_err(|e| acp::Error::internal_error().data(e))?;

        Ok(acp::KillTerminalCommandResponse::new())
    }

    async fn release_terminal(
        &self,
        args: acp::ReleaseTerminalRequest,
    ) -> acp::Result<acp::ReleaseTerminalResponse> {
        let command_id = args.terminal_id.to_string();
        let tmux_state = self
            .app_handle
            .state::<Mutex<tmux_runtime::TmuxRuntimeState>>();
        let command = {
            let mut state = tmux_state.lock().await;
            state.remove_command(&command_id)
        }
        .ok_or_else(|| {
            acp::Error::invalid_params().data(serde_json::json!({
                "reason": "unknown tmux terminal id",
                "terminalId": command_id
            }))
        })?;

        if let Err(err) = tmux_runtime::kill_pane(&command.pane_id).await {
            log::warn!(
                "Failed to kill pane '{}' while releasing terminal '{}': {}",
                command.pane_id,
                command_id,
                err
            );
        }

        Ok(acp::ReleaseTerminalResponse::new())
    }
}

fn requested_tmux_mode(meta: Option<&acp::Meta>) -> Option<tmux_runtime::TmuxCommandMode> {
    meta.and_then(|meta| meta.get("neoai_tmux_mode"))
        .and_then(|value| value.as_str())
        .and_then(tmux_runtime::TmuxCommandMode::from_config_str)
}

fn current_linux_env() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    {
        #[cfg(target_env = "musl")]
        {
            return Some("musl");
        }

        #[cfg(not(target_env = "musl"))]
        {
            return Some("gnu");
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn codex_install_lock() -> &'static tokio::sync::Mutex<()> {
    CODEX_INSTALL_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn codex_binary_name_for_os(os: &str) -> &'static str {
    if os == "windows" {
        DEFAULT_AGENT_PATH_WINDOWS
    } else {
        DEFAULT_AGENT_PATH
    }
}

fn codex_binary_name_current() -> &'static str {
    codex_binary_name_for_os(std::env::consts::OS)
}

fn is_default_agent_path(agent_path: &str) -> bool {
    let path = agent_path.trim();
    path == DEFAULT_AGENT_PATH || path == DEFAULT_AGENT_PATH_WINDOWS
}

fn resolve_codex_asset_for(os: &str, arch: &str, linux_env: Option<&str>) -> Option<CodexAsset> {
    match (os, arch, linux_env) {
        (
            "macos",
            "aarch64",
            _,
        ) => Some(CodexAsset {
            target: "aarch64-apple-darwin",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-aarch64-apple-darwin.tar.gz",
            sha256: "edfb6128a2972325f4767af6ee58b512de59dd8e7bc1e4c90d27ada3e9f9b84b",
        }),
        (
            "macos",
            "x86_64",
            _,
        ) => Some(CodexAsset {
            target: "x86_64-apple-darwin",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-x86_64-apple-darwin.tar.gz",
            sha256: "393bf04bf1270065e2b73a1bbdcf46dab1154f48b50bd64f5c1daff03c1ed317",
        }),
        (
            "linux",
            "aarch64",
            Some("gnu"),
        ) => Some(CodexAsset {
            target: "aarch64-unknown-linux-gnu",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-aarch64-unknown-linux-gnu.tar.gz",
            sha256: "52ef6fa1ccae7b9e102cff9ee20d7abe7498ee22d1219dc8e1858a75f60f757c",
        }),
        (
            "linux",
            "aarch64",
            Some("musl"),
        ) => Some(CodexAsset {
            target: "aarch64-unknown-linux-musl",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-aarch64-unknown-linux-musl.tar.gz",
            sha256: "45b3ec332643b5306e82edb70744e3e9329f1406a7200e0a0c79f8f8efe957dc",
        }),
        (
            "linux",
            "x86_64",
            Some("gnu"),
        ) => Some(CodexAsset {
            target: "x86_64-unknown-linux-gnu",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-x86_64-unknown-linux-gnu.tar.gz",
            sha256: "59531026a0542a4ca9f18d73b445c20ab36d4882dda145c4ab27a4a46196d1ad",
        }),
        (
            "linux",
            "x86_64",
            Some("musl"),
        ) => Some(CodexAsset {
            target: "x86_64-unknown-linux-musl",
            binary_name: DEFAULT_AGENT_PATH,
            archive: ArchiveFormat::TarGz,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-x86_64-unknown-linux-musl.tar.gz",
            sha256: "7280d7e93f353d6481a402914639e50c1527f538d15dfd47c4138fc8c03f98f5",
        }),
        (
            "windows",
            "aarch64",
            _,
        ) => Some(CodexAsset {
            target: "aarch64-pc-windows-msvc",
            binary_name: DEFAULT_AGENT_PATH_WINDOWS,
            archive: ArchiveFormat::Zip,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-aarch64-pc-windows-msvc.zip",
            sha256: "df00960eb5cc5f1543335702fbdf95f084d903d7702c4723d1375bb6056215dc",
        }),
        (
            "windows",
            "x86_64",
            _,
        ) => Some(CodexAsset {
            target: "x86_64-pc-windows-msvc",
            binary_name: DEFAULT_AGENT_PATH_WINDOWS,
            archive: ArchiveFormat::Zip,
            url: "https://github.com/zed-industries/codex-acp/releases/download/v0.9.2/codex-acp-0.9.2-x86_64-pc-windows-msvc.zip",
            sha256: "250648ced2645dce61a915b69515dc8e55d7836764faead7f27142ae064dadb4",
        }),
        _ => None,
    }
}

fn resolve_current_codex_asset() -> Result<CodexAsset, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let linux_env = current_linux_env();

    resolve_codex_asset_for(os, arch, linux_env).ok_or_else(|| {
        let env = linux_env.unwrap_or("n/a");
        format!(
            "No vendored codex-acp release available for os='{}', arch='{}', env='{}'",
            os, arch, env
        )
    })
}

fn emit_install_status(app_handle: &tauri::AppHandle, phase: &str, message: impl Into<String>) {
    let _ = app_handle.emit(
        "acp-install-status",
        &AcpInstallStatusEvent {
            phase: phase.to_string(),
            message: message.into(),
            version: Some(CODEX_ACP_VERSION.to_string()),
        },
    );
}

fn spawn_agent_process(agent_path: &str) -> Result<tokio::process::Child, std::io::Error> {
    tokio::process::Command::new(agent_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
}

fn codex_vendor_root_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
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

fn codex_install_path(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(codex_vendor_root_dir(app_handle)?
        .join("agents")
        .join("codex-acp")
        .join(CODEX_ACP_VERSION)
        .join(codex_binary_name_current()))
}

async fn download_release_asset(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .user_agent("neoai/0.1.0")
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("Download request failed: {e}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("Download failed with HTTP status {status}"));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read download body: {e}"))?;

    Ok(bytes.to_vec())
}

fn verify_sha256(bytes: &[u8], expected_hex: &str) -> Result<(), String> {
    let actual_hex = hex::encode(Sha256::digest(bytes));
    if actual_hex.eq_ignore_ascii_case(expected_hex) {
        Ok(())
    } else {
        Err(format!(
            "Checksum mismatch (expected {}, got {})",
            expected_hex, actual_hex
        ))
    }
}

fn extract_binary_from_tar_gz(
    bytes: &[u8],
    expected_name: &str,
    output_path: &Path,
) -> Result<(), String> {
    let cursor = std::io::Cursor::new(bytes);
    let decoder = flate2::read::GzDecoder::new(cursor);
    let mut archive = tar::Archive::new(decoder);

    let entries = archive
        .entries()
        .map_err(|e| format!("Failed to read tar entries: {e}"))?;

    for entry_result in entries {
        let mut entry = entry_result.map_err(|e| format!("Failed to read tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("Failed to read tar entry path: {e}"))?;

        if path.file_name() == Some(OsStr::new(expected_name)) {
            let mut output = fs::File::create(output_path)
                .map_err(|e| format!("Failed to create temp binary file: {e}"))?;
            io::copy(&mut entry, &mut output)
                .map_err(|e| format!("Failed to extract binary from tar archive: {e}"))?;
            return Ok(());
        }
    }

    Err(format!(
        "Downloaded archive did not contain expected binary '{}'",
        expected_name
    ))
}

fn extract_binary_from_zip(
    bytes: &[u8],
    expected_name: &str,
    output_path: &Path,
) -> Result<(), String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open zip archive: {e}"))?;

    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("Failed to read zip entry: {e}"))?;

        if file.is_dir() {
            continue;
        }

        let path = Path::new(file.name());
        if path.file_name() == Some(OsStr::new(expected_name)) {
            let mut output = fs::File::create(output_path)
                .map_err(|e| format!("Failed to create temp binary file: {e}"))?;
            io::copy(&mut file, &mut output)
                .map_err(|e| format!("Failed to extract binary from zip archive: {e}"))?;
            return Ok(());
        }
    }

    Err(format!(
        "Downloaded archive did not contain expected binary '{}'",
        expected_name
    ))
}

fn extract_binary_from_archive(
    bytes: &[u8],
    archive_format: ArchiveFormat,
    expected_name: &str,
    output_path: &Path,
) -> Result<(), String> {
    match archive_format {
        ArchiveFormat::TarGz => extract_binary_from_tar_gz(bytes, expected_name, output_path),
        ArchiveFormat::Zip => extract_binary_from_zip(bytes, expected_name, output_path),
    }
}

fn ensure_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let metadata =
            fs::metadata(path).map_err(|e| format!("Failed to read binary permissions: {e}"))?;
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|e| format!("Failed to set binary permissions: {e}"))?;
    }

    Ok(())
}

async fn ensure_vendored_codex_acp(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    let _install_guard = codex_install_lock().lock().await;

    emit_install_status(
        app_handle,
        "resolving",
        "Locating managed codex-acp release for your platform...",
    );

    let asset = resolve_current_codex_asset()?;
    let install_path = codex_install_path(app_handle)?;

    if install_path.exists() {
        ensure_executable(&install_path)?;
        emit_install_status(
            app_handle,
            "starting",
            "Using existing managed codex-acp installation...",
        );
        return Ok(install_path);
    }

    let parent = install_path
        .parent()
        .ok_or_else(|| "Failed to resolve installation directory".to_string())?;

    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create installation directory: {e}"))?;

    emit_install_status(
        app_handle,
        "downloading",
        format!(
            "Downloading codex-acp {} ({})...",
            CODEX_ACP_VERSION, asset.target
        ),
    );

    let archive_bytes = download_release_asset(asset.url).await?;

    emit_install_status(app_handle, "verifying", "Verifying download integrity...");
    verify_sha256(&archive_bytes, asset.sha256)?;

    emit_install_status(app_handle, "extracting", "Extracting codex-acp binary...");

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_path = parent.join(format!("{}.tmp-{}", asset.binary_name, nonce));

    extract_binary_from_archive(&archive_bytes, asset.archive, asset.binary_name, &temp_path)?;
    ensure_executable(&temp_path)?;

    emit_install_status(
        app_handle,
        "installing",
        format!(
            "Installing managed codex-acp {} for neoai...",
            CODEX_ACP_VERSION
        ),
    );

    if install_path.exists() {
        let _ = fs::remove_file(&temp_path);
    } else if let Err(e) = fs::rename(&temp_path, &install_path) {
        if install_path.exists() {
            let _ = fs::remove_file(&temp_path);
        } else {
            return Err(format!("Failed to finalize codex-acp installation: {e}"));
        }
    }

    emit_install_status(app_handle, "starting", "Starting AI agent...");
    Ok(install_path)
}

/// Runs on a dedicated thread with a LocalSet. Owns the !Send ACP connection
/// and processes commands from the Send world via channels.
async fn acp_worker(
    app_handle: tauri::AppHandle,
    agent_path: String,
    pending_permission_requests: PendingPermissionRequests,
    permission_request_counter: Arc<AtomicU64>,
    session_terminal_bindings: SessionTerminalBindings,
    mut cmd_rx: mpsc::Receiver<AcpCommand>,
    ready_tx: oneshot::Sender<Result<(), String>>,
) {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            emit_install_status(&app_handle, "starting", "Starting AI agent...");

            let mut child = match spawn_agent_process(&agent_path) {
                Ok(child) => child,
                Err(spawn_err)
                    if spawn_err.kind() == std::io::ErrorKind::NotFound
                        && is_default_agent_path(&agent_path) =>
                {
                    match ensure_vendored_codex_acp(&app_handle).await {
                        Ok(vendored_path) => {
                            let vendored_path_str = vendored_path.to_string_lossy().to_string();
                            match spawn_agent_process(&vendored_path_str) {
                                Ok(child) => child,
                                Err(e) => {
                                    let err_msg = format!(
                                        "Installed codex-acp at '{}' but failed to spawn it: {}. Install manually from {}",
                                        vendored_path.display(),
                                        e,
                                        CODEX_RELEASES_URL,
                                    );
                                    emit_install_status(&app_handle, "error", err_msg.clone());
                                    let _ = ready_tx.send(Err(err_msg));
                                    return;
                                }
                            }
                        }
                        Err(install_err) => {
                            let err_msg = format!(
                                "Failed to prepare managed codex-acp for neoai: {}. Install manually from {}",
                                install_err, CODEX_RELEASES_URL
                            );
                            emit_install_status(&app_handle, "error", err_msg.clone());
                            let _ = ready_tx.send(Err(err_msg));
                            return;
                        }
                    }
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(format!("Failed to spawn agent '{}': {}", agent_path, e)));
                    return;
                }
            };

            let agent_stdin = match child.stdin.take() {
                Some(stdin) => stdin.compat_write(),
                None => {
                    let err_msg = "Failed to take agent stdin".to_string();
                    emit_install_status(&app_handle, "error", err_msg.clone());
                    let _ = ready_tx.send(Err(err_msg));
                    return;
                }
            };
            let agent_stdout = match child.stdout.take() {
                Some(stdout) => stdout.compat(),
                None => {
                    let err_msg = "Failed to take agent stdout".to_string();
                    emit_install_status(&app_handle, "error", err_msg.clone());
                    let _ = ready_tx.send(Err(err_msg));
                    return;
                }
            };

            let handler = AcpClientHandler {
                app_handle: app_handle.clone(),
                pending_permission_requests: pending_permission_requests.clone(),
                permission_request_counter: permission_request_counter.clone(),
                session_terminal_bindings: session_terminal_bindings.clone(),
            };

            let (conn, io_future) = acp::ClientSideConnection::new(
                handler,
                agent_stdin,
                agent_stdout,
                |fut| {
                    tokio::task::spawn_local(fut);
                },
            );

            // Drive I/O in background
            tokio::task::spawn_local(io_future);

            // Initialize handshake
            let tmux_available = tmux_runtime::detect_tmux_available().await.is_ok();
            let mut capability_meta = acp::Meta::new();
            capability_meta.insert(
                "terminal_output".to_string(),
                serde_json::Value::Bool(tmux_available),
            );
            let init_result = conn
                .initialize(
                    acp::InitializeRequest::new(acp::ProtocolVersion::V1)
                        .client_capabilities(
                            acp::ClientCapabilities::new()
                                .fs(
                                    acp::FileSystemCapability::new()
                                        .read_text_file(true)
                                        .write_text_file(true),
                                )
                                .terminal(tmux_available)
                                .meta(capability_meta),
                        )
                        .client_info(acp::Implementation::new("neoai", "0.1.0").title("neoai Terminal IDE")),
                )
                .await;

            match init_result {
                Ok(resp) => {
                    log::info!(
                        "ACP agent initialized: {:?}",
                        resp.agent_info.as_ref().map(|i| &i.name)
                    );
                    emit_install_status(&app_handle, "done", "AI agent is ready.");
                    let _ = ready_tx.send(Ok(()));
                }
                Err(e) => {
                    let err_msg = format!("ACP initialize failed: {}", e);
                    emit_install_status(&app_handle, "error", err_msg.clone());
                    let _ = ready_tx.send(Err(err_msg));
                    return;
                }
            }

            // Process commands from the Send world
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    AcpCommand::CreateSession {
                        working_dir,
                        terminal_id,
                        reply,
                    } => {
                        let result = conn
                            .new_session(acp::NewSessionRequest::new(working_dir))
                            .await;
                        match result {
                            Ok(resp) => {
                                let sid = resp.session_id.to_string();
                                session_terminal_bindings
                                    .lock()
                                    .await
                                    .insert(sid.clone(), terminal_id);
                                let _ = reply.send(Ok(sid));
                            }
                            Err(e) => {
                                let _ =
                                    reply.send(Err(format!("Failed to create session: {}", e)));
                            }
                        }
                    }
                    AcpCommand::Prompt {
                        session_id,
                        messages,
                        context,
                        reply,
                    } => {
                        let mut prompt_blocks: Vec<acp::ContentBlock> = Vec::new();
                        if let Some(ctx) = context {
                            prompt_blocks.push(ctx.into());
                        }
                        for msg in messages {
                            prompt_blocks.push(msg.into());
                        }

                        let result = conn
                            .prompt(acp::PromptRequest::new(session_id, prompt_blocks))
                            .await;
                        match result {
                            Ok(resp) => {
                                let stop_reason = format!("{:?}", resp.stop_reason);
                                let _ = app_handle.emit(
                                    "acp-event",
                                    &AcpEvent::Done {
                                        stop_reason: stop_reason.clone(),
                                    },
                                );
                                let _ = reply.send(Ok(stop_reason));
                            }
                            Err(e) => {
                                let _ = reply.send(Err(format!("Prompt failed: {}", e)));
                            }
                        }
                    }
                    AcpCommand::Shutdown => {
                        break;
                    }
                }
            }

            let mut pending = pending_permission_requests.lock().await;
            for (_, tx) in pending.drain() {
                let _ = tx.send(acp::RequestPermissionOutcome::Cancelled);
            }
            drop(pending);
            session_terminal_bindings.lock().await.clear();

            // Clean up
            let _ = child.kill().await;
        })
        .await;
}

// -- Managed state --

pub struct AcpClientState {
    cmd_tx: Option<mpsc::Sender<AcpCommand>>,
    worker_handle: Option<std::thread::JoinHandle<()>>,
    status: AgentStatus,
    pending_permission_requests: PendingPermissionRequests,
    permission_request_counter: Arc<AtomicU64>,
    session_terminal_bindings: SessionTerminalBindings,
}

impl AcpClientState {
    pub fn new() -> Self {
        Self {
            cmd_tx: None,
            worker_handle: None,
            status: AgentStatus::Stopped,
            pending_permission_requests: Arc::new(Mutex::new(std::collections::HashMap::new())),
            permission_request_counter: Arc::new(AtomicU64::new(1)),
            session_terminal_bindings: Arc::new(Mutex::new(std::collections::HashMap::new())),
        }
    }
}

// -- Tauri IPC commands --

async fn cancel_pending_permission_requests(
    pending_permission_requests: &PendingPermissionRequests,
) {
    let mut pending = pending_permission_requests.lock().await;
    for (_, tx) in pending.drain() {
        let _ = tx.send(acp::RequestPermissionOutcome::Cancelled);
    }
}

#[tauri::command]
pub async fn acp_start_agent(
    state: tauri::State<'_, Mutex<AcpClientState>>,
    app_handle: tauri::AppHandle,
    agent_path: String,
) -> Result<(), String> {
    let mut acp_state = state.lock().await;

    if acp_state.cmd_tx.is_some() {
        return Err("Agent already running. Stop it first.".to_string());
    }

    acp_state.status = AgentStatus::Starting;

    acp_state.session_terminal_bindings.lock().await.clear();
    cancel_pending_permission_requests(&acp_state.pending_permission_requests).await;

    let (cmd_tx, cmd_rx) = mpsc::channel::<AcpCommand>(32);
    let (ready_tx, ready_rx) = oneshot::channel();

    let handle = app_handle.clone();
    let path = agent_path.clone();
    let pending_permission_requests = acp_state.pending_permission_requests.clone();
    let permission_request_counter = acp_state.permission_request_counter.clone();
    let session_terminal_bindings = acp_state.session_terminal_bindings.clone();

    // Spawn a dedicated thread with its own tokio runtime + LocalSet
    let worker_handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed to create ACP worker runtime");

        rt.block_on(acp_worker(
            handle,
            path,
            pending_permission_requests,
            permission_request_counter,
            session_terminal_bindings,
            cmd_rx,
            ready_tx,
        ));
    });

    // Wait for initialization to complete
    let init_result = ready_rx
        .await
        .map_err(|_| "Worker thread died".to_string())?;

    match init_result {
        Ok(()) => {
            acp_state.cmd_tx = Some(cmd_tx);
            acp_state.worker_handle = Some(worker_handle);
            acp_state.status = AgentStatus::Running;
            Ok(())
        }
        Err(e) => {
            acp_state.status = AgentStatus::Error(e.clone());
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn acp_stop_agent(state: tauri::State<'_, Mutex<AcpClientState>>) -> Result<(), String> {
    let (pending_permission_requests, session_terminal_bindings, tx, handle) = {
        let mut acp_state = state.lock().await;
        (
            acp_state.pending_permission_requests.clone(),
            acp_state.session_terminal_bindings.clone(),
            acp_state.cmd_tx.take(),
            acp_state.worker_handle.take(),
        )
    };

    cancel_pending_permission_requests(&pending_permission_requests).await;
    session_terminal_bindings.lock().await.clear();

    if let Some(tx) = tx {
        let _ = tx.send(AcpCommand::Shutdown).await;
    }

    // The worker thread will exit after processing Shutdown
    if let Some(handle) = handle {
        let _ = handle.join();
    }

    let mut acp_state = state.lock().await;
    acp_state.status = AgentStatus::Stopped;
    Ok(())
}

#[tauri::command]
pub async fn acp_agent_status(
    state: tauri::State<'_, Mutex<AcpClientState>>,
) -> Result<AgentStatus, String> {
    let acp_state = state.lock().await;
    Ok(acp_state.status.clone())
}

#[tauri::command]
pub async fn acp_create_session(
    state: tauri::State<'_, Mutex<AcpClientState>>,
    working_dir: String,
    terminal_id: String,
) -> Result<String, String> {
    let tx = {
        let acp_state = state.lock().await;
        acp_state
            .cmd_tx
            .as_ref()
            .cloned()
            .ok_or("No agent running")?
    };

    let (reply_tx, reply_rx) = oneshot::channel();

    tx.send(AcpCommand::CreateSession {
        working_dir: PathBuf::from(&working_dir),
        terminal_id,
        reply: reply_tx,
    })
    .await
    .map_err(|_| "Agent worker died".to_string())?;

    reply_rx
        .await
        .map_err(|_| "Agent worker died".to_string())?
}

#[tauri::command]
pub async fn acp_unbind_terminal(
    state: tauri::State<'_, Mutex<AcpClientState>>,
    terminal_id: String,
) -> Result<(), String> {
    let session_terminal_bindings = {
        let acp_state = state.lock().await;
        acp_state.session_terminal_bindings.clone()
    };

    let mut bindings = session_terminal_bindings.lock().await;
    bindings.retain(|_, bound_terminal_id| bound_terminal_id != &terminal_id);
    Ok(())
}

#[tauri::command]
pub async fn acp_send_prompt(
    state: tauri::State<'_, Mutex<AcpClientState>>,
    _app_handle: tauri::AppHandle,
    session_id: String,
    messages: Vec<String>,
    context: Option<String>,
) -> Result<String, String> {
    let tx = {
        let acp_state = state.lock().await;
        acp_state
            .cmd_tx
            .as_ref()
            .cloned()
            .ok_or("No agent running")?
    };

    let (reply_tx, reply_rx) = oneshot::channel();

    tx.send(AcpCommand::Prompt {
        session_id,
        messages,
        context,
        reply: reply_tx,
    })
    .await
    .map_err(|_| "Agent worker died".to_string())?;

    reply_rx
        .await
        .map_err(|_| "Agent worker died".to_string())?
}

#[tauri::command]
pub async fn acp_respond_permission_request(
    state: tauri::State<'_, Mutex<AcpClientState>>,
    request_id: String,
    option_id: Option<String>,
) -> Result<(), String> {
    let acp_state = state.lock().await;
    let pending_permission_requests = acp_state.pending_permission_requests.clone();
    drop(acp_state);

    let tx = pending_permission_requests
        .lock()
        .await
        .remove(&request_id)
        .ok_or_else(|| format!("Unknown permission request: {}", request_id))?;

    let outcome = match option_id {
        Some(option_id) => {
            acp::RequestPermissionOutcome::Selected(acp::SelectedPermissionOutcome::new(option_id))
        }
        None => acp::RequestPermissionOutcome::Cancelled,
    };

    tx.send(outcome)
        .map_err(|_| "Permission request is no longer active".to_string())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_default_agent_paths() {
        assert!(is_default_agent_path("codex-acp"));
        assert!(is_default_agent_path("codex-acp.exe"));
        assert!(!is_default_agent_path("/usr/local/bin/codex-acp"));
    }

    #[test]
    fn resolves_release_assets_for_known_targets() {
        let mac = resolve_codex_asset_for("macos", "aarch64", None).expect("missing mac asset");
        assert_eq!(mac.target, "aarch64-apple-darwin");
        assert_eq!(mac.archive, ArchiveFormat::TarGz);

        let linux =
            resolve_codex_asset_for("linux", "x86_64", Some("gnu")).expect("missing linux asset");
        assert_eq!(linux.target, "x86_64-unknown-linux-gnu");
        assert_eq!(linux.archive, ArchiveFormat::TarGz);

        let windows =
            resolve_codex_asset_for("windows", "x86_64", None).expect("missing windows asset");
        assert_eq!(windows.target, "x86_64-pc-windows-msvc");
        assert_eq!(windows.archive, ArchiveFormat::Zip);
    }

    #[test]
    fn checksum_verification_detects_mismatch() {
        let abc_sha256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert!(verify_sha256(b"abc", abc_sha256).is_ok());
        assert!(verify_sha256(b"abc", "deadbeef").is_err());
    }
}
