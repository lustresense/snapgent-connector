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
    "SnapgentSecurity",
    ".venv",
    "venv",
    "node_modules",
    "target",
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
        let candidate = local_root(candidate).context("Workspace unavailable")?;
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

fn protected_component(name: &std::ffi::OsStr) -> bool {
    let name = name.to_string_lossy();
    [".git", ".hg", ".svn", "snapgentsecurity"]
        .iter()
        .any(|value| name.eq_ignore_ascii_case(value))
}

pub fn protected_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(component, Component::Normal(name) if protected_component(name))
    })
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
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        ))
                {
                    bail!("Windows device paths and alternate data streams are not permitted.");
                }
            }
        }
    }
    // Validate every component before touching the filesystem. Never probe an
    // absolute, device, UNC or parent-relative path supplied by an agent.
    if path
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        bail!("Use a workspace-relative path without parent traversal");
    }
    if protected_path(path) {
        bail!("Protected metadata and bridge security directories are not accessible");
    }
    let root = local_root(root)?;
    let mut current = root.clone();
    for component in path.components() {
        if let Component::Normal(name) = component {
            current.push(name);
            match fs::symlink_metadata(&current) {
                Ok(meta) => {
                    if is_link_or_reparse(&meta) {
                        bail!("Symbolic links and reparse points are not permitted");
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
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

fn is_link_or_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

pub fn local_root(path: &Path) -> Result<PathBuf> {
    if protected_path(path) {
        bail!("Protected metadata and bridge security directories are not accessible");
    }
    if !path.is_absolute() {
        bail!("Select an absolute local directory");
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match path.components().next() {
            Some(Component::Prefix(p))
                if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
            _ => bail!("Network shares and device paths are not supported in local mode"),
        }
    }
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => bail!("Parent traversal in workspace path is not permitted"),
            Component::CurDir => {}
            Component::Normal(name) => {
                current.push(name);
                let meta = fs::symlink_metadata(&current)?;
                if is_link_or_reparse(&meta) || !meta.is_dir() {
                    bail!("Workspace path must contain only real directories");
                }
            }
            other => current.push(other.as_os_str()),
        }
    }
    let root = fs::canonicalize(&current)?;
    if protected_path(&root) {
        bail!("Protected metadata and bridge security directories are not accessible");
    }
    if !root.is_dir() {
        bail!("Workspace must be a directory");
    }
    #[cfg(windows)]
    {
        use std::path::Prefix;
        match root.components().next() {
            Some(Component::Prefix(p))
                if matches!(p.kind(), Prefix::Disk(_) | Prefix::VerbatimDisk(_)) => {}
            _ => bail!("Resolved workspace is not a local drive path"),
        }
    }
    Ok(root)
}

pub fn safe_directory(path: &Path) -> bool {
    if protected_path(path) {
        return false;
    }
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
        assert!(target(&root, ".git/config").is_err());
        assert!(target(&root, "nested/.GIT/hooks/pre-commit").is_err());
        assert!(target(&root, "SnapgentSecurity/bridge-token").is_err());
        let private = a.join("SnapgentSecurity");
        fs::create_dir(&private).unwrap();
        assert!(local_root(&private).is_err());
        assert!(!safe_directory(&private));
        assert!(target(&root, "sub/file.txt").unwrap().starts_with(&root));
    }
}
