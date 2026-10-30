//! Profiles. We read and write the official `launcher_profiles.json` so the
//! two launchers can share profiles; launcher-specific extras (loader,
//! performance preset, Quick Play target...) live in `profiles_extra.json`.

use crate::paths::{write_atomic, Paths};
use crate::perf::PerfPreset;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OfficialProfile {
    #[serde(default)]
    pub name: String,
    /// `latest-release`, `latest-snapshot` or `custom`.
    #[serde(default = "custom", rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub created: String,
    #[serde(default)]
    pub last_used: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub last_version_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub game_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub java_args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    /// Anything else the official launcher stores; preserved verbatim.
    #[serde(flatten)]
    pub other: BTreeMap<String, Value>,
}

fn custom() -> String {
    "custom".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Loader {
    #[default]
    Vanilla,
    Fabric,
    Quilt,
    /// Fabric for 1.3 - 1.13 via meta.legacyfabric.net.
    #[serde(rename = "legacyfabric")]
    LegacyFabric,
    Forge,
    #[serde(rename = "neoforge")]
    NeoForge,
}

impl Loader {
    pub const ALL: [Loader; 6] = [
        Loader::Vanilla,
        Loader::Fabric,
        Loader::Quilt,
        Loader::Forge,
        Loader::NeoForge,
        Loader::LegacyFabric,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Loader::Vanilla => "Vanilla",
            Loader::Fabric => "Fabric",
            Loader::Quilt => "Quilt",
            Loader::LegacyFabric => "Legacy Fabric (1.3 - 1.13)",
            Loader::Forge => "Forge",
            Loader::NeoForge => "NeoForge",
        }
    }

    /// Short description for the GUI.
    pub fn hint(self) -> &'static str {
        match self {
            Loader::Vanilla => "No mods. Fastest to install.",
            Loader::Fabric => "Lightweight modern loader. Best choice for the performance pack.",
            Loader::Quilt => "Fabric-compatible loader; runs Fabric mods too.",
            Loader::LegacyFabric => "Fabric ported to old versions (1.3 to 1.13).",
            Loader::Forge => {
                "The classic loader for big mod packs. Installed with Forge's own installer."
            }
            Loader::NeoForge => {
                "Modern fork of Forge (1.20.2+). Installed with NeoForge's own installer."
            }
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Loader::Vanilla => "vanilla",
            Loader::Fabric => "fabric",
            Loader::Quilt => "quilt",
            Loader::LegacyFabric => "legacyfabric",
            Loader::Forge => "forge",
            Loader::NeoForge => "neoforge",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "vanilla" | "none" => Some(Loader::Vanilla),
            "fabric" => Some(Loader::Fabric),
            "quilt" => Some(Loader::Quilt),
            "legacyfabric" => Some(Loader::LegacyFabric),
            "forge" => Some(Loader::Forge),
            "neoforge" => Some(Loader::NeoForge),
            _ => None,
        }
    }

    pub fn is_modded(self) -> bool {
        self != Loader::Vanilla
    }

    /// Modrinth loader names to query for this loader, in order of preference.
    pub fn modrinth_loaders(self) -> &'static [&'static str] {
        match self {
            Loader::Vanilla => &[],
            Loader::Fabric => &["fabric"],
            Loader::Quilt => &["quilt", "fabric"],
            Loader::LegacyFabric => &["legacy-fabric", "fabric"],
            Loader::Forge => &["forge"],
            Loader::NeoForge => &["neoforge"],
        }
    }
}

/// What to do right after the game starts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum QuickPlay {
    #[default]
    None,
    /// Join a singleplayer world by its save folder name.
    Singleplayer { world: String },
    /// Join a server (`host` or `host:port`).
    Multiplayer { address: String },
    /// Join a Realm by id.
    Realms { id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProfileExtra {
    pub loader: Loader,
    pub perf: PerfPreset,
    /// 0 = automatic.
    pub memory_mb: u64,
    pub quick_play: QuickPlay,
    /// Install the FPS/startup mod pack (Fabric only).
    pub perf_mods: bool,
    /// Extra Modrinth project slugs to install (Fabric only).
    pub extra_mods: Vec<String>,
    /// Pinned loader version (empty = latest stable/recommended).
    #[serde(alias = "fabric_loader")]
    pub loader_version: String,
}

impl Default for ProfileExtra {
    fn default() -> Self {
        Self {
            loader: Loader::Vanilla,
            perf: PerfPreset::Smooth,
            memory_mb: 0,
            quick_play: QuickPlay::None,
            perf_mods: true,
            extra_mods: Vec::new(),
            loader_version: String::new(),
        }
    }
}

/// A profile as the launcher sees it: official fields plus our extras.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub key: String,
    pub official: OfficialProfile,
    pub extra: ProfileExtra,
}

impl Profile {
    pub fn new(name: &str, version_id: &str) -> Self {
        let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        Self {
            key: uuid::Uuid::new_v4().simple().to_string(),
            official: OfficialProfile {
                name: name.to_string(),
                kind: "custom".into(),
                created: now.clone(),
                last_used: now,
                icon: Some("Grass".into()),
                last_version_id: version_id.to_string(),
                ..Default::default()
            },
            extra: ProfileExtra::default(),
        }
    }

    pub fn display_name(&self) -> String {
        if !self.official.name.is_empty() {
            return self.official.name.clone();
        }
        match self.official.kind.as_str() {
            "latest-release" => "Latest release".into(),
            "latest-snapshot" => "Latest snapshot".into(),
            _ => self.official.last_version_id.clone(),
        }
    }

    pub fn game_dir(&self, paths: &Paths) -> PathBuf {
        match &self.official.game_dir {
            Some(d) if !d.is_empty() => PathBuf::from(d),
            _ => paths.root.clone(),
        }
    }

    pub fn touch(&mut self) {
        self.official.last_used =
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    }
}

#[derive(Debug, Clone, Default)]
pub struct Profiles {
    pub profiles: Vec<Profile>,
    /// Other top-level keys of launcher_profiles.json (settings, version...).
    pub other: BTreeMap<String, Value>,
    pub selected: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ExtraFile {
    #[serde(default)]
    selected: Option<String>,
    #[serde(default)]
    profiles: BTreeMap<String, ProfileExtra>,
}

impl Profiles {
    pub fn load(paths: &Paths) -> Result<Self> {
        let mut out = Profiles::default();
        if let Ok(text) = std::fs::read_to_string(paths.profiles_json()) {
            let mut v: Value =
                serde_json::from_str(&text).context("parsing launcher_profiles.json")?;
            if let Some(obj) = v.as_object_mut() {
                if let Some(Value::Object(profiles)) = obj.remove("profiles") {
                    for (key, p) in profiles {
                        let official: OfficialProfile =
                            serde_json::from_value(p).unwrap_or_default();
                        out.profiles.push(Profile {
                            key,
                            official,
                            extra: ProfileExtra::default(),
                        });
                    }
                }
                for (k, v) in obj.iter() {
                    out.other.insert(k.clone(), v.clone());
                }
            }
        }
        let extra: ExtraFile = std::fs::read_to_string(paths.profiles_extra_json())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        for p in &mut out.profiles {
            if let Some(e) = extra.profiles.get(&p.key) {
                p.extra = e.clone();
            }
        }
        out.selected = extra
            .selected
            .filter(|s| out.profiles.iter().any(|p| &p.key == s));
        if out.selected.is_none() {
            out.selected = out
                .profiles
                .iter()
                .max_by(|a, b| a.official.last_used.cmp(&b.official.last_used))
                .map(|p| p.key.clone());
        }
        Ok(out)
    }

    /// Create the two default profiles the official launcher ships with.
    pub fn ensure_defaults(&mut self) -> bool {
        let mut changed = false;
        if !self
            .profiles
            .iter()
            .any(|p| p.official.kind == "latest-release")
        {
            let mut p = Profile::new("", "latest-release");
            p.official.kind = "latest-release".into();
            p.official.icon = Some("Grass".into());
            self.selected.get_or_insert(p.key.clone());
            self.profiles.push(p);
            changed = true;
        }
        if !self
            .profiles
            .iter()
            .any(|p| p.official.kind == "latest-snapshot")
        {
            let mut p = Profile::new("", "latest-snapshot");
            p.official.kind = "latest-snapshot".into();
            p.official.icon = Some("Crafting_Table".into());
            self.profiles.push(p);
            changed = true;
        }
        changed
    }

    pub fn save(&self, paths: &Paths) -> Result<()> {
        let mut root = serde_json::Map::new();
        let mut profiles = serde_json::Map::new();
        for p in &self.profiles {
            profiles.insert(p.key.clone(), serde_json::to_value(&p.official)?);
        }
        root.insert("profiles".into(), Value::Object(profiles));
        for (k, v) in &self.other {
            root.insert(k.clone(), v.clone());
        }
        root.entry("settings")
            .or_insert_with(|| serde_json::json!({}));
        root.entry("version").or_insert(Value::from(3));
        let text = serde_json::to_string_pretty(&Value::Object(root))?;
        write_atomic(&paths.profiles_json(), text.as_bytes())
            .context("saving launcher_profiles.json")?;

        let extra = ExtraFile {
            selected: self.selected.clone(),
            profiles: self
                .profiles
                .iter()
                .map(|p| (p.key.clone(), p.extra.clone()))
                .collect(),
        };
        let text = serde_json::to_string_pretty(&extra)?;
        write_atomic(&paths.profiles_extra_json(), text.as_bytes())
            .context("saving profiles_extra.json")?;
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.key == key)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Profile> {
        self.profiles.iter_mut().find(|p| p.key == key)
    }
    /// Find by key or (case-insensitive) display name.
    pub fn find(&self, name_or_key: &str) -> Option<&Profile> {
        self.get(name_or_key).or_else(|| {
            self.profiles
                .iter()
                .find(|p| p.display_name().eq_ignore_ascii_case(name_or_key))
        })
    }
    pub fn selected(&self) -> Option<&Profile> {
        self.selected.as_deref().and_then(|k| self.get(k))
    }
    pub fn remove(&mut self, key: &str) {
        self.profiles.retain(|p| p.key != key);
        if self.selected.as_deref() == Some(key) {
            self.selected = self.profiles.first().map(|p| p.key.clone());
        }
    }
}

/// Global launcher settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Azure application (client) id used for Microsoft sign-in.
    pub msa_client_id: String,
    /// Parallel downloads.
    pub parallel_downloads: usize,
    /// Hash every file on each launch (slow) instead of trusting sizes.
    pub verify_on_launch: bool,
    /// Keep the launcher window open while the game runs.
    pub keep_open: bool,
    /// Let the launcher download Mojang's Java runtimes.
    pub auto_java: bool,
    /// CurseForge API key (free at console.curseforge.com); enables the CurseForge catalog.
    pub curseforge_api_key: String,
    /// Look and feel of the GUI.
    pub appearance: Appearance,
}

/// Everything about how the GUI looks. All fields have sensible defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    /// Built-in theme name (`dark`, `midnight`, `light`, `grass`, `nether`, `ocean`, `custom`).
    pub theme: String,
    /// Accent colour (buttons, highlights), RGB.
    pub accent: [u8; 3],
    /// Window background, RGB (custom theme).
    pub background: [u8; 3],
    /// Panel / card colour, RGB (custom theme).
    pub panel: [u8; 3],
    /// Text colour, RGB (custom theme).
    pub text: [u8; 3],
    /// Whole-UI zoom, 0.7 .. 1.6.
    pub zoom: f32,
    /// Corner rounding in points, 0 .. 16.
    pub rounding: u8,
    /// Optional background picture (PNG/JPEG path).
    pub background_image: String,
    /// How much to darken the background picture, 0 .. 100.
    pub background_dim: u8,
    /// Profiles list on the right instead of the left.
    pub sidebar_right: bool,
    /// Tighter spacing.
    pub compact: bool,
    /// Text on the big button.
    pub play_label: String,
    /// Show one-line hints next to options.
    pub show_hints: bool,
    /// Translucent cards over the background.
    pub glass: bool,
    /// Draw our own title bar instead of the OS window frame.
    pub custom_frame: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme: "dark".into(),
            accent: [0x2e, 0xb8, 0x6a],
            background: [0x11, 0x13, 0x16],
            panel: [0x1a, 0x1d, 0x22],
            text: [0xe6, 0xe6, 0xe6],
            zoom: 1.0,
            rounding: 8,
            background_image: String::new(),
            background_dim: 55,
            sidebar_right: false,
            compact: false,
            play_label: "PLAY".into(),
            show_hints: true,
            glass: true,
            custom_frame: true,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            msa_client_id: String::new(),
            parallel_downloads: 32,
            verify_on_launch: false,
            keep_open: true,
            auto_java: true,
            curseforge_api_key: String::new(),
            appearance: Appearance::default(),
        }
    }
}

impl Settings {
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.settings_json())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(&paths.settings_json(), text.as_bytes()).context("saving settings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_official_file() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        std::fs::write(
            paths.profiles_json(),
            r#"{"profiles":{"abc":{"name":"My 1.20","type":"custom","lastVersionId":"1.20.4","javaArgs":"-Xmx3G","created":"2024-01-01T00:00:00.000Z","lastUsed":"2024-02-01T00:00:00.000Z","icon":"Furnace","skipJreVersionCheck":true}},"settings":{"crashAssistance":true},"version":3}"#,
        )
        .unwrap();
        let mut p = Profiles::load(&paths).unwrap();
        assert_eq!(p.profiles.len(), 1);
        assert_eq!(p.profiles[0].official.java_args.as_deref(), Some("-Xmx3G"));
        assert_eq!(p.selected.as_deref(), Some("abc"));
        p.profiles[0].extra.loader = Loader::Fabric;
        p.profiles[0].extra.quick_play = QuickPlay::Multiplayer {
            address: "mc.hypixel.net".into(),
        };
        assert!(p.ensure_defaults());
        p.save(&paths).unwrap();

        let p2 = Profiles::load(&paths).unwrap();
        assert_eq!(p2.profiles.len(), 3);
        let mine = p2.get("abc").unwrap();
        assert_eq!(mine.extra.loader, Loader::Fabric);
        assert_eq!(
            mine.extra.quick_play,
            QuickPlay::Multiplayer {
                address: "mc.hypixel.net".into()
            }
        );
        // unknown official fields are preserved
        assert_eq!(
            mine.official.other.get("skipJreVersionCheck"),
            Some(&Value::Bool(true))
        );
        let raw: Value =
            serde_json::from_str(&std::fs::read_to_string(paths.profiles_json()).unwrap()).unwrap();
        assert_eq!(raw["settings"]["crashAssistance"], Value::Bool(true));
        assert_eq!(raw["version"], Value::from(3));
    }
}
