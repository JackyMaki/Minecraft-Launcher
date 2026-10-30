//! Mod installation from Modrinth, including the "performance pack" that
//! gives vanilla-like Fabric profiles smooth frame rates and fast world loads.

use crate::download::Task;
use crate::http;
use crate::paths::{write_atomic, Paths};
use crate::profiles::Loader;
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::time::Duration;

pub fn api_base() -> String {
    std::env::var("MC_LAUNCHER_MODRINTH_URL")
        .unwrap_or_else(|_| "https://api.modrinth.com/v2".into())
}

/// Client-side performance mods (Modrinth slugs). Order matters only for logs.
pub const PERF_MODS: &[(&str, &str)] = &[
    ("fabric-api", "Fabric API (required by most mods)"),
    ("sodium", "Sodium: modern rendering engine, large FPS boost"),
    (
        "lithium",
        "Lithium: game logic optimisations, no behaviour changes",
    ),
    ("ferrite-core", "FerriteCore: lower memory usage"),
    (
        "immediatelyfast",
        "ImmediatelyFast: faster immediate-mode rendering (UI, text)",
    ),
    (
        "entityculling",
        "Entity Culling: skip rendering of hidden entities",
    ),
    (
        "modernfix",
        "ModernFix: much faster game start-up and lower memory",
    ),
    (
        "dynamic-fps",
        "Dynamic FPS: stop wasting GPU when unfocused",
    ),
    (
        "krypton",
        "Krypton: faster networking, quicker server joins",
    ),
    (
        "c2me-fabric",
        "C2ME: parallel chunk loading, worlds open much faster",
    ),
];

#[derive(Debug, Clone, Deserialize)]
pub struct ModVersion {
    pub id: String,
    pub project_id: String,
    #[serde(default)]
    pub version_number: String,
    #[serde(default)]
    pub version_type: String,
    #[serde(default)]
    pub date_published: String,
    #[serde(default)]
    pub files: Vec<ModFile>,
    #[serde(default)]
    pub dependencies: Vec<Dependency>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModFile {
    pub url: String,
    pub filename: String,
    #[serde(default)]
    pub primary: bool,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dependency {
    #[serde(default)]
    pub project_id: Option<String>,
    #[serde(default)]
    pub version_id: Option<String>,
    #[serde(default)]
    pub dependency_type: String,
}

/// Which jar we manage for each project, so updates replace old files.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManagedMods {
    /// game version these entries were resolved for
    #[serde(default)]
    pub game_version: String,
    /// loader these entries were resolved for
    #[serde(default)]
    pub loader: String,
    /// project id (or slug) -> file name
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl ManagedMods {
    fn path(mods_dir: &Path) -> std::path::PathBuf {
        mods_dir.join(".launcher_mods.json")
    }
    pub fn load(mods_dir: &Path) -> Self {
        std::fs::read_to_string(Self::path(mods_dir))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, mods_dir: &Path) -> Result<()> {
        write_atomic(
            &Self::path(mods_dir),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )?;
        Ok(())
    }
}

/// Newest compatible version of a project for `game` on any of `loaders`.
pub fn resolve(
    paths: &Paths,
    project: &str,
    game: &str,
    loaders: &[&str],
) -> Result<Option<ModVersion>> {
    let loaders_q = loaders
        .iter()
        .map(|l| format!("%22{l}%22"))
        .collect::<Vec<_>>()
        .join("%2C");
    let url = format!(
        "{}/project/{project}/version?loaders=%5B{loaders_q}%5D&game_versions=%5B%22{}%22%5D",
        api_base(),
        urlencode(game)
    );
    let cache = paths.cache_dir().join("modrinth").join(format!(
        "{project}_{}_{}.json",
        loaders.join("-"),
        sanitize(game)
    ));
    let list: Vec<ModVersion> = http::get_json_cached(&url, &cache, Duration::from_secs(24 * 3600))
        .with_context(|| format!("querying Modrinth for {project}"))?;
    if list.is_empty() {
        return Ok(None);
    }
    // Modrinth returns newest first; prefer releases, then betas, then alphas.
    let pick = ["release", "beta", "alpha"]
        .iter()
        .find_map(|t| {
            list.iter()
                .find(|v| &v.version_type == t && !v.files.is_empty())
        })
        .or_else(|| list.iter().find(|v| !v.files.is_empty()))
        .cloned();
    Ok(pick)
}

fn primary_file(v: &ModVersion) -> Option<&ModFile> {
    v.files
        .iter()
        .find(|f| f.primary)
        .or_else(|| v.files.first())
}

/// Install `slugs` (plus their required dependencies) into `mods_dir`.
/// Returns the list of installed file names. Nothing is downloaded when the
/// managed set is already up to date for this game version and all files exist.
#[allow(clippy::too_many_arguments)]
pub fn install(
    paths: &Paths,
    mods_dir: &Path,
    slugs: &[String],
    game: &str,
    loader: Loader,
    verify: bool,
    parallel: usize,
    sink: &ProgressSink,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(mods_dir)?;
    let mut managed = ManagedMods::load(mods_dir);
    let loaders = loader.modrinth_loaders();
    let fast_path = managed.game_version == game
        && managed.loader == loader.id()
        && slugs.iter().all(|s| managed.files.contains_key(s))
        && managed.files.values().all(|f| mods_dir.join(f).is_file())
        && !verify;
    if fast_path {
        return Ok(managed.files.values().cloned().collect());
    }

    report(
        sink,
        Progress::Phase(format!(
            "Resolving {} mod(s) for {game} ({})",
            slugs.len(),
            loader.label()
        )),
    );
    let mut tasks: Vec<Task> = Vec::new();
    let mut new_files: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: Vec<String> = slugs.to_vec();
    let mut missing: Vec<String> = Vec::new();
    while let Some(project) = queue.pop() {
        if !visited.insert(project.clone()) {
            continue;
        }
        let Some(v) = resolve(paths, &project, game, loaders)? else {
            missing.push(project);
            continue;
        };
        if visited.contains(&v.project_id) && v.project_id != project {
            continue;
        }
        visited.insert(v.project_id.clone());
        let Some(f) = primary_file(&v) else { continue };
        report(
            sink,
            Progress::Log(format!(
                "  {project} -> {} ({})",
                v.version_number, f.filename
            )),
        );
        new_files.insert(project.clone(), f.filename.clone());
        new_files.insert(v.project_id.clone(), f.filename.clone());
        tasks.push(
            Task::new(&f.url, mods_dir.join(&f.filename))
                .sha1(f.hashes.get("sha1").cloned())
                .size(Some(f.size)),
        );
        for d in &v.dependencies {
            if d.dependency_type == "required" {
                if let Some(pid) = &d.project_id {
                    if !visited.contains(pid) {
                        queue.push(pid.clone());
                    }
                }
            }
        }
    }
    if !missing.is_empty() {
        report(
            sink,
            Progress::Log(format!(
                "  no {} build for {game}, skipped: {}",
                loader.label(),
                missing.join(", ")
            )),
        );
    }
    // Remove previously managed jars that are no longer wanted / were updated.
    let keep: HashSet<&String> = new_files.values().collect();
    for old in managed.files.values() {
        if !keep.contains(old) {
            let p = mods_dir.join(old);
            if p.is_file() {
                let _ = std::fs::remove_file(&p);
                report(sink, Progress::Log(format!("  removed old {old}")));
            }
        }
    }
    report(
        sink,
        Progress::Phase(format!("Downloading {} mod file(s)", tasks.len())),
    );
    crate::download::fetch_all(&tasks, verify, parallel, sink)?;
    managed.game_version = game.to_string();
    managed.loader = loader.id().to_string();
    managed.files = new_files;
    managed.save(mods_dir)?;
    let mut names: Vec<String> = managed
        .files
        .values()
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    names.sort();
    Ok(names)
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn validate_slug(s: &str) -> Result<String> {
    let s = s.trim().to_ascii_lowercase();
    if s.is_empty()
        || !s
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow!("'{s}' is not a valid Modrinth project slug"));
    }
    Ok(s)
}
