//! Latest-release lookups on GitHub (no account needed).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

impl Release {
    pub fn version(&self) -> String {
        self.tag_name.trim_start_matches(['v', 'V']).to_string()
    }
}

pub fn client() -> Result<Client> {
    Ok(Client::builder()
        .user_agent(concat!("CyberModManager/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .build()?)
}

pub fn latest_release(client: &Client, repo: &str) -> Result<Release> {
    let resp = client
        .get(format!("https://api.github.com/repos/{repo}/releases/latest"))
        .header("Accept", "application/vnd.github+json")
        .send()
        .with_context(|| format!("asking GitHub about {repo}"))?;
    let status = resp.status();
    if status == reqwest::StatusCode::FORBIDDEN || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        bail!("GitHub rate limit reached (60 requests per hour without an account), try again later");
    }
    if !status.is_success() {
        bail!("GitHub returned {status} for {repo}");
    }
    resp.json().with_context(|| format!("decoding release info for {repo}"))
}
