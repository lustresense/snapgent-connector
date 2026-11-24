use crate::{
    discovery::hidden_command,
    paths::{MAX_FILE_CHARS, relative, target},
};
use anyhow::{Result, bail};
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Child, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

// Terminate this known running child. Windows taskkill /T attempts to include
// descendants; detached descendants can escape, so cancellation is not rollback.
fn terminate(child: &mut Child) {
    if !matches!(child.try_wait(), Ok(None)) { return; }
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = hidden_command("taskkill.exe").args(["/PID", &pid, "/T", "/F"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
    let _ = child.kill();
    let _ = child.wait();
}
pub fn run(root: &Path, command: &[String], timeout: u64, cancelled: &AtomicBool) -> Result<String> {
    if cancelled.load(Ordering::Acquire) { bail!("Command cancelled before execution"); }
    let Some(program) = command.first() else {
        bail!("Command cannot be empty.");
    };
    let mut process = hidden_command(program);
    process.env_clear();
    for (key, value) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if matches!(
            name.as_str(),
            "PATH"
                | "PATHEXT"
                | "SYSTEMROOT"
                | "WINDIR"
                | "COMSPEC"
                | "TEMP"
                | "TMP"
                | "USERPROFILE"
                | "HOME"
                | "APPDATA"
                | "LOCALAPPDATA"
                | "PROGRAMFILES"
                | "PROGRAMFILES(X86)"
                | "PROGRAMDATA"
                | "HOMEDRIVE"
                | "HOMEPATH"
                | "LANG"
                | "CARGO_HOME"
                | "RUSTUP_HOME"
                | "TERM"
                | "NUMBER_OF_PROCESSORS"
        ) || name.starts_with("LC_")
            || name.starts_with("XDG_")
        {
            process.env(key, value);
        }
    }
    let mut child = process
        .args(&command[1..])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    fn drain(mut input: impl Read + Send + 'static) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            let mut output = Vec::new();
            let mut buf = [0u8; 8192];
            loop {
                match input.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let remaining = (MAX_FILE_CHARS * 4).saturating_sub(output.len());
                        output.extend_from_slice(&buf[..n.min(remaining)]);
                    }
                }
            }
            output
        })
    }
    let stdout = drain(child.stdout.take().expect("piped stdout"));
    let stderr = drain(child.stderr.take().expect("piped stderr"));
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Acquire) {
            terminate(&mut child);
            bail!("Command cancelled; verify any partial changes");
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {},
            Err(e) => { terminate(&mut child); return Err(e.into()); }
        }
        if started.elapsed() >= Duration::from_secs(timeout) {
            terminate(&mut child);
            bail!("Command timed out after {timeout} seconds; verify any partial changes");
        }
        thread::sleep(Duration::from_millis(25));
    };
    // A descendant can retain inherited pipes after its parent exits.
    // Bound the drain so the GUI cannot hang forever joining reader threads.
    let drain_started = Instant::now();
    while !stdout.is_finished() || !stderr.is_finished() {
        if cancelled.load(Ordering::Acquire) || drain_started.elapsed() >= Duration::from_secs(2) {
            bail!("Command ended but output pipes remained open; verify the result");
        }
        thread::sleep(Duration::from_millis(20));
    }
    let out = stdout.join().unwrap_or_default();
    let err = stderr.join().unwrap_or_default();
    let out = String::from_utf8_lossy(&out);
    let err = String::from_utf8_lossy(&err);
    let text = if !out.trim().is_empty() {
        out.trim()
    } else if !err.trim().is_empty() {
        err.trim()
    } else {
        "(no output)"
    };
    let text: String = text.chars().take(MAX_FILE_CHARS).collect();
    if !status.success() {
        bail!(
            "Command exited with status {}:\n{text}",
            status.code().unwrap_or(-1)
        );
    }
    Ok(text)
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| (*s).into()).collect()
}

pub fn shell(root: &Path, command: &str, args: &[String], timeout: u64, cancelled: &AtomicBool) -> Result<String> {
    if command.trim().is_empty() {
        bail!("Command cannot be empty.");
    }
    let mut parts = if cfg!(windows) {
        strings(&["cmd.exe", "/d", "/c", command])
    } else {
        vec![command.to_owned()]
    };
    parts.extend_from_slice(args);
    run(root, &parts, timeout, cancelled)
}

pub fn git(root: &Path, name: &str, args: &Value, cancelled: &AtomicBool) -> Result<String> {
    let mut timeout = 30;
    let command = match name {
        "git_status" => strings(&["git", "status", "--short", "--branch"]),
        "git_diff" => {
            let mut c = strings(&["git", "diff", "--"]);
            if let Some(path) = args["path"].as_str().filter(|s| !s.is_empty()) {
                c.push(relative(root, &target(root, path)?));
            }
            c
        }
        "git_log" => {
            let limit = args["limit"].as_i64().unwrap_or(10);
            if !(1..=50).contains(&limit) {
                bail!("limit must be between 1 and 50.");
            }
            vec![
                "git".into(),
                "log".into(),
                format!("-{limit}"),
                "--oneline".into(),
                "--decorate".into(),
            ]
        }
        "git_branches" => strings(&["git", "branch", "--format=%(refname:short)"]),
        "git_add" => {
            let paths = args["paths"]
                .as_array()
                .filter(|p| !p.is_empty())
                .ok_or_else(|| anyhow::anyhow!("paths must be a non-empty list."))?;
            let mut relatives = Vec::new();
            for path in paths {
                relatives.push(relative(
                    root,
                    &target(
                        root,
                        path.as_str()
                            .ok_or_else(|| anyhow::anyhow!("Invalid path"))?,
                    )?,
                ));
            }
            if relatives.iter().any(|p| p == ".") {
                bail!("Stage specific paths, not the whole workspace.");
            }
            let mut command = strings(&["git", "add", "--"]);
            command.extend(relatives.clone());
            run(root, &command, timeout, cancelled)?;
            return Ok(format!(
                "Staged {} path(s): {}",
                relatives.len(),
                relatives.join(", ")
            ));
        }
        "git_commit" => {
            let message = args["message"].as_str().unwrap_or("").trim();
            if message.is_empty() {
                bail!("Commit message cannot be empty.");
            }
            timeout = 120;
            strings(&["git", "commit", "-m", message])
        }
        _ => bail!("Unknown Git tool: {name}"),
    };
    run(root, &command, timeout, cancelled)
}

pub fn project_script(root: &Path, script: &str, cancelled: &AtomicBool) -> Result<String> {
    if !["build", "check", "format", "lint", "test", "typecheck"].contains(&script) {
        bail!("Script must be one of: build, check, format, lint, test, typecheck.");
    }
    let package: Value = serde_json::from_str(&fs::read_to_string(target(root, "package.json")?)?)?;
    if package["scripts"].get(script).is_none() {
        bail!("package.json does not define the '{script}' script.");
    }
    let manager = if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else if root.join("bun.lock").exists() || root.join("bun.lockb").exists() {
        "bun"
    } else {
        "npm"
    };
    shell(root, manager, &strings(&["run", script]), 300, cancelled)
}

pub fn run_tests(root: &Path, cancelled: &AtomicBool) -> Result<String> {
    if target(root, "package.json")?.is_file() {
        return project_script(root, "test", cancelled);
    }
    if target(root, "pyproject.toml")?.is_file() || target(root, "pytest.ini")?.is_file() {
        return if cfg!(windows) {
            run(root, &strings(&["py.exe", "-3", "-m", "pytest"]), 300, cancelled)
        } else {
            run(root, &strings(&["python3", "-m", "pytest"]), 300, cancelled)
        };
    }
    if fs::read_dir(root)?.flatten().any(|entry| {
        matches!(
            entry.path().extension().and_then(|s| s.to_str()),
            Some("sln" | "csproj")
        )
    }) {
        return run(root, &strings(&["dotnet", "test"]), 300, cancelled);
    }
    bail!("No supported test setup found (package.json, pytest, or .NET project).")
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[test]
    fn running_command_is_cancelled_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let flag = std::sync::Arc::new(AtomicBool::new(false));
        let trigger = flag.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            trigger.store(true, Ordering::Release);
        });
        let start = Instant::now();
        let result = if cfg!(windows) {
            run(dir.path(), &strings(&["ping.exe", "-n", "30", "127.0.0.1"]), 30, &flag)
        } else {
            run(dir.path(), &strings(&["sleep", "30"]), 30, &flag)
        };
        thread.join().unwrap();
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(8));
    }
}
