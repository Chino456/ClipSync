#[cfg(target_os = "linux")]
use std::{env, fs, io, path::PathBuf, process::Command};

#[cfg(target_os = "linux")]
const UUID: &str = "clipsync@local";
#[cfg(target_os = "linux")]
const EXTENSION_JS: &str = include_str!("../../../gnome-extension/extension.js");
#[cfg(target_os = "linux")]
const METADATA_JSON: &str = include_str!("../../../gnome-extension/metadata.json");

#[cfg(target_os = "linux")]
pub fn install() -> Result<(), Box<dyn std::error::Error>> {
    let home = env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is not set"))?;
    let directory = PathBuf::from(home)
        .join(".local/share/gnome-shell/extensions")
        .join(UUID);
    fs::create_dir_all(&directory)?;
    fs::write(directory.join("extension.js"), EXTENSION_JS)?;
    fs::write(directory.join("metadata.json"), METADATA_JSON)?;

    println!("Installed GNOME extension at {}", directory.display());
    println!("Log out and back in once, then run:");
    println!("  gnome-extensions enable {UUID}");
    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn install() -> Result<(), Box<dyn std::error::Error>> {
    Err("the GNOME extension is available only on Linux".into())
}

#[cfg(target_os = "linux")]
pub fn is_enabled() -> bool {
    let Ok(output) = Command::new("gnome-extensions")
        .args(["list", "--enabled"])
        .output()
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == UUID)
}
