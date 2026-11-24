#![forbid(unsafe_code)]
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, Command},
    sync::{Mutex as AsyncMutex, oneshot},
    task::JoinHandle,
};

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>;
const MAX_RESPONSE: usize = 128 * 1024 * 1024;

pub struct StdioMcp {
    child: AsyncMutex<Child>,
    stdin: AsyncMutex<ChildStdin>,
    pending: Pending,
    next_id: AtomicU64,
    reader: JoinHandle<()>,
    stderr: JoinHandle<()>,
    pub tools: Vec<Value>,
}
impl StdioMcp {
    pub async fn start(config: &Value, root: &Path) -> Result<Self> {
        let program = config["command"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .context("MCP server command is missing")?;
        let mut command = Command::new(program);
        if let Some(args) = config.get("args") {
            for arg in args.as_array().context("MCP args must be an array")? {
                command.arg(arg.as_str().context("MCP args must be strings")?);
            }
        }
        let mut allowed: HashSet<String> = "PATH SYSTEMROOT WINDIR COMSPEC PATHEXT TEMP TMP USERPROFILE HOME APPDATA LOCALAPPDATA PROGRAMFILES PROGRAMFILES(X86) PROGRAMW6432 PROGRAMDATA SYSTEMDRIVE HOMEDRIVE HOMEPATH USERNAME LANG VSCODE_WORKSPACE_PATH VSCODE_WORKSPACE ZS_WORKSPACE ZS_ZED_WORKSPACE ZS_ZED_DB DISPLAY WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS".split_whitespace().map(str::to_owned).collect();
        if let Some(inherit) = config.get("inherit_env") {
            for key in inherit.as_array().context("inherit_env must be an array")? {
                allowed.insert(
                    key.as_str()
                        .context("Invalid inherited environment variable")?
                        .to_ascii_uppercase(),
                );
            }
        }
        command.env_clear();
        for (key, value) in std::env::vars_os() {
            let upper = key.to_string_lossy().to_ascii_uppercase();
            if allowed.contains(&upper) || upper.starts_with("LC_") || upper.starts_with("XDG_") {
                command.env(key, value);
            }
        }
        if let Some(env) = config.get("env") {
            for (key, value) in env.as_object().context("MCP env must be an object")? {
                if let Some(s) = value.as_str() {
                    command.env(key, s);
                } else {
                    command.env(key, value.to_string());
                }
            }
        }
        command.env_remove("ZS_WORKSPACE_LOCK_FILE");
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000);
        }
        let mut child = command
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Could not launch MCP process")?;
        let stdin = child.stdin.take().context("Missing MCP stdin")?;
        let stdout = child.stdout.take().context("Missing MCP stdout")?;
        let err = child.stderr.take().context("Missing MCP stderr")?;
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let requests = pending.clone();
        let reader = tokio::spawn(async move {
            let mut stdout = BufReader::new(stdout);
            let mut line = Vec::new();
            let mut oversized = false;
            loop {
                let Ok(bytes) = stdout.fill_buf().await else {
                    break;
                };
                if bytes.is_empty() {
                    break;
                }
                let end = bytes.iter().position(|b| *b == b'\n');
                let n = end.map_or(bytes.len(), |i| i + 1);
                if !oversized && line.len() + n <= MAX_RESPONSE {
                    line.extend_from_slice(&bytes[..n]);
                } else {
                    oversized = true;
                    line.clear();
                }
                stdout.consume(n);
                if end.is_some() {
                    if oversized {
                        if let Ok(mut p) = requests.lock() {
                            for (_, tx) in p.drain() {
                                let _ = tx.send(
                                    json!({"error":{"message":"MCP response exceeded 128 MB"}}),
                                );
                            }
                        }
                    } else if let Ok(response) = serde_json::from_slice::<Value>(&line) {
                        if let Some(id) = response["id"].as_u64() {
                            if let Ok(mut p) = requests.lock() {
                                if let Some(tx) = p.remove(&id) {
                                    let _ = tx.send(response);
                                }
                            }
                        }
                    }
                    line.clear();
                    oversized = false;
                }
            }
            if let Ok(mut p) = requests.lock() {
                p.clear();
            }
        });
        let stderr = tokio::spawn(async move {
            let mut err = BufReader::new(err);
            loop {
                let Ok(bytes) = err.fill_buf().await else {
                    break;
                };
                let n = bytes.len();
                if n == 0 {
                    break;
                }
                err.consume(n);
            }
        });
        let mut server = Self {
            child: AsyncMutex::new(child),
            stdin: AsyncMutex::new(stdin),
            pending,
            next_id: AtomicU64::new(1),
            reader,
            stderr,
            tools: Vec::new(),
        };
        server.request("initialize",json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"snapgent-bridge","version":"3.9.14"}}),12).await?;
        server
            .send(json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
            .await?;
        let result = server.request("tools/list", json!({}), 20).await?;
        server.tools = result["tools"]
            .as_array()
            .context("Invalid MCP tool catalog")?
            .iter()
            .filter(|v| v["name"].is_string())
            .cloned()
            .collect();
        Ok(server)
    }
    async fn send(&self, value: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&value)?;
        bytes.push(b'\n');
        let mut stdin = self.stdin.lock().await;
        stdin.write_all(&bytes).await?;
        stdin.flush().await?;
        Ok(())
    }
    pub async fn request(&self, method: &str, params: Value, timeout: u64) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending
            .lock()
            .map_err(|_| anyhow::anyhow!("MCP pending lock unavailable"))?
            .insert(id, tx);
        let result = tokio::time::timeout(Duration::from_secs(timeout), async {
            self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
                .await?;
            let response = rx.await.context("MCP server disconnected")?;
            if let Some(error) = response.get("error") {
                bail!(
                    "{}",
                    error["message"].as_str().unwrap_or("MCP request failed")
                );
            }
            Ok(response["result"].clone())
        })
        .await;
        if let Ok(mut p) = self.pending.lock() {
            p.remove(&id);
        }
        result.context("MCP request timed out")?
    }
    pub async fn alive(&self) -> bool {
        !self.reader.is_finished() && matches!(self.child.lock().await.try_wait(), Ok(None))
    }
    pub async fn call(&self, name: &str, args: Value, timeout: u64) -> Result<Value> {
        self.request("tools/call", json!({"name":name,"arguments":args}), timeout)
            .await
    }
    pub async fn stop(&self) {
        self.reader.abort();
        self.stderr.abort();
        if let Ok(mut p) = self.pending.lock() {
            p.clear();
        }
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
    }
}
impl Drop for StdioMcp {
    fn drop(&mut self) {
        self.reader.abort();
        self.stderr.abort();
        if let Ok(mut p) = self.pending.lock() {
            p.clear();
        }
    }
}
