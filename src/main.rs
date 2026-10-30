//! Command line entry point. Running without arguments opens the GUI.

use anyhow::{anyhow, Context, Result};
use clap::{Args, Parser, Subcommand};
use flint_launcher::auth::{Account, Accounts};
use flint_launcher::manifest::VersionManifest;
use flint_launcher::paths::Paths;
use flint_launcher::perf::PerfPreset;
use flint_launcher::profiles::{Loader, Profile, Profiles, QuickPlay, Resolution, Settings};
use flint_launcher::session::{self, Overrides};
use flint_launcher::{Progress, ProgressSink};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "flint", version, about = "Flint: a fast open-source Minecraft launcher", long_about = None)]
struct Cli {
    /// Game directory (defaults to the standard .minecraft folder).
    #[arg(long, global = true, env = "MC_LAUNCHER_ROOT")]
    root: Option<PathBuf>,
    /// Verbose logging.
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Open the graphical launcher (default).
    Gui,
    /// List available versions.
    Versions {
        /// Include snapshots.
        #[arg(short, long)]
        snapshots: bool,
        /// Include old alpha/beta versions.
        #[arg(short, long)]
        all: bool,
        /// Only installed versions.
        #[arg(short, long)]
        installed: bool,
        /// Refresh the manifest from Mojang.
        #[arg(long)]
        refresh: bool,
    },
    /// Download a version (libraries, assets, Java) without launching it.
    Install {
        /// Version id, `latest-release` or `latest-snapshot`.
        version: String,
        /// Mod loader: fabric, quilt, forge, neoforge, legacyfabric.
        #[arg(long)]
        loader: Option<String>,
        /// Shorthand for `--loader fabric`.
        #[arg(long)]
        fabric: bool,
        /// Re-hash every file.
        #[arg(long)]
        verify: bool,
    },
    /// Launch a profile (or an ad-hoc version).
    Launch(LaunchArgs),
    /// Manage profiles.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
    /// Sign in with a Microsoft account (device code flow) or add an offline account.
    Login {
        /// Create an offline account with this name instead.
        #[arg(long)]
        offline: Option<String>,
    },
    /// Manage accounts.
    Accounts {
        #[command(subcommand)]
        cmd: AccountCmd,
    },
    /// Show Java installations the launcher can see.
    Java,
    /// List singleplayer worlds and saved servers (for Quick Play).
    Worlds {
        /// Profile whose game directory to inspect.
        profile: Option<String>,
    },
    /// Install mods from Modrinth into a modded profile's mods folder.
    Mods {
        /// Profile name.
        profile: String,
        /// Modrinth project slugs (e.g. sodium iris). Empty = performance pack.
        slugs: Vec<String>,
    },
    /// Search the mod catalog (Modrinth, or CurseForge with --curseforge).
    Search {
        /// Words to search for (empty = most popular).
        query: Vec<String>,
        /// Minecraft version filter, e.g. 1.21.1.
        #[arg(long)]
        version: Option<String>,
        /// Loader filter: fabric, quilt, forge, neoforge.
        #[arg(long)]
        loader: Option<String>,
        /// Search resource packs or shaders instead of mods: mod | resourcepack | shader
        #[arg(long, default_value = "mod")]
        kind: String,
        /// Use CurseForge (needs `settings curseforge_api_key=...`).
        #[arg(long)]
        curseforge: bool,
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
    /// Show or change launcher settings.
    Settings {
        /// `key=value` pairs: msa_client_id, curseforge_api_key, parallel_downloads, verify_on_launch, keep_open, auto_java
        set: Vec<String>,
    },
}

#[derive(Args, Clone)]
struct LaunchArgs {
    /// Profile name or key. Omit to use the selected profile.
    profile: Option<String>,
    /// Launch this version instead of the profile's.
    #[arg(long)]
    version: Option<String>,
    /// Mod loader for an ad-hoc `--version` launch: fabric, quilt, forge, neoforge, legacyfabric.
    #[arg(long)]
    loader: Option<String>,
    /// Shorthand for `--loader fabric`.
    #[arg(long)]
    fabric: bool,
    /// Jump straight into this singleplayer world (save folder name).
    #[arg(long, short = 'w')]
    world: Option<String>,
    /// Jump straight onto this server (host[:port]).
    #[arg(long, short = 's')]
    server: Option<String>,
    /// Join this Realm id.
    #[arg(long)]
    realm: Option<String>,
    /// Play offline with this name (no Microsoft account).
    #[arg(long)]
    offline: Option<String>,
    /// Performance preset: vanilla, smooth, max.
    #[arg(long)]
    preset: Option<String>,
    /// Heap size in MiB.
    #[arg(long)]
    memory: Option<u64>,
    /// Demo mode.
    #[arg(long)]
    demo: bool,
    /// Print the command instead of running it.
    #[arg(long)]
    dry_run: bool,
    /// Return immediately after starting the game instead of streaming its log.
    #[arg(long)]
    detach: bool,
}

#[derive(Subcommand)]
enum ProfileCmd {
    /// List profiles.
    List,
    /// Create a profile.
    Add {
        name: String,
        /// Version id, `latest-release` or `latest-snapshot`.
        #[arg(long, default_value = "latest-release")]
        version: String,
        /// Mod loader: fabric, quilt, forge, neoforge, legacyfabric (default: vanilla).
        #[arg(long)]
        loader: Option<String>,
        /// Shorthand for `--loader fabric`.
        #[arg(long)]
        fabric: bool,
        /// Pin a loader version (default: latest stable / recommended).
        #[arg(long)]
        loader_version: Option<String>,
        /// vanilla | smooth | max
        #[arg(long, default_value = "smooth")]
        preset: String,
        #[arg(long)]
        memory: Option<u64>,
        #[arg(long)]
        world: Option<String>,
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        game_dir: Option<PathBuf>,
        /// Do not install the performance mod pack (Fabric only).
        #[arg(long)]
        no_perf_mods: bool,
        /// Extra JVM args.
        #[arg(long)]
        java_args: Option<String>,
        /// Window size, e.g. 1920x1080.
        #[arg(long)]
        resolution: Option<String>,
    },
    /// Change a profile. Same flags as `add`; only given flags change.
    Set {
        name: String,
        #[arg(long)]
        rename: Option<String>,
        #[arg(long)]
        version: Option<String>,
        /// vanilla, fabric, quilt, forge, neoforge, legacyfabric
        #[arg(long)]
        loader: Option<String>,
        #[arg(long)]
        loader_version: Option<String>,
        #[arg(long)]
        preset: Option<String>,
        #[arg(long)]
        memory: Option<u64>,
        /// World name, or "none".
        #[arg(long)]
        world: Option<String>,
        /// Server address, or "none".
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        game_dir: Option<PathBuf>,
        #[arg(long)]
        perf_mods: Option<bool>,
        #[arg(long)]
        java_args: Option<String>,
        #[arg(long)]
        java: Option<PathBuf>,
        #[arg(long)]
        resolution: Option<String>,
    },
    /// Delete a profile.
    Remove { name: String },
    /// Select the default profile.
    Select { name: String },
}

#[derive(Subcommand)]
enum AccountCmd {
    List,
    Remove { name: String },
    Select { name: String },
}

fn main() {
    let cli = Cli::parse();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(if cli.verbose { "debug" } else { "warn" }),
    )
    .format_timestamp_millis()
    .init();
    let paths = cli
        .root
        .clone()
        .map(Paths::new)
        .unwrap_or_else(Paths::default);
    if let Err(e) = run(cli, paths) {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli, paths: Paths) -> Result<()> {
    let _ = std::fs::create_dir_all(paths.cache_dir());
    match cli.cmd.unwrap_or(Cmd::Gui) {
        Cmd::Gui => flint_launcher::gui::run(paths),
        Cmd::Versions {
            snapshots,
            all,
            installed,
            refresh,
        } => {
            let manifest = load_manifest(&paths, refresh)?;
            let local = installed_versions(&paths);
            if installed {
                for v in &local {
                    println!("{v}");
                }
                return Ok(());
            }
            println!(
                "latest release: {}   latest snapshot: {}",
                manifest.latest.release, manifest.latest.snapshot
            );
            for v in &manifest.versions {
                let show = match v.kind.as_str() {
                    "release" => true,
                    "snapshot" => snapshots || all,
                    _ => all,
                };
                if show {
                    let mark = if local.contains(&v.id) { "*" } else { " " };
                    println!(
                        "{mark} {:<24} {:<10} {}",
                        v.id,
                        v.kind,
                        &v.release_time[..v.release_time.len().min(10)]
                    );
                }
            }
            for v in local.iter().filter(|v| manifest.get(v).is_none()) {
                println!("* {v:<24} {:<10} (installed, modded/custom)", "custom");
            }
            Ok(())
        }
        Cmd::Install {
            version,
            loader,
            fabric,
            verify,
        } => {
            let settings = Settings::load(&paths);
            let manifest = load_manifest(&paths, false).ok();
            let mut p = Profile::new("install", &version);
            p.extra.loader = parse_loader(loader.as_deref(), fabric)?;
            let mut opts = flint_launcher::install::InstallOptions::from(&settings);
            opts.verify = verify;
            let (tx, rx) = mpsc::channel();
            let printer = spawn_printer(rx);
            let res = flint_launcher::install::prepare_profile(
                &paths,
                manifest.as_ref(),
                &p,
                &opts,
                &Some(tx),
            );
            let _ = printer.join();
            let inst = res?;
            println!(
                "installed {} (Java {} at {})",
                inst.version_id,
                inst.java_major,
                inst.java.display()
            );
            Ok(())
        }
        Cmd::Launch(a) => launch(&paths, a),
        Cmd::Profile { cmd } => profile_cmd(&paths, cmd),
        Cmd::Login { offline } => {
            let mut accounts = Accounts::load(&paths);
            if let Some(name) = offline {
                let acc = Account::offline(name.trim());
                println!("offline account '{}' ({})", acc.username, acc.uuid_dashed());
                accounts.upsert(acc);
                accounts.save(&paths)?;
                return Ok(());
            }
            let settings = Settings::load(&paths);
            let client_id = flint_launcher::auth::msa_client_id(Some(&settings.msa_client_id))?;
            let dc = flint_launcher::auth::begin_device_code(&client_id)?;
            println!(
                "Open {} and enter the code: {}",
                dc.verification_uri, dc.user_code
            );
            let _ = open::that(&dc.verification_uri);
            let tokens = flint_launcher::auth::wait_device_code(&client_id, &dc)?;
            let acc = flint_launcher::auth::complete_login(tokens)?;
            println!("signed in as {} ({})", acc.username, acc.uuid_dashed());
            accounts.upsert(acc);
            accounts.save(&paths)?;
            Ok(())
        }
        Cmd::Accounts { cmd } => {
            let mut accounts = Accounts::load(&paths);
            match cmd {
                AccountCmd::List => {
                    for a in &accounts.accounts {
                        let sel = if accounts.selected.as_deref() == Some(&a.id) {
                            "*"
                        } else {
                            " "
                        };
                        println!("{sel} {:<16} {:?} {}", a.username, a.kind, a.uuid_dashed());
                    }
                }
                AccountCmd::Remove { name } => {
                    let id = find_account(&accounts, &name)?.id.clone();
                    accounts.remove(&id);
                    accounts.save(&paths)?;
                }
                AccountCmd::Select { name } => {
                    let id = find_account(&accounts, &name)?.id.clone();
                    accounts.selected = Some(id);
                    accounts.save(&paths)?;
                }
            }
            Ok(())
        }
        Cmd::Java => {
            for j in flint_launcher::java::discover(&paths) {
                println!("Java {:<3} {:<20} {}", j.major, j.version, j.path.display());
            }
            Ok(())
        }
        Cmd::Worlds { profile } => {
            let profiles = Profiles::load(&paths)?;
            let game_dir = match profile {
                Some(n) => profiles
                    .find(&n)
                    .ok_or_else(|| anyhow!("no profile '{n}'"))?
                    .game_dir(&paths),
                None => profiles
                    .selected()
                    .map(|p| p.game_dir(&paths))
                    .unwrap_or_else(|| paths.root.clone()),
            };
            println!("worlds in {}:", game_dir.join("saves").display());
            for w in session::list_worlds(&game_dir) {
                println!("  {w}");
            }
            println!("servers:");
            for (name, ip) in session::list_servers(&game_dir) {
                println!("  {name:<24} {ip}");
            }
            Ok(())
        }
        Cmd::Mods { profile, slugs } => {
            let settings = Settings::load(&paths);
            let mut profiles = Profiles::load(&paths)?;
            let manifest = load_manifest(&paths, false).ok();
            let p = profiles
                .find(&profile)
                .ok_or_else(|| anyhow!("no profile '{profile}'"))?
                .clone();
            if !p.extra.loader.is_modded() {
                return Err(anyhow!(
                    "profile '{profile}' is vanilla (use `profile set {profile} --loader fabric` first)"
                ));
            }
            let game = manifest
                .as_ref()
                .map(|m| m.resolve_alias(&p.official.last_version_id).to_string())
                .unwrap_or(p.official.last_version_id.clone());
            let mut all: Vec<String> = Vec::new();
            if slugs.is_empty() || p.extra.perf_mods {
                all.extend(
                    flint_launcher::mods::PERF_MODS
                        .iter()
                        .map(|(s, _)| s.to_string()),
                );
            }
            let mut extra = p.extra.extra_mods.clone();
            for s in &slugs {
                let s = flint_launcher::mods::validate_slug(s)?;
                if !extra.contains(&s) {
                    extra.push(s.clone());
                }
                if !all.contains(&s) {
                    all.push(s);
                }
            }
            let (tx, rx) = mpsc::channel();
            let printer = spawn_printer(rx);
            let res = flint_launcher::mods::install(
                &paths,
                &p.game_dir(&paths),
                &all,
                &game,
                p.extra.loader,
                &settings.curseforge_api_key,
                false,
                settings.parallel_downloads,
                &Some(tx),
            );
            let _ = printer.join();
            let files = res?;
            if let Some(pm) = profiles.get_mut(&p.key) {
                pm.extra.extra_mods = extra;
            }
            profiles.save(&paths)?;
            println!("{} mod file(s) in place", files.len());
            Ok(())
        }
        Cmd::Search {
            query,
            version,
            loader,
            kind,
            curseforge,
            limit,
        } => {
            use flint_launcher::mods::{Kind, SearchQuery};
            let settings = Settings::load(&paths);
            let kind = match kind.to_ascii_lowercase().as_str() {
                "mod" | "mods" => Kind::Mod,
                "resourcepack" | "rp" | "resourcepacks" => Kind::ResourcePack,
                "shader" | "shaders" => Kind::Shader,
                other => return Err(anyhow!("unknown kind '{other}'")),
            };
            let q = SearchQuery {
                text: query.join(" "),
                kind,
                loader: parse_loader(loader.as_deref(), false)?,
                game_version: version.unwrap_or_default(),
                offset: 0,
                limit,
            };
            let page = if curseforge {
                flint_launcher::mods::search_curseforge(&q, &settings.curseforge_api_key)?
            } else {
                flint_launcher::mods::search_modrinth(&q)?
            };
            println!("{} result(s), showing {}:", page.total, page.hits.len());
            for h in &page.hits {
                println!("{:<40} {:>10} dl  {}", h.reference, h.downloads, h.title);
                println!("{:<40}               {}", "", truncate(&h.description, 90));
            }
            println!("\nAdd one to a profile:  flint mods <profile> <reference>");
            Ok(())
        }
        Cmd::Settings { set } => {
            let mut s = Settings::load(&paths);
            for kv in &set {
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow!("expected key=value, got '{kv}'"))?;
                match k {
                    "msa_client_id" => s.msa_client_id = v.to_string(),
                    "curseforge_api_key" => s.curseforge_api_key = v.to_string(),
                    "parallel_downloads" => {
                        s.parallel_downloads = v.parse().context("parallel_downloads")?
                    }
                    "verify_on_launch" => {
                        s.verify_on_launch = v.parse().context("verify_on_launch")?
                    }
                    "keep_open" => s.keep_open = v.parse().context("keep_open")?,
                    "auto_java" => s.auto_java = v.parse().context("auto_java")?,
                    _ => return Err(anyhow!("unknown setting '{k}'")),
                }
            }
            if !set.is_empty() {
                s.save(&paths)?;
            }
            println!("{}", serde_json::to_string_pretty(&s)?);
            println!("game directory: {}", paths.root.display());
            Ok(())
        }
    }
}

fn truncate(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

fn parse_loader(loader: Option<&str>, fabric: bool) -> Result<Loader> {
    match loader {
        Some(l) => Loader::parse(l).ok_or_else(|| {
            anyhow!("unknown loader '{l}' (vanilla, fabric, quilt, forge, neoforge, legacyfabric)")
        }),
        None if fabric => Ok(Loader::Fabric),
        None => Ok(Loader::Vanilla),
    }
}

fn load_manifest(paths: &Paths, refresh: bool) -> Result<VersionManifest> {
    let age = if refresh {
        Duration::ZERO
    } else {
        Duration::from_secs(3600)
    };
    VersionManifest::load(paths, age).context("loading the version manifest (are you online?)")
}

fn installed_versions(paths: &Paths) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(paths.versions())
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().into_string().ok())
                .filter(|n| paths.version_json(n).is_file())
                .collect()
        })
        .unwrap_or_default();
    out.sort();
    out
}

fn spawn_printer(rx: mpsc::Receiver<Progress>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut last_pct: i64 = -1;
        for p in rx {
            match p {
                Progress::Phase(s) => {
                    last_pct = -1;
                    println!("» {s}");
                }
                Progress::Step { done, total } if total > 0 => {
                    let pct = (done * 100 / total) as i64;
                    if pct != last_pct && (pct % 10 == 0 || done == total) {
                        last_pct = pct;
                        println!("  {done}/{total}");
                    }
                }
                Progress::Step { .. } => {}
                Progress::Log(s) => println!("{s}"),
                Progress::Done => {}
                Progress::Failed(e) => eprintln!("failed: {e}"),
            }
        }
    })
}

fn find_account<'a>(accounts: &'a Accounts, name: &str) -> Result<&'a Account> {
    accounts
        .accounts
        .iter()
        .find(|a| a.id == name || a.username.eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("no account '{name}'"))
}

fn parse_resolution(s: &str) -> Result<Resolution> {
    let (w, h) = s
        .to_ascii_lowercase()
        .split_once('x')
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .ok_or_else(|| anyhow!("resolution must look like 1920x1080"))?;
    Ok(Resolution {
        width: w.trim().parse()?,
        height: h.trim().parse()?,
    })
}

fn launch(paths: &Paths, a: LaunchArgs) -> Result<()> {
    let settings = Settings::load(paths);
    let mut profiles = Profiles::load(paths)?;
    if profiles.ensure_defaults() {
        profiles.save(paths)?;
    }
    // Manifest: use cache if fresh, else try network; tolerate offline if everything is installed.
    let manifest = match VersionManifest::load(paths, Duration::from_secs(3600)) {
        Ok(m) => Some(m),
        Err(e) => {
            log::warn!("version manifest unavailable: {e}");
            VersionManifest::load_cached(paths)
        }
    };

    let mut profile = match (&a.profile, &a.version) {
        (Some(name), _) => profiles
            .find(name)
            .ok_or_else(|| anyhow!("no profile '{name}'"))?
            .clone(),
        (None, Some(v)) => {
            let mut p = Profile::new(&format!("Ad-hoc {v}"), v);
            p.extra.loader = parse_loader(a.loader.as_deref(), a.fabric)?;
            p
        }
        (None, None) => profiles
            .selected()
            .ok_or_else(|| anyhow!("no profile selected; run `flint profile add ...`"))?
            .clone(),
    };
    if let Some(v) = &a.version {
        profile.official.last_version_id = v.clone();
        if a.loader.is_some() || a.fabric {
            profile.extra.loader = parse_loader(a.loader.as_deref(), a.fabric)?;
        }
    }

    // Account.
    let mut accounts = Accounts::load(paths);
    let (tx, rx) = mpsc::channel::<Progress>();
    let sink: ProgressSink = Some(tx);
    let printer = spawn_printer(rx);
    let account = if let Some(name) = &a.offline {
        Account::offline(name.trim())
    } else {
        let id = accounts
            .selected
            .clone()
            .ok_or_else(|| anyhow!("no account. Run `flint login` or use `--offline <name>`"))?;
        session::fresh_account(paths, &settings, &mut accounts, &id, &sink)?
    };

    let quick_play = if let Some(w) = &a.world {
        Some(QuickPlay::Singleplayer { world: w.clone() })
    } else if let Some(s) = &a.server {
        Some(QuickPlay::Multiplayer { address: s.clone() })
    } else {
        a.realm
            .as_ref()
            .map(|r| QuickPlay::Realms { id: r.clone() })
    };
    let preset = match &a.preset {
        Some(p) => Some(
            PerfPreset::parse(p)
                .ok_or_else(|| anyhow!("unknown preset '{p}' (vanilla|smooth|max)"))?,
        ),
        None => None,
    };
    let overrides = Overrides {
        quick_play,
        preset,
        memory_mb: a.memory,
        demo: a.demo,
    };

    log::debug!("launching profile {}", profile.display_name());
    let planned = session::plan(
        paths,
        &settings,
        manifest.as_ref(),
        &profile,
        &account,
        &overrides,
        &sink,
    );
    let (cmd, _installed) = match planned {
        Ok(x) => x,
        Err(e) => {
            drop(sink);
            let _ = printer.join();
            return Err(e);
        }
    };
    if a.dry_run {
        drop(sink);
        let _ = printer.join();
        println!("{}", cmd.render(Some(&account.access_token)));
        return Ok(());
    }
    if let Some(p) = profiles.get_mut(&profile.key) {
        p.touch();
        profiles.selected = Some(profile.key.clone());
        let _ = profiles.save(paths);
    }
    log::info!("{}", cmd.render(Some(&account.access_token)));
    let child = cmd
        .spawn()
        .with_context(|| format!("starting {}", cmd.program.display()))?;
    println!("» Minecraft started (pid {})", child.id());
    if a.detach {
        drop(sink);
        let _ = printer.join();
        return Ok(());
    }
    let code = session::pump_output(child, sink)?;
    let _ = printer.join();
    if code != 0 {
        return Err(anyhow!("Minecraft exited with code {code}"));
    }
    Ok(())
}

fn profile_cmd(paths: &Paths, cmd: ProfileCmd) -> Result<()> {
    let mut profiles = Profiles::load(paths)?;
    if profiles.ensure_defaults() {
        profiles.save(paths)?;
    }
    match cmd {
        ProfileCmd::List => {
            for p in &profiles.profiles {
                let sel = if profiles.selected.as_deref() == Some(&p.key) {
                    "*"
                } else {
                    " "
                };
                let qp = match &p.extra.quick_play {
                    QuickPlay::None => String::new(),
                    QuickPlay::Singleplayer { world } => format!("  -> world '{world}'"),
                    QuickPlay::Multiplayer { address } => format!("  -> server {address}"),
                    QuickPlay::Realms { id } => format!("  -> realm {id}"),
                };
                println!(
                    "{sel} {:<24} {:<22} {:<12} {:<8}{qp}",
                    p.display_name(),
                    p.official.last_version_id,
                    p.extra.loader.id(),
                    format!("{:?}", p.extra.perf).to_lowercase()
                );
            }
        }
        ProfileCmd::Add {
            name,
            version,
            loader,
            fabric,
            loader_version,
            preset,
            memory,
            world,
            server,
            game_dir,
            no_perf_mods,
            java_args,
            resolution,
        } => {
            if profiles.find(&name).is_some() {
                return Err(anyhow!("a profile named '{name}' already exists"));
            }
            let mut p = Profile::new(&name, &version);
            p.extra.loader = parse_loader(loader.as_deref(), fabric)?;
            p.extra.loader_version = loader_version.unwrap_or_default();
            p.extra.perf =
                PerfPreset::parse(&preset).ok_or_else(|| anyhow!("unknown preset '{preset}'"))?;
            p.extra.memory_mb = memory.unwrap_or(0);
            p.extra.perf_mods = !no_perf_mods;
            if let Some(w) = world {
                p.extra.quick_play = QuickPlay::Singleplayer { world: w };
            } else if let Some(s) = server {
                p.extra.quick_play = QuickPlay::Multiplayer { address: s };
            }
            p.official.game_dir = game_dir.map(|d| d.to_string_lossy().into_owned());
            p.official.java_args = java_args;
            p.official.resolution = resolution.as_deref().map(parse_resolution).transpose()?;
            profiles.selected = Some(p.key.clone());
            println!("created profile '{}' ({})", p.display_name(), p.key);
            profiles.profiles.push(p);
            profiles.save(paths)?;
        }
        ProfileCmd::Set {
            name,
            rename,
            version,
            loader,
            loader_version,
            preset,
            memory,
            world,
            server,
            game_dir,
            perf_mods,
            java_args,
            java,
            resolution,
        } => {
            let key = profiles
                .find(&name)
                .ok_or_else(|| anyhow!("no profile '{name}'"))?
                .key
                .clone();
            let p = profiles.get_mut(&key).unwrap();
            if let Some(n) = rename {
                p.official.name = n;
            }
            if let Some(v) = version {
                p.official.last_version_id = v;
            }
            if let Some(l) = loader {
                p.extra.loader = parse_loader(Some(&l), false)?;
            }
            if let Some(lv) = loader_version {
                p.extra.loader_version = lv;
            }
            if let Some(pr) = preset {
                p.extra.perf =
                    PerfPreset::parse(&pr).ok_or_else(|| anyhow!("unknown preset '{pr}'"))?;
            }
            if let Some(m) = memory {
                p.extra.memory_mb = m;
            }
            if let Some(w) = world {
                p.extra.quick_play = if w.eq_ignore_ascii_case("none") {
                    QuickPlay::None
                } else {
                    QuickPlay::Singleplayer { world: w }
                };
            }
            if let Some(s) = server {
                p.extra.quick_play = if s.eq_ignore_ascii_case("none") {
                    QuickPlay::None
                } else {
                    QuickPlay::Multiplayer { address: s }
                };
            }
            if let Some(d) = game_dir {
                p.official.game_dir = Some(d.to_string_lossy().into_owned());
            }
            if let Some(pm) = perf_mods {
                p.extra.perf_mods = pm;
            }
            if let Some(ja) = java_args {
                p.official.java_args = if ja.is_empty() { None } else { Some(ja) };
            }
            if let Some(j) = java {
                p.official.java_dir = Some(j.to_string_lossy().into_owned());
            }
            if let Some(r) = resolution {
                p.official.resolution = Some(parse_resolution(&r)?);
            }
            profiles.save(paths)?;
            println!("updated '{name}'");
        }
        ProfileCmd::Remove { name } => {
            let key = profiles
                .find(&name)
                .ok_or_else(|| anyhow!("no profile '{name}'"))?
                .key
                .clone();
            profiles.remove(&key);
            profiles.save(paths)?;
            println!("removed '{name}'");
        }
        ProfileCmd::Select { name } => {
            let key = profiles
                .find(&name)
                .ok_or_else(|| anyhow!("no profile '{name}'"))?
                .key
                .clone();
            profiles.selected = Some(key);
            profiles.save(paths)?;
        }
    }
    Ok(())
}
