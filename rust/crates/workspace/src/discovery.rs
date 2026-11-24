use anyhow::{Result, bail};
use std::{path::PathBuf, process::Command};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    Local,
}

impl Editor {
    pub fn id(self) -> &'static str {
        "local"
    }
    pub fn display(self) -> &'static str {
        "Local Workspace"
    }
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

pub fn active_workspace(_editor: Editor) -> Result<PathBuf> {
    bail!("Select and approve an explicit local workspace folder first.")
}

pub fn workspace_choices(_editor: Editor) -> Vec<PathBuf> {
    Vec::new()
}
