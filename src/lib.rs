//! Core of the launcher: version metadata, downloading, Java provisioning,
//! authentication, profiles and the launch command builder.
//!
//! Everything in here is UI-agnostic. The CLI (`src/main.rs`) and the GUI
//! (`src/gui`) are thin layers on top of these modules.

pub mod assets;
pub mod auth;
pub mod download;
pub mod fabric;
pub mod forge;
pub mod http;
pub mod install;
pub mod java;
pub mod launch;
pub mod manifest;
pub mod mods;
pub mod natives;
pub mod paths;
pub mod perf;
pub mod profiles;
pub mod rules;
pub mod session;
pub mod version;

pub const LAUNCHER_NAME: &str = "minecraft-launcher";
pub const LAUNCHER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Progress events emitted by long running operations (install, download).
#[derive(Debug, Clone)]
pub enum Progress {
    /// A new phase started (e.g. "Downloading libraries").
    Phase(String),
    /// `done` of `total` units finished in the current phase.
    Step { done: u64, total: u64 },
    /// Free-form log line.
    Log(String),
    /// The operation finished successfully.
    Done,
    /// The operation failed.
    Failed(String),
}

/// Sink for progress events. `None` discards everything.
pub type ProgressSink = Option<std::sync::mpsc::Sender<Progress>>;

pub fn report(sink: &ProgressSink, p: Progress) {
    if let Progress::Phase(s) = &p {
        log::debug!("phase: {s}");
    }
    if let Some(tx) = sink {
        let _ = tx.send(p);
    }
}
pub mod gui;
