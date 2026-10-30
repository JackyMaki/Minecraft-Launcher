//! Java runtime discovery and provisioning.
//!
//! Like the official launcher we download Mojang's own JRE builds
//! (`java-runtime-gamma`, `java-runtime-delta`, ...) so every version gets a
//! Java it is known to work with. A system Java is used as a fallback.

use crate::download::Task;
use crate::http;
use crate::paths::Paths;
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const RUNTIME_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/v1/products/java-runtime/2ec0cc96c44e5a76b9c8b7c39df7210883d12871/all.json";

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeEntry {
    pub manifest: RuntimeManifestRef,
    pub version: RuntimeVersion,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeManifestRef {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeVersion {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeFiles {
    pub files: HashMap<String, RuntimeFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeFile {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub executable: bool,
    #[serde(default)]
    pub downloads: Option<RuntimeDownloads>,
    #[serde(default)]
    pub target: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeDownloads {
    pub raw: RuntimeRaw,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuntimeRaw {
    pub sha1: String,
    pub size: u64,
    pub url: String,
}

/// Map a Java major version to the Mojang runtime component that ships it.
pub fn component_for_major(major: u32) -> &'static str {
    match major {
        0..=8 => "jre-legacy",
        9..=16 => "java-runtime-alpha",
        17 => "java-runtime-gamma",
        _ => "java-runtime-delta",
    }
}

/// Path of the `java` executable for a managed runtime, if installed.
pub fn managed_java(paths: &Paths, component: &str) -> Option<PathBuf> {
    let platform = crate::paths::java_runtime_platform();
    let home = paths
        .runtimes()
        .join(component)
        .join(platform)
        .join(component);
    let candidates = if cfg!(target_os = "macos") {
        vec![home.join("jre.bundle/Contents/Home/bin/java")]
    } else if cfg!(target_os = "windows") {
        vec![home.join("bin/javaw.exe"), home.join("bin/java.exe")]
    } else {
        vec![home.join("bin/java")]
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Download (or verify) the Mojang runtime for `component`. Returns the java binary.
pub fn ensure_managed_java(
    paths: &Paths,
    component: &str,
    verify: bool,
    sink: &ProgressSink,
) -> Result<PathBuf> {
    if !verify {
        if let Some(j) = managed_java(paths, component) {
            return Ok(j);
        }
    }
    report(
        sink,
        Progress::Phase(format!("Fetching Java runtime ({component})")),
    );
    let url = std::env::var("MC_LAUNCHER_JAVA_MANIFEST_URL")
        .unwrap_or_else(|_| RUNTIME_MANIFEST_URL.into());
    let all: HashMap<String, HashMap<String, Vec<RuntimeEntry>>> = http::get_json_cached(
        &url,
        &paths.cache_dir().join("java_runtime_all.json"),
        Duration::from_secs(6 * 3600),
    )?;
    let platform = crate::paths::java_runtime_platform();
    let entry = all
        .get(platform)
        .and_then(|m| m.get(component))
        .and_then(|v| v.first())
        .ok_or_else(|| anyhow!("no Mojang Java runtime '{component}' for platform {platform}"))?;
    let files: RuntimeFiles = http::get_json_cached(
        &entry.manifest.url,
        &paths
            .cache_dir()
            .join(format!("java_{component}_{platform}.json")),
        Duration::from_secs(7 * 24 * 3600),
    )?;
    let home = paths
        .runtimes()
        .join(component)
        .join(platform)
        .join(component);
    let mut tasks = Vec::new();
    let mut links = Vec::new();
    for (rel, f) in &files.files {
        let dest = home.join(rel);
        match f.kind.as_str() {
            "directory" => std::fs::create_dir_all(&dest)?,
            "file" => {
                if let Some(d) = &f.downloads {
                    let mut t = Task::new(&d.raw.url, &dest)
                        .sha1(Some(d.raw.sha1.clone()))
                        .size(Some(d.raw.size));
                    t.executable = f.executable;
                    tasks.push(t);
                }
            }
            "link" => {
                if let Some(target) = &f.target {
                    links.push((dest, target.clone()));
                }
            }
            _ => {}
        }
    }
    report(
        sink,
        Progress::Phase(format!(
            "Downloading Java {} ({} files)",
            entry.version.name,
            tasks.len()
        )),
    );
    crate::download::fetch_all(&tasks, verify, 16, sink)?;
    #[cfg(unix)]
    for (dest, target) in links {
        if !dest.exists() {
            crate::paths::ensure_parent(&dest)?;
            let _ = std::os::unix::fs::symlink(&target, &dest);
        }
    }
    #[cfg(not(unix))]
    let _ = links;
    std::fs::write(
        paths
            .runtimes()
            .join(component)
            .join(platform)
            .join(".version"),
        &entry.version.name,
    )?;
    managed_java(paths, component)
        .ok_or_else(|| anyhow!("Java runtime downloaded but 'java' binary not found"))
}

/// Information about a Java executable obtained from `java -version`.
#[derive(Debug, Clone)]
pub struct JavaInfo {
    pub path: PathBuf,
    pub major: u32,
    pub version: String,
}

/// Probe a Java executable.
pub fn probe(path: &Path) -> Option<JavaInfo> {
    let mut cmd = Command::new(path);
    cmd.arg("-version").env_remove("JAVA_TOOL_OPTIONS");
    let out = cmd.output().ok()?;
    let text =
        String::from_utf8_lossy(&out.stderr).to_string() + &String::from_utf8_lossy(&out.stdout);
    let version = text
        .lines()
        .find_map(|l| l.split('"').nth(1).map(|s| s.to_string()))?;
    Some(JavaInfo {
        path: path.to_path_buf(),
        major: parse_major(&version),
        version,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ProbeCache {
    /// path -> (mtime secs, size, major, version)
    entries: HashMap<String, (u64, u64, u32, String)>,
}

/// Probe with an on-disk cache keyed by the binary's mtime and size, so a
/// warm launch never has to start a JVM just to ask its version.
pub fn probe_cached(paths: &Paths, path: &Path) -> Option<JavaInfo> {
    let cache_file = paths.cache_dir().join("java_probe.json");
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let key = path.to_string_lossy().into_owned();
    let mut cache: ProbeCache = std::fs::read_to_string(&cache_file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    if let Some((m, s, major, version)) = cache.entries.get(&key) {
        if *m == mtime && *s == meta.len() {
            return Some(JavaInfo {
                path: path.to_path_buf(),
                major: *major,
                version: version.clone(),
            });
        }
    }
    let info = probe(path)?;
    cache
        .entries
        .insert(key, (mtime, meta.len(), info.major, info.version.clone()));
    if let Ok(text) = serde_json::to_string(&cache) {
        let _ = crate::paths::write_atomic(&cache_file, text.as_bytes());
    }
    Some(info)
}

/// `1.8.0_392` -> 8, `17.0.8` -> 17, `21` -> 21.
pub fn parse_major(version: &str) -> u32 {
    let mut it = version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty());
    let first: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    if first == 1 {
        it.next().and_then(|s| s.parse().ok()).unwrap_or(first)
    } else {
        first
    }
}

/// Find Java installations on this machine (JAVA_HOME, PATH, common dirs, managed).
pub fn discover(paths: &Paths) -> Vec<JavaInfo> {
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    let mut candidates: Vec<PathBuf> = Vec::new();
    for component in [
        "java-runtime-delta",
        "java-runtime-gamma",
        "java-runtime-alpha",
        "jre-legacy",
    ] {
        if let Some(j) = managed_java(paths, component) {
            candidates.push(j);
        }
    }
    if let Ok(home) = std::env::var("JAVA_HOME") {
        candidates.push(PathBuf::from(home).join("bin").join(exe));
    }
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join(exe));
        }
    }
    let roots: &[&str] = if cfg!(target_os = "windows") {
        &[
            "C:\\Program Files\\Java",
            "C:\\Program Files\\Eclipse Adoptium",
            "C:\\Program Files\\Microsoft",
            "C:\\Program Files\\Zulu",
        ]
    } else if cfg!(target_os = "macos") {
        &["/Library/Java/JavaVirtualMachines"]
    } else {
        &["/usr/lib/jvm"]
    };
    for root in roots {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                candidates.push(p.join("bin").join(exe));
                candidates.push(p.join("Contents/Home/bin").join(exe));
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in candidates {
        let Ok(real) = std::fs::canonicalize(&c) else {
            continue;
        };
        if !seen.insert(real.clone()) {
            continue;
        }
        if let Some(info) = probe_cached(paths, &real) {
            out.push(info);
        }
    }
    out
}

/// Pick a Java for `major`: an explicit override, then a managed runtime, then
/// (optionally after downloading one) any discovered Java with a matching major.
pub fn resolve_java(
    paths: &Paths,
    major: u32,
    override_path: Option<&Path>,
    allow_download: bool,
    sink: &ProgressSink,
) -> Result<PathBuf> {
    if let Some(p) = override_path {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        return Err(anyhow!("configured Java not found: {}", p.display()));
    }
    let component = component_for_major(major);
    if let Some(j) = managed_java(paths, component) {
        return Ok(j);
    }
    let discovered = discover(paths);
    // Exact major first, then anything newer (Minecraft tolerates newer Java).
    if let Some(j) = discovered.iter().find(|j| j.major == major) {
        return Ok(j.path.clone());
    }
    if allow_download {
        match ensure_managed_java(paths, component, false, sink) {
            Ok(j) => return Ok(j),
            Err(e) => log::warn!("could not download Mojang Java runtime: {e}"),
        }
    }
    if let Some(j) = discovered
        .iter()
        .filter(|j| j.major >= major)
        .min_by_key(|j| j.major)
    {
        report(
            sink,
            Progress::Log(format!("Using system Java {} (wanted {major})", j.version)),
        );
        return Ok(j.path.clone());
    }
    Err(anyhow!(
        "no suitable Java {major} found and the Mojang runtime could not be downloaded. Install Java {major} or set a Java path in the profile."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn majors() {
        assert_eq!(parse_major("1.8.0_392"), 8);
        assert_eq!(parse_major("17.0.8"), 17);
        assert_eq!(parse_major("21"), 21);
        assert_eq!(parse_major("21.0.4+7-LTS"), 21);
    }
    #[test]
    fn components() {
        assert_eq!(component_for_major(8), "jre-legacy");
        assert_eq!(component_for_major(16), "java-runtime-alpha");
        assert_eq!(component_for_major(17), "java-runtime-gamma");
        assert_eq!(component_for_major(21), "java-runtime-delta");
    }
}
