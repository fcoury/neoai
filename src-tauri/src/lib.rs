mod acp_client;
mod app_config;
mod ghostty_embed;
mod nvim_bridge;
mod socket_manager;
mod tmux_runtime;

use ghostty_embed::{with_manager, GhosttyOptions, GhosttyRect};
use socket_manager::SocketManager;
use tauri::Manager;
use tokio::sync::Mutex;

#[tauri::command]
fn ghostty_create(
    window: tauri::Window,
    id: String,
    rect: GhosttyRect,
    options: Option<GhosttyOptions>,
) -> Result<(), String> {
    let options = options.unwrap_or_default();
    let (tx, rx) = std::sync::mpsc::channel();
    let window_clone = window.clone();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.create(&window_clone, id, rect, options));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_create failed".to_string()))
}

#[tauri::command]
fn ghostty_update_rect(window: tauri::Window, id: String, rect: GhosttyRect) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let window_clone = window.clone();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.update_rect(&window_clone, &id, rect));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_update_rect failed".to_string()))
}

#[tauri::command]
fn ghostty_destroy(window: tauri::Window, id: String) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.destroy(&id));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_destroy failed".to_string()))
}

#[tauri::command]
fn ghostty_set_visible(window: tauri::Window, id: String, visible: bool) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.set_visible(&id, visible));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_set_visible failed".to_string()))
}

#[tauri::command]
fn ghostty_focus(window: tauri::Window, id: String, focused: bool) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.focus(&id, focused));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_focus failed".to_string()))
}

#[tauri::command]
fn ghostty_write_text(window: tauri::Window, id: String, text: String) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();

    window
        .run_on_main_thread(move || {
            let res = with_manager(|manager| manager.write_text(&id, &text));
            let _ = tx.send(res);
        })
        .map_err(|e| e.to_string())?;

    rx.recv()
        .unwrap_or_else(|_| Err("ghostty_write_text failed".to_string()))
}

#[tauri::command]
async fn get_socket_path(
    state: tauri::State<'_, std::sync::Mutex<SocketManager>>,
    terminal_id: String,
) -> Result<String, String> {
    let mut mgr = state.lock().map_err(|e| e.to_string())?;
    let path = mgr.socket_path(&terminal_id);
    mgr.register(path.clone());
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
async fn remove_socket_path(
    state: tauri::State<'_, std::sync::Mutex<SocketManager>>,
    tmux_state: tauri::State<'_, Mutex<tmux_runtime::TmuxRuntimeState>>,
    terminal_id: String,
) -> Result<(), String> {
    {
        let mut mgr = state.lock().map_err(|e| e.to_string())?;
        let path = mgr.socket_path(&terminal_id);
        mgr.remove_socket(&path);
    }

    let (session_name, pane_ids) = {
        let mut tmux = tmux_state.lock().await;
        tmux.remove_terminal(&terminal_id)
    };
    for pane_id in pane_ids {
        let _ = tmux_runtime::kill_pane(&pane_id).await;
    }
    if let Some(session_name) = session_name {
        let _ = tmux_runtime::kill_session(&session_name).await;
    }

    Ok(())
}

#[tauri::command]
async fn tmux_status(
    tmux_state: tauri::State<'_, Mutex<tmux_runtime::TmuxRuntimeState>>,
    terminal_id: String,
) -> Result<tmux_runtime::TmuxStatus, String> {
    let availability = tmux_runtime::detect_tmux_available().await;
    let available = availability.is_ok();
    let error = availability.err();
    let mut tmux = tmux_state.lock().await;
    Ok(tmux.snapshot_for_terminal(&terminal_id, available, error))
}

#[tauri::command]
async fn tmux_enable_for_terminal(
    tmux_state: tauri::State<'_, Mutex<tmux_runtime::TmuxRuntimeState>>,
    terminal_id: String,
    enabled: bool,
) -> Result<tmux_runtime::TmuxStatus, String> {
    let availability = tmux_runtime::detect_tmux_available().await;
    let available = availability.is_ok();
    let error = availability.err();
    let mut tmux = tmux_state.lock().await;
    tmux.set_terminal_enabled(&terminal_id, enabled);
    Ok(tmux.snapshot_for_terminal(&terminal_id, available, error))
}

#[tauri::command]
async fn nvim_start_in_tmux(
    window: tauri::Window,
    tmux_state: tauri::State<'_, Mutex<tmux_runtime::TmuxRuntimeState>>,
    terminal_id: String,
    socket_path: String,
    cwd: Option<String>,
    allow_fallback: Option<bool>,
) -> Result<tmux_runtime::StartNvimResult, String> {
    let allow_fallback = allow_fallback.unwrap_or(false);

    let (tmux_enabled, assigned_session_name, assigned_names) = {
        let mut tmux = tmux_state.lock().await;
        (
            tmux.terminal_enabled(&terminal_id),
            tmux.session_name(&terminal_id),
            tmux.assigned_session_names(),
        )
    };

    if !tmux_enabled {
        ghostty_write_text(
            window,
            terminal_id,
            format!("nvim --listen {socket_path}\n"),
        )?;
        return Ok(tmux_runtime::StartNvimResult {
            launch_mode: "direct".to_string(),
            session_name: None,
            message: "Started Neovim without tmux for this terminal.".to_string(),
        });
    }

    let tmux_available = match tmux_runtime::detect_tmux_available().await {
        Ok(()) => true,
        Err(err) => {
            if !allow_fallback {
                return Ok(tmux_runtime::StartNvimResult {
                    launch_mode: "tmuxUnavailable".to_string(),
                    session_name: None,
                    message: format!(
                        "tmux is not available. Install tmux or continue without tmux. ({err})"
                    ),
                });
            }
            false
        }
    };

    if tmux_available {
        let cwd_path = cwd.as_deref().map(std::path::Path::new);
        let session_name = if let Some(existing) = assigned_session_name {
            existing
        } else {
            let base_name = tmux_runtime::session_base_name(cwd_path, &terminal_id);
            let chosen =
                tmux_runtime::find_available_session_name(&base_name, &assigned_names).await?;
            let mut tmux = tmux_state.lock().await;
            tmux.set_session_name(&terminal_id, chosen.clone());
            chosen
        };

        tmux_runtime::prepare_nvim_window(&session_name, &socket_path, cwd_path).await?;
        ghostty_write_text(
            window,
            terminal_id,
            format!("tmux new-session -A -s {session_name}\n"),
        )?;

        return Ok(tmux_runtime::StartNvimResult {
            launch_mode: "tmux".to_string(),
            session_name: Some(session_name),
            message: "Started Neovim inside tmux session.".to_string(),
        });
    }

    ghostty_write_text(
        window,
        terminal_id,
        format!("nvim --listen {socket_path}\n"),
    )?;
    Ok(tmux_runtime::StartNvimResult {
        launch_mode: "direct".to_string(),
        session_name: None,
        message: "Started Neovim without tmux fallback.".to_string(),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Clean up sockets left behind by crashed instances
    SocketManager::cleanup_stale();

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Mutex::new(nvim_bridge::NvimBridgeState::new()))
        .manage(Mutex::new(acp_client::AcpClientState::new()))
        .manage(std::sync::Mutex::new(app_config::AppConfigState::default()))
        .manage(Mutex::new(tmux_runtime::TmuxRuntimeState::new()))
        .manage(std::sync::Mutex::new(SocketManager::new()))
        .invoke_handler(tauri::generate_handler![
            // Ghostty
            ghostty_create,
            ghostty_update_rect,
            ghostty_destroy,
            ghostty_set_visible,
            ghostty_focus,
            ghostty_write_text,
            // Neovim bridge
            nvim_bridge::nvim_connect,
            nvim_bridge::nvim_disconnect,
            nvim_bridge::nvim_connection_status,
            nvim_bridge::nvim_probe_health,
            nvim_bridge::nvim_reinject_keymaps,
            nvim_bridge::nvim_get_context,
            nvim_bridge::nvim_get_diagnostics,
            nvim_bridge::nvim_get_buffer_content,
            nvim_bridge::nvim_apply_edit,
            nvim_bridge::nvim_apply_edits,
            nvim_bridge::nvim_exec_command,
            // ACP agent
            acp_client::acp_start_agent,
            acp_client::acp_stop_agent,
            acp_client::acp_agent_status,
            acp_client::acp_create_session,
            acp_client::acp_send_prompt,
            acp_client::acp_respond_permission_request,
            // tmux
            tmux_status,
            tmux_enable_for_terminal,
            nvim_start_in_tmux,
            // Socket management
            get_socket_path,
            remove_socket_path,
        ]);

    #[cfg(all(debug_assertions, feature = "mcp-debug"))]
    {
        builder = builder.plugin(tauri_plugin_mcp_bridge::init());
    }

    let app = builder
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    if let Some(config_state) = app.try_state::<std::sync::Mutex<app_config::AppConfigState>>() {
        match config_state.lock() {
            Ok(mut state) => {
                if let Err(err) = state.initialize(&app.handle()) {
                    log::warn!("Failed to initialize NeoAI config.toml: {}", err);
                } else if let Some(path) = state.config_path() {
                    log::info!("Loaded NeoAI configuration from '{}'", path.display());
                }
            }
            Err(_) => {
                log::warn!("Failed to lock NeoAI app config state");
            }
        }
    }

    app.run(|_handle, event| {
        if let tauri::RunEvent::Exit = event {
            if let Some(state) = _handle.try_state::<std::sync::Mutex<SocketManager>>() {
                if let Ok(mut mgr) = state.inner().lock() {
                    mgr.cleanup_all();
                }
            }
        }
    });
}
