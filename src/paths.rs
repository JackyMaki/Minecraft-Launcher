//! Directory layout. We use the same `.minecraft` directory as the official
//! launcher so existing worlds, resource packs, options and screenshots are
//! picked up unchanged.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct Paths {
    /// The game root (`.minecraft`). Shared with the official launcher.
    pub root: PathBuf,
}

impl Default for Paths {
    fn default() -> Self {
        Self::new(Self::default_root())
    }
}

impl Paths {
    /// Resolve the default game directory for the current platform, honouring
    /// the `MC_LAUNCHER_ROOT` environment variable as an override.
    pub fn default_root() -> PathBuf {
        if let Ok(v) = std::env::var("MC_LAUNCHER_ROOT") {
            if !v.is_empty() {
                return PathBuf::from(v);
            }
        }
        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = dirs::data_dir() {
                // %APPDATA%\.minecraft
                return appdata.join(".minecraft");
            }
        }
        #[cfg(target_os = "macos")]
        {
            if let Some(home) = dirs::home_dir() {
                return home
                    .join("Library")
                    .join("Application Support")
                    .join("minecraft");
            }
        }
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".minecraft")
    }

    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn versions(&self) -> PathBuf {
        self.root.join("versions")
    }
    pub fn version_dir(&self, id: &str) -> PathBuf {
        self.versions().join(id)
    }
    pub fn version_json(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.json"))
    }
    pub fn version_jar(&self, id: &str) -> PathBuf {
        self.version_dir(id).join(format!("{id}.jar"))
    }
    pub fn natives_dir(&self, id: &str) -> PathBuf {
        self.version_dir(id).join("natives")
    }
    pub fn libraries(&self) -> PathBuf {
        self.root.join("libraries")
    }
    pub fn assets(&self) -> PathBuf {
        self.root.join("assets")
    }
    pub fn asset_indexes(&self) -> PathBuf {
        self.assets().join("indexes")
    }
    pub fn asset_objects(&self) -> PathBuf {
        self.assets().join("objects")
    }
    pub fn asset_virtual(&self, index: &str) -> PathBuf {
        self.assets().join("virtual").join(index)
    }
    pub fn log_configs(&self) -> PathBuf {
        self.assets().join("log_configs")
    }
    pub fn runtimes(&self) -> PathBuf {
        self.root.join("runtime")
    }
    /// Launcher-private state (caches, accounts, extra profile settings).
    pub fn launcher_dir(&self) -> PathBuf {
        let dir = self.root.join("flint");
        let legacy = self.root.join("launcher_fast");
        if !dir.exists() && legacy.is_dir() {
            let _ = std::fs::rename(&legacy, &dir);
        }
        dir
    }
    pub fn cache_dir(&self) -> PathBuf {
        self.launcher_dir().join("cache")
    }
    /// JVM class-data-sharing archives, one per version. Speeds up JVM start.
    pub fn cds_dir(&self) -> PathBuf {
        self.launcher_dir().join("cds")
    }
    pub fn profiles_json(&self) -> PathBuf {
        self.root.join("launcher_profiles.json")
    }
    pub fn profiles_extra_json(&self) -> PathBuf {
        self.launcher_dir().join("profiles_extra.json")
    }
    pub fn accounts_json(&self) -> PathBuf {
        self.launcher_dir().join("accounts.json")
    }
    pub fn settings_json(&self) -> PathBuf {
        self.launcher_dir().join("settings.json")
    }
}

/// Create all parent directories of `file`.
pub fn ensure_parent(file: &Path) -> std::io::Result<()> {
    if let Some(p) = file.parent() {
        std::fs::create_dir_all(p)?;
    }
    Ok(())
}

/// Atomically write `data` to `path` (write to temp file, then rename).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    ensure_parent(path)?;
    let tmp = path.with_extension(format!(
        "{}.tmp{}",
        path.extension().and_then(|e| e.to_str()).unwrap_or(""),
        std::process::id()
    ));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Current OS name in Mojang's vocabulary: `windows`, `osx` or `linux`.
pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

/// `"64"` or `"32"`, used for the `${arch}` placeholder in natives classifiers.
pub fn arch_bits() -> &'static str {
    if cfg!(target_pointer_width = "64") {
        "64"
    } else {
        "32"
    }
}

/// Mojang's java-runtime platform key for this machine.
pub fn java_runtime_platform() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => "windows-x64",
        ("windows", "x86") => "windows-x86",
        ("windows", "aarch64") => "windows-arm64",
        ("macos", "aarch64") => "mac-os-arm64",
        ("macos", _) => "mac-os",
        ("linux", "x86") => "linux-i386",
        _ => "linux",
    }
}

pub fn classpath_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}
