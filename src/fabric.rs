//! Fabric loader installation through meta.fabricmc.net. Writes a version
//! JSON that `inheritsFrom` the vanilla version, exactly like the official
//! Fabric installer does.

use crate::http;
use crate::paths::{write_atomic, Paths};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

pub fn meta_base() -> String {
    std::env::var("MC_LAUNCHER_FABRIC_META_URL")
        .unwrap_or_else(|_| "https://meta.fabricmc.net/v2".into())
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderEntry {
    pub loader: LoaderVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct LoaderVersion {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GameEntry {
    pub version: String,
    #[serde(default)]
    pub stable: bool,
}

pub fn version_id(loader: &str, game: &str) -> String {
    format!("fabric-loader-{loader}-{game}")
}

/// Loader versions compatible with `game`, newest first.
pub fn loader_versions(paths: &Paths, game: &str) -> Result<Vec<LoaderEntry>> {
    http::get_json_cached(
        &format!("{}/versions/loader/{game}", meta_base()),
        &paths.cache_dir().join(format!("fabric_loader_{game}.json")),
        Duration::from_secs(6 * 3600),
    )
}

/// All game versions Fabric supports.
pub fn game_versions(paths: &Paths) -> Result<Vec<GameEntry>> {
    http::get_json_cached(
        &format!("{}/versions/game", meta_base()),
        &paths.cache_dir().join("fabric_game.json"),
        Duration::from_secs(6 * 3600),
    )
}

/// Already-installed Fabric version id for `game`, if any.
pub fn installed_for(paths: &Paths, game: &str) -> Option<String> {
    let suffix = format!("-{game}");
    let rd = std::fs::read_dir(paths.versions()).ok()?;
    let mut found: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("fabric-loader-") && n.ends_with(&suffix))
        .filter(|n| paths.version_json(n).is_file())
        .collect();
    found.sort();
    found.pop()
}

/// Install Fabric for `game` (loader = latest stable unless pinned) and return
/// the version id. Reuses an existing installation when possible so a warm
/// launch needs no network.
pub fn install(paths: &Paths, game: &str, pinned_loader: Option<&str>) -> Result<String> {
    if let Some(l) = pinned_loader.filter(|s| !s.is_empty()) {
        let id = version_id(l, game);
        if paths.version_json(&id).is_file() {
            return Ok(id);
        }
    } else if let Some(id) = installed_for(paths, game) {
        return Ok(id);
    }
    let loader = match pinned_loader.filter(|s| !s.is_empty()) {
        Some(l) => l.to_string(),
        None => {
            let list = loader_versions(paths, game)?;
            list.iter()
                .find(|e| e.loader.stable)
                .or_else(|| list.first())
                .map(|e| e.loader.version.clone())
                .ok_or_else(|| anyhow!("Fabric does not support Minecraft {game}"))?
        }
    };
    let url = format!(
        "{}/versions/loader/{game}/{loader}/profile/json",
        meta_base()
    );
    let text = http::get_string(&url).context("downloading Fabric profile")?;
    let json: serde_json::Value = serde_json::from_str(&text).context("parsing Fabric profile")?;
    let id = json["id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| version_id(&loader, game));
    write_atomic(&paths.version_json(&id), text.as_bytes())?;
    Ok(id)
}
