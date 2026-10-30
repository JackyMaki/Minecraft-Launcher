//! Fabric-style loaders (Fabric, Quilt, Legacy Fabric). All three publish a
//! "meta" service with the same shape, and a profile JSON that `inheritsFrom`
//! the vanilla version, exactly like their official installers produce.

use crate::http;
use crate::paths::{write_atomic, Paths};
use crate::profiles::Loader;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Flavor {
    pub loader: Loader,
    /// Version id prefix produced by the official installer.
    pub prefix: &'static str,
    pub env_override: &'static str,
    pub default_base: &'static str,
    /// Path under the base: `/versions/loader/{game}` lives under this.
    pub api: &'static str,
}

pub const FABRIC: Flavor = Flavor {
    loader: Loader::Fabric,
    prefix: "fabric-loader-",
    env_override: "MC_LAUNCHER_FABRIC_META_URL",
    default_base: "https://meta.fabricmc.net",
    api: "/v2",
};
pub const QUILT: Flavor = Flavor {
    loader: Loader::Quilt,
    prefix: "quilt-loader-",
    env_override: "MC_LAUNCHER_QUILT_META_URL",
    default_base: "https://meta.quiltmc.org",
    api: "/v3",
};
pub const LEGACY_FABRIC: Flavor = Flavor {
    loader: Loader::LegacyFabric,
    prefix: "fabric-loader-",
    env_override: "MC_LAUNCHER_LEGACYFABRIC_META_URL",
    default_base: "https://meta.legacyfabric.net",
    api: "/v2",
};

impl Flavor {
    pub fn for_loader(loader: Loader) -> Option<Flavor> {
        match loader {
            Loader::Fabric => Some(FABRIC),
            Loader::Quilt => Some(QUILT),
            Loader::LegacyFabric => Some(LEGACY_FABRIC),
            _ => None,
        }
    }

    /// Base URL including the API version; an environment override replaces it whole.
    pub fn meta_base(&self) -> String {
        match std::env::var(self.env_override) {
            Ok(v) if !v.trim().is_empty() => v.trim_end_matches('/').to_string(),
            _ => format!("{}{}", self.default_base, self.api),
        }
    }

    pub fn version_id(&self, loader_version: &str, game: &str) -> String {
        format!("{}{loader_version}-{game}", self.prefix)
    }
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

/// Loader versions compatible with `game`, newest first.
pub fn loader_versions(paths: &Paths, flavor: Flavor, game: &str) -> Result<Vec<LoaderEntry>> {
    http::get_json_cached(
        &format!("{}/versions/loader/{game}", flavor.meta_base()),
        &paths
            .cache_dir()
            .join(format!("{}_loader_{game}.json", flavor.loader.id())),
        Duration::from_secs(6 * 3600),
    )
}

/// All game versions this loader supports.
pub fn game_versions(paths: &Paths, flavor: Flavor) -> Result<Vec<GameEntry>> {
    http::get_json_cached(
        &format!("{}/versions/game", flavor.meta_base()),
        &paths
            .cache_dir()
            .join(format!("{}_game.json", flavor.loader.id())),
        Duration::from_secs(6 * 3600),
    )
}

/// Already-installed version id of this flavor for `game`, if any.
pub fn installed_for(paths: &Paths, flavor: Flavor, game: &str) -> Option<String> {
    let suffix = format!("-{game}");
    let rd = std::fs::read_dir(paths.versions()).ok()?;
    let mut found: Vec<String> = rd
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(flavor.prefix) && n.ends_with(&suffix))
        .filter(|n| paths.version_json(n).is_file())
        .collect();
    found.sort();
    found.pop()
}

/// Install the loader for `game` (latest stable unless pinned) and return the
/// version id. Reuses an existing installation so a warm launch needs no network.
pub fn install(paths: &Paths, flavor: Flavor, game: &str, pinned: Option<&str>) -> Result<String> {
    let pinned = pinned.filter(|s| !s.trim().is_empty()).map(|s| s.trim());
    if let Some(l) = pinned {
        let id = flavor.version_id(l, game);
        if paths.version_json(&id).is_file() {
            return Ok(id);
        }
    } else if let Some(id) = installed_for(paths, flavor, game) {
        return Ok(id);
    }
    let loader_version = match pinned {
        Some(l) => l.to_string(),
        None => {
            let list = loader_versions(paths, flavor, game)
                .with_context(|| format!("fetching {} loader versions", flavor.loader.label()))?;
            list.iter()
                .find(|e| e.loader.stable)
                .or_else(|| list.first())
                .map(|e| e.loader.version.clone())
                .ok_or_else(|| {
                    anyhow!(
                        "{} does not support Minecraft {game}",
                        flavor.loader.label()
                    )
                })?
        }
    };
    let url = format!(
        "{}/versions/loader/{game}/{loader_version}/profile/json",
        flavor.meta_base()
    );
    let text = http::get_string(&url)
        .with_context(|| format!("downloading {} profile", flavor.loader.label()))?;
    let json: serde_json::Value = serde_json::from_str(&text).context("parsing loader profile")?;
    let id = json["id"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| flavor.version_id(&loader_version, game));
    write_atomic(&paths.version_json(&id), text.as_bytes())?;
    Ok(id)
}
