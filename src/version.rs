//! Model of a `versions/<id>/<id>.json` file (Mojang format, also used by
//! Fabric/Quilt/Forge/NeoForge installers via `inheritsFrom`).

use crate::rules::{allowed, Rule, RuleContext};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VersionJson {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minecraft_arguments: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arguments: Option<Arguments>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asset_index: Option<AssetIndexRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<HashMap<String, Download>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_version: Option<JavaVersion>,
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logging: Option<HashMap<String, LoggingConfig>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minimum_launcher_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compliance_level: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Arguments {
    #[serde(default)]
    pub game: Vec<Argument>,
    #[serde(default)]
    pub jvm: Vec<Argument>,
}

/// Either a plain string or `{ "rules": [...], "value": "x" | ["x", "y"] }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Argument {
    Plain(String),
    Conditional {
        #[serde(default)]
        rules: Vec<Rule>,
        value: ArgValue,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ArgValue {
    One(String),
    Many(Vec<String>),
}

impl ArgValue {
    pub fn as_vec(&self) -> Vec<String> {
        match self {
            ArgValue::One(s) => vec![s.clone()],
            ArgValue::Many(v) => v.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AssetIndexRef {
    pub id: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_size: Option<u64>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Download {
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct JavaVersion {
    pub component: String,
    pub major_version: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Library {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<LibraryDownloads>,
    /// Maven repository base URL (Fabric/Forge style libraries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha1: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    /// os -> classifier, e.g. `"linux": "natives-linux"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natives: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extract: Option<Extract>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LibraryDownloads {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact: Option<Download>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub classifiers: Option<HashMap<String, Download>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Extract {
    #[serde(default)]
    pub exclude: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoggingConfig {
    #[serde(default)]
    pub argument: String,
    pub file: LoggingFile,
    #[serde(default, rename = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoggingFile {
    pub id: String,
    #[serde(default)]
    pub sha1: String,
    #[serde(default)]
    pub size: u64,
    pub url: String,
}

/// A concrete file to fetch, derived from a library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFile {
    /// Path relative to the `libraries` directory.
    pub path: String,
    pub url: String,
    pub sha1: Option<String>,
    pub size: Option<u64>,
    /// `true` for native jars that must be extracted rather than put on the classpath.
    pub native: bool,
    pub extract_exclude: Vec<String>,
}

/// Parsed Maven coordinates `group:artifact:version[:classifier][@ext]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MavenCoord {
    pub group: String,
    pub artifact: String,
    pub version: String,
    pub classifier: Option<String>,
    pub ext: String,
}

impl MavenCoord {
    pub fn parse(name: &str) -> Result<Self> {
        let (main, ext) = match name.rsplit_once('@') {
            Some((m, e)) => (m, e.to_string()),
            None => (name, "jar".to_string()),
        };
        let parts: Vec<&str> = main.split(':').collect();
        if parts.len() < 3 {
            return Err(anyhow!("invalid maven coordinate: {name}"));
        }
        Ok(Self {
            group: parts[0].to_string(),
            artifact: parts[1].to_string(),
            version: parts[2].to_string(),
            classifier: parts.get(3).map(|s| s.to_string()),
            ext,
        })
    }

    pub fn path(&self) -> String {
        let mut file = format!("{}-{}", self.artifact, self.version);
        if let Some(c) = &self.classifier {
            file.push('-');
            file.push_str(c);
        }
        format!(
            "{}/{}/{}/{}.{}",
            self.group.replace('.', "/"),
            self.artifact,
            self.version,
            file,
            self.ext
        )
    }

    /// Key used for de-duplicating libraries: everything except the version.
    pub fn dedupe_key(&self) -> String {
        match &self.classifier {
            Some(c) => format!("{}:{}:{}", self.group, self.artifact, c),
            None => format!("{}:{}", self.group, self.artifact),
        }
    }
}

impl Library {
    pub fn is_allowed(&self, ctx: &RuleContext) -> bool {
        allowed(&self.rules, ctx)
    }

    /// Resolve the files this library contributes on the current platform.
    pub fn files(&self, ctx: &RuleContext) -> Result<Vec<LibraryFile>> {
        let mut out = Vec::new();
        if !self.is_allowed(ctx) {
            return Ok(out);
        }
        let coord = MavenCoord::parse(&self.name)?;
        let exclude = self
            .extract
            .as_ref()
            .map(|e| e.exclude.clone())
            .unwrap_or_default();

        // Main artifact.
        let has_native_classifier = self.natives.is_some();
        if let Some(dl) = self.downloads.as_ref().and_then(|d| d.artifact.as_ref()) {
            out.push(LibraryFile {
                path: dl.path.clone().unwrap_or_else(|| coord.path()),
                url: dl.url.clone(),
                sha1: non_empty(&dl.sha1),
                size: Some(dl.size).filter(|s| *s > 0),
                native: false,
                extract_exclude: exclude.clone(),
            });
        } else if self.downloads.is_none() || !has_native_classifier {
            // Maven-style library (Fabric, Forge, ...). Default repo is Mojang's.
            let base = self
                .url
                .clone()
                .unwrap_or_else(|| "https://libraries.minecraft.net/".to_string());
            let base = if base.ends_with('/') {
                base
            } else {
                format!("{base}/")
            };
            let path = coord.path();
            out.push(LibraryFile {
                url: format!("{base}{path}"),
                path,
                sha1: self.sha1.clone(),
                size: self.size,
                native: false,
                extract_exclude: exclude.clone(),
            });
        }

        // Legacy natives via classifier.
        if let Some(natives) = &self.natives {
            if let Some(classifier) = natives.get(&ctx.os_name) {
                let classifier = classifier.replace("${arch}", crate::paths::arch_bits());
                let classifiers = self.downloads.as_ref().and_then(|d| d.classifiers.as_ref());
                if let Some(dl) = classifiers.and_then(|c| c.get(&classifier)) {
                    let mut c2 = coord.clone();
                    c2.classifier = Some(classifier.clone());
                    out.push(LibraryFile {
                        path: dl.path.clone().unwrap_or_else(|| c2.path()),
                        url: dl.url.clone(),
                        sha1: non_empty(&dl.sha1),
                        size: Some(dl.size).filter(|s| *s > 0),
                        native: true,
                        extract_exclude: exclude.clone(),
                    });
                } else if self.downloads.is_none() {
                    let mut c2 = coord.clone();
                    c2.classifier = Some(classifier.clone());
                    let path = c2.path();
                    out.push(LibraryFile {
                        url: format!("https://libraries.minecraft.net/{path}"),
                        path,
                        sha1: None,
                        size: None,
                        native: true,
                        extract_exclude: exclude.clone(),
                    });
                }
            }
        }
        // Modern natives: a separate library entry whose name ends in a natives
        // classifier and that only exists for one OS. Those must be extracted too.
        if self.natives.is_none() {
            if let Some(c) = &coord.classifier {
                if c.starts_with("natives-") {
                    for f in out.iter_mut() {
                        f.native = true;
                    }
                }
            }
        }
        Ok(out)
    }
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

impl VersionJson {
    pub fn parse(text: &str) -> Result<Self> {
        serde_json::from_str(text).context("parsing version JSON")
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&text)
    }

    pub fn is_legacy(&self) -> bool {
        self.arguments.is_none() && self.minecraft_arguments.is_some()
    }

    /// Merge `self` (the child, e.g. a Fabric profile) on top of `parent`.
    /// Follows the official launcher semantics: child libraries come first,
    /// child arguments are appended, scalar fields in the child win.
    pub fn merge_with_parent(mut self, parent: VersionJson) -> VersionJson {
        let mut merged = parent;
        merged.id = self.id.clone();
        merged.inherits_from = None;
        if self.main_class.is_some() {
            merged.main_class = self.main_class.take();
        }
        if self.minecraft_arguments.is_some() {
            merged.minecraft_arguments = self.minecraft_arguments.take();
        }
        if self.asset_index.is_some() {
            merged.asset_index = self.asset_index.take();
        }
        if self.assets.is_some() {
            merged.assets = self.assets.take();
        }
        if self.java_version.is_some() {
            merged.java_version = self.java_version.take();
        }
        if self.kind.is_some() {
            merged.kind = self.kind.take();
        }
        if self.release_time.is_some() {
            merged.release_time = self.release_time.take();
        }
        if self.time.is_some() {
            merged.time = self.time.take();
        }
        if self.jar.is_some() {
            merged.jar = self.jar.take();
        }
        if let Some(d) = self.downloads.take() {
            merged.downloads.get_or_insert_with(HashMap::new).extend(d);
        }
        if let Some(l) = self.logging.take() {
            merged.logging.get_or_insert_with(HashMap::new).extend(l);
        }
        if let Some(args) = self.arguments.take() {
            let m = merged.arguments.get_or_insert_with(Arguments::default);
            m.game.extend(args.game);
            m.jvm.extend(args.jvm);
        }
        let mut libs = self.libraries;
        libs.extend(merged.libraries);
        merged.libraries = libs;
        merged
    }

    /// All library files for this platform, de-duplicated so that the first
    /// occurrence of a `group:artifact[:classifier]` wins (child overrides parent).
    pub fn library_files(&self, ctx: &RuleContext) -> Result<Vec<LibraryFile>> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();
        for lib in &self.libraries {
            if !lib.is_allowed(ctx) {
                continue;
            }
            let coord = MavenCoord::parse(&lib.name)?;
            let key = coord.dedupe_key();
            let files = lib.files(ctx)?;
            for f in files {
                let k = if f.native {
                    format!("{key}#native")
                } else {
                    key.clone()
                };
                if seen.insert(k) {
                    out.push(f);
                }
            }
        }
        Ok(out)
    }

    /// Jar id to put on the classpath (`jar` field or own id).
    pub fn jar_id(&self) -> &str {
        self.jar.as_deref().unwrap_or(&self.id)
    }

    pub fn required_java_major(&self) -> u32 {
        if let Some(j) = &self.java_version {
            return j.major_version;
        }
        // Heuristic for very old JSONs that have no javaVersion field.
        8
    }
}

/// Load a version and recursively resolve `inheritsFrom`.
pub fn load_resolved(paths: &crate::paths::Paths, id: &str) -> Result<VersionJson> {
    let mut chain = vec![];
    let mut current = id.to_string();
    let mut guard = 0;
    loop {
        let v = VersionJson::load(&paths.version_json(&current))
            .with_context(|| format!("loading version '{current}'"))?;
        let parent = v.inherits_from.clone();
        chain.push(v);
        match parent {
            Some(p) => {
                guard += 1;
                if guard > 10 {
                    return Err(anyhow!("inheritsFrom chain too deep for {id}"));
                }
                current = p;
            }
            None => break,
        }
    }
    // chain[0] is the child, last is the root parent.
    let mut merged = chain.pop().unwrap();
    while let Some(child) = chain.pop() {
        merged = child.merge_with_parent(merged);
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maven_paths() {
        let c = MavenCoord::parse("org.ow2.asm:asm:9.7.1").unwrap();
        assert_eq!(c.path(), "org/ow2/asm/asm/9.7.1/asm-9.7.1.jar");
        let c = MavenCoord::parse("org.lwjgl:lwjgl:3.3.3:natives-linux").unwrap();
        assert_eq!(
            c.path(),
            "org/lwjgl/lwjgl/3.3.3/lwjgl-3.3.3-natives-linux.jar"
        );
        let c = MavenCoord::parse("net.minecraft:client:1.21:srg@txt").unwrap();
        assert_eq!(c.path(), "net/minecraft/client/1.21/client-1.21-srg.txt");
    }

    #[test]
    fn merge_child_wins() {
        let parent = VersionJson {
            id: "1.21".into(),
            main_class: Some("net.minecraft.client.main.Main".into()),
            libraries: vec![Library {
                name: "a:b:1".into(),
                ..Default::default()
            }],
            arguments: Some(Arguments {
                game: vec![Argument::Plain("--p".into())],
                jvm: vec![],
            }),
            ..Default::default()
        };
        let child = VersionJson {
            id: "fabric".into(),
            inherits_from: Some("1.21".into()),
            main_class: Some("knot".into()),
            libraries: vec![Library {
                name: "a:b:2".into(),
                ..Default::default()
            }],
            arguments: Some(Arguments {
                game: vec![Argument::Plain("--c".into())],
                jvm: vec![Argument::Plain("-Dx".into())],
            }),
            ..Default::default()
        };
        let m = child.merge_with_parent(parent);
        assert_eq!(m.id, "fabric");
        assert_eq!(m.main_class.as_deref(), Some("knot"));
        assert_eq!(m.libraries[0].name, "a:b:2");
        assert_eq!(m.libraries.len(), 2);
        let files = m.library_files(&RuleContext::current()).unwrap();
        assert_eq!(files.len(), 1, "dedupe keeps the child's version");
        assert!(files[0].path.contains("b-2.jar"));
        let args = m.arguments.unwrap();
        assert_eq!(args.game.len(), 2);
        assert_eq!(args.jvm.len(), 1);
    }
}
