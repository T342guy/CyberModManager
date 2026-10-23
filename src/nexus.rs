//! Minimal Nexus Mods v1 API client.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::{Client, Response};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::Deserialize;

pub const GAME_DOMAIN: &str = "cyberpunk2077";
const API: &str = "https://api.nexusmods.com/v1";

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    pub name: String,
    #[serde(default)]
    pub is_premium: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileInfo {
    pub file_id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub category_name: Option<String>,
    #[serde(default)]
    pub file_name: String,
    #[serde(default)]
    pub size_kb: Option<u64>,
    #[serde(default)]
    pub uploaded_timestamp: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileUpdate {
    pub old_file_id: u64,
    pub new_file_id: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FileList {
    pub files: Vec<FileInfo>,
    #[serde(default)]
    pub file_updates: Vec<FileUpdate>,
}

impl FileList {
    /// Follow the update chain from `file_id` to the newest replacement, if any.
    pub fn newest_replacement(&self, file_id: u64) -> Option<&FileInfo> {
        let mut cur = file_id;
        // Bounded walk in case the chain has a cycle.
        for _ in 0..self.file_updates.len() {
            match self.file_updates.iter().find(|u| u.old_file_id == cur) {
                Some(u) => cur = u.new_file_id,
                None => break,
            }
        }
        (cur != file_id).then(|| self.files.iter().find(|f| f.file_id == cur)).flatten()
    }

    /// Visible (not archived/removed) files, main files first, newest first.
    pub fn visible(&self) -> Vec<&FileInfo> {
        let rank = |f: &FileInfo| match f.category_name.as_deref() {
            Some("MAIN") => 0,
            Some("UPDATE") => 1,
            Some("OPTIONAL") => 2,
            Some("MISCELLANEOUS") => 3,
            Some("OLD_VERSION") => 5,
            _ => 4,
        };
        let mut out: Vec<&FileInfo> = self
            .files
            .iter()
            .filter(|f| !matches!(f.category_name.as_deref(), Some("ARCHIVED") | Some("REMOVED")))
            .collect();
        out.sort_by_key(|f| (rank(f), std::cmp::Reverse(f.uploaded_timestamp)));
        out
    }
}

#[derive(Debug, Clone, Deserialize)]
struct DownloadLink {
    #[serde(rename = "URI")]
    uri: String,
}

pub struct Nexus {
    client: Client,
}

impl Nexus {
    pub fn new(api_key: &str) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert("apikey", HeaderValue::from_str(api_key.trim()).context("invalid API key")?);
        headers.insert("Application-Name", HeaderValue::from_static("CyberModManager"));
        headers.insert("Application-Version", HeaderValue::from_static(env!("CARGO_PKG_VERSION")));
        let client = Client::builder()
            .user_agent(concat!("CyberModManager/", env!("CARGO_PKG_VERSION")))
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(20))
            .build()?;
        Ok(Self { client })
    }

    fn get_json<T: for<'de> Deserialize<'de>>(&self, path: &str) -> Result<T> {
        let resp = self.client.get(format!("{API}{path}")).send().with_context(|| format!("GET {path}"))?;
        let resp = check(resp)?;
        resp.json().with_context(|| format!("decoding {path}"))
    }

    pub fn validate(&self) -> Result<User> {
        self.get_json("/users/validate.json")
    }

    pub fn mod_info(&self, mod_id: u64) -> Result<ModInfo> {
        self.get_json(&format!("/games/{GAME_DOMAIN}/mods/{mod_id}.json"))
    }

    pub fn files(&self, mod_id: u64) -> Result<FileList> {
        self.get_json(&format!("/games/{GAME_DOMAIN}/mods/{mod_id}/files.json"))
    }

    /// Premium users can call this freely; free users need the key/expires
    /// pair from an nxm:// link.
    pub fn download_url(&self, mod_id: u64, file_id: u64, nxm_key: Option<(&str, u64)>) -> Result<String> {
        let mut path = format!("/games/{GAME_DOMAIN}/mods/{mod_id}/files/{file_id}/download_link.json");
        if let Some((key, expires)) = nxm_key {
            path.push_str(&format!("?key={key}&expires={expires}"));
        }
        let links: Vec<DownloadLink> = self.get_json(&path).map_err(|e| {
            if e.to_string().contains("403") {
                e.context(
                    "Nexus refused the download. Direct downloads need Premium; free accounts must use \
                     the \"Mod Manager Download\" button on the mod page (nxm link).",
                )
            } else {
                e
            }
        })?;
        links.into_iter().next().map(|l| l.uri).context("Nexus returned no download links")
    }
}

/// Download `url` into `dir/file_name`, reporting (downloaded, total) bytes.
/// Used for pre-signed Nexus CDN links and GitHub release assets, so no API
/// key is sent.
pub fn download(url: &str, dir: &Path, file_name: &str, mut progress: impl FnMut(u64, Option<u64>)) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let name = Path::new(file_name).file_name().context("bad file name")?;
    let dest = dir.join(name);
    let part = dest.with_extension("part");
    let client = Client::builder().user_agent(concat!("CyberModManager/", env!("CARGO_PKG_VERSION"))).build()?;
    let mut resp = client.get(url).send().context("starting download")?.error_for_status().context("download failed")?;
    let total = resp.content_length();
    let mut out = File::create(&part)?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        let n = resp.read(&mut buf).context("download interrupted")?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    out.flush()?;
    std::fs::rename(&part, &dest)?;
    Ok(dest)
}

fn check(resp: Response) -> Result<Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().unwrap_or_default();
    let msg = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(str::to_string))
        .unwrap_or(body);
    bail!("Nexus API error {status}: {msg}")
}

/// Accept "12345", or a mod page URL like https://www.nexusmods.com/cyberpunk2077/mods/12345?tab=files
pub fn parse_mod_ref(input: &str) -> Option<u64> {
    let s = input.trim();
    if let Ok(id) = s.parse() {
        return Some(id);
    }
    let after = s.split("/mods/").nth(1)?;
    after.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mod_ref_parsing() {
        assert_eq!(parse_mod_ref(" 107 "), Some(107));
        assert_eq!(parse_mod_ref("https://www.nexusmods.com/cyberpunk2077/mods/107?tab=files"), Some(107));
        assert_eq!(parse_mod_ref("nope"), None);
    }

    #[test]
    fn update_chain() {
        let list: FileList = serde_json::from_str(
            r#"{"files":[{"file_id":1,"name":"a","category_name":"OLD_VERSION"},
                         {"file_id":3,"name":"c","version":"1.2","category_name":"MAIN"}],
                "file_updates":[{"old_file_id":1,"new_file_id":2},{"old_file_id":2,"new_file_id":3}]}"#,
        )
        .unwrap();
        assert_eq!(list.newest_replacement(1).map(|f| f.file_id), Some(3));
        assert!(list.newest_replacement(3).is_none());
        assert_eq!(list.visible()[0].file_id, 3);
    }
}
