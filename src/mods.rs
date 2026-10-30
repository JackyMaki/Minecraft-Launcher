//! Mod catalog: search and install from Modrinth (whole catalog) and, with an
//! API key, CurseForge. Mods, resource packs and shader packs are supported.
//! Also holds the "performance pack" that makes modded profiles fast.

use crate::download::Task;
use crate::http;
use crate::paths::{write_atomic, Paths};
use crate::profiles::Loader;
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub fn api_base() -> String {
    std::env::var("MC_LAUNCHER_MODRINTH_URL")
        .unwrap_or_else(|_| "https://api.modrinth.com/v2".into())
}

pub fn curseforge_base() -> String {
    std::env::var("MC_LAUNCHER_CURSEFORGE_URL")
        .unwrap_or_else(|_| "https://api.curseforge.com/v1".into())
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

// ---------------------------------------------------------------- references

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Modrinth,
    CurseForge,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Modrinth => "Modrinth",
            Source::CurseForge => "CurseForge",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Mod,
    ResourcePack,
    Shader,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Mod, Kind::ResourcePack, Kind::Shader];
    pub fn label(self) -> &'static str {
        match self {
            Kind::Mod => "Mods",
            Kind::ResourcePack => "Resource packs",
            Kind::Shader => "Shaders",
        }
    }
    /// Folder under the game directory.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Mod => "mods",
            Kind::ResourcePack => "resourcepacks",
            Kind::Shader => "shaderpacks",
        }
    }
    fn modrinth_type(self) -> &'static str {
        match self {
            Kind::Mod => "mod",
            Kind::ResourcePack => "resourcepack",
            Kind::Shader => "shader",
        }
    }
    fn curseforge_class(self) -> u32 {
        match self {
            Kind::Mod => 6,
            Kind::ResourcePack => 12,
            Kind::Shader => 6552,
        }
    }
}

/// A reference to something installable, stored in profiles as a string:
/// `sodium` (Modrinth mod), `modrinth:shader:complementary-reimagined`,
/// `curseforge:mod:238222`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModRef {
    pub source: Source,
    pub kind: Kind,
    pub id: String,
}

impl ModRef {
    pub fn modrinth(slug: &str) -> Self {
        Self {
            source: Source::Modrinth,
            kind: Kind::Mod,
            id: slug.to_string(),
        }
    }

    pub fn parse(s: &str) -> Result<Self> {
        let s = s.trim();
        let parts: Vec<&str> = s.split(':').collect();
        let (source, kind, id) = match parts.as_slice() {
            [id] => (Source::Modrinth, Kind::Mod, *id),
            ["modrinth" | "mr", id] => (Source::Modrinth, Kind::Mod, *id),
            ["curseforge" | "cf", id] => (Source::CurseForge, Kind::Mod, *id),
            [src, kind, id] => {
                let source = match *src {
                    "modrinth" | "mr" => Source::Modrinth,
                    "curseforge" | "cf" => Source::CurseForge,
                    _ => return Err(anyhow!("unknown source in '{s}'")),
                };
                let kind = match *kind {
                    "mod" => Kind::Mod,
                    "resourcepack" | "rp" => Kind::ResourcePack,
                    "shader" => Kind::Shader,
                    _ => return Err(anyhow!("unknown kind in '{s}'")),
                };
                (source, kind, *id)
            }
            _ => return Err(anyhow!("'{s}' is not a valid mod reference")),
        };
        let id = id.trim().to_ascii_lowercase();
        let ok = match source {
            Source::Modrinth => {
                !id.is_empty()
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            }
            Source::CurseForge => !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()),
        };
        if !ok {
            return Err(anyhow!("'{s}' is not a valid {} id", source.label()));
        }
        Ok(Self { source, kind, id })
    }

    pub fn key(&self) -> String {
        let src = match self.source {
            Source::Modrinth => "modrinth",
            Source::CurseForge => "curseforge",
        };
        let kind = match self.kind {
            Kind::Mod => "mod",
            Kind::ResourcePack => "resourcepack",
            Kind::Shader => "shader",
        };
        if self.source == Source::Modrinth && self.kind == Kind::Mod {
            self.id.clone()
        } else {
            format!("{src}:{kind}:{}", self.id)
        }
    }
}

impl std::fmt::Display for ModRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key())
    }
}

pub fn validate_slug(s: &str) -> Result<String> {
    ModRef::parse(s).map(|r| r.key())
}

// ---------------------------------------------------------------- search

/// One catalog entry as shown in the mod browser.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub reference: String,
    pub source: Source,
    pub kind: Kind,
    pub title: String,
    pub author: String,
    pub description: String,
    pub downloads: u64,
    pub icon_url: String,
    pub page_url: String,
    /// Game versions this project supports (Modrinth only).
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    pub text: String,
    pub kind: Kind,
    pub loader: Loader,
    pub game_version: String,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchPage {
    pub hits: Vec<Hit>,
    pub total: u64,
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

/// Search Modrinth's catalog.
pub fn search_modrinth(q: &SearchQuery) -> Result<SearchPage> {
    let mut facets: Vec<Vec<String>> =
        vec![vec![format!("project_type:{}", q.kind.modrinth_type())]];
    if !q.game_version.is_empty() {
        facets.push(vec![format!("versions:{}", q.game_version)]);
    }
    if q.kind == Kind::Mod && q.loader.is_modded() {
        facets.push(
            q.loader
                .modrinth_loaders()
                .iter()
                .map(|l| format!("categories:{l}"))
                .collect(),
        );
    }
    let facets_json = serde_json::to_string(&facets)?;
    let url = format!(
        "{}/search?query={}&facets={}&limit={}&offset={}&index={}",
        api_base(),
        urlencode(&q.text),
        urlencode(&facets_json),
        q.limit.clamp(1, 100),
        q.offset,
        if q.text.trim().is_empty() {
            "downloads"
        } else {
            "relevance"
        }
    );
    let v: serde_json::Value = http::get_json(&url).context("searching Modrinth")?;
    let mut page = SearchPage {
        hits: Vec::new(),
        total: v["total_hits"].as_u64().unwrap_or(0),
    };
    for h in v["hits"].as_array().cloned().unwrap_or_default() {
        let slug = h["slug"].as_str().unwrap_or("").to_string();
        if slug.is_empty() {
            continue;
        }
        let r = ModRef {
            source: Source::Modrinth,
            kind: q.kind,
            id: slug.clone(),
        };
        page.hits.push(Hit {
            reference: r.key(),
            source: Source::Modrinth,
            kind: q.kind,
            title: h["title"].as_str().unwrap_or(&slug).to_string(),
            author: h["author"].as_str().unwrap_or("").to_string(),
            description: h["description"].as_str().unwrap_or("").to_string(),
            downloads: h["downloads"].as_u64().unwrap_or(0),
            icon_url: h["icon_url"].as_str().unwrap_or("").to_string(),
            page_url: format!("https://modrinth.com/{}/{slug}", q.kind.modrinth_type()),
            versions: h["versions"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok(page)
}

fn curseforge_loader_type(loader: Loader) -> Option<u32> {
    match loader {
        Loader::Forge => Some(1),
        Loader::Fabric | Loader::LegacyFabric => Some(4),
        Loader::Quilt => Some(5),
        Loader::NeoForge => Some(6),
        Loader::Vanilla => None,
    }
}

fn cf_get(url: &str, api_key: &str) -> Result<serde_json::Value> {
    if api_key.trim().is_empty() {
        return Err(anyhow!("CurseForge needs an API key (Settings → CurseForge API key; free at console.curseforge.com)"));
    }
    let resp = http::agent()
        .get(url)
        .set("x-api-key", api_key.trim())
        .set("Accept", "application/json")
        .call();
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(403, _)) => {
            return Err(anyhow!("CurseForge rejected the API key (403)"))
        }
        Err(ureq::Error::Status(code, r)) => {
            return Err(anyhow!(
                "CurseForge error {code}: {}",
                r.into_string().unwrap_or_default()
            ))
        }
        Err(e) => return Err(anyhow!("CurseForge: {e}")),
    };
    resp.into_json().context("parsing CurseForge response")
}

/// Search CurseForge's catalog (needs an API key).
pub fn search_curseforge(q: &SearchQuery, api_key: &str) -> Result<SearchPage> {
    let mut url = format!(
        "{}/mods/search?gameId=432&classId={}&searchFilter={}&pageSize={}&index={}&sortField={}&sortOrder=desc",
        curseforge_base(),
        q.kind.curseforge_class(),
        urlencode(&q.text),
        q.limit.clamp(1, 50),
        q.offset,
        if q.text.trim().is_empty() { 2 } else { 1 } // 2 = popularity, 1 = featured/relevance
    );
    if !q.game_version.is_empty() {
        url.push_str(&format!("&gameVersion={}", urlencode(&q.game_version)));
    }
    if q.kind == Kind::Mod {
        if let Some(t) = curseforge_loader_type(q.loader) {
            url.push_str(&format!("&modLoaderType={t}"));
        }
    }
    let v = cf_get(&url, api_key)?;
    let mut page = SearchPage {
        hits: Vec::new(),
        total: v["pagination"]["totalCount"].as_u64().unwrap_or(0),
    };
    for h in v["data"].as_array().cloned().unwrap_or_default() {
        let id = h["id"].as_u64().unwrap_or(0);
        if id == 0 {
            continue;
        }
        let r = ModRef {
            source: Source::CurseForge,
            kind: q.kind,
            id: id.to_string(),
        };
        page.hits.push(Hit {
            reference: r.key(),
            source: Source::CurseForge,
            kind: q.kind,
            title: h["name"].as_str().unwrap_or("").to_string(),
            author: h["authors"][0]["name"].as_str().unwrap_or("").to_string(),
            description: h["summary"].as_str().unwrap_or("").to_string(),
            downloads: h["downloadCount"].as_u64().unwrap_or(0),
            icon_url: h["logo"]["thumbnailUrl"].as_str().unwrap_or("").to_string(),
            page_url: h["links"]["websiteUrl"].as_str().unwrap_or("").to_string(),
            versions: Vec::new(),
        });
    }
    Ok(page)
}

// ---------------------------------------------------------------- resolve

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

/// Newest compatible Modrinth version of a project for `game` on any of `loaders`.
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

/// A resolved CurseForge file.
#[derive(Debug, Clone)]
pub struct CfFile {
    pub file_name: String,
    pub url: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub required_mod_ids: Vec<u64>,
}

/// Newest compatible CurseForge file for a mod id.
pub fn resolve_curseforge(
    paths: &Paths,
    mod_id: &str,
    kind: Kind,
    game: &str,
    loader: Loader,
    api_key: &str,
) -> Result<Option<CfFile>> {
    let mut url = format!(
        "{}/mods/{mod_id}/files?gameVersion={}&pageSize=50",
        curseforge_base(),
        urlencode(game)
    );
    if kind == Kind::Mod {
        if let Some(t) = curseforge_loader_type(loader) {
            url.push_str(&format!("&modLoaderType={t}"));
        }
    }
    let cache = paths.cache_dir().join("curseforge").join(format!(
        "{mod_id}_{}_{}.json",
        loader.id(),
        sanitize(game)
    ));
    let v: serde_json::Value = match http::read_cache(&cache, Duration::from_secs(24 * 3600)) {
        Some(t) => serde_json::from_str(&t)?,
        None => {
            let v = cf_get(&url, api_key)?;
            let _ = write_atomic(&cache, v.to_string().as_bytes());
            v
        }
    };
    let files = v["data"].as_array().cloned().unwrap_or_default();
    // Newest release first, falling back to beta/alpha.
    let mut sorted = files.clone();
    sorted.sort_by(|a, b| {
        b["fileDate"]
            .as_str()
            .unwrap_or("")
            .cmp(a["fileDate"].as_str().unwrap_or(""))
    });
    let pick = (1..=3).find_map(|rt| {
        sorted
            .iter()
            .find(|f| f["releaseType"].as_u64() == Some(rt))
    });
    let Some(f) = pick else { return Ok(None) };
    let id = f["id"].as_u64().unwrap_or(0);
    let file_name = f["fileName"].as_str().unwrap_or("").to_string();
    // Authors can opt out of API downloads; the CDN path is still derivable.
    let url = match f["downloadUrl"].as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => format!(
            "https://edge.forgecdn.net/files/{}/{}/{}",
            id / 1000,
            id % 1000,
            urlencode(&file_name)
        ),
    };
    let sha1 = f["hashes"]
        .as_array()
        .and_then(|a| a.iter().find(|h| h["algo"].as_u64() == Some(1)))
        .and_then(|h| h["value"].as_str().map(String::from));
    let required_mod_ids = f["dependencies"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|d| d["relationType"].as_u64() == Some(3))
                .filter_map(|d| d["modId"].as_u64())
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(CfFile {
        file_name,
        url,
        size: f["fileLength"].as_u64().unwrap_or(0),
        sha1,
        required_mod_ids,
    }))
}

// ---------------------------------------------------------------- install

/// Which file we manage for each reference, so updates replace old files and
/// removed entries are cleaned up. Stored at `<game_dir>/.elo_mods.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManagedMods {
    #[serde(default)]
    pub game_version: String,
    #[serde(default)]
    pub loader: String,
    /// reference key -> relative path (e.g. `mods/sodium-0.6.jar`)
    #[serde(default)]
    pub files: BTreeMap<String, String>,
}

impl ManagedMods {
    fn path(game_dir: &Path) -> PathBuf {
        game_dir.join(".elo_mods.json")
    }
    pub fn load(game_dir: &Path) -> Self {
        let p = Self::path(game_dir);
        let text = std::fs::read_to_string(&p)
            .or_else(|_| std::fs::read_to_string(game_dir.join(".flint_mods.json")))
            .or_else(|_| {
                std::fs::read_to_string(game_dir.join("mods").join(".launcher_mods.json"))
            });
        let mut m: Self = text
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        // Legacy entries were bare file names inside mods/.
        for v in m.files.values_mut() {
            if !v.contains('/') {
                *v = format!("mods/{v}");
            }
        }
        m
    }
    pub fn save(&self, game_dir: &Path) -> Result<()> {
        write_atomic(
            &Self::path(game_dir),
            serde_json::to_string_pretty(self)?.as_bytes(),
        )?;
        Ok(())
    }
}

/// Install `refs` (plus their required dependencies) into the game directory.
/// Returns the managed file list. Nothing is downloaded when the managed set
/// already matches this game version/loader and all files exist.
#[allow(clippy::too_many_arguments)]
pub fn install(
    paths: &Paths,
    game_dir: &Path,
    refs: &[String],
    game: &str,
    loader: Loader,
    curseforge_key: &str,
    verify: bool,
    parallel: usize,
    sink: &ProgressSink,
) -> Result<Vec<String>> {
    std::fs::create_dir_all(game_dir)?;
    let mut managed = ManagedMods::load(game_dir);
    let wanted: Vec<ModRef> = refs.iter().filter_map(|r| ModRef::parse(r).ok()).collect();
    let fast_path = managed.game_version == game
        && managed.loader == loader.id()
        && wanted.iter().all(|r| managed.files.contains_key(&r.key()))
        && managed.files.values().all(|f| game_dir.join(f).is_file())
        && !verify;
    if fast_path {
        return Ok(managed.files.values().cloned().collect());
    }

    report(
        sink,
        Progress::Phase(format!(
            "Resolving {} mod(s) for {game} ({})",
            wanted.len(),
            loader.label()
        )),
    );
    let loaders = loader.modrinth_loaders();
    let mut tasks: Vec<Task> = Vec::new();
    let mut new_files: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut queue: Vec<ModRef> = wanted.clone();
    let mut missing: Vec<String> = Vec::new();
    while let Some(r) = queue.pop() {
        if !visited.insert(r.key()) {
            continue;
        }
        match r.source {
            Source::Modrinth => {
                let Some(v) = resolve(paths, &r.id, game, loaders)? else {
                    missing.push(r.key());
                    continue;
                };
                let pid_key = ModRef {
                    source: Source::Modrinth,
                    kind: r.kind,
                    id: v.project_id.clone(),
                }
                .key();
                if visited.contains(&pid_key) && v.project_id != r.id {
                    continue;
                }
                visited.insert(pid_key.clone());
                let Some(f) = primary_file(&v) else { continue };
                let rel = format!("{}/{}", r.kind.dir(), f.filename);
                report(
                    sink,
                    Progress::Log(format!(
                        "  {} -> {} ({})",
                        r.key(),
                        v.version_number,
                        f.filename
                    )),
                );
                new_files.insert(r.key(), rel.clone());
                new_files.insert(pid_key, rel.clone());
                tasks.push(
                    Task::new(&f.url, game_dir.join(&rel))
                        .sha1(f.hashes.get("sha1").cloned())
                        .size(Some(f.size)),
                );
                if r.kind == Kind::Mod {
                    for d in &v.dependencies {
                        if d.dependency_type == "required" {
                            if let Some(pid) = &d.project_id {
                                queue.push(ModRef {
                                    source: Source::Modrinth,
                                    kind: Kind::Mod,
                                    id: pid.clone(),
                                });
                            }
                        }
                    }
                }
            }
            Source::CurseForge => {
                let Some(f) =
                    resolve_curseforge(paths, &r.id, r.kind, game, loader, curseforge_key)?
                else {
                    missing.push(r.key());
                    continue;
                };
                let rel = format!("{}/{}", r.kind.dir(), f.file_name);
                report(
                    sink,
                    Progress::Log(format!("  {} -> {}", r.key(), f.file_name)),
                );
                new_files.insert(r.key(), rel.clone());
                tasks.push(
                    Task::new(&f.url, game_dir.join(&rel))
                        .sha1(f.sha1.clone())
                        .size(Some(f.size)),
                );
                if r.kind == Kind::Mod {
                    for dep in f.required_mod_ids {
                        queue.push(ModRef {
                            source: Source::CurseForge,
                            kind: Kind::Mod,
                            id: dep.to_string(),
                        });
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
    // Remove previously managed files that are no longer wanted / were updated.
    let keep: HashSet<&String> = new_files.values().collect();
    for old in managed.files.values() {
        if !keep.contains(old) {
            let p = game_dir.join(old);
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
    managed.save(game_dir)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refs_roundtrip() {
        let r = ModRef::parse("sodium").unwrap();
        assert_eq!(r, ModRef::modrinth("sodium"));
        assert_eq!(r.key(), "sodium");
        let r = ModRef::parse("cf:238222").unwrap();
        assert_eq!(r.source, Source::CurseForge);
        assert_eq!(r.key(), "curseforge:mod:238222");
        let r = ModRef::parse("modrinth:shader:complementary-reimagined").unwrap();
        assert_eq!(r.kind, Kind::Shader);
        assert_eq!(ModRef::parse(&r.key()).unwrap(), r);
        assert!(ModRef::parse("cf:not-a-number").is_err());
        assert!(ModRef::parse("bad slug!").is_err());
    }
}
