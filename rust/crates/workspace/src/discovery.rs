use anyhow::{Result, bail};
use rusqlite::{Connection, OpenFlags};
use serde_json::Value;
use std::{env, fs, path::PathBuf, process::Command, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    Zed,
    VsCode,
}

impl Editor {
    pub fn id(self) -> &'static str {
        match self {
            Self::Zed => "zed",
            Self::VsCode => "vscode",
        }
    }
    pub fn display(self) -> &'static str {
        match self {
            Self::Zed => "Zed",
            Self::VsCode => "VS Code",
        }
    }
}

fn directory(value: impl Into<PathBuf>) -> Option<PathBuf> {
    let path = fs::canonicalize(value.into()).ok()?;
    path.is_dir().then_some(path)
}

pub fn hidden_command(program: &str) -> Command {
    let mut command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
}

fn zed_database() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZS_ZED_DB") {
        return Some(path.into());
    }
    let base = if cfg!(windows) {
        PathBuf::from(env::var_os("LOCALAPPDATA")?).join("Zed")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(env::var_os("HOME")?).join("Library/Application Support/Zed")
    } else {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))?
            .join("zed")
    };
    Some(base.join("db/0-stable/db.sqlite"))
}

fn zed_folder() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZS_ZED_WORKSPACE") {
        return directory(path);
    }
    let database = zed_database()?;
    if !database.is_file() {
        return None;
    }
    let connection =
        Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
    connection.busy_timeout(Duration::from_secs(1)).ok()?;
    let (paths, remote): (String, Option<rusqlite::types::Value>) = connection.query_row("SELECT paths, remote_connection_id FROM workspaces ORDER BY timestamp DESC, workspace_id DESC LIMIT 1", [], |row| Ok((row.get(0)?, row.get(1)?))).ok()?;
    if remote.is_some() {
        return None;
    }
    let mut paths = paths.lines();
    let path = paths.next()?;
    if path.trim().is_empty() || paths.next().is_some() {
        return None;
    }
    let path = PathBuf::from(path);
    if !path.is_absolute() {
        return None;
    }
    directory(path)
}

fn vscode_folder() -> Option<PathBuf> {
    if let Some(path) = env::var_os("ZS_WORKSPACE").or_else(|| env::var_os("VSCODE_WORKSPACE")) {
        if let Some(path) = directory(path) {
            return Some(path);
        }
    }
    #[cfg(windows)]
    {
        let output = hidden_command("tasklist").arg("/NH").output().ok()?;
        let text = String::from_utf8_lossy(&output.stdout);
        if !output.status.success()
            || !["Code.exe", "Code - Insiders.exe", "VSCodium.exe"]
                .iter()
                .any(|name| text.contains(name))
        {
            return None;
        }
    }
    let base = if cfg!(windows) {
        PathBuf::from(env::var_os("APPDATA")?)
    } else if cfg!(target_os = "macos") {
        PathBuf::from(env::var_os("HOME")?).join("Library/Application Support")
    } else {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))?
    };
    for product in [
        "Code",
        "Code - Insiders",
        "VSCodium",
        "codium",
        "Code - OSS",
    ] {
        let Ok(text) =
            fs::read_to_string(base.join(product).join("User/globalStorage/storage.json"))
        else {
            continue;
        };
        let Ok(data) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let state = &data["windowsState"];
        let mut folders = Vec::new();
        if let Some(folder) = state["lastActiveWindow"]["folder"].as_str() {
            folders.push(folder);
        }
        if let Some(windows) = state["openedWindows"].as_array() {
            folders.extend(
                windows
                    .iter()
                    .filter_map(|window| window["folder"].as_str()),
            );
        }
        for folder in folders {
            let Ok(uri) = url::Url::parse(folder) else {
                continue;
            };
            let Ok(path) = uri.to_file_path() else {
                continue;
            };
            if let Some(path) = directory(path) {
                return Some(path);
            }
        }
    }
    None
}

pub fn active_workspace(editor: Editor) -> Result<PathBuf> {
    let result = match editor {
        Editor::Zed => zed_folder(),
        Editor::VsCode => vscode_folder(),
    };
    if let Some(path) = result {
        Ok(path)
    } else {
        bail!(
            "Open one local project folder in {} before using the MCP server.",
            editor.display()
        )
    }
}

/// Recent local folders, not a guarantee that each window is currently open.
pub fn workspace_choices(editor: Editor) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(path) = active_workspace(editor) {
        roots.push(path);
    }
    if editor == Editor::Zed {
        if let Some(db) = zed_database() {
            if let Ok(connection) =
                Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)
            {
                let _ = connection.busy_timeout(Duration::from_millis(250));
                if let Ok(mut statement) = connection.prepare("SELECT paths FROM workspaces WHERE remote_connection_id IS NULL ORDER BY timestamp DESC, workspace_id DESC LIMIT 40") {
                    if let Ok(rows) = statement.query_map([], |row| row.get::<_, String>(0)) {
                        for value in rows.flatten() {
                            let mut lines = value.lines();
                            if let Some(line) = lines.next() {
                                if lines.next().is_none() && PathBuf::from(line).is_absolute() {
                                    if let Some(path) = directory(line) {
                                        if !roots.contains(&path) { roots.push(path); }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    roots
}
