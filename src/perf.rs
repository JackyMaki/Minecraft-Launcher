//! Performance presets: JVM flags tuned for a smooth client, memory sizing,
//! and JVM class-data-sharing archives that cut JVM start-up time.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PerfPreset {
    /// Exactly what the official launcher passes.
    Vanilla,
    /// Low-pause G1 tuning (Aikar-style), automatic memory, CDS archive. Default.
    #[default]
    Smooth,
    /// Generational ZGC on Java 21+ for the fewest stutters; more RAM.
    Max,
}

impl PerfPreset {
    pub const ALL: [PerfPreset; 3] = [PerfPreset::Vanilla, PerfPreset::Smooth, PerfPreset::Max];

    pub fn label(self) -> &'static str {
        match self {
            PerfPreset::Vanilla => "Vanilla (official defaults)",
            PerfPreset::Smooth => "Smooth (recommended)",
            PerfPreset::Max => "Max (ZGC, more RAM)",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "vanilla" | "default" => Some(PerfPreset::Vanilla),
            "smooth" => Some(PerfPreset::Smooth),
            "max" => Some(PerfPreset::Max),
            _ => None,
        }
    }
}

/// Total physical memory in MiB.
pub fn total_memory_mb() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    (sys.total_memory() / (1024 * 1024)).max(1024)
}

/// Pick a heap size (MiB) for a preset, unless the user chose one explicitly.
pub fn heap_mb(preset: PerfPreset, explicit_mb: Option<u64>, modded: bool) -> u64 {
    if let Some(mb) = explicit_mb.filter(|m| *m >= 512) {
        return mb;
    }
    let total = total_memory_mb();
    match preset {
        PerfPreset::Vanilla => 2048.min(total / 2).max(1024),
        PerfPreset::Smooth => {
            let base = (total / 4).clamp(2048, 6144);
            if modded {
                (base + 2048).min(total / 2)
            } else {
                base
            }
        }
        PerfPreset::Max => (total / 3).clamp(3072, 12288),
    }
}

/// JVM flags for a preset. `java_major` selects what the runtime supports.
/// `cds_archive` is where the class data sharing archive may be stored.
pub fn jvm_flags(
    preset: PerfPreset,
    java_major: u32,
    heap_mb: u64,
    cds_archive: Option<&Path>,
) -> Vec<String> {
    let mut f: Vec<String> = Vec::new();
    f.push(format!("-Xmx{heap_mb}M"));
    match preset {
        PerfPreset::Vanilla => {
            f.push("-Xss1M".into());
            f.extend(
                [
                    "-XX:+UnlockExperimentalVMOptions",
                    "-XX:+UseG1GC",
                    "-XX:G1NewSizePercent=20",
                    "-XX:G1ReservePercent=20",
                    "-XX:MaxGCPauseMillis=50",
                    "-XX:G1HeapRegionSize=32M",
                ]
                .map(String::from),
            );
        }
        PerfPreset::Smooth => {
            // Fixed heap avoids resize hitches; Aikar's low-pause G1 settings.
            f.push(format!("-Xms{heap_mb}M"));
            f.push("-Xss1M".into());
            f.extend(aikar_g1());
        }
        PerfPreset::Max => {
            f.push(format!("-Xms{heap_mb}M"));
            f.push("-Xss1M".into());
            if java_major >= 21 {
                f.push("-XX:+UseZGC".into());
                if java_major < 23 {
                    f.push("-XX:+ZGenerational".into());
                }
                f.push("-XX:+UnlockExperimentalVMOptions".into());
                f.push("-XX:+DisableExplicitGC".into());
            } else {
                f.extend(aikar_g1());
            }
        }
    }
    // Faster JVM start-up through application class-data sharing (JDK 19+).
    if preset != PerfPreset::Vanilla && java_major >= 19 {
        if let Some(cds) = cds_archive {
            f.push("-XX:+AutoCreateSharedArchive".into());
            f.push(format!("-XX:SharedArchiveFile={}", cds.display()));
        }
    }
    // Harmless everywhere, important for 1.7-1.18 (Log4Shell) and for correct text.
    f.push("-Dlog4j2.formatMsgNoLookups=true".into());
    f.push("-Dfile.encoding=UTF-8".into());
    if java_major >= 18 {
        f.push("-Dstdout.encoding=UTF-8".into());
        f.push("-Dstderr.encoding=UTF-8".into());
    }
    f
}

fn aikar_g1() -> Vec<String> {
    [
        "-XX:+UseG1GC",
        "-XX:+ParallelRefProcEnabled",
        "-XX:MaxGCPauseMillis=40",
        "-XX:+UnlockExperimentalVMOptions",
        "-XX:+DisableExplicitGC",
        "-XX:G1NewSizePercent=30",
        "-XX:G1MaxNewSizePercent=40",
        "-XX:G1HeapRegionSize=8M",
        "-XX:G1ReservePercent=20",
        "-XX:G1HeapWastePercent=5",
        "-XX:G1MixedGCCountTarget=4",
        "-XX:InitiatingHeapOccupancyPercent=15",
        "-XX:G1MixedGCLiveThresholdPercent=90",
        "-XX:G1RSetUpdatingPauseTimePercent=5",
        "-XX:SurvivorRatio=32",
        "-XX:+PerfDisableSharedMem",
        "-XX:MaxTenuringThreshold=1",
        "-Dusing.aikars.flags=https://mcflags.emc.gs",
        "-Daikars.new.flags=true",
    ]
    .map(String::from)
    .to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_by_java() {
        let f = jvm_flags(PerfPreset::Max, 21, 4096, Some(Path::new("/tmp/x.jsa")));
        assert!(f.contains(&"-XX:+UseZGC".to_string()));
        assert!(f.contains(&"-XX:+ZGenerational".to_string()));
        assert!(f.iter().any(|a| a.starts_with("-XX:SharedArchiveFile=")));
        let f = jvm_flags(PerfPreset::Max, 17, 4096, Some(Path::new("/tmp/x.jsa")));
        assert!(f.contains(&"-XX:+UseG1GC".to_string()));
        assert!(!f.iter().any(|a| a.starts_with("-XX:SharedArchiveFile=")));
        let f = jvm_flags(PerfPreset::Vanilla, 8, 2048, None);
        assert_eq!(f[0], "-Xmx2048M");
        assert!(f.contains(&"-XX:G1HeapRegionSize=32M".to_string()));
    }

    #[test]
    fn explicit_heap_wins() {
        assert_eq!(heap_mb(PerfPreset::Smooth, Some(3000), false), 3000);
        assert!(heap_mb(PerfPreset::Smooth, None, false) >= 1024);
    }
}
