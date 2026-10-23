//! Locate the Steam (Proton) install of Cyberpunk 2077.

use std::fs;
use std::path::{Path, PathBuf};

const APP_ID: &str = "1091500";

fn steam_roots() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return Vec::new() };
    let mut roots: Vec<PathBuf> = [
        ".steam/steam",
        ".steam/root",
        ".local/share/Steam",
        ".var/app/com.valvesoftware.Steam/.local/share/Steam",
        "snap/steam/common/.local/share/Steam",
    ]
    .iter()
    .map(|r| home.join(r))
    .filter(|p| p.join("steamapps").is_dir())
    .filter_map(|p| p.canonicalize().ok())
    .collect();
    roots.dedup();
    roots
}

/// Extract every value for `key` from a Valve KeyValues (.vdf/.acf) text.
fn vdf_values(text: &str, key: &str) -> Vec<String> {
    let quoted_key = format!("\"{key}\"");
    text.lines()
        .filter_map(|line| {
            let rest = line.trim().strip_prefix(&quoted_key)?;
            let value = rest.trim().strip_prefix('"')?.strip_suffix('"')?;
            Some(value.replace("\\\\", "\\"))
        })
        .collect()
}

fn library_folders(steam_root: &Path) -> Vec<PathBuf> {
    let mut libs = vec![steam_root.to_path_buf()];
    if let Ok(text) = fs::read_to_string(steam_root.join("steamapps/libraryfolders.vdf")) {
        libs.extend(vdf_values(&text, "path").into_iter().map(PathBuf::from));
    }
    libs
}

fn game_in_library(lib: &Path) -> Option<PathBuf> {
    let steamapps = lib.join("steamapps");
    let manifest = fs::read_to_string(steamapps.join(format!("appmanifest_{APP_ID}.acf"))).ok();
    let installdir = manifest
        .and_then(|m| vdf_values(&m, "installdir").into_iter().next())
        .unwrap_or_else(|| "Cyberpunk 2077".into());
    let dir = steamapps.join("common").join(installdir);
    looks_like_game_dir(&dir).then_some(dir)
}

pub fn looks_like_game_dir(dir: &Path) -> bool {
    dir.join("bin/x64/Cyberpunk2077.exe").is_file() || dir.join("archive/pc/content").is_dir()
}

pub fn detect() -> Option<PathBuf> {
    steam_roots()
        .iter()
        .flat_map(|root| library_folders(root))
        .find_map(|lib| game_in_library(&lib))
}

/// True if Cyberpunk2077.exe appears in any running process's command line.
pub fn is_running() -> bool {
    let Ok(procs) = fs::read_dir("/proc") else { return false };
    procs.flatten().any(|p| {
        fs::read(p.path().join("cmdline"))
            .map(|c| String::from_utf8_lossy(&c).contains("Cyberpunk2077.exe"))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY_VDF: &str = r#""libraryfolders"
{
	"0"
	{
		"path"		"/home/me/.local/share/Steam"
		"label"		""
		"apps"
		{
			"228980"		"0"
		}
	}
	"1"
	{
		"path"		"/mnt/games/SteamLibrary"
		"apps"
		{
			"1091500"		"70000000000"
		}
	}
}"#;

    #[test]
    fn parses_library_paths() {
        assert_eq!(
            vdf_values(LIBRARY_VDF, "path"),
            ["/home/me/.local/share/Steam", "/mnt/games/SteamLibrary"]
        );
    }

    #[test]
    fn finds_game_in_library() {
        let t = tempfile::tempdir().unwrap();
        let steamapps = t.path().join("steamapps");
        fs::create_dir_all(steamapps.join("common/CP/bin/x64")).unwrap();
        fs::write(steamapps.join("common/CP/bin/x64/Cyberpunk2077.exe"), "").unwrap();
        fs::write(
            steamapps.join("appmanifest_1091500.acf"),
            "\"AppState\"\n{\n\t\"appid\"\t\t\"1091500\"\n\t\"installdir\"\t\t\"CP\"\n}",
        )
        .unwrap();
        assert_eq!(game_in_library(t.path()), Some(steamapps.join("common/CP")));
    }
}
