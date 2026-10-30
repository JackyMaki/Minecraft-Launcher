//! The global version manifest (`version_manifest_v2.json`).

use crate::http;
use crate::paths::Paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct VersionManifest {
    pub latest: Latest,
    pub versions: Vec<VersionEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Latest {
    #[serde(default)]
    pub release: String,
    #[serde(default)]
    pub snapshot: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct VersionEntry {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
    #[serde(default)]
    pub time: String,
    #[serde(default)]
    pub release_time: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub compliance_level: u32,
}

impl VersionManifest {
    /// Load the manifest, serving a cached copy younger than `max_age` without
    /// touching the network. Pass `Duration::ZERO` to force a refresh.
    pub fn load(paths: &Paths, max_age: Duration) -> Result<Self> {
        let url = std::env::var("MC_LAUNCHER_MANIFEST_URL").unwrap_or_else(|_| MANIFEST_URL.into());
        http::get_json_cached(
            &url,
            &paths.cache_dir().join("version_manifest_v2.json"),
            max_age,
        )
    }

    /// Load only from the disk cache (never blocks on the network).
    pub fn load_cached(paths: &Paths) -> Option<Self> {
        http::read_cache_any_age(&paths.cache_dir().join("version_manifest_v2.json"))
    }

    pub fn get(&self, id: &str) -> Option<&VersionEntry> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// Resolve aliases used in launcher_profiles.json.
    pub fn resolve_alias<'a>(&'a self, id: &'a str) -> &'a str {
        match id {
            "latest-release" => &self.latest.release,
            "latest-snapshot" => &self.latest.snapshot,
            other => other,
        }
    }
}

/// Human friendly kind names, in the order the GUI shows them.
pub const KINDS: &[(&str, &str)] = &[
    ("release", "Releases"),
    ("snapshot", "Snapshots"),
    ("old_beta", "Old Beta"),
    ("old_alpha", "Old Alpha"),
];
