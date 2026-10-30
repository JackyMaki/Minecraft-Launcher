//! Asset index handling (sounds, languages, textures for very old versions).

use crate::download::Task;
use crate::paths::Paths;
use crate::version::AssetIndexRef;
use crate::{report, Progress, ProgressSink};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssetIndex {
    #[serde(default)]
    pub objects: HashMap<String, AssetObject>,
    /// Pre-1.7.3: assets are laid out by name under `assets/virtual/<id>`.
    #[serde(default, rename = "virtual")]
    pub is_virtual: bool,
    /// Pre-1.6: assets live in `<game>/resources`.
    #[serde(default)]
    pub map_to_resources: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AssetObject {
    pub hash: String,
    #[serde(default)]
    pub size: u64,
}

impl AssetObject {
    pub fn rel_path(&self) -> String {
        format!("{}/{}", &self.hash[..2], self.hash)
    }
    pub fn url(&self) -> String {
        let base =
            std::env::var("MC_LAUNCHER_RESOURCES_URL").unwrap_or_else(|_| RESOURCES_URL.into());
        format!("{}/{}", base.trim_end_matches('/'), self.rel_path())
    }
}

/// Ensure the asset index file exists locally and parse it.
pub fn ensure_index(paths: &Paths, idx: &AssetIndexRef, verify: bool) -> Result<AssetIndex> {
    let dest = paths.asset_indexes().join(format!("{}.json", idx.id));
    let task = Task::new(&idx.url, &dest)
        .sha1(Some(idx.sha1.clone()))
        .size(Some(idx.size));
    if !crate::download::is_satisfied(&task, verify) {
        crate::download::fetch_one(&task)?;
    }
    let text =
        std::fs::read_to_string(&dest).with_context(|| format!("reading {}", dest.display()))?;
    serde_json::from_str(&text).context("parsing asset index")
}

/// Download all missing asset objects and lay out legacy copies if needed.
pub fn ensure_objects(
    paths: &Paths,
    index_id: &str,
    index: &AssetIndex,
    game_dir: &Path,
    verify: bool,
    parallel: usize,
    sink: &ProgressSink,
) -> Result<()> {
    report(sink, Progress::Phase("Checking assets".into()));
    let objects = paths.asset_objects();
    let mut tasks: Vec<Task> = Vec::with_capacity(index.objects.len());
    let mut seen = std::collections::HashSet::new();
    for obj in index.objects.values() {
        if !seen.insert(obj.hash.clone()) {
            continue;
        }
        tasks.push(
            Task::new(obj.url(), objects.join(obj.rel_path()))
                .sha1(Some(obj.hash.clone()))
                .size(Some(obj.size)),
        );
    }
    report(
        sink,
        Progress::Phase(format!("Downloading assets ({} objects)", tasks.len())),
    );
    crate::download::fetch_all(&tasks, verify, parallel, sink)?;

    if index.is_virtual || index.map_to_resources {
        let target_root = if index.map_to_resources {
            game_dir.join("resources")
        } else {
            paths.asset_virtual(index_id)
        };
        report(sink, Progress::Phase("Laying out legacy assets".into()));
        for (name, obj) in &index.objects {
            let dst = target_root.join(name);
            let src = objects.join(obj.rel_path());
            let ok = std::fs::metadata(&dst)
                .map(|m| m.len() == obj.size)
                .unwrap_or(false);
            if !ok {
                crate::paths::ensure_parent(&dst)?;
                std::fs::copy(&src, &dst).with_context(|| format!("copying asset {name}"))?;
            }
        }
    }
    Ok(())
}
