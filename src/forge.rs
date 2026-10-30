//! Forge and NeoForge. Both ship an installer jar that patches the client and
//! writes a version folder; the only robust way to install them is to run that
//! installer headlessly (`--installClient`), which is what we do here.

use crate::download::Task;
use crate::http;
use crate::paths::Paths;
use crate::profiles::Loader;
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Context, Result};
use std::collections::BTreeSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn forge_files_base() -> String {
    std::env::var("MC_LAUNCHER_FORGE_FILES_URL")
        .unwrap_or_else(|_| "https://files.minecraftforge.net/net/minecraftforge/forge".into())
}
fn forge_maven_base() -> String {
    std::env::var("MC_LAUNCHER_FORGE_MAVEN_URL")
        .unwrap_or_else(|_| "https://maven.minecraftforge.net/net/minecraftforge/forge".into())
}
fn neoforge_maven_base() -> String {
    std::env::var("MC_LAUNCHER_NEOFORGE_MAVEN_URL")
        .unwrap_or_else(|_| "https://maven.neoforged.net/releases/net/neoforged".into())
}

/// Forge builds for a Minecraft version, newest last (e.g. `52.0.21`).
pub fn forge_versions(paths: &Paths, game: &str) -> Result<Vec<String>> {
    let meta: serde_json::Value = http::get_json_cached(
        &format!("{}/maven-metadata.json", forge_files_base()),
        &paths.cache_dir().join("forge_maven_metadata.json"),
        Duration::from_secs(6 * 3600),
    )
    .context("fetching Forge version list")?;
    let list = meta[game]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|v| v.strip_prefix(&format!("{game}-")).map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(list)
}

/// Recommended (else latest) Forge build for `game`.
pub fn forge_recommended(paths: &Paths, game: &str) -> Result<String> {
    if let Ok(promos) = http::get_json_cached::<serde_json::Value>(
        &format!("{}/promotions_slim.json", forge_files_base()),
        &paths.cache_dir().join("forge_promotions_slim.json"),
        Duration::from_secs(6 * 3600),
    ) {
        for key in [format!("{game}-recommended"), format!("{game}-latest")] {
            if let Some(v) = promos["promos"][&key].as_str() {
                return Ok(v.to_string());
            }
        }
    }
    forge_versions(paths, game)?
        .last()
        .cloned()
        .ok_or_else(|| anyhow!("Forge has no builds for Minecraft {game}"))
}

/// NeoForge builds for `game`, newest last (e.g. `21.1.80`). NeoForge numbers
/// its releases after the Minecraft version: 1.21.1 -> 21.1.x.
pub fn neoforge_versions(paths: &Paths, game: &str) -> Result<Vec<String>> {
    let cache = paths.cache_dir().join("neoforge_maven_metadata.xml");
    let text = match http::read_cache(&cache, Duration::from_secs(6 * 3600)) {
        Some(t) => t,
        None => {
            let t = http::get_string(&format!(
                "{}/neoforge/maven-metadata.xml",
                neoforge_maven_base()
            ))
            .context("fetching NeoForge version list")?;
            let _ = crate::paths::write_atomic(&cache, t.as_bytes());
            t
        }
    };
    let prefix = neoforge_prefix(game)
        .ok_or_else(|| anyhow!("NeoForge needs Minecraft 1.20.2 or newer (got {game})"))?;
    let mut out: Vec<String> = Vec::new();
    for v in xml_versions(&text) {
        if v.starts_with(&prefix) {
            out.push(v);
        }
    }
    Ok(out)
}

fn xml_versions(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<version>") {
        let after = &rest[i + 9..];
        let Some(j) = after.find("</version>") else {
            break;
        };
        out.push(after[..j].trim().to_string());
        rest = &after[j + 10..];
    }
    out
}

/// `1.21.1` -> `21.1.`, `1.21` -> `21.0.`, `1.20.2` -> `20.2.`.
pub fn neoforge_prefix(game: &str) -> Option<String> {
    let mut it = game.split('.');
    let major: u32 = it.next()?.parse().ok()?;
    let minor: u32 = it.next()?.parse().ok()?;
    let patch: u32 = it.next().unwrap_or("0").parse().ok()?;
    if major != 1 || minor < 20 || (minor == 20 && patch < 2) {
        return None;
    }
    Some(format!("{minor}.{patch}."))
}

pub fn neoforge_recommended(paths: &Paths, game: &str) -> Result<String> {
    let list = neoforge_versions(paths, game)?;
    list.iter()
        .rev()
        .find(|v| !v.contains("beta"))
        .or_else(|| list.last())
        .cloned()
        .ok_or_else(|| anyhow!("NeoForge has no builds for Minecraft {game}"))
}

fn installer_url(loader: Loader, game: &str, version: &str) -> String {
    match loader {
        Loader::Forge => format!(
            "{}/{game}-{version}/forge-{game}-{version}-installer.jar",
            forge_maven_base()
        ),
        _ => format!(
            "{}/neoforge/{version}/neoforge-{version}-installer.jar",
            neoforge_maven_base()
        ),
    }
}

/// Installed version id for this loader + game, if any.
pub fn installed_for(
    paths: &Paths,
    loader: Loader,
    game: &str,
    pinned: Option<&str>,
) -> Option<String> {
    let mut found: Vec<String> = version_dirs(paths)
        .into_iter()
        .filter(|n| matches_loader(n, loader, game))
        .filter(|n| pinned.map(|p| n.contains(p)).unwrap_or(true))
        .collect();
    found.sort();
    found.pop()
}

fn matches_loader(id: &str, loader: Loader, game: &str) -> bool {
    let lower = id.to_ascii_lowercase();
    match loader {
        Loader::Forge => {
            lower.contains("forge")
                && !lower.contains("neoforge")
                && lower.starts_with(&format!("{game}-"))
        }
        Loader::NeoForge => {
            if let Some(prefix) = neoforge_prefix(game) {
                lower.starts_with("neoforge-") && lower[9..].starts_with(&prefix)
            } else {
                false
            }
        }
        _ => false,
    }
}

fn version_dirs(paths: &Paths) -> BTreeSet<String> {
    std::fs::read_dir(paths.versions())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| paths.version_json(n).is_file())
                .collect()
        })
        .unwrap_or_default()
}

/// Install Forge/NeoForge for `game` with the official installer and return
/// the new version id. The vanilla version must already be installed.
pub fn install(
    paths: &Paths,
    loader: Loader,
    game: &str,
    pinned: Option<&str>,
    java: &Path,
    sink: &ProgressSink,
) -> Result<String> {
    let pinned = pinned.filter(|s| !s.trim().is_empty()).map(|s| s.trim());
    if let Some(id) = installed_for(paths, loader, game, pinned) {
        return Ok(id);
    }
    let version = match (pinned, loader) {
        (Some(p), _) => p.to_string(),
        (None, Loader::Forge) => forge_recommended(paths, game)?,
        (None, _) => neoforge_recommended(paths, game)?,
    };
    report(
        sink,
        Progress::Phase(format!(
            "Downloading {} {version} installer",
            loader.label()
        )),
    );
    let url = installer_url(loader, game, &version);
    let installer: PathBuf = paths
        .launcher_dir()
        .join("installers")
        .join(format!("{}-{game}-{version}-installer.jar", loader.id()));
    let task = Task::new(&url, &installer);
    if !installer.is_file() {
        crate::download::fetch_one(&task).with_context(|| format!("downloading {url}"))?;
    }

    // The installers refuse to run without a launcher profiles file.
    let lp = paths.profiles_json();
    if !lp.is_file() {
        crate::paths::write_atomic(&lp, br#"{"profiles":{},"settings":{},"version":3}"#)?;
    }
    let before = version_dirs(paths);
    let profiles_before = std::fs::read_to_string(&lp).unwrap_or_default();

    report(
        sink,
        Progress::Phase(format!(
            "Running {} installer (this takes a minute)",
            loader.label()
        )),
    );
    let mut cmd = Command::new(java);
    cmd.arg("-jar")
        .arg(&installer)
        .arg("--installClient")
        .arg(&paths.root)
        .current_dir(&paths.root)
        .env_remove("JAVA_TOOL_OPTIONS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .with_context(|| format!("starting {}", java.display()))?;
    let mut tail: Vec<String> = Vec::new();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let s2 = sink.clone();
    let err_thread = std::thread::spawn(move || {
        let mut lines = Vec::new();
        if let Some(e) = stderr {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                report(&s2, Progress::Log(format!("  [installer] {line}")));
                lines.push(line);
            }
        }
        lines
    });
    if let Some(o) = stdout {
        for line in BufReader::new(o).lines().map_while(Result::ok) {
            report(sink, Progress::Log(format!("  [installer] {line}")));
            tail.push(line);
            if tail.len() > 20 {
                tail.remove(0);
            }
        }
    }
    let err_lines = err_thread.join().unwrap_or_default();
    let status = child.wait()?;

    // The installer adds its own profile to launcher_profiles.json; keep ours.
    if !profiles_before.is_empty() {
        let _ = crate::paths::write_atomic(&lp, profiles_before.as_bytes());
    }
    if !status.success() {
        let mut msg = format!(
            "{} installer failed (exit {})",
            loader.label(),
            status.code().unwrap_or(-1)
        );
        for l in err_lines
            .iter()
            .rev()
            .take(5)
            .rev()
            .chain(tail.iter().rev().take(3).rev())
        {
            msg.push_str("\n  ");
            msg.push_str(l);
        }
        return Err(anyhow!(msg));
    }
    let after = version_dirs(paths);
    let mut new: Vec<String> = after.difference(&before).cloned().collect();
    new.retain(|n| matches_loader(n, loader, game));
    new.sort();
    new.pop()
        .or_else(|| installed_for(paths, loader, game, Some(&version)))
        .ok_or_else(|| {
            anyhow!(
                "{} installer finished but no version folder appeared",
                loader.label()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neoforge_prefixes() {
        assert_eq!(neoforge_prefix("1.21.1").as_deref(), Some("21.1."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert_eq!(neoforge_prefix("1.20.2").as_deref(), Some("20.2."));
        assert_eq!(neoforge_prefix("1.20.1"), None);
        assert_eq!(neoforge_prefix("1.12.2"), None);
    }

    #[test]
    fn xml() {
        let v = xml_versions("<metadata><versioning><versions><version>21.1.1</version><version> 21.1.2-beta </version></versions></versioning></metadata>");
        assert_eq!(v, vec!["21.1.1", "21.1.2-beta"]);
    }

    #[test]
    fn loader_matching() {
        assert!(matches_loader(
            "1.21.1-forge-52.0.21",
            Loader::Forge,
            "1.21.1"
        ));
        assert!(matches_loader(
            "1.12.2-forge-14.23.5.2860",
            Loader::Forge,
            "1.12.2"
        ));
        assert!(!matches_loader(
            "1.21.1-forge-52.0.21",
            Loader::Forge,
            "1.21"
        ));
        assert!(matches_loader(
            "neoforge-21.1.80",
            Loader::NeoForge,
            "1.21.1"
        ));
        assert!(!matches_loader(
            "neoforge-21.0.80",
            Loader::NeoForge,
            "1.21.1"
        ));
    }
}
