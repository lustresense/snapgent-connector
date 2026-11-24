use crate::paths::{IGNORED, MAX_FILE_CHARS, MAX_RESULTS, relative, safe_directory, target};
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

pub fn read_text(path: &Path) -> Result<String> {
    let data = fs::read(path)?;
    Ok(String::from_utf8_lossy(&data)
        .replace("\r\n", "\n")
        .replace('\r', "\n"))
}

pub fn read_file(root: &Path, path: &str) -> Result<String> {
    let p = target(root, path)?;
    if !p.is_file() {
        bail!("File not found: {path}");
    }
    let text = read_text(&p)?;
    if text.chars().count() > MAX_FILE_CHARS {
        Ok(text.chars().take(MAX_FILE_CHARS).collect::<String>() + "\n[truncated]")
    } else {
        Ok(text)
    }
}

fn parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

pub fn write_file(root: &Path, path: &str, content: &str, create: bool) -> Result<String> {
    let p = target(root, path)?;
    parent(&p)?;
    if create {
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&p)?
            .write_all(content.as_bytes())?;
    } else {
        fs::write(&p, content)?;
    }
    Ok(format!(
        "{} {}",
        if create { "Created" } else { "Saved" },
        relative(root, &p)
    ))
}

pub fn list_dir(root: &Path, path: &str) -> Result<Value> {
    let p = target(root, path)?;
    let mut entries = fs::read_dir(p)?
        .filter_map(|e| e.ok())
        .filter(|e| target(root, &relative(root, &e.path())).is_ok())
        .collect::<Vec<_>>();
    entries.sort_by_key(|e| {
        (
            !e.path().is_dir(),
            e.file_name().to_string_lossy().to_lowercase(),
        )
    });
    Ok(json!(entries.into_iter().map(|e| json!({"name":e.file_name().to_string_lossy(),"path":relative(root,&e.path()),"type":if e.path().is_dir(){"directory"}else{"file"}})).collect::<Vec<_>>()))
}

pub fn walk(root: &Path, base: &Path, depth: usize, include_dirs: bool) -> Vec<PathBuf> {
    fn visit(
        root: &Path,
        p: &Path,
        depth: usize,
        max_depth: usize,
        include_dirs: bool,
        out: &mut Vec<PathBuf>,
    ) {
        if !safe_directory(p) {
            return;
        }
        let Ok(entries) = fs::read_dir(p) else { return };
        let mut dirs = Vec::new();
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if target(root, &relative(root, &path)).is_err() {
                continue;
            }
            if path.is_dir() {
                if safe_directory(&path)
                    && !IGNORED.contains(&entry.file_name().to_string_lossy().as_ref())
                {
                    dirs.push(path);
                }
            } else if path.is_file() {
                files.push(path);
            }
        }
        dirs.sort();
        files.sort();
        if include_dirs {
            out.extend(dirs.iter().cloned());
        }
        out.extend(files);
        if depth < max_depth {
            for dir in dirs {
                visit(root, &dir, depth + 1, max_depth, include_dirs, out);
            }
        }
    }
    let mut out = Vec::new();
    if base.is_file() {
        out.push(base.to_owned());
    } else {
        visit(root, base, 0, depth, include_dirs, &mut out);
    }
    out
}

pub fn list_tree(root: &Path, path: &str, depth: usize) -> Result<Value> {
    let p = target(root, path)?;
    if !p.is_dir() {
        bail!("Not a directory: {path}");
    }
    Ok(json!(
        walk(root, &p, depth.min(8), true)
            .into_iter()
            .take(MAX_RESULTS)
            .map(
                |p| json!({"path":relative(root,&p),"type":if p.is_dir(){"directory"}else{"file"}})
            )
            .collect::<Vec<_>>()
    ))
}

pub fn find_files(root: &Path, path: &str, pattern: &str) -> Result<Value> {
    let p = target(root, path)?;
    if !p.is_dir() {
        bail!("Not a directory: {path}");
    }
    let pattern = glob::Pattern::new(pattern)?;
    let options = glob::MatchOptions {
        case_sensitive: !cfg!(windows),
        require_literal_separator: false,
        require_literal_leading_dot: false,
    };
    Ok(json!(
        walk(root, &p, usize::MAX, false)
            .into_iter()
            .filter(|f| pattern.matches_with(
                &f.file_name().unwrap_or_default().to_string_lossy(),
                options
            ) || pattern.matches_with(&relative(&p, f), options))
            .take(MAX_RESULTS)
            .map(|f| relative(root, &f))
            .collect::<Vec<_>>()
    ))
}

pub fn search(
    root: &Path,
    path: &str,
    query: &str,
    pattern: &str,
    case_sensitive: bool,
) -> Result<Value> {
    if query.trim().is_empty() {
        bail!("Search query cannot be empty.");
    }
    let p = target(root, path)?;
    if !p.exists() {
        bail!("Path not found: {path}");
    }
    let pattern = glob::Pattern::new(pattern)?;
    let needle = if case_sensitive {
        query.to_owned()
    } else {
        query.to_lowercase()
    };
    let mut out = Vec::new();
    for f in walk(root, &p, usize::MAX, false) {
        if !pattern.matches_with(
            &f.file_name().unwrap_or_default().to_string_lossy(),
            glob::MatchOptions {
                case_sensitive: !cfg!(windows),
                require_literal_separator: false,
                require_literal_leading_dot: false,
            },
        ) {
            continue;
        }
        let Ok(safe) = target(root, &relative(root, &f)) else {
            continue;
        };
        let Ok(text) = read_text(&safe) else { continue };
        for (i, line) in text.lines().enumerate() {
            let haystack = if case_sensitive {
                line.to_owned()
            } else {
                line.to_lowercase()
            };
            if haystack.contains(&needle) {
                out.push(json!({"path":relative(root,&f),"line":i+1,"text":line.chars().take(500).collect::<String>()}));
                if out.len() >= MAX_RESULTS {
                    return Ok(json!(out));
                }
            }
        }
    }
    Ok(json!(out))
}

pub fn info(root: &Path, path: &str) -> Result<Value> {
    let p = target(root, path)?;
    let m = fs::metadata(&p)?;
    Ok(
        json!({"path":relative(root,&p),"type":if m.is_dir(){"directory"}else{"file"},"size_bytes":m.len(),"modified":m.modified()?.duration_since(UNIX_EPOCH)?.as_secs_f64(),"suffix":p.extension().map(|s|format!(".{}",s.to_string_lossy())).unwrap_or_default()}),
    )
}

pub fn edit(root: &Path, path: &str, old: &str, new: &str, all: bool) -> Result<String> {
    if old.is_empty() {
        bail!("old_text cannot be empty.");
    }
    let p = target(root, path)?;
    let text = read_text(&p)?;
    let count = text.matches(old).count();
    if count == 0 {
        bail!("The exact old_text was not found.");
    }
    if !all && count != 1 {
        bail!("old_text occurs {count} times; provide a unique snippet or set replace_all.");
    }
    let updated = if all {
        text.replace(old, new)
    } else {
        text.replacen(old, new, 1)
    };
    fs::write(&p, updated)?;
    Ok(format!(
        "Updated {} ({} replacement(s))",
        relative(root, &p),
        if all { count } else { 1 }
    ))
}

pub fn patch(root: &Path, path: &str, edits: &[Value]) -> Result<String> {
    if edits.is_empty() {
        bail!("edits must be a non-empty list.");
    }
    let p = target(root, path)?;
    let mut text = read_text(&p)?;
    for (i, edit) in edits.iter().enumerate() {
        let old = edit["old_text"]
            .as_str()
            .filter(|v| !v.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Edit {} needs a non-empty old_text", i + 1))?;
        let new = edit["new_text"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Edit {} needs a string new_text", i + 1))?;
        let count = text.matches(old).count();
        if count != 1 {
            bail!(
                "Edit {}: old_text occurs {count} times; it must match exactly once. No changes were written.",
                i + 1
            );
        }
        text = text.replacen(old, new, 1);
    }
    fs::write(&p, text)?;
    Ok(format!(
        "Patched {} ({} edit(s))",
        relative(root, &p),
        edits.len()
    ))
}

pub fn directory(root: &Path, path: &str) -> Result<String> {
    let p = target(root, path)?;
    fs::create_dir_all(&p)?;
    Ok(format!("Directory ready: {}", relative(root, &p)))
}
pub fn transfer(root: &Path, source: &str, destination: &str, copy: bool) -> Result<String> {
    let a = target(root, source)?;
    let b = target(root, destination)?;
    if !a.is_file() {
        bail!("File not found: {source}");
    }
    if b.exists() {
        bail!("Destination already exists: {destination}");
    }
    parent(&b)?;
    if copy {
        let mut input = fs::File::open(&a)?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&b)?;
        std::io::copy(&mut input, &mut output)?;
    } else {
        fs::rename(&a, &b)?;
    }
    Ok(format!(
        "{} {} to {}",
        if copy { "Copied" } else { "Moved" },
        relative(root, &a),
        relative(root, &b)
    ))
}
pub fn delete(root: &Path, path: &str) -> Result<String> {
    let p = target(root, path)?;
    if p == root
        || p.strip_prefix(root)?
            .components()
            .any(|c| c.as_os_str() == ".git")
    {
        bail!("Refusing to delete protected path: {path}");
    }
    if !p.is_file() {
        bail!("Not a file: {path}");
    }
    fs::remove_file(&p)?;
    Ok(format!("Deleted {}", relative(root, &p)))
}
