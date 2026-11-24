use crate::{State, manager::Manager};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct Session {
    last: Option<(String, Instant, Value)>,
}
fn error(id: &Value, kind: &str, message: impl ToString) -> Value {
    json!({"type":"tool_result","id":id,"ok":false,"kind":kind,"error":message.to_string()})
}
fn same_selected_path(a: &std::path::Path, b: &std::path::Path) -> bool {
    if a == b { return true; }
    #[cfg(windows)]
    {
        let a = a.to_string_lossy();
        let b = b.to_string_lossy();
        return a.trim_start_matches("\\\\?\\").eq_ignore_ascii_case(b.trim_start_matches("\\\\?\\"));
    }
    #[cfg(not(windows))]
    false
}
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(m) => {
            let sorted: std::collections::BTreeMap<_, _> =
                m.iter().map(|(k, v)| (k.clone(), canonical(v))).collect();
            json!(sorted)
        }
        Value::Array(a) => json!(a.iter().map(canonical).collect::<Vec<_>>()),
        _ => value.clone(),
    }
}

pub async fn handle(state: Arc<State>, session: &mut Session, request: Value) -> Option<Value> {
    let id = request["id"].clone();
    let kind = request["type"].as_str().unwrap_or("");
    if *state.shutdown.borrow() {
        return Some(error(&id, "disconnected", "Bridge is stopping"));
    }
    let response = match kind {
        "ping" => json!({"type":"pong","id":id}),
        "list_tools" => {
            let mut data = state.manager.snapshot().await;
            data["type"] = json!("tools");
            data["id"] = id;
            data
        }
        "stream_status" => {
            let mut data = state.events.snapshot();
            data["type"] = json!("stream_status");
            data["id"] = id;
            data
        }
        "stream" | "progress" => {
            let mut payload = request.as_object().cloned().unwrap_or_default();
            for key in ["type", "frame", "id"] {
                payload.remove(key);
            }
            if let Some(frame) = request["frame"].as_object() {
                payload.extend(frame.clone());
            }
            let event = payload
                .remove("event")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| kind.into());
            let event = event
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .take(64)
                .collect::<String>();
            payload.insert("source".into(), json!("client"));
            let event = if matches!(event.as_str(), "log" | "tool_start" | "tool_end" | "") {
                "progress"
            } else {
                event.as_str()
            };
            state.events.publish(event, Value::Object(payload));
            return None;
        }
        "studio_status" => {
            let process = tokio::task::spawn_blocking(|| {
                let mut cmd =
                    std::process::Command::new(if cfg!(windows) { "tasklist" } else { "pgrep" });
                if cfg!(windows) {
                    cmd.arg("/NH");
                } else {
                    cmd.args(["-x", "RobloxStudio"]);
                }
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                cmd.output().ok().map(|out| {
                    if cfg!(windows) {
                        String::from_utf8_lossy(&out.stdout).contains("RobloxStudioBeta.exe")
                    } else {
                        out.status.success()
                    }
                })
            })
            .await
            .ok()
            .flatten();
            let (studio, app) = if let Ok((_, name, backend)) = state
                .manager
                .resolve("get_studio_state", Some("roblox"))
                .await
            {
                match Manager::call(backend, name, json!({}), 8).await {
                    Ok(result) => {
                        let text = result["content"].to_string().to_lowercase();
                        let alive = !result["isError"].as_bool().unwrap_or(false)
                            && !text.contains("unable to find an active studio instance")
                            && !text.contains("previously active studio has disconnected");
                        (Some(alive), if alive { Some(true) } else { process })
                    }
                    Err(_) => (Some(false), process),
                }
            } else {
                (None, None)
            };
            json!({"type":"studio_status","id":id,"studio":studio,"studio_app":app,"studio_proc":process})
        }
        "restart_mcp" => {
            if !state
                .approvals
                .ask(
                    "Restart MCP servers and release workspace locks",
                    json!({}),
                    false,
                )
                .await
            {
                json!({"type":"mcp_status","id":id,"ok":false,"error":"Local approval denied"})
            } else {
                match state.manager.restart().await {
                    Ok(()) => {
                        let snap = state.manager.snapshot().await;
                        state.publish_catalog(snap.clone());
                        json!({"type":"mcp_status","id":id,"ok":true,"alive":snap["mcp_alive"],"tools":snap["tools"],"servers":snap["servers"]})
                    }
                    Err(e) => json!({"type":"mcp_status","id":id,"ok":false,"error":e.to_string()}),
                }
            }
        }
        "add_server" | "remove_server" => {
            let details = json!({"server_id":request["server_id"],"command":request["command"],"args":request["args"],"env_keys":request["env"].as_object().map(|m|m.keys().cloned().collect::<Vec<_>>())});
            if !state
                .approvals
                .ask("Change MCP executable configuration", details, false)
                .await
            {
                json!({"type":"server_changed","id":id,"ok":false,"error":"Local approval denied"})
            } else {
                match state
                    .manager
                    .change(&request, kind == "remove_server")
                    .await
                {
                    Ok(()) => {
                        state.publish_catalog(state.manager.snapshot().await);
                        json!({"type":"server_changed","id":id,"ok":true,"restarting":false})
                    }
                    Err(e) => {
                        json!({"type":"server_changed","id":id,"ok":false,"error":e.to_string()})
                    }
                }
            }
        }
        "workspace_list" => {
            let editor = match request["server"].as_str() {
                Some("local") => Some(snapgent_workspace::Editor::Local),
                _ => None,
            };
            match editor {
                Some(editor) => match tokio::task::spawn_blocking(move || {
                    snapgent_workspace::workspace_choices(editor)
                })
                .await
                {
                    Ok(roots) => {
                        json!({"type":"tool_result","id":id,"ok":true,"workspaces":roots.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>() })
                    }
                    Err(e) => error(&id, "tool_error", e),
                },
                None => error(&id, "tool_error", "Unknown workspace editor"),
            }
        }
        "workspace_bind" => {
            let sid = request["session"].as_str().unwrap_or("");
            let server = request["server"].as_str().unwrap_or("");
            let editor = match server {
                "local" => Some(snapgent_workspace::Editor::Local),
                _ => None,
            };
            let requested = request["workspace"].as_str().map(std::path::PathBuf::from);
            if !snapgent_security::valid_nonce(sid) || editor.is_none() {
                error(&id, "permission_denied", "Invalid workspace binding")
            } else if let Some(path) = requested {
                let editor = editor.unwrap();
                let existing = state.manager.binding_root(sid, server).await;
                if !path.is_absolute() {
                    error(&id, "permission_denied", "Select an absolute local directory")
                } else if existing.as_ref().is_some_and(|old| !same_selected_path(old, &path)) {
                    error(&id, "permission_denied", "Session is bound to a different workspace")
                } else if existing.is_none() && !state.approvals.ask(
                    "Bind agent to workspace",
                    json!({"server":server,"workspace":path.to_string_lossy()}),
                    false,
                ).await {
                    error(&id, "permission_denied", "Workspace binding denied")
                } else if *state.shutdown.borrow() {
                    error(&id, "disconnected", "Bridge is stopping")
                } else {
                    match tokio::task::spawn_blocking(move || snapgent_workspace::Workspace::bound(editor, &path)).await {
                        Ok(Ok(workspace)) => {
                            let root = workspace.fixed_root().expect("bound root").to_path_buf();
                            if existing.as_ref().is_some_and(|old| old != &root) {
                                error(&id, "permission_denied", "Session is bound to a different workspace")
                            } else {
                                match state.manager.bind_session(sid, server, workspace).await {
                                    Ok(()) => json!({"type":"tool_result","id":id,"ok":true,"text":root.to_string_lossy()}),
                                    Err(e) => error(&id, "tool_error", e),
                                }
                            }
                        }
                        Ok(Err(e)) => error(&id, "tool_error", e),
                        Err(e) => error(&id, "tool_error", e),
                    }
                }
            } else {
                error(&id, "permission_denied", "Select the exact workspace first")
            }
        }
        "workspace_release" => {
            if let Some(sid) = request["session"]
                .as_str()
                .filter(|s| snapgent_security::valid_nonce(s))
            {
                state.manager.release_session(sid).await;
            }
            json!({"type":"tool_result","id":id,"ok":true})
        }
        "workspace_probe" => {
            let sid = request["session"].as_str().unwrap_or("");
            if request["server"].as_str() != Some("local") || !snapgent_security::valid_nonce(sid) {
                error(
                    &id,
                    "workspace_required",
                    "Select and approve a local workspace first",
                )
            } else if let Some(root) = state.manager.binding_root(sid, "local").await {
                if snapgent_workspace::Workspace::bound(snapgent_workspace::Editor::Local, &root)
                    .is_ok_and(|w| w.fixed_root() == Some(root.as_path()))
                {
                    json!({"type":"tool_result","id":id,"ok":true})
                } else {
                    error(&id, "tool_error", "Workspace unavailable")
                }
            } else {
                error(
                    &id,
                    "workspace_unbound",
                    "Select and approve a local workspace first",
                )
            }
        }
        "call_tool" => call(state, session, request).await,
        _ => json!({"type":"error","id":id,"error":format!("Unknown bridge request: {kind}")}),
    };
    Some(response)
}
async fn call(state: Arc<State>, session: &mut Session, request: Value) -> Value {
    let id = &request["id"];
    let Some(name) = request["name"].as_str() else {
        return error(id, "tool_error", "Invalid tool name");
    };
    let args = request
        .get("arguments")
        .cloned()
        .filter(|v| !v.is_null())
        .unwrap_or(json!({}));
    if !args.is_object() {
        return error(id, "tool_error", "Invalid arguments");
    }
    let selected = request["server"].as_str().filter(|s| !s.is_empty());
    let turn = request["turn"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let binding = request["session"].as_str();
    let sig = canonical(
        &json!({"name":name,"args":args,"turn":turn,"server":selected,"session":binding}),
    )
    .to_string();
    let ttl = Duration::from_secs(if turn.is_some() { 300 } else { 5 });
    if !request["force"].as_bool().unwrap_or(false) {
        if let Some((old, at, response)) = &session.last {
            if *old == sig && at.elapsed() <= ttl {
                let mut out = response.clone();
                out["id"] = id.clone();
                out["duplicate"] = json!(true);
                return out;
            }
        }
    }
    session.last = None;
    let (server, bare, mut backend) = match state.manager.resolve(name, selected).await {
        Ok(v) => v,
        Err(e) => return error(id, "unknown_tool", e),
    };
    if server == "local" {
        let Some(sid) = binding else {
            return error(
                id,
                "workspace_required",
                "Select a workspace in the Snapgent bar and update the extension if needed.",
            );
        };
        backend = match state.manager.session_backend(sid, &server).await {
            Ok(backend) => backend,
            Err(e) => return error(id, "workspace_unbound", e),
        };
    }
    if server == "local"
        && matches!(
            bare.as_str(),
            "run_command"
                | "run_project_script"
                | "run_tests"
                | "git_commit"
                | "git_add"
                | "git_diff"
                | "git_status"
                | "git_log"
                | "git_branches"
        )
        && !state
            .approvals
            .ask(
                "Execute local workspace operation",
                json!({"server":server,"workspace":match &backend { crate::manager::Backend::Builtin(w) => w.fixed_root().map(|p| p.to_string_lossy().into_owned()), _ => None },"tool":bare,"arguments":args}),
                true,
            )
            .await
    {
        return error(id, "permission_denied", "Local approval denied");
    }
    if *state.shutdown.borrow() {
        return error(id, "disconnected", "Bridge is stopping");
    }
    let guard = state.events.begin(id.clone(), name, &server);
    let timeout = (request["timeout"].as_u64().unwrap_or(120000) / 1000).clamp(1, 300);
    let result = match Manager::call(backend, bare, args, timeout).await {
        Ok(v) => v,
        Err(e) => {
            guard.finish(false, 0);
            return error(id, "tool_error", e);
        }
    };
    let mut texts = Vec::new();
    let mut images = Vec::new();
    if let Some(content) = result["content"].as_array() {
        for item in content {
            match item["type"].as_str() {
                Some("text") => {
                    if let Some(text) = item["text"].as_str() {
                        texts.push(text.to_owned());
                    }
                }
                Some("image") => {
                    if let Some(data) = item["data"].as_str() {
                        images.push(json!({"data":data,"mimeType":item["mimeType"].as_str().unwrap_or("image/png")}));
                    }
                }
                _ => {}
            }
        }
    }
    if texts.is_empty() {
        if let Some(structured) = result.get("structuredContent") {
            texts.push(structured.to_string());
        }
    }
    let text = texts.join("\n");
    let ok = !result["isError"].as_bool().unwrap_or(false);
    guard.finish(ok, text.chars().count());
    if !ok {
        return error(
            id,
            "tool_error",
            if text.is_empty() {
                "MCP tool failed".into()
            } else {
                text
            },
        );
    }
    let response = json!({"type":"tool_result","id":id,"ok":true,"text":text,"images":images});
    session.last = Some((sig, Instant::now(), response.clone()));
    response
}
