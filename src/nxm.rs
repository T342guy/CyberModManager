//! `nxm://` links (Nexus "Mod Manager Download" button), the "Open with"
//! desktop entry, and single-instance hand-off of both.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};

use crate::nexus::GAME_DOMAIN;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NxmLink {
    pub mod_id: u64,
    pub file_id: u64,
    pub key: Option<String>,
    pub expires: Option<u64>,
}

impl NxmLink {
    pub fn parse(s: &str) -> Result<Self> {
        let url = url::Url::parse(s.trim()).context("invalid nxm link")?;
        if url.scheme() != "nxm" {
            bail!("not an nxm:// link");
        }
        let game = url.host_str().unwrap_or_default();
        if !game.eq_ignore_ascii_case(GAME_DOMAIN) {
            bail!("link is for '{game}', not Cyberpunk 2077");
        }
        let segs: Vec<&str> = url.path_segments().map(|s| s.collect()).unwrap_or_default();
        let (mod_id, file_id) = match segs.as_slice() {
            ["mods", m, "files", f] => (m.parse()?, f.parse()?),
            _ => bail!("unexpected nxm link path: {}", url.path()),
        };
        let query = |k: &str| url.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
        Ok(Self {
            mod_id,
            file_id,
            key: query("key"),
            expires: query("expires").and_then(|e| e.parse().ok()),
        })
    }
}

/// Try handing `link` (an nxm:// link or archive path) to a running instance. Returns true if one accepted it.
pub fn forward_to_running(link: &str) -> bool {
    let Ok(mut stream) = UnixStream::connect(crate::paths::AppPaths::ipc_socket()) else {
        return false;
    };
    writeln!(stream, "{link}").is_ok()
}

/// Listen for links from later invocations; `on_link` is called on a background thread.
pub fn listen(on_link: impl Fn(String) + Send + 'static) -> Result<()> {
    let path = crate::paths::AppPaths::ipc_socket();
    if path.exists() {
        if UnixStream::connect(&path).is_ok() {
            bail!("another instance is already listening");
        }
        std::fs::remove_file(&path)?; // stale socket from a crash
    }
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut line = String::new();
            if BufReader::new(stream).read_line(&mut line).is_ok() && !line.trim().is_empty() {
                on_link(line.trim().to_string());
            }
        }
    });
    Ok(())
}

const DESKTOP_FILE: &str = "cyber-mod-manager-nxm.desktop";

/// Register this executable as the system handler for nxm:// links.
pub fn register_handler() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let apps = directories::BaseDirs::new()
        .context("no home directory")?
        .data_dir()
        .join("applications");
    std::fs::create_dir_all(&apps)?;
    let desktop = apps.join(DESKTOP_FILE);
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Cyber Mod Manager (nxm handler)\n\
         Exec=\"{}\" %u\nMimeType=x-scheme-handler/nxm;\nNoDisplay=true\nTerminal=false\n",
        exe.display()
    );
    std::fs::write(&desktop, content)?;
    let status = Command::new("xdg-mime")
        .args(["default", DESKTOP_FILE, "x-scheme-handler/nxm"])
        .status()
        .context("running xdg-mime (is xdg-utils installed?)")?;
    if !status.success() {
        bail!("xdg-mime failed with {status}");
    }
    let _ = Command::new("update-desktop-database").arg(&apps).status();
    Ok(desktop)
}

const OPEN_WITH_FILE: &str = "cyber-mod-manager.desktop";
const ARCHIVE_TYPES: &str =
    "application/zip;application/x-7z-compressed;application/vnd.rar;application/x-rar;application/x-rar-compressed;";

fn open_with_path() -> Result<PathBuf> {
    Ok(directories::BaseDirs::new().context("no home directory")?.data_dir().join("applications").join(OPEN_WITH_FILE))
}

pub fn open_with_registered() -> bool {
    open_with_path().is_ok_and(|p| p.is_file())
}

/// Offer this app under "Open with" for mod archives, without making it the
/// default for them (archives keep opening in the usual app).
pub fn register_open_with() -> Result<PathBuf> {
    let exe = std::env::current_exe()?;
    let desktop = open_with_path()?;
    let apps = desktop.parent().context("no applications dir")?;
    std::fs::create_dir_all(apps)?;
    let content = format!(
        "[Desktop Entry]\nType=Application\nName=Cyber Mod Manager\nComment=Install Cyberpunk 2077 mods\n\
         Exec=\"{}\" %F\nMimeType={ARCHIVE_TYPES}\nCategories=Game;Utility;\nTerminal=false\n",
        exe.display()
    );
    std::fs::write(&desktop, content)?;
    let _ = Command::new("update-desktop-database").arg(apps).status();
    Ok(desktop)
}

pub fn unregister_open_with() -> Result<()> {
    let desktop = open_with_path()?;
    if desktop.is_file() {
        std::fs::remove_file(&desktop)?;
    }
    if let Some(apps) = desktop.parent() {
        let _ = Command::new("update-desktop-database").arg(apps).status();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_free_user_link() {
        let l = NxmLink::parse("nxm://cyberpunk2077/mods/107/files/98765?key=abcDEF&expires=1700000000&user_id=42")
            .unwrap();
        assert_eq!(
            l,
            NxmLink { mod_id: 107, file_id: 98765, key: Some("abcDEF".into()), expires: Some(1700000000) }
        );
    }

    #[test]
    fn parses_premium_link_and_rejects_other_games() {
        let l = NxmLink::parse("nxm://Cyberpunk2077/mods/1/files/2").unwrap();
        assert_eq!((l.mod_id, l.file_id, l.key), (1, 2, None));
        assert!(NxmLink::parse("nxm://skyrimspecialedition/mods/1/files/2").is_err());
        assert!(NxmLink::parse("https://nexusmods.com").is_err());
    }
}
