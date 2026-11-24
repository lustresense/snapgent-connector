use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use snapgent_mcp::StdioMcp;
use snapgent_workspace::{Editor, Workspace};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, atomic::{AtomicBool, Ordering}},
};
use tokio::sync::{Mutex, RwLock};

#[derive(Clone)]
pub enum Backend {
    Builtin(Arc<Workspace>),
    Stdio(Arc<StdioMcp>),
    Offline,
}
#[derive(Clone)]
struct Server {
    backend: Backend,
    tools: Vec<Value>,
}

pub struct Manager {
    root: PathBuf,
    config_path: PathBuf,
    servers: RwLock<BTreeMap<String, Server>>,
    session_pins: Mutex<HashMap<(String, String), Arc<Workspace>>>,
    bound_roots: Mutex<HashMap<PathBuf, std::sync::Weak<Workspace>>>,
    admin: Mutex<()>,
}

impl Manager {
    pub fn new(root: PathBuf) -> Self {
        let config_path = root.join("config.json");
        Self::with_config(root, config_path)
    }
    pub fn with_config(root: PathBuf, config_path: PathBuf) -> Self {
        Self {
            root,
            config_path,
            servers: RwLock::new(BTreeMap::new()),
            session_pins: Mutex::new(HashMap::new()),
            bound_roots: Mutex::new(HashMap::new()),
            admin: Mutex::new(()),
        }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    fn config_path(&self) -> PathBuf {
        self.config_path.clone()
    }
    fn config(&self) -> Result<Value> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(json!({"mcpServers":{}}));
        }
        let config: Value = serde_json::from_slice(&std::fs::read(path)?)?;
        if !config["mcpServers"].is_object() {
            bail!("config.json must contain an mcpServers object");
        }
        Ok(config)
    }
    pub async fn start(&self) -> Result<()> {
        let _guard = self.admin.lock().await;
        self.reload(false).await
    }
    async fn reload(&self, release_pins: bool) -> Result<()> {
        let config = self.config()?;
        self.stop_inner().await;
        if release_pins {
            self.session_pins.lock().await.clear();
        }
        let workspace = Arc::new(Workspace::new(Editor::Local));
        self.servers.write().await.insert(
            "local".into(),
            Server {
                tools: workspace.tools(),
                backend: Backend::Builtin(workspace),
            },
        );
        let mut tasks = tokio::task::JoinSet::new();
        for (id, entry) in config["mcpServers"]
            .as_object()
            .context("Invalid configuration")?
        {
            if !valid_id(id) || !entry.is_object() {
                continue;
            }
            if matches!(id.as_str(), "local" | "zed" | "vscode") {
                continue;
            } else if entry["enabled"].as_bool() == Some(true) {
                self.servers.write().await.insert(
                    id.clone(),
                    Server {
                        backend: Backend::Offline,
                        tools: vec![],
                    },
                );
                let id = id.clone();
                let entry = entry.clone();
                let root = self.root.clone();
                tasks.spawn(async move {
                    let result = StdioMcp::start(&entry, &root).await;
                    (id, result)
                });
            }
        }
        while let Some(result) = tasks.join_next().await {
            if let Ok((id, result)) = result {
                if let Ok(server) = result {
                    let tools = server.tools.clone();
                    self.servers.write().await.insert(
                        id,
                        Server {
                            backend: Backend::Stdio(Arc::new(server)),
                            tools,
                        },
                    );
                }
            }
        }
        Ok(())
    }
    async fn stop_inner(&self) {
        let old = std::mem::take(&mut *self.servers.write().await);
        for server in old.into_values() {
            if let Backend::Stdio(server) = server.backend {
                server.stop().await;
            }
        }
    }
    pub async fn stop(&self) {
        let _guard = self.admin.lock().await;
        self.stop_inner().await;
    }
    pub async fn restart(&self) -> Result<()> {
        let _guard = self.admin.lock().await;
        self.reload(true).await
    }
    pub async fn change(&self, request: &Value, remove: bool) -> Result<()> {
        let _guard = self.admin.lock().await;
        let id = request["server_id"]
            .as_str()
            .filter(|s| valid_id(s))
            .context("Invalid MCP server id")?;
        if id == "local" || (matches!(id, "zed" | "vscode") && !remove) {
            bail!("Protected built-in MCP server id");
        }
        let mut config = self.config()?;
        let servers = config["mcpServers"]
            .as_object_mut()
            .context("Invalid configuration")?;
        if remove {
            servers.remove(id);
        } else {
            let command = request["command"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .context("MCP server command is required")?;
            let args = request.get("args").cloned().unwrap_or(json!([]));
            let env = request.get("env").cloned().unwrap_or(json!({}));
            if !args
                .as_array()
                .is_some_and(|a| a.iter().all(Value::is_string))
                || !env.is_object()
            {
                bail!("Invalid MCP args or env");
            }
            servers.insert(
                id.into(),
                json!({"command":command,"args":args,"env":env,"enabled":true}),
            );
        }
        let path = self.config_path();
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&config)?)?;
        std::fs::rename(&tmp, &path)?;
        self.reload(false).await
    }
    async fn entries(&self) -> Vec<(String, Server, bool, Vec<Value>)> {
        let entries = self.servers.read().await.clone();
        let mut out = Vec::new();
        for (id, server) in entries {
            let (alive, tools) = match &server.backend {
                Backend::Builtin(_) => (true, server.tools.clone()),
                Backend::Stdio(s) => {
                    let alive = s.alive().await;
                    (alive, if alive { server.tools.clone() } else { vec![] })
                }
                Backend::Offline => (false, vec![]),
            };
            out.push((id, server, alive, tools));
        }
        out
    }
    pub async fn snapshot(&self) -> Value {
        let entries = self.entries().await;
        let mut counts = HashMap::new();
        for (_, _, _, tools) in &entries {
            for tool in tools {
                if let Some(name) = tool["name"].as_str() {
                    *counts.entry(name.to_owned()).or_insert(0usize) += 1;
                }
            }
        }
        let mut catalog = Vec::new();
        let mut statuses = Vec::new();
        let mut alive = false;
        for (id, _, ready, tools) in entries {
            alive |= ready;
            statuses.push(json!({"id":id,"alive":ready,"tools":tools.len()}));
            for tool in tools {
                let Some(bare) = tool["name"].as_str() else {
                    continue;
                };
                let name = if counts.get(bare).copied().unwrap_or(0) > 1 {
                    format!("{id}/{bare}")
                } else {
                    bare.into()
                };
                catalog.push(json!({"name":name,"server":id,"description":tool.get("description").cloned().unwrap_or(json!("")),"inputSchema":tool.get("inputSchema").cloned().unwrap_or(json!({"type":"object","properties":{}}))}));
            }
        }
        json!({"type":"connected","bridge_version":"3.9.15","workspace_binding":true,"mcp_alive":alive,"tools":catalog,"servers":statuses})
    }
    pub async fn bind_session(
        &self,
        session: &str,
        server: &str,
        workspace: Workspace,
    ) -> Result<()> {
        if !snapgent_security::valid_nonce(session) || server != "local" {
            bail!("Invalid workspace session");
        }
        let mut pins = self.session_pins.lock().await;
        let key = (session.to_owned(), server.to_owned());
        if let Some(existing) = pins.get(&key) {
            if existing.fixed_root() == workspace.fixed_root() && existing.available() {
                return Ok(());
            }
            bail!("Session bound to a different workspace");
        }
        if pins.len() >= 128 {
            bail!("Workspace session limit reached");
        }
        let root = workspace
            .fixed_root()
            .context("Expected an explicitly bound workspace")?
            .to_path_buf();
        let mut roots = self.bound_roots.lock().await;
        roots.retain(|_, weak| weak.strong_count() > 0);
        let shared = roots
            .get(&root)
            .and_then(std::sync::Weak::upgrade)
            .unwrap_or_else(|| Arc::new(workspace));
        roots.insert(root, Arc::downgrade(&shared));
        pins.insert(key, shared);
        Ok(())
    }
    pub async fn binding_root(&self, session: &str, server: &str) -> Option<PathBuf> {
        self.session_pins
            .lock()
            .await
            .get(&(session.to_owned(), server.to_owned()))
            .and_then(|w| w.fixed_root().map(Path::to_path_buf))
    }
    pub async fn session_backend(&self, session: &str, server: &str) -> Result<Backend> {
        if !snapgent_security::valid_nonce(session) {
            bail!("Invalid workspace session");
        }
        let pins = self.session_pins.lock().await;
        let workspace = pins
            .get(&(session.to_owned(), server.to_owned()))
            .context("Workspace session not bound. Reapprove the saved workspace.")?;
        Ok(Backend::Builtin(workspace.clone()))
    }
    pub async fn release_session(&self, session: &str) {
        self.session_pins
            .lock()
            .await
            .retain(|(id, _), _| id != session);
    }
    pub async fn resolve(
        &self,
        name: &str,
        selected: Option<&str>,
    ) -> Result<(String, String, Backend)> {
        let (qualified, bare) = name
            .split_once('/')
            .map_or((None, name), |(a, b)| (Some(a), b));
        if qualified.is_some() && selected.is_some() && qualified != selected {
            bail!("MCP tool not available for the selected server: {name}");
        }
        let mut matches = Vec::new();
        for (id, server, alive, tools) in self.entries().await {
            if !alive || qualified.or(selected).is_some_and(|s| s != id) {
                continue;
            }
            if tools.iter().any(|t| t["name"].as_str() == Some(bare)) {
                matches.push((id, bare.to_owned(), server.backend));
            }
        }
        if matches.len() != 1 {
            bail!("Unknown or unavailable MCP tool: {name}");
        }
        Ok(matches.remove(0))
    }
    pub async fn call(backend: Backend, name: String, args: Value, timeout: u64) -> Result<Value> {
        match backend {
            Backend::Builtin(workspace) => {
                struct CancelOnDrop(Arc<AtomicBool>);
                impl Drop for CancelOnDrop {
                    fn drop(&mut self) { self.0.store(true, Ordering::Release); }
                }
                let cancelled = Arc::new(AtomicBool::new(false));
                let _cancel = CancelOnDrop(cancelled.clone());
                let result = tokio::task::spawn_blocking(move || {
                    workspace.call_cancelable(&name, &args, &cancelled)
                }).await??;
                let text = result
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| result.to_string());
                Ok(
                    json!({"content":[{"type":"text","text":text}],"structuredContent":{"result":result}}),
                )
            }
            Backend::Stdio(server) => server.call(&name, args, timeout).await,
            Backend::Offline => bail!("MCP server is offline"),
        }
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
