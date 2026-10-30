//! Orchestrates everything that must be on disk before a launch: version
//! JSON (and parents), client jar, libraries, natives, assets, log config,
//! Java runtime, Fabric loader and mods.

use crate::download::Task;
use crate::manifest::VersionManifest;
use crate::paths::Paths;
use crate::profiles::{Loader, Profile, Settings};
use crate::rules::RuleContext;
use crate::version::{load_resolved, VersionJson};
use crate::{report, Progress, ProgressSink};
use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct InstallOptions {
    pub verify: bool,
    pub parallel: usize,
    pub auto_java: bool,
    pub java_override: Option<PathBuf>,
}

impl From<&Settings> for InstallOptions {
    fn from(s: &Settings) -> Self {
        Self {
            verify: s.verify_on_launch,
            parallel: s.parallel_downloads,
            auto_java: s.auto_java,
            java_override: None,
        }
    }
}

/// Result of a successful installation.
#[derive(Debug, Clone)]
pub struct Installed {
    pub version_id: String,
    pub version: VersionJson,
    pub java: PathBuf,
    pub java_major: u32,
    pub modded: bool,
}

/// Make sure `versions/<id>/<id>.json` exists, fetching it from the manifest
/// when needed, then recurse into `inheritsFrom`.
pub fn ensure_version_json(
    paths: &Paths,
    manifest: Option<&VersionManifest>,
    id: &str,
    verify: bool,
) -> Result<()> {
    let file = paths.version_json(id);
    let entry = manifest.and_then(|m| m.get(id));
    let need = match entry {
        Some(e) if !e.sha1.is_empty() => {
            let t = Task::new(&e.url, &file).sha1(Some(e.sha1.clone()));
            !crate::download::is_satisfied(&t, verify || !file.exists())
        }
        _ => !file.is_file(),
    };
    if need {
        let e = entry.ok_or_else(|| {
            anyhow!("unknown version '{id}' (not installed and not in the version manifest)")
        })?;
        crate::download::fetch_one(&Task::new(&e.url, &file).sha1(Some(e.sha1.clone())))?;
    }
    let v = VersionJson::load(&file)?;
    if let Some(parent) = &v.inherits_from {
        ensure_version_json(paths, manifest, parent, verify)?;
    }
    Ok(())
}

/// Install a (vanilla or modded) version id and resolve Java for it.
pub fn install_version(
    paths: &Paths,
    manifest: Option<&VersionManifest>,
    id: &str,
    game_dir: &Path,
    opts: &InstallOptions,
    sink: &ProgressSink,
) -> Result<Installed> {
    report(sink, Progress::Phase(format!("Resolving {id}")));
    ensure_version_json(paths, manifest, id, opts.verify)?;
    let version = load_resolved(paths, id)?;
    let ctx = RuleContext::current();

    // Libraries + client jar.
    report(sink, Progress::Phase("Checking libraries".into()));
    let libs = version.library_files(&ctx)?;
    let lib_dir = paths.libraries();
    let mut tasks: Vec<Task> = Vec::with_capacity(libs.len() + 2);
    for l in &libs {
        let dest = lib_dir.join(&l.path);
        if l.url.is_empty() {
            // Forge/NeoForge generate some jars locally with their installer.
            if !dest.is_file() {
                return Err(anyhow!(
                    "library {} is missing and has no download URL; re-run the loader installer",
                    l.path
                ));
            }
            continue;
        }
        tasks.push(Task::new(&l.url, dest).sha1(l.sha1.clone()).size(l.size));
    }
    let jar_id = version.jar_id().to_string();
    if let Some(client) = version.downloads.as_ref().and_then(|d| d.get("client")) {
        tasks.push(
            Task::new(&client.url, paths.version_jar(&jar_id))
                .sha1(Some(client.sha1.clone()))
                .size(Some(client.size)),
        );
    } else if !paths.version_jar(&jar_id).is_file() {
        return Err(anyhow!(
            "version {id} has no client download and {}.jar is missing",
            jar_id
        ));
    }
    if let Some(client) = version.logging.as_ref().and_then(|l| l.get("client")) {
        tasks.push(
            Task::new(&client.file.url, paths.log_configs().join(&client.file.id))
                .sha1(Some(client.file.sha1.clone()))
                .size(Some(client.file.size)),
        );
    }
    report(
        sink,
        Progress::Phase(format!("Downloading libraries ({} files)", tasks.len())),
    );
    crate::download::fetch_all(&tasks, opts.verify, opts.parallel, sink)?;

    // Natives.
    let natives_dir = paths.natives_dir(&version.id);
    let mut extracted = 0;
    for l in libs.iter().filter(|l| l.native) {
        extracted += crate::natives::extract_natives(
            &lib_dir.join(&l.path),
            &natives_dir,
            &l.extract_exclude,
        )
        .with_context(|| format!("extracting natives from {}", l.path))?;
    }
    if extracted > 0 {
        report(
            sink,
            Progress::Log(format!("Extracted {extracted} native file(s)")),
        );
    }

    // Assets.
    if let Some(idx) = &version.asset_index {
        let index = crate::assets::ensure_index(paths, idx, opts.verify)?;
        crate::assets::ensure_objects(
            paths,
            &idx.id,
            &index,
            game_dir,
            opts.verify,
            opts.parallel,
            sink,
        )?;
    }

    // Java.
    report(sink, Progress::Phase("Checking Java".into()));
    let major = version.required_java_major();
    let java = crate::java::resolve_java(
        paths,
        major,
        opts.java_override.as_deref(),
        opts.auto_java,
        sink,
    )?;
    let java_major = crate::java::probe_cached(paths, &java)
        .map(|j| j.major)
        .unwrap_or(major);

    Ok(Installed {
        version_id: version.id.clone(),
        version,
        java,
        java_major,
        modded: false,
    })
}

/// Install everything a profile needs (Fabric + mods when selected).
pub fn prepare_profile(
    paths: &Paths,
    manifest: Option<&VersionManifest>,
    profile: &Profile,
    opts: &InstallOptions,
    sink: &ProgressSink,
) -> Result<Installed> {
    let game_dir = profile.game_dir(paths);
    std::fs::create_dir_all(&game_dir)?;
    let base_id = profile.official.last_version_id.clone();
    let base_id = match manifest {
        Some(m) => m.resolve_alias(&base_id).to_string(),
        None if base_id.starts_with("latest-") => {
            return Err(anyhow!(
            "cannot resolve '{base_id}' without the version manifest (offline and never cached)"
        ))
        }
        None => base_id,
    };
    let mut opts = opts.clone();
    if let Some(j) = &profile.official.java_dir {
        if !j.is_empty() {
            opts.java_override = Some(PathBuf::from(j));
        }
    }

    if profile.extra.loader == Loader::Vanilla {
        return install_version(paths, manifest, &base_id, &game_dir, &opts, sink);
    }

    // Modded: vanilla files first (every loader inherits them), then the loader.
    let loader = profile.extra.loader;
    let pinned = if profile.extra.loader_version.trim().is_empty() {
        None
    } else {
        Some(profile.extra.loader_version.trim())
    };
    let modded_id = match crate::fabric::Flavor::for_loader(loader) {
        Some(flavor) => {
            ensure_version_json(paths, manifest, &base_id, opts.verify)?;
            report(
                sink,
                Progress::Phase(format!("{} loader for {base_id}", loader.label())),
            );
            crate::fabric::install(paths, flavor, &base_id, pinned)?
        }
        None => {
            // Forge / NeoForge need the vanilla client fully installed and a Java to run their installer.
            if let Some(id) = crate::forge::installed_for(paths, loader, &base_id, pinned) {
                id
            } else {
                let vanilla = install_version(paths, manifest, &base_id, &game_dir, &opts, sink)?;
                crate::forge::install(paths, loader, &base_id, pinned, &vanilla.java, sink)?
            }
        }
    };
    let mut installed = install_version(paths, manifest, &modded_id, &game_dir, &opts, sink)?;
    installed.modded = true;

    let mut slugs: Vec<String> = Vec::new();
    if profile.extra.perf_mods {
        slugs.extend(crate::mods::PERF_MODS.iter().map(|(s, _)| s.to_string()));
    }
    for m in &profile.extra.extra_mods {
        if let Ok(s) = crate::mods::validate_slug(m) {
            if !slugs.contains(&s) {
                slugs.push(s);
            }
        }
    }
    if !slugs.is_empty() {
        crate::mods::install(
            paths,
            &game_dir.join("mods"),
            &slugs,
            &base_id,
            loader,
            opts.verify,
            opts.parallel,
            sink,
        )?;
    }
    Ok(installed)
}
