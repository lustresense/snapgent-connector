use anyhow::{Context, Result, bail};
use std::{
    fs,
    path::{Component, Path, PathBuf},
};

pub const MAX_RESULTS: usize = 500;
pub const MAX_FILE_CHARS: usize = 200_000;
pub const IGNORED: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".venv",
    "venv",
    "node_modules",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".next",
    "dist",
    "build",
];

#[derive(Default)]
pub struct WorkspacePin(Option<PathBuf>);
impl WorkspacePin {
    pub fn pin(&mut self, candidate: &Path) -> Result<PathBuf> {
        let candidate = fs::canonicalize(candidate).context("Workspace unavailable")?;
        if !candidate.is_dir() {
            bail!("Workspace must be a directory.");
        }
        if let Some(root) = &self.0 {
            if root != &candidate {
                bail!(
                    "Workspace changed. Restart MCP intentionally before accessing the new project."
                );
            }
        } else {
            self.0 = Some(candidate.clone());
        }
        Ok(candidate)
    }
}

pub fn target(root: &Path, input: &str) -> Result<PathBuf> {
    let path = Path::new(if input.is_empty() { "." } else { input });
    #[cfg(windows)]
    {
        if input.starts_with("\\\\?\\") || input.starts_with("\\\\.\\") {
            bail!("Windows device namespace is not permitted");
        }
        for component in path.components() {
            if let Component::Normal(name) = component {
                let s = name.to_string_lossy();
                let stem = s
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .trim_end()
                    .to_ascii_uppercase();
                if s.contains(':')
                    || s.ends_with(' ')
                    || s.ends_with('.')
                    || matches!(
                        stem.as_str(),
                        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
                    )
                    || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                        && matches!(
                            stem.get(3..),
                            Some(
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "Â¹"
                                    | "Â²"
                                    | "Â³"
                            )
                        ))
                {
                    bail!("Windows device paths and alternate data streams are not permitted.");
                }
            }
        }
    }
    let root = fs::canonicalize(root)?;
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let mut current = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                current.push(component.as_os_str());
            }
            Component::ParentDir => {
                if !current.pop() {
                    bail!("Path escapes workspace root");
                }
            }
            other => {
                current.push(other.as_os_str());
                match fs::symlink_metadata(&current) {
                    Ok(_) => current = fs::canonicalize(&current)?,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    if !current.starts_with(&root) {
        bail!("Path escapes workspace root: {input}");
    }
    Ok(current)
}

pub fn relative(root: &Path, path: &Path) -> String {
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    if relative.is_empty() {
        ".".into()
    } else {
        relative
    }
}

pub fn safe_directory(path: &Path) -> bool {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return false;
    };
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape_and_pins_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let mut pin = WorkspacePin::default();
        let root = pin.pin(&a).unwrap();
        assert!(pin.pin(&b).is_err());
        assert!(target(&root, "../b/file.txt").is_err());
        assert!(target(&root, "sub/file.txt").unwrap().starts_with(&root));
    }
}
