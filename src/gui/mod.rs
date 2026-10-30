//! Native GUI built with egui. Starts instantly: everything slow runs on
//! background threads and the window renders from cached data.

#[cfg(feature = "gui")]
mod app;

#[cfg(feature = "gui")]
pub fn run(paths: crate::paths::Paths) -> anyhow::Result<()> {
    app::run(paths)
}

#[cfg(not(feature = "gui"))]
pub fn run(_paths: crate::paths::Paths) -> anyhow::Result<()> {
    anyhow::bail!(
        "this build has no GUI (compiled without the `gui` feature); use the CLI subcommands"
    )
}
