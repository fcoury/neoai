use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};
use serde::{Deserialize, Serialize};

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DbProject {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub is_expanded: bool,
    pub folders: Vec<DbFolder>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DbFolder {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub path: String,
    pub branch: String,
    pub is_active: bool,
    pub screenshot_path: Option<String>,
    pub last_used_at: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DbChatMessage {
    pub id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    pub system_kind: Option<String>,
    pub context: Option<serde_json::Value>,
    pub diagnostics: Option<serde_json::Value>,
    pub proposed_edits: Option<serde_json::Value>,
    pub edit_status: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapState {
    pub projects: Vec<DbProject>,
    pub active_folder_id: Option<String>,
    pub settings: HashMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MigrationPayload {
    pub projects: Option<Vec<MigrationProject>>,
    pub active_folder_id: Option<String>,
    pub chat_messages: Option<Vec<DbChatMessage>>,
    pub auto_apply: Option<bool>,
    pub sidebar_width: Option<f64>,
    pub active_panel: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MigrationProject {
    pub id: String,
    pub name: String,
    pub root_path: String,
    pub folders: Vec<MigrationFolder>,
    pub is_expanded: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MigrationFolder {
    pub id: String,
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub is_active: Option<bool>,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self, String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create database directory '{}': {e}",
                    parent.display()
                )
            })?;
        }

        let conn = Connection::open(path)
            .map_err(|e| format!("Failed to open sqlite database '{}': {e}", path.display()))?;

        conn.execute_batch(
            r#"
            PRAGMA foreign_keys = ON;
            PRAGMA journal_mode = WAL;

            CREATE TABLE IF NOT EXISTS projects (
              id TEXT PRIMARY KEY,
              name TEXT NOT NULL,
              root_path TEXT NOT NULL UNIQUE,
              is_expanded INTEGER NOT NULL DEFAULT 1,
              created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );

            CREATE TABLE IF NOT EXISTS folders (
              id TEXT PRIMARY KEY,
              project_id TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
              name TEXT NOT NULL,
              path TEXT NOT NULL UNIQUE,
              branch TEXT NOT NULL DEFAULT '',
              is_active INTEGER NOT NULL DEFAULT 0,
              screenshot_path TEXT,
              last_used_at INTEGER,
              created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );

            CREATE TABLE IF NOT EXISTS chat_messages (
              id TEXT PRIMARY KEY,
              folder_id TEXT NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
              role TEXT NOT NULL CHECK(role IN ('user', 'assistant', 'system')),
              content TEXT NOT NULL,
              timestamp INTEGER NOT NULL,
              system_kind TEXT,
              context_json TEXT,
              diagnostics_json TEXT,
              proposed_edits_json TEXT,
              edit_status TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_chat_folder_ts ON chat_messages(folder_id, timestamp);
            CREATE INDEX IF NOT EXISTS idx_folders_project ON folders(project_id);
            CREATE INDEX IF NOT EXISTS idx_folders_last_used ON folders(last_used_at DESC);

            CREATE TABLE IF NOT EXISTS settings (
              key TEXT PRIMARY KEY,
              value TEXT NOT NULL
            );
            "#,
        )
        .map_err(|e| format!("Failed to initialize sqlite schema: {e}"))?;

        Ok(Self { conn })
    }

    fn parse_project_row(row: &Row<'_>) -> Result<DbProject, rusqlite::Error> {
        Ok(DbProject {
            id: row.get(0)?,
            name: row.get(1)?,
            root_path: row.get(2)?,
            is_expanded: row.get::<_, i64>(3)? != 0,
            folders: Vec::new(),
        })
    }

    fn parse_folder_row(row: &Row<'_>) -> Result<DbFolder, rusqlite::Error> {
        Ok(DbFolder {
            id: row.get(0)?,
            project_id: row.get(1)?,
            name: row.get(2)?,
            path: row.get(3)?,
            branch: row.get(4)?,
            is_active: row.get::<_, i64>(5)? != 0,
            screenshot_path: row.get(6)?,
            last_used_at: row.get(7)?,
        })
    }

    fn parse_message_row(row: &Row<'_>) -> Result<DbChatMessage, rusqlite::Error> {
        let parse_json = |raw: Option<String>| {
            raw.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        };

        Ok(DbChatMessage {
            id: row.get(0)?,
            role: row.get(1)?,
            content: row.get(2)?,
            timestamp: row.get(3)?,
            system_kind: row.get(4)?,
            context: parse_json(row.get(5)?),
            diagnostics: parse_json(row.get(6)?),
            proposed_edits: parse_json(row.get(7)?),
            edit_status: row.get(8)?,
        })
    }

    fn load_projects_internal(conn: &Connection) -> Result<Vec<DbProject>, String> {
        let mut projects_stmt = conn
            .prepare(
                "SELECT id, name, root_path, is_expanded FROM projects ORDER BY created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare projects query: {e}"))?;

        let mut projects: Vec<DbProject> = projects_stmt
            .query_map([], Self::parse_project_row)
            .map_err(|e| format!("Failed to query projects: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to parse projects: {e}"))?;

        let mut folders_stmt = conn
            .prepare(
                "SELECT id, project_id, name, path, branch, is_active, screenshot_path, last_used_at
                 FROM folders
                 ORDER BY COALESCE(last_used_at, 0) DESC, created_at ASC",
            )
            .map_err(|e| format!("Failed to prepare folders query: {e}"))?;

        let folders: Vec<DbFolder> = folders_stmt
            .query_map([], Self::parse_folder_row)
            .map_err(|e| format!("Failed to query folders: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to parse folders: {e}"))?;

        for folder in folders {
            if let Some(project) = projects
                .iter_mut()
                .find(|project| project.id == folder.project_id)
            {
                project.folders.push(folder);
            }
        }

        Ok(projects)
    }

    fn active_folder_id_internal(conn: &Connection) -> Result<Option<String>, String> {
        conn.query_row(
            "SELECT id FROM folders WHERE is_active = 1 LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to read active folder: {e}"))
    }

    fn load_settings_internal(conn: &Connection) -> Result<HashMap<String, String>, String> {
        let mut stmt = conn
            .prepare("SELECT key, value FROM settings")
            .map_err(|e| format!("Failed to prepare settings query: {e}"))?;

        let rows = stmt
            .query_map([], |row| {
                let key: String = row.get(0)?;
                let value: String = row.get(1)?;
                Ok((key, value))
            })
            .map_err(|e| format!("Failed to query settings: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to parse settings: {e}"))?;

        Ok(rows.into_iter().collect())
    }

    fn to_json_string(value: &Option<serde_json::Value>) -> Result<Option<String>, String> {
        match value {
            Some(v) => serde_json::to_string(v)
                .map(Some)
                .map_err(|e| format!("Failed to serialize JSON field: {e}")),
            None => Ok(None),
        }
    }

    fn insert_message_internal(
        tx: &Transaction<'_>,
        folder_id: &str,
        message: &DbChatMessage,
    ) -> Result<(), String> {
        let context_json = Self::to_json_string(&message.context)?;
        let diagnostics_json = Self::to_json_string(&message.diagnostics)?;
        let proposed_edits_json = Self::to_json_string(&message.proposed_edits)?;

        tx.execute(
            r#"
            INSERT OR REPLACE INTO chat_messages (
              id, folder_id, role, content, timestamp, system_kind, context_json, diagnostics_json, proposed_edits_json, edit_status
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
            "#,
            params![
                message.id,
                folder_id,
                message.role,
                message.content,
                message.timestamp,
                message.system_kind,
                context_json,
                diagnostics_json,
                proposed_edits_json,
                message.edit_status,
            ],
        )
        .map_err(|e| format!("Failed to save message '{}': {e}", message.id))?;

        Ok(())
    }

    fn insert_migration_project(
        tx: &Transaction<'_>,
        project: &MigrationProject,
    ) -> Result<(), String> {
        tx.execute(
            "INSERT OR IGNORE INTO projects (id, name, root_path, is_expanded) VALUES (?1, ?2, ?3, ?4)",
            params![
                project.id,
                project.name,
                project.root_path,
                if project.is_expanded.unwrap_or(true) { 1 } else { 0 }
            ],
        )
        .map_err(|e| format!("Failed to migrate project '{}': {e}", project.id))?;

        for folder in &project.folders {
            tx.execute(
                r#"
                INSERT OR IGNORE INTO folders (
                  id, project_id, name, path, branch, is_active
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                "#,
                params![
                    folder.id,
                    project.id,
                    folder.name,
                    folder.path,
                    folder.branch.clone().unwrap_or_default(),
                    if folder.is_active.unwrap_or(false) {
                        1
                    } else {
                        0
                    }
                ],
            )
            .map_err(|e| format!("Failed to migrate folder '{}': {e}", folder.id))?;
        }

        Ok(())
    }

    fn set_active_folder_internal(
        conn: &Connection,
        folder_id: Option<&str>,
    ) -> Result<(), String> {
        conn.execute("UPDATE folders SET is_active = 0", [])
            .map_err(|e| format!("Failed to clear active folder flags: {e}"))?;

        if let Some(folder_id) = folder_id {
            conn.execute(
                "UPDATE folders SET is_active = 1 WHERE id = ?1",
                params![folder_id],
            )
            .map_err(|e| format!("Failed to set active folder '{folder_id}': {e}"))?;
        }

        Ok(())
    }
}

pub fn resolve_app_root_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let root = PathBuf::from(home).join(".neoai");
    fs::create_dir_all(&root)
        .map_err(|e| format!("Failed to create app root '{}': {e}", root.display()))?;
    Ok(root)
}

pub fn resolve_db_path() -> Result<PathBuf, String> {
    Ok(resolve_app_root_dir()?.join("neoai.db"))
}

#[tauri::command]
pub fn db_bootstrap_state(
    state: tauri::State<'_, Mutex<Database>>,
) -> Result<BootstrapState, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    let projects = Database::load_projects_internal(&guard.conn)?;
    let active_folder_id = Database::active_folder_id_internal(&guard.conn)?;
    let settings = Database::load_settings_internal(&guard.conn)?;

    Ok(BootstrapState {
        projects,
        active_folder_id,
        settings,
    })
}

#[tauri::command]
pub fn db_load_projects(
    state: tauri::State<'_, Mutex<Database>>,
) -> Result<Vec<DbProject>, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    Database::load_projects_internal(&guard.conn)
}

#[tauri::command]
pub fn db_add_project(
    state: tauri::State<'_, Mutex<Database>>,
    id: String,
    name: String,
    root_path: String,
    folder_id: String,
    folder_name: String,
    folder_path: String,
) -> Result<DbProject, String> {
    let mut guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    let tx = guard
        .conn
        .transaction()
        .map_err(|e| format!("Failed to open transaction: {e}"))?;

    tx.execute(
        "INSERT INTO projects (id, name, root_path, is_expanded) VALUES (?1, ?2, ?3, 1)",
        params![id, name, root_path],
    )
    .map_err(|e| format!("Failed to add project: {e}"))?;

    tx.execute(
        "INSERT INTO folders (id, project_id, name, path, branch, is_active) VALUES (?1, ?2, ?3, ?4, '', 0)",
        params![folder_id, id, folder_name, folder_path],
    )
    .map_err(|e| format!("Failed to add initial folder: {e}"))?;

    tx.commit()
        .map_err(|e| format!("Failed to commit project insert: {e}"))?;

    let projects = Database::load_projects_internal(&guard.conn)?;
    projects
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| "Project inserted but not found".to_string())
}

#[tauri::command]
pub fn db_remove_project(
    state: tauri::State<'_, Mutex<Database>>,
    project_id: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute("DELETE FROM projects WHERE id = ?1", params![project_id])
        .map_err(|e| format!("Failed to remove project: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_toggle_project(
    state: tauri::State<'_, Mutex<Database>>,
    project_id: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute(
            "UPDATE projects SET is_expanded = CASE WHEN is_expanded = 1 THEN 0 ELSE 1 END WHERE id = ?1",
            params![project_id],
        )
        .map_err(|e| format!("Failed to toggle project expansion: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_add_folder(
    state: tauri::State<'_, Mutex<Database>>,
    id: String,
    project_id: String,
    name: String,
    path: String,
) -> Result<DbFolder, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute(
            "INSERT INTO folders (id, project_id, name, path, branch, is_active) VALUES (?1, ?2, ?3, ?4, '', 0)",
            params![id, project_id, name, path],
        )
        .map_err(|e| format!("Failed to add folder: {e}"))?;

    let mut stmt = guard
        .conn
        .prepare(
            "SELECT id, project_id, name, path, branch, is_active, screenshot_path, last_used_at FROM folders WHERE id = ?1",
        )
        .map_err(|e| format!("Failed to prepare folder query: {e}"))?;

    stmt.query_row(params![id], Database::parse_folder_row)
        .map_err(|e| format!("Failed to load inserted folder: {e}"))
}

#[tauri::command]
pub fn db_remove_folder(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute("DELETE FROM folders WHERE id = ?1", params![folder_id])
        .map_err(|e| format!("Failed to remove folder: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_set_active_folder(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: Option<String>,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    Database::set_active_folder_internal(&guard.conn, folder_id.as_deref())
}

#[tauri::command]
pub fn db_load_messages(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: String,
) -> Result<Vec<DbChatMessage>, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    let mut stmt = guard
        .conn
        .prepare(
            r#"
            SELECT id, role, content, timestamp, system_kind, context_json, diagnostics_json, proposed_edits_json, edit_status
            FROM chat_messages
            WHERE folder_id = ?1
            ORDER BY timestamp ASC
            LIMIT 200
            "#,
        )
        .map_err(|e| format!("Failed to prepare messages query: {e}"))?;

    let rows = stmt
        .query_map(params![folder_id], Database::parse_message_row)
        .map_err(|e| format!("Failed to query messages: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to parse messages: {e}"))?;

    Ok(rows)
}

#[tauri::command]
pub fn db_save_message(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: String,
    message: DbChatMessage,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    let tx = guard
        .conn
        .transaction()
        .map_err(|e| format!("Failed to open transaction: {e}"))?;
    Database::insert_message_internal(&tx, &folder_id, &message)?;
    tx.commit()
        .map_err(|e| format!("Failed to commit message save: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_update_message(
    state: tauri::State<'_, Mutex<Database>>,
    message_id: String,
    content: Option<String>,
    edit_status: Option<String>,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;

    if content.is_none() && edit_status.is_none() {
        return Ok(());
    }

    guard
        .conn
        .execute(
            "UPDATE chat_messages SET content = COALESCE(?1, content), edit_status = COALESCE(?2, edit_status) WHERE id = ?3",
            params![content, edit_status, message_id],
        )
        .map_err(|e| format!("Failed to update message: {e}"))?;

    Ok(())
}

#[tauri::command]
pub fn db_clear_messages(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute(
            "DELETE FROM chat_messages WHERE folder_id = ?1",
            params![folder_id],
        )
        .map_err(|e| format!("Failed to clear messages: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_update_folder_session(
    state: tauri::State<'_, Mutex<Database>>,
    folder_id: String,
    screenshot_path: Option<String>,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute(
            "UPDATE folders SET screenshot_path = ?1, last_used_at = unixepoch() WHERE id = ?2",
            params![screenshot_path, folder_id],
        )
        .map_err(|e| format!("Failed to update folder session metadata: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_get_setting(
    state: tauri::State<'_, Mutex<Database>>,
    key: String,
) -> Result<Option<String>, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .query_row(
            "SELECT value FROM settings WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|e| format!("Failed to get setting: {e}"))
}

#[tauri::command]
pub fn db_set_setting(
    state: tauri::State<'_, Mutex<Database>>,
    key: String,
    value: String,
) -> Result<(), String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    guard
        .conn
        .execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )
        .map_err(|e| format!("Failed to set setting: {e}"))?;
    Ok(())
}

#[tauri::command]
pub fn db_get_all_settings(
    state: tauri::State<'_, Mutex<Database>>,
) -> Result<HashMap<String, String>, String> {
    let guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;
    Database::load_settings_internal(&guard.conn)
}

#[tauri::command]
pub fn db_migrate_from_localstorage(
    state: tauri::State<'_, Mutex<Database>>,
    payload: MigrationPayload,
) -> Result<(), String> {
    let mut guard = state.lock().map_err(|e| format!("DB lock poisoned: {e}"))?;

    let tx = guard
        .conn
        .transaction()
        .map_err(|e| format!("Failed to open migration transaction: {e}"))?;

    let projects = payload.projects.unwrap_or_default();
    for project in &projects {
        Database::insert_migration_project(&tx, project)?;
    }

    let mut available_folders = Vec::new();
    for project in &projects {
        for folder in &project.folders {
            available_folders.push(folder.id.clone());
        }
    }

    let chosen_active_folder = payload
        .active_folder_id
        .clone()
        .or_else(|| available_folders.first().cloned());

    tx.execute("UPDATE folders SET is_active = 0", [])
        .map_err(|e| format!("Failed to clear active folders during migration: {e}"))?;

    if let Some(active_folder_id) = &chosen_active_folder {
        tx.execute(
            "UPDATE folders SET is_active = 1 WHERE id = ?1",
            params![active_folder_id],
        )
        .map_err(|e| format!("Failed to set active folder during migration: {e}"))?;
    }

    if let Some(sidebar_width) = payload.sidebar_width {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES ('sidebar_width', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![sidebar_width.to_string()],
        )
        .map_err(|e| format!("Failed to migrate sidebar width: {e}"))?;
    }

    if let Some(active_panel) = payload.active_panel {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES ('active_panel', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![active_panel],
        )
        .map_err(|e| format!("Failed to migrate active panel setting: {e}"))?;
    }

    if let Some(auto_apply) = payload.auto_apply {
        tx.execute(
            "INSERT INTO settings (key, value) VALUES ('auto_apply', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![if auto_apply { "true" } else { "false" }],
        )
        .map_err(|e| format!("Failed to migrate auto_apply setting: {e}"))?;
    }

    let existing_folder_ids: HashSet<String> = {
        let mut stmt = tx
            .prepare("SELECT id FROM folders")
            .map_err(|e| format!("Failed to prepare folder id query: {e}"))?;
        let ids = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query folder ids: {e}"))?
            .collect::<Result<HashSet<_>, _>>()
            .map_err(|e| format!("Failed to parse folder ids: {e}"))?;
        ids
    };

    let chat_messages = payload.chat_messages.unwrap_or_default();
    let target_folder_id =
        chosen_active_folder.or_else(|| existing_folder_ids.iter().next().cloned());

    if let Some(target_folder_id) = target_folder_id {
        if existing_folder_ids.contains(&target_folder_id) {
            for message in &chat_messages {
                Database::insert_message_internal(&tx, &target_folder_id, message)?;
            }
        }
    } else if !chat_messages.is_empty() {
        let legacy_blob = serde_json::to_string(&chat_messages)
            .map_err(|e| format!("Failed to serialize legacy chat messages: {e}"))?;
        tx.execute(
            "INSERT INTO settings (key, value) VALUES ('legacy_chat_messages', ?1) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![legacy_blob],
        )
        .map_err(|e| format!("Failed to persist legacy chat messages: {e}"))?;
    }

    tx.commit()
        .map_err(|e| format!("Failed to commit migration transaction: {e}"))?;

    Ok(())
}
