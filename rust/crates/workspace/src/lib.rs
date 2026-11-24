#![forbid(unsafe_code)]
mod discovery;
mod files;
mod paths;
mod process;

use anyhow::{Result, bail};
pub use discovery::{Editor, active_workspace, workspace_choices};
use paths::{WorkspacePin, relative, target};
use serde_json::{Value, json};
use std::{fs, path::Path, sync::Mutex};

pub struct Workspace {
    editor: Editor,
    pin: Mutex<WorkspacePin>,
    fixed_root: Option<std::path::PathBuf>,
}
impl Workspace {
    pub fn new(editor: Editor) -> Self {
        Self {
            editor,
            pin: Mutex::new(WorkspacePin::default()),
            fixed_root: None,
        }
    }
    pub fn bound(editor: Editor, root: &Path) -> Result<Self> {
        let mut workspace = Self::new(editor);
        let root = fs::canonicalize(root)?;
        if !root.is_dir() {
            bail!("Workspace must be a directory");
        }
        workspace.fixed_root = Some(root);
        Ok(workspace)
    }
    pub fn fixed_root(&self) -> Option<&Path> {
        self.fixed_root.as_deref()
    }
    pub fn available(&self) -> bool {
        match &self.fixed_root {
            Some(root) => fs::canonicalize(root).is_ok_and(|p| p == *root && p.is_dir()),
            None => active_workspace(self.editor).is_ok(),
        }
    }
    pub fn tools(&self) -> Vec<Value> {
        let text = include_str!("../../../app-cases/workspace-tools.json");
        let data: Value = serde_json::from_str(text).expect("valid embedded tool schema");
        let mut tools = data
            .as_array()
            .or_else(|| data["tools"].as_array())
            .expect("tool schema array")
            .clone();
        if self.editor == Editor::Zed {
            for tool in &mut tools {
                if let Some(desc) = tool["description"].as_str() {
                    tool["description"] = json!(desc.replace("VS Code", "Zed"));
                }
            }
        }
        tools
    }
    pub fn call(&self, name: &str, args: &Value) -> Result<Value> {
        let candidate = match &self.fixed_root {
            Some(root) => {
                let resolved = fs::canonicalize(root)?;
                if resolved != *root || !resolved.is_dir() {
                    bail!("Bound workspace is no longer available");
                }
                resolved
            }
            None => active_workspace(self.editor)?,
        };
        // Serialize operations on this workspace through dispatch.
        let mut pin = self
            .pin
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace lock unavailable"))?;
        let root = pin.pin(&candidate)?;
        dispatch(&root, name, args)
    }
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid argument: {key}"))
}
fn optional<'a>(args: &'a Value, key: &str, default: &'a str) -> &'a str {
    args[key].as_str().unwrap_or(default)
}
fn array<'a>(args: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    args[key]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing or invalid array: {key}"))
}
fn strings(args: &Value, key: &str) -> Result<Vec<String>> {
    array(args, key)?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Invalid string in {key}"))
        })
        .collect()
}

pub fn workspace_info(root: &Path) -> Value {
    json!({"name":root.file_name().unwrap_or_default().to_string_lossy(),"path":root.to_string_lossy().trim_start_matches("\\\\?\\"),"is_git_repository":root.join(".git").exists()})
}
fn project_info(root: &Path) -> Result<Value> {
    let mut manifests = Vec::new();
    for pattern in [
        "package.json",
        "pyproject.toml",
        "Cargo.toml",
        "go.mod",
        "pom.xml",
        "*.sln",
    ] {
        let glob = glob::Pattern::new(pattern)?;
        let mut candidates = fs::read_dir(root)?
            .flatten()
            .map(|e| e.path())
            .filter(|p| glob.matches(&p.file_name().unwrap_or_default().to_string_lossy()))
            .collect::<Vec<_>>();
        candidates.sort();
        for p in candidates.into_iter().take(10) {
            let rel = relative(root, &p);
            let p = target(root, &rel)?;
            if !p.is_file() {
                continue;
            }
            let mut manifest =
                json!({"path":rel,"name":p.file_name().unwrap_or_default().to_string_lossy()});
            if p.file_name().and_then(|s| s.to_str()) == Some("package.json") {
                if let Ok(data) = serde_json::from_str::<Value>(&files::read_text(&p)?) {
                    manifest["scripts"] = data["scripts"].clone();
                    if let Some(name) = data["name"].as_str() {
                        manifest["name"] = json!(name);
                    }
                }
            }
            manifests.push(manifest);
        }
    }
    Ok(json!({"workspace":workspace_info(root),"manifests":manifests}))
}

pub fn dispatch(root: &Path, name: &str, a: &Value) -> Result<Value> {
    let path = || required(a, "path");
    let result = match name {
        "get_workspace_info" => workspace_info(root),
        "get_project_info" => project_info(root)?,
        "list_dir" => files::list_dir(root, optional(a, "path", "."))?,
        "list_tree" => files::list_tree(
            root,
            optional(a, "path", "."),
            a["max_depth"].as_u64().unwrap_or(3).min(8) as usize,
        )?,
        "find_files" => {
            files::find_files(root, optional(a, "path", "."), optional(a, "pattern", "*"))?
        }
        "search_files" => files::search(
            root,
            path()?,
            required(a, "query")?,
            optional(a, "file_pattern", "*"),
            a["case_sensitive"].as_bool().unwrap_or(false),
        )?,
        "read_file" => json!(files::read_file(root, path()?)?),
        "read_files" => {
            let paths = strings(a, "paths")?;
            if paths.len() > 50 {
                bail!("Read at most 50 files per request.");
            }
            let mut out = Vec::new();
            for path in paths {
                let content = files::read_file(root, &path)?;
                out.push(json!({"path":path,"content":content}));
            }
            json!(out)
        }
        "read_file_lines" => {
            let start = a["start_line"].as_i64().unwrap_or(1);
            let end = a["end_line"].as_i64().unwrap_or(200);
            if start < 1 || end < start || end - start > 999 {
                bail!("Invalid line range; read at most 1000 lines.");
            }
            let p = target(root, path()?)?;
            if !p.is_file() {
                bail!("Not a file");
            }
            let text = files::read_text(&p)?;
            json!(
                text.lines()
                    .enumerate()
                    .skip((start - 1) as usize)
                    .take((end - start + 1) as usize)
                    .map(|(i, line)| format!("{}: {line}", i + 1))
                    .collect::<Vec<_>>()
                    .join("\n")
                    .chars()
                    .take(paths::MAX_FILE_CHARS)
                    .collect::<String>()
            )
        }
        "get_file_info" => files::info(root, path()?)?,
        "get_file_outline" => {
            let text = files::read_file(root, path()?)?;
            let patterns = [
                r"^\s*(?:async\s+)?def\s+([A-Za-z_]\w*)",
                r"^\s*class\s+([A-Za-z_]\w*)",
                r"^\s*(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)",
                r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s*)?(?:function|\([^)]*\)\s*=>|[A-Za-z_$][\w$]*\s*=>)",
            ];
            let patterns = patterns
                .iter()
                .map(|p| regex::Regex::new(p))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out = Vec::new();
            for (i, line) in text.lines().enumerate() {
                for pattern in &patterns {
                    if let Some(m) = pattern.captures(line) {
                        out.push(json!({"name":&m[1],"line":i+1}));
                        break;
                    }
                }
                if out.len() >= paths::MAX_RESULTS {
                    break;
                }
            }
            json!(out)
        }
        "create_file" => json!(files::write_file(
            root,
            path()?,
            optional(a, "content", ""),
            true
        )?),
        "write_file" => json!(files::write_file(
            root,
            path()?,
            required(a, "content")?,
            false
        )?),
        "edit_file" => json!(files::edit(
            root,
            path()?,
            required(a, "old_text")?,
            required(a, "new_text")?,
            a["replace_all"].as_bool().unwrap_or(false)
        )?),
        "create_directory" => json!(files::directory(root, path()?)?),
        "move_file" | "copy_file" => json!(files::transfer(
            root,
            required(a, "source")?,
            required(a, "destination")?,
            name == "copy_file"
        )?),
        "git_status" | "git_diff" | "git_log" | "git_branches" | "git_add" | "git_commit" => {
            json!(process::git(root, name, a)?)
        }
        "run_command" => json!(process::shell(
            root,
            required(a, "command")?,
            &if a["args"].is_null() {
                Vec::new()
            } else {
                strings(a, "args")?
            },
            30
        )?),
        "run_project_script" => json!(process::project_script(root, required(a, "script")?)?),
        "run_tests" => json!(process::run_tests(root)?),
        "delete_file" => json!(files::delete(root, path()?)?),
        "apply_patch" => json!(files::patch(root, path()?, array(a, "edits")?)?),
        _ => bail!("Unknown tool: {name}"),
    };
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn independently_bound_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let one = Workspace::bound(Editor::Zed, &a).unwrap();
        let two = Workspace::bound(Editor::Zed, &b).unwrap();
        one.call("create_file", &json!({"path":"identity.txt","content":"A"}))
            .unwrap();
        two.call("create_file", &json!({"path":"identity.txt","content":"B"}))
            .unwrap();
        assert_eq!(
            one.call("read_file", &json!({"path":"identity.txt"}))
                .unwrap(),
            json!("A")
        );
        assert_eq!(
            two.call("read_file", &json!({"path":"identity.txt"}))
                .unwrap(),
            json!("B")
        );
        assert!(
            one.call("read_file", &json!({"path":"../b/identity.txt"}))
                .is_err()
        );
    }
    #[test]
    fn read_lines_beyond_full_file_output_limit() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::write(
            root.join("large.txt"),
            format!(
                "{}sentinel\n",
                "abcdefghij\n".repeat(paths::MAX_FILE_CHARS / 10 + 1)
            ),
        )
        .unwrap();
        let n = paths::MAX_FILE_CHARS / 10 + 2;
        let result = dispatch(
            &root,
            "read_file_lines",
            &json!({"path":"large.txt","start_line":n,"end_line":n}),
        )
        .unwrap();
        assert_eq!(result, json!(format!("{n}: sentinel")));
    }
    #[test]
    fn workspace_schema_and_edit_contract() {
        assert_eq!(Workspace::new(Editor::Zed).tools().len(), 28);
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        dispatch(
            &root,
            "create_file",
            &json!({"path":"hello.txt","content":"hello world"}),
        )
        .unwrap();
        assert!(dispatch(&root, "create_file", &json!({"path":"hello.txt"})).is_err());
        assert!(dispatch(&root,"apply_patch",&json!({"path":"hello.txt","edits":[{"old_text":"hello","new_text":"hi"},{"old_text":"missing","new_text":"x"}]})).is_err());
        assert_eq!(
            dispatch(&root, "read_file", &json!({"path":"hello.txt"})).unwrap(),
            json!("hello world")
        );
        dispatch(
            &root,
            "edit_file",
            &json!({"path":"hello.txt","old_text":"hello","new_text":"hi"}),
        )
        .unwrap();
        assert_eq!(
            dispatch(&root, "read_file", &json!({"path":"hello.txt"})).unwrap(),
            json!("hi world")
        );
        assert!(
            dispatch(
                &root,
                "write_file",
                &json!({"path":"../outside.txt","content":"bad"})
            )
            .is_err()
        );
    }
}
