use anyhow::{Context, Result, bail};
use std::{env, fs, io::Write, path::PathBuf};

pub struct Settings {
    pub root: PathBuf,
    pub directory: PathBuf,
    pub token: String,
    pub extension_id: String,
}
impl Settings {
    pub fn load() -> Result<Self> {
        let exe = env::current_exe()?;
        // A fresh clone has no config.json (it is intentionally gitignored).
        // Locate the native project using its tracked extension manifest.
        let cwd = env::current_dir().ok();
        let is_project = |p: &std::path::Path| {
            p.join("snapgent-extension/manifest.json").is_file()
        };
        let root = exe
            .ancestors()
            .skip(1)
            .find(|p| is_project(p))
            .map(|p| p.to_path_buf())
            .or_else(|| cwd.clone().filter(|p| is_project(p)))
            .or_else(|| {
                exe.ancestors()
                    .skip(1)
                    .find(|p| p.join("config.json").is_file())
                    .map(|p| p.to_path_buf())
            })
            .or_else(|| cwd.filter(|p| p.join("config.json").is_file()))
            .context("Place Snapgent.exe in the Snapgent project directory")?;
        let directory = env::var_os("LOCALAPPDATA")
            .or_else(|| env::var_os("XDG_STATE_HOME"))
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
            .context("Local application data directory unavailable")?
            .join("SnapgentSecurity");
        fs::create_dir_all(&directory)?;
        let token_path = directory.join("bridge-token");
        let token = if token_path.exists() {
            fs::read_to_string(&token_path)?.trim().to_owned()
        } else {
            let token = snapgent_security::fresh_nonce()?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&token_path)?;
            file.write_all(token.as_bytes())?;
            token
        };
        if !snapgent_security::valid_nonce(&token) {
            bail!("Stored bridge credential is invalid");
        }
        let extension_id = fs::read_to_string(directory.join("trusted-extension-id"))
            .unwrap_or_default()
            .trim()
            .to_owned();
        Ok(Self {
            root,
            directory,
            token,
            extension_id,
        })
    }
    pub fn valid_id(id: &str) -> bool {
        id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b))
    }
    pub fn save_origin(&mut self) -> Result<()> {
        self.extension_id = self.extension_id.trim().to_owned();
        if !Self::valid_id(&self.extension_id) {
            bail!("Enter the 32-character extension ID from chrome://extensions");
        }
        fs::write(
            self.directory.join("trusted-extension-id"),
            &self.extension_id,
        )?;
        Ok(())
    }
    pub fn origins(&self) -> Vec<String> {
        if Self::valid_id(&self.extension_id) {
            vec![format!("chrome-extension://{}", self.extension_id)]
        } else {
            vec![]
        }
    }
}
