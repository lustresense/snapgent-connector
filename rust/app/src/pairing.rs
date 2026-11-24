use crate::settings::Settings;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{Read, Write};

const HOST: &str = "local.snapgent.pairing";

pub fn native_request(origin: &str) -> Result<()> {
    let settings = Settings::load()?;
    if !Settings::valid_id(&settings.extension_id)
        || !settings
            .origins()
            .iter()
            .any(|allowed| allowed == origin.trim_end_matches('/'))
    {
        bail!("Untrusted extension origin");
    }
    let mut input = std::io::stdin().lock();
    let mut header = [0u8; 4];
    input.read_exact(&mut header)?;
    let len = u32::from_le_bytes(header) as usize;
    if len == 0 || len > 4096 {
        bail!("Invalid native request size");
    }
    let mut bytes = vec![0; len];
    input.read_exact(&mut bytes)?;
    let message: Value = serde_json::from_slice(&bytes)?;
    if message["type"] != "pair" {
        bail!("Unsupported native request");
    }
    let reply = serde_json::to_vec(&json!({"ok":true,"token":settings.token}))?;
    let mut out = std::io::stdout().lock();
    out.write_all(&(reply.len() as u32).to_le_bytes())?;
    out.write_all(&reply)?;
    out.flush()?;
    Ok(())
}

#[cfg(windows)]
pub fn register(settings: &Settings) -> Result<()> {
    use std::os::windows::process::CommandExt;
    if !Settings::valid_id(&settings.extension_id) {
        bail!("Save a valid extension ID first");
    }
    let exe = std::env::current_exe()?.canonicalize()?;
    let exe = exe
        .to_str()
        .context("Executable path is not UTF-8")?
        .trim_start_matches("\\\\?\\");
    let manifest = settings.directory.join("native-messaging.json");
    let data = json!({"name":HOST,"description":"Snapgent local pairing","path":exe,"type":"stdio","allowed_origins":[format!("chrome-extension://{}/",settings.extension_id)]});
    std::fs::write(&manifest, serde_json::to_vec_pretty(&data)?)?;
    for browser in [
        "Google\\Chrome",
        "Microsoft\\Edge",
        "BraveSoftware\\Brave-Browser",
        "Chromium",
        "Vivaldi",
    ] {
        let key = format!("HKCU\\Software\\{browser}\\NativeMessagingHosts\\{HOST}");
        let status = std::process::Command::new("reg.exe")
            .args(["add", &key, "/ve", "/t", "REG_SZ", "/d"])
            .arg(&manifest)
            .args(["/f"])
            .creation_flags(0x08000000)
            .status()?;
        if !status.success() {
            bail!("Native Messaging registration failed for {browser}");
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn register(_settings: &Settings) -> Result<()> {
    bail!("Automatic registration is currently supported on Windows only; use manual pairing")
}
