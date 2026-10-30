use crate::auth::{Account, Accounts};
use crate::java::JavaInfo;
use crate::manifest::VersionManifest;
use crate::paths::Paths;
use crate::perf::PerfPreset;
use crate::profiles::{Loader, Profile, Profiles, QuickPlay, Resolution, Settings};
use crate::session::{self, Overrides};
use crate::Progress;
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const ACCENT: Color32 = Color32::from_rgb(0x3c, 0x8d, 0x2f);
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x4c, 0xa8, 0x3a);
const DANGER: Color32 = Color32::from_rgb(0xc0, 0x39, 0x2b);

pub fn run(paths: Paths) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1040.0, 660.0])
            .with_min_inner_size([780.0, 520.0])
            .with_title("Minecraft Launcher")
            .with_icon(std::sync::Arc::new(icon())),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        "Minecraft Launcher",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, paths)))),
    )
    .map_err(|e| anyhow::anyhow!("GUI error: {e}"))
}

/// Messages from background threads.
enum Msg {
    Progress(Progress),
    GameStarted(u32),
    GameExited(i32),
    Manifest(Result<VersionManifest>),
    LoginCode { uri: String, code: String },
    LoginDone(Result<Account>),
    JavaList(Vec<JavaInfo>),
}

#[derive(PartialEq, Clone, Copy)]
enum QpKind {
    None,
    World,
    Server,
    Realm,
}

struct App {
    paths: Paths,
    settings: Settings,
    profiles: Profiles,
    accounts: Accounts,
    manifest: Option<VersionManifest>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    // background work
    busy: bool,
    phase: String,
    step: (u64, u64),
    game_pid: Option<u32>,
    log: VecDeque<String>,
    show_log: bool,
    status: String,
    error: Option<String>,

    // version picker
    version_filter: String,
    show_release: bool,
    show_snapshot: bool,
    show_old: bool,

    // editors
    qp_kind: QpKind,
    qp_text: String,
    extra_mods_text: String,
    resolution_text: String,
    worlds: Vec<String>,
    servers: Vec<(String, String)>,
    worlds_dir: Option<PathBuf>,
    editing_key: Option<String>,

    // dialogs
    show_settings: bool,
    show_accounts: bool,
    show_new_profile: bool,
    new_profile_name: String,
    login_in_progress: bool,
    login_uri: String,
    login_code: String,
    offline_name: String,
    java_list: Vec<JavaInfo>,
    java_loading: bool,

    dirty_at: Option<Instant>,
    started: Instant,
    first_frame_logged: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paths: Paths) -> Self {
        let started = Instant::now();
        style(&cc.egui_ctx);
        let _ = std::fs::create_dir_all(paths.cache_dir());
        let settings = Settings::load(&paths);
        let mut profiles = Profiles::load(&paths).unwrap_or_default();
        if profiles.ensure_defaults() {
            let _ = profiles.save(&paths);
        }
        let accounts = Accounts::load(&paths);
        let manifest = VersionManifest::load_cached(&paths);
        let (tx, rx) = mpsc::channel();

        // Refresh the manifest in the background; the UI shows the cache meanwhile.
        {
            let tx = tx.clone();
            let paths = paths.clone();
            let ctx = cc.egui_ctx.clone();
            std::thread::spawn(move || {
                let r = VersionManifest::load(&paths, Duration::from_secs(1800));
                let _ = tx.send(Msg::Manifest(r));
                ctx.request_repaint();
            });
        }

        let mut app = Self {
            paths,
            settings,
            profiles,
            accounts,
            manifest,
            tx,
            rx,
            busy: false,
            phase: String::new(),
            step: (0, 0),
            game_pid: None,
            log: VecDeque::new(),
            show_log: false,
            status: String::new(),
            error: None,
            version_filter: String::new(),
            show_release: true,
            show_snapshot: false,
            show_old: false,
            qp_kind: QpKind::None,
            qp_text: String::new(),
            extra_mods_text: String::new(),
            resolution_text: String::new(),
            worlds: Vec::new(),
            servers: Vec::new(),
            worlds_dir: None,
            editing_key: None,
            show_settings: false,
            show_accounts: false,
            show_new_profile: false,
            new_profile_name: String::new(),
            login_in_progress: false,
            login_uri: String::new(),
            login_code: String::new(),
            offline_name: String::new(),
            java_list: Vec::new(),
            java_loading: false,
            dirty_at: None,
            started,
            first_frame_logged: false,
        };
        app.sync_editors();
        app
    }

    fn selected_profile(&self) -> Option<&Profile> {
        self.profiles.selected()
    }

    /// Copy profile fields into the editor scratch strings.
    fn sync_editors(&mut self) {
        let Some(p) = self.selected_profile().cloned() else {
            return;
        };
        if self.editing_key.as_deref() == Some(&p.key) {
            return;
        }
        self.editing_key = Some(p.key.clone());
        (self.qp_kind, self.qp_text) = match &p.extra.quick_play {
            QuickPlay::None => (QpKind::None, String::new()),
            QuickPlay::Singleplayer { world } => (QpKind::World, world.clone()),
            QuickPlay::Multiplayer { address } => (QpKind::Server, address.clone()),
            QuickPlay::Realms { id } => (QpKind::Realm, id.clone()),
        };
        self.extra_mods_text = p.extra.extra_mods.join(", ");
        self.resolution_text = p
            .official
            .resolution
            .as_ref()
            .map(|r| format!("{}x{}", r.width, r.height))
            .unwrap_or_default();
        let dir = p.game_dir(&self.paths);
        if self.worlds_dir.as_ref() != Some(&dir) {
            self.worlds = session::list_worlds(&dir);
            self.servers = session::list_servers(&dir);
            self.worlds_dir = Some(dir);
        }
    }

    fn mark_dirty(&mut self) {
        self.dirty_at = Some(Instant::now());
    }

    fn save_if_due(&mut self) {
        if let Some(t) = self.dirty_at {
            if t.elapsed() > Duration::from_millis(400) {
                self.dirty_at = None;
                if let Err(e) = self.profiles.save(&self.paths) {
                    self.error = Some(format!("Could not save profiles: {e:#}"));
                }
            }
        }
    }

    fn push_log(&mut self, line: String) {
        if self.log.len() > 4000 {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(Progress::Phase(s)) => {
                    self.phase = s.clone();
                    self.step = (0, 0);
                    self.push_log(format!("» {s}"));
                }
                Msg::Progress(Progress::Step { done, total }) => self.step = (done, total),
                Msg::Progress(Progress::Log(s)) => self.push_log(s),
                Msg::Progress(Progress::Done) => {
                    self.busy = false;
                    self.phase.clear();
                }
                Msg::Progress(Progress::Failed(e)) => {
                    self.busy = false;
                    self.phase.clear();
                    self.error = Some(e.clone());
                    self.push_log(format!("ERROR: {e}"));
                    self.show_log = true;
                }
                Msg::GameStarted(pid) => {
                    self.game_pid = Some(pid);
                    self.busy = false;
                    self.phase.clear();
                    self.status = format!("Minecraft is running (pid {pid})");
                    if !self.settings.keep_open {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Msg::GameExited(code) => {
                    self.game_pid = None;
                    self.status = if code == 0 {
                        "Minecraft closed".into()
                    } else {
                        format!("Minecraft exited with code {code}")
                    };
                    if code != 0 {
                        self.show_log = true;
                    }
                }
                Msg::Manifest(Ok(m)) => {
                    self.manifest = Some(m);
                    if self.status.is_empty() {
                        self.status = "Version list updated".into();
                    }
                }
                Msg::Manifest(Err(e)) => {
                    self.push_log(format!("version manifest: {e:#}"));
                    if self.manifest.is_none() {
                        self.error = Some("Could not fetch the version list (offline?). Already installed versions still launch; see the log for details.".into());
                    } else {
                        self.status = "Offline: using cached version list".into();
                    }
                }
                Msg::LoginCode { uri, code } => {
                    self.login_uri = uri;
                    self.login_code = code;
                }
                Msg::LoginDone(Ok(acc)) => {
                    self.login_in_progress = false;
                    self.login_uri.clear();
                    self.login_code.clear();
                    self.status = format!("Signed in as {}", acc.username);
                    self.accounts.upsert(acc);
                    let _ = self.accounts.save(&self.paths);
                }
                Msg::LoginDone(Err(e)) => {
                    self.login_in_progress = false;
                    self.login_uri.clear();
                    self.login_code.clear();
                    self.error = Some(format!("Sign-in failed: {e:#}"));
                }
                Msg::JavaList(list) => {
                    self.java_list = list;
                    self.java_loading = false;
                }
            }
        }
    }

    fn play(&mut self, ctx: &egui::Context) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.error = Some("Select or create a profile first.".into());
            return;
        };
        let Some(account_id) = self.accounts.selected.clone() else {
            self.error = Some("Add an account first (Sign in, or play offline).".into());
            self.show_accounts = true;
            return;
        };
        if let Some(p) = self.profiles.get_mut(&profile.key) {
            p.touch();
        }
        let _ = self.profiles.save(&self.paths);
        self.busy = true;
        self.error = None;
        self.log.clear();
        self.phase = "Preparing".into();
        self.step = (0, 0);
        self.status.clear();

        let paths = self.paths.clone();
        let settings = self.settings.clone();
        let manifest = self.manifest.clone();
        let mut accounts = self.accounts.clone();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (ptx, prx) = mpsc::channel::<Progress>();
            let sink = Some(ptx);
            // Forward progress to the UI.
            let fwd_tx = tx.clone();
            let fwd_ctx = ctx.clone();
            let forwarder = std::thread::spawn(move || {
                for p in prx {
                    let _ = fwd_tx.send(Msg::Progress(p));
                    fwd_ctx.request_repaint();
                }
            });
            let result = (|| -> Result<std::process::Child> {
                let account =
                    session::fresh_account(&paths, &settings, &mut accounts, &account_id, &sink)?;
                let (cmd, _) = session::plan(
                    &paths,
                    &settings,
                    manifest.as_ref(),
                    &profile,
                    &account,
                    &Overrides::default(),
                    &sink,
                )?;
                crate::report(
                    &sink,
                    Progress::Log(cmd.render(Some(&account.access_token))),
                );
                cmd.spawn()
            })();
            match result {
                Ok(child) => {
                    let _ = tx.send(Msg::GameStarted(child.id()));
                    ctx.request_repaint();
                    let code = session::pump_output(child, sink.clone()).unwrap_or(-1);
                    drop(sink);
                    let _ = forwarder.join();
                    let _ = tx.send(Msg::GameExited(code));
                }
                Err(e) => {
                    drop(sink);
                    let _ = forwarder.join();
                    let _ = tx.send(Msg::Progress(Progress::Failed(format!("{e:#}"))));
                }
            }
            ctx.request_repaint();
        });
    }

    fn start_login(&mut self, ctx: &egui::Context) {
        let client = match crate::auth::msa_client_id(Some(&self.settings.msa_client_id)) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.show_settings = true;
                return;
            }
        };
        self.login_in_progress = true;
        self.error = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let res = (|| -> Result<Account> {
                let dc = crate::auth::begin_device_code(&client)?;
                let _ = tx.send(Msg::LoginCode {
                    uri: dc.verification_uri.clone(),
                    code: dc.user_code.clone(),
                });
                ctx.request_repaint();
                let _ = open::that(&dc.verification_uri);
                let tokens = crate::auth::wait_device_code(&client, &dc)?;
                crate::auth::complete_login(tokens)
            })();
            let _ = tx.send(Msg::LoginDone(res));
            ctx.request_repaint();
        });
    }

    fn detect_java(&mut self, ctx: &egui::Context) {
        if self.java_loading {
            return;
        }
        self.java_loading = true;
        let tx = self.tx.clone();
        let paths = self.paths.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::JavaList(crate::java::discover(&paths)));
            ctx.request_repaint();
        });
    }

    // ---------------------------------------------------------------- UI

    fn top_bar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x14, 0x1c, 0x14))
            .inner_margin(egui::Margin::symmetric(14, 8));
        egui::TopBottomPanel::top("top")
            .frame(frame)
            .show(ctx, |ui| {
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::hover());
                    paint_block(ui.painter(), rect);
                    ui.label(
                        RichText::new("Minecraft Launcher")
                            .size(20.0)
                            .strong()
                            .color(Color32::from_rgb(0xe8, 0xf0, 0xe0)),
                    );
                    ui.label(RichText::new(format!("v{}", crate::LAUNCHER_VERSION)).weak());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("⚙ Settings").clicked() {
                            self.show_settings = true;
                        }
                        let acc_label = self
                            .accounts
                            .selected()
                            .map(|a| format!("👤 {}", a.username))
                            .unwrap_or_else(|| "👤 No account".to_string());
                        egui::ComboBox::from_id_salt("account")
                            .selected_text(acc_label)
                            .show_ui(ui, |ui| {
                                let mut pick: Option<String> = None;
                                for a in &self.accounts.accounts {
                                    let sel = self.accounts.selected.as_deref() == Some(&a.id);
                                    let kind = match a.kind {
                                        crate::auth::AccountKind::Microsoft => "Microsoft",
                                        crate::auth::AccountKind::Offline => "offline",
                                    };
                                    if ui
                                        .selectable_label(sel, format!("{} ({kind})", a.username))
                                        .clicked()
                                    {
                                        pick = Some(a.id.clone());
                                    }
                                }
                                ui.separator();
                                if ui.button("Manage accounts…").clicked() {
                                    self.show_accounts = true;
                                }
                                if let Some(id) = pick {
                                    self.accounts.selected = Some(id);
                                    let _ = self.accounts.save(&self.paths);
                                }
                            });
                    });
                });
                ui.add_space(4.0);
            });
    }

    fn side_panel(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x1a, 0x1e, 0x1a))
            .inner_margin(egui::Margin::symmetric(8, 6));
        egui::SidePanel::left("profiles")
            .resizable(false)
            .exact_width(250.0)
            .frame(frame)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.heading("Profiles");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("+ New").clicked() {
                            self.show_new_profile = true;
                            self.new_profile_name.clear();
                        }
                    });
                });
                ui.separator();
                let mut pick: Option<String> = None;
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for p in &self.profiles.profiles {
                        let selected = self.profiles.selected.as_deref() == Some(&p.key);
                        let version = self
                            .manifest
                            .as_ref()
                            .map(|m| m.resolve_alias(&p.official.last_version_id).to_string())
                            .unwrap_or_else(|| p.official.last_version_id.clone());
                        let color = match p.extra.loader {
                            Loader::Vanilla => ACCENT_HOVER,
                            Loader::Fabric | Loader::LegacyFabric => {
                                Color32::from_rgb(0xdb, 0xb7, 0x6c)
                            }
                            Loader::Quilt => Color32::from_rgb(0xc7, 0x7d, 0xff),
                            Loader::Forge => Color32::from_rgb(0x5c, 0x8a, 0xd6),
                            Loader::NeoForge => Color32::from_rgb(0xf1, 0x8a, 0x3c),
                        };
                        let qp = match &p.extra.quick_play {
                            QuickPlay::None => String::new(),
                            QuickPlay::Singleplayer { world } => format!("  ▶ {world}"),
                            QuickPlay::Multiplayer { address } => format!("  ▶ {address}"),
                            QuickPlay::Realms { .. } => "  ▶ Realm".into(),
                        };
                        let mut job = egui::text::LayoutJob::default();
                        let font = egui::FontId::proportional(14.0);
                        let small = egui::FontId::proportional(12.0);
                        let text_color = ui.visuals().text_color();
                        job.append(
                            &format!("     {}", p.display_name()),
                            0.0,
                            egui::TextFormat {
                                font_id: font,
                                color: text_color,
                                ..Default::default()
                            },
                        );
                        job.append(
                            &format!("\n     {version}{qp}"),
                            0.0,
                            egui::TextFormat {
                                font_id: small,
                                color: text_color.gamma_multiply(0.7),
                                ..Default::default()
                            },
                        );
                        let resp = ui
                            .with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                                ui.selectable_label(selected, job)
                            })
                            .inner;
                        let dot = resp.rect.left_center() + egui::vec2(12.0, 0.0);
                        ui.painter().circle_filled(dot, 5.0, color);
                        if resp.clicked() {
                            pick = Some(p.key.clone());
                        }
                    }
                });
                if let Some(k) = pick {
                    self.profiles.selected = Some(k);
                    self.editing_key = None;
                    self.sync_editors();
                    self.mark_dirty();
                }
            });
    }

    fn version_picker(&mut self, ui: &mut egui::Ui, current: &mut String) -> bool {
        let mut changed = false;
        let shown = match (&self.manifest, current.as_str()) {
            (Some(m), "latest-release") => format!("Latest release ({})", m.latest.release),
            (Some(m), "latest-snapshot") => format!("Latest snapshot ({})", m.latest.snapshot),
            (_, v) => v.to_string(),
        };
        egui::ComboBox::from_id_salt("version")
            .width(360.0)
            .selected_text(shown)
            .show_ui(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.version_filter)
                            .hint_text("filter…")
                            .desired_width(120.0),
                    );
                    ui.checkbox(&mut self.show_release, "Releases");
                    ui.checkbox(&mut self.show_snapshot, "Snapshots");
                    ui.checkbox(&mut self.show_old, "Old");
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        if let Some(m) = &self.manifest {
                            changed |= ui
                                .selectable_value(
                                    current,
                                    "latest-release".into(),
                                    format!("Latest release ({})", m.latest.release),
                                )
                                .clicked();
                            changed |= ui
                                .selectable_value(
                                    current,
                                    "latest-snapshot".into(),
                                    format!("Latest snapshot ({})", m.latest.snapshot),
                                )
                                .clicked();
                            let filter = self.version_filter.to_ascii_lowercase();
                            for v in &m.versions {
                                let show = match v.kind.as_str() {
                                    "release" => self.show_release,
                                    "snapshot" => self.show_snapshot,
                                    _ => self.show_old,
                                };
                                if !show
                                    || (!filter.is_empty()
                                        && !v.id.to_ascii_lowercase().contains(&filter))
                                {
                                    continue;
                                }
                                let date = v.release_time.get(..10).unwrap_or("");
                                changed |= ui
                                    .selectable_value(
                                        current,
                                        v.id.clone(),
                                        format!("{:<20} {date}", v.id),
                                    )
                                    .clicked();
                            }
                        } else {
                            ui.label("Version list not loaded yet…");
                        }
                        // Installed custom versions (Forge, OptiFine, manual installs...)
                        if let Ok(rd) = std::fs::read_dir(self.paths.versions()) {
                            let mut customs: Vec<String> = rd
                                .flatten()
                                .filter_map(|e| e.file_name().into_string().ok())
                                .filter(|n| self.paths.version_json(n).is_file())
                                .filter(|n| {
                                    self.manifest
                                        .as_ref()
                                        .map(|m| m.get(n).is_none())
                                        .unwrap_or(true)
                                })
                                .filter(|n| !n.starts_with("fabric-loader-"))
                                .collect();
                            customs.sort();
                            if !customs.is_empty() {
                                ui.separator();
                                ui.label(RichText::new("Installed custom versions").weak());
                                for c in customs {
                                    changed |= ui.selectable_value(current, c.clone(), c).clicked();
                                }
                            }
                        }
                    });
            });
        changed
    }

    fn central(&mut self, ctx: &egui::Context) {
        let bg = egui::Frame::new()
            .fill(Color32::from_rgb(0x22, 0x26, 0x22))
            .inner_margin(egui::Margin::same(12));
        egui::CentralPanel::default().frame(bg).show(ctx, |ui| {
            let Some(key) = self.profiles.selected.clone() else {
                ui.centered_and_justified(|ui| ui.label("Create a profile to get started."));
                return;
            };
            let Some(mut p) = self.profiles.get(&key).cloned() else { return };
            let mut changed = false;

            let card = egui::Frame::new()
                .fill(ui.visuals().panel_fill)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(16))
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_rgb(0x33, 0x3a, 0x33)));
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
              card.show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                // Name
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Name").strong().color(ACCENT_HOVER));
                    let mut name = p.display_name();
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(300.0));
                    if r.changed() {
                        p.official.name = name;
                        changed = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("🗑 Delete").fill(DANGER.gamma_multiply(0.6))).clicked() {
                            self.profiles.remove(&key);
                            self.editing_key = None;
                            self.mark_dirty();
                        }
                    });
                });
                if !self.profiles.profiles.iter().any(|x| x.key == key) {
                    return;
                }
                ui.add_space(8.0);

                // Version
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Version").strong().color(ACCENT_HOVER));
                    let mut v = p.official.last_version_id.clone();
                    if self.version_picker(ui, &mut v) {
                        p.official.last_version_id = v;
                        changed = true;
                    }
                });
                ui.add_space(8.0);

                // Loader
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Loader").strong().color(ACCENT_HOVER));
                    egui::ComboBox::from_id_salt("loader")
                        .selected_text(p.extra.loader.label())
                        .show_ui(ui, |ui| {
                            for l in Loader::ALL {
                                changed |= ui
                                    .selectable_value(&mut p.extra.loader, l, l.label())
                                    .on_hover_text(l.hint())
                                    .clicked();
                            }
                        });
                    ui.label(RichText::new(p.extra.loader.hint()).weak());
                });
                if p.extra.loader.is_modded() {
                    ui.indent("modded", |ui| {
                        changed |= ui
                            .checkbox(&mut p.extra.perf_mods, "Install the performance pack (Sodium, Lithium, C2ME, ModernFix, …)")
                            .on_hover_text(format!(
                                "Mods without a {} build are skipped automatically.\n\n{}",
                                p.extra.loader.label(),
                                crate::mods::PERF_MODS.iter().map(|(s, d)| format!("{s}: {d}")).collect::<Vec<_>>().join("\n")
                            ))
                            .changed();
                        ui.horizontal(|ui| {
                            ui.label("Extra Modrinth mods:");
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut self.extra_mods_text)
                                    .hint_text("iris, zoomify, …")
                                    .desired_width(300.0),
                            );
                            if r.changed() {
                                p.extra.extra_mods = self
                                    .extra_mods_text
                                    .split(',')
                                    .filter_map(|s| crate::mods::validate_slug(s).ok())
                                    .collect();
                                changed = true;
                            }
                        });
                    });
                }
                ui.add_space(8.0);

                // Performance
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Performance").strong().color(ACCENT_HOVER));
                    egui::ComboBox::from_id_salt("preset")
                        .selected_text(p.extra.perf.label())
                        .show_ui(ui, |ui| {
                            for pr in PerfPreset::ALL {
                                changed |= ui.selectable_value(&mut p.extra.perf, pr, pr.label()).clicked();
                            }
                        });
                });
                ui.horizontal(|ui| {
                    ui.label("Memory");
                    let auto = crate::perf::heap_mb(p.extra.perf, None, p.extra.loader.is_modded());
                    let total = crate::perf::total_memory_mb();
                    let r = ui.add(
                        egui::Slider::new(&mut p.extra.memory_mb, 0..=total.min(32768))
                            .step_by(256.0)
                            .custom_formatter(move |v, _| if v < 512.0 { format!("Auto ({auto} MiB)") } else { format!("{v:.0} MiB") })
                            .custom_parser(|s| s.trim_end_matches(" MiB").parse::<f64>().ok()),
                    );
                    if r.changed() {
                        if p.extra.memory_mb < 512 {
                            p.extra.memory_mb = 0;
                        }
                        changed = true;
                    }
                });
                ui.add_space(8.0);

                // Quick Play
                ui.label(RichText::new("Quick Play (skip the menus)").strong().color(ACCENT_HOVER));
                ui.horizontal(|ui| {
                    let mut k = self.qp_kind;
                    ui.radio_value(&mut k, QpKind::None, "Main menu");
                    ui.radio_value(&mut k, QpKind::World, "Singleplayer world");
                    ui.radio_value(&mut k, QpKind::Server, "Server");
                    ui.radio_value(&mut k, QpKind::Realm, "Realm");
                    if k != self.qp_kind {
                        self.qp_kind = k;
                        self.qp_text.clear();
                        changed = true;
                    }
                });
                match self.qp_kind {
                    QpKind::None => {}
                    QpKind::World => {
                        ui.horizontal(|ui| {
                            ui.label("World:");
                            egui::ComboBox::from_id_salt("world")
                                .width(260.0)
                                .selected_text(if self.qp_text.is_empty() { "choose a world…" } else { &self.qp_text })
                                .show_ui(ui, |ui| {
                                    if self.worlds.is_empty() {
                                        ui.label("No worlds found in this game directory yet.");
                                    }
                                    for w in self.worlds.clone() {
                                        if ui.selectable_label(self.qp_text == w, &w).clicked() {
                                            self.qp_text = w;
                                            changed = true;
                                        }
                                    }
                                });
                            if ui.add(egui::TextEdit::singleline(&mut self.qp_text).hint_text("or type the save folder name").desired_width(220.0)).changed() {
                                changed = true;
                            }
                        });
                    }
                    QpKind::Server => {
                        ui.horizontal(|ui| {
                            ui.label("Server:");
                            if ui.add(egui::TextEdit::singleline(&mut self.qp_text).hint_text("play.example.net:25565").desired_width(260.0)).changed() {
                                changed = true;
                            }
                            if !self.servers.is_empty() {
                                egui::ComboBox::from_id_salt("servers").selected_text("saved servers").show_ui(ui, |ui| {
                                    for (name, ip) in self.servers.clone() {
                                        if ui.selectable_label(false, format!("{name}  ({ip})")).clicked() {
                                            self.qp_text = ip;
                                            changed = true;
                                        }
                                    }
                                });
                            }
                        });
                    }
                    QpKind::Realm => {
                        ui.horizontal(|ui| {
                            ui.label("Realm id:");
                            if ui.add(egui::TextEdit::singleline(&mut self.qp_text).desired_width(200.0)).changed() {
                                changed = true;
                            }
                        });
                    }
                }
                if changed {
                    let t = self.qp_text.trim().to_string();
                    p.extra.quick_play = match (self.qp_kind, t.is_empty()) {
                        (QpKind::None, _) | (_, true) => QuickPlay::None,
                        (QpKind::World, false) => QuickPlay::Singleplayer { world: t },
                        (QpKind::Server, false) => QuickPlay::Multiplayer { address: t },
                        (QpKind::Realm, false) => QuickPlay::Realms { id: t },
                    };
                }
                ui.add_space(8.0);

                // Advanced
                egui::CollapsingHeader::new("Advanced").show(ui, |ui| {
                    egui::Grid::new("adv").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label("Game directory");
                        let mut gd = p.official.game_dir.clone().unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut gd).hint_text(self.paths.root.to_string_lossy()).desired_width(420.0)).changed() {
                            p.official.game_dir = if gd.trim().is_empty() { None } else { Some(gd) };
                            self.worlds_dir = None;
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Java executable");
                        let mut jd = p.official.java_dir.clone().unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut jd).hint_text("automatic (Mojang runtime)").desired_width(420.0)).changed() {
                            p.official.java_dir = if jd.trim().is_empty() { None } else { Some(jd) };
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Extra JVM arguments");
                        let mut ja = p.official.java_args.clone().unwrap_or_default();
                        if ui.add(egui::TextEdit::singleline(&mut ja).hint_text("-Dfoo=bar").desired_width(420.0)).changed() {
                            p.official.java_args = if ja.trim().is_empty() { None } else { Some(ja) };
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Window size");
                        if ui.add(egui::TextEdit::singleline(&mut self.resolution_text).hint_text("1920x1080").desired_width(140.0)).changed() {
                            p.official.resolution = parse_res(&self.resolution_text);
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Loader version");
                        if ui.add(egui::TextEdit::singleline(&mut p.extra.loader_version).hint_text("latest stable / recommended").desired_width(200.0)).changed() {
                            changed = true;
                        }
                        ui.end_row();
                    });
                });
              });
            });

            if changed {
                if let Some(slot) = self.profiles.get_mut(&key) {
                    *slot = p;
                }
                self.mark_dirty();
                if self.worlds_dir.is_none() {
                    self.editing_key = None;
                    self.sync_editors();
                }
            }
        });
    }

    fn bottom(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x1a, 0x1e, 0x1a))
            .inner_margin(egui::Margin::symmetric(14, 10));
        egui::TopBottomPanel::bottom("bottom")
            .frame(frame)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let can_play = !self.busy && self.profiles.selected.is_some();
                    let label = if self.busy {
                        "…"
                    } else if self.game_pid.is_some() {
                        "▶ PLAY AGAIN"
                    } else {
                        "▶ PLAY"
                    };
                    let btn = egui::Button::new(
                        RichText::new(label)
                            .size(22.0)
                            .strong()
                            .color(Color32::WHITE),
                    )
                    .fill(if can_play {
                        ACCENT
                    } else {
                        ACCENT.gamma_multiply(0.4)
                    })
                    .min_size(egui::vec2(220.0, 48.0));
                    if ui
                        .add_enabled(can_play, btn)
                        .on_hover_text("Install anything missing and start the game")
                        .clicked()
                    {
                        self.play(ctx);
                    }
                    ui.vertical(|ui| {
                        if self.busy {
                            let (done, total) = self.step;
                            let frac = if total > 0 {
                                done as f32 / total as f32
                            } else {
                                0.0
                            };
                            let text = if total > 0 {
                                format!("{}  {done}/{total}", self.phase)
                            } else {
                                self.phase.clone()
                            };
                            ui.add(egui::ProgressBar::new(frac).text(text).animate(total == 0));
                        } else if let Some(e) = &self.error {
                            ui.colored_label(DANGER, format!("⚠ {e}"));
                        } else if !self.status.is_empty() {
                            ui.label(&self.status);
                        } else {
                            let p = self.selected_profile();
                            let hint = match p.map(|p| &p.extra.quick_play) {
                                Some(QuickPlay::Singleplayer { world }) => {
                                    format!("Will open world “{world}” directly.")
                                }
                                Some(QuickPlay::Multiplayer { address }) => {
                                    format!("Will connect to {address} directly.")
                                }
                                Some(QuickPlay::Realms { .. }) => {
                                    "Will join the Realm directly.".into()
                                }
                                _ => format!("Ready. Game folder: {}", self.paths.root.display()),
                            };
                            ui.label(RichText::new(hint).weak());
                        }
                        ui.horizontal(|ui| {
                            ui.toggle_value(&mut self.show_log, "📜 Log");
                            if self.error.is_some() && ui.small_button("dismiss").clicked() {
                                self.error = None;
                            }
                            if ui.small_button("📂 Open game folder").clicked() {
                                let _ = open::that(&self.paths.root);
                            }
                        });
                    });
                });
                if self.show_log {
                    ui.separator();
                    egui::ScrollArea::vertical()
                        .max_height(180.0)
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            let text: String = self
                                .log
                                .iter()
                                .map(|l| l.as_str())
                                .collect::<Vec<_>>()
                                .join("\n");
                            ui.add(
                                egui::TextEdit::multiline(&mut text.as_str())
                                    .code_editor()
                                    .desired_width(f32::INFINITY)
                                    .desired_rows(8),
                            );
                        });
                }
                ui.add_space(6.0);
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // New profile
        if self.show_new_profile {
            let mut open = true;
            let mut create = false;
            egui::Window::new("New profile")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        let r = ui.text_edit_singleline(&mut self.new_profile_name);
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            create = true;
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Create").clicked() {
                            create = true;
                        }
                    });
                });
            if create {
                let name = if self.new_profile_name.trim().is_empty() {
                    "New profile".to_string()
                } else {
                    self.new_profile_name.trim().to_string()
                };
                let p = Profile::new(&name, "latest-release");
                self.profiles.selected = Some(p.key.clone());
                self.profiles.profiles.push(p);
                self.editing_key = None;
                self.sync_editors();
                self.mark_dirty();
                open = false;
            }
            self.show_new_profile = open;
        }

        // Accounts
        if self.show_accounts {
            let mut open = true;
            let mut start_login = false;
            let mut remove: Option<String> = None;
            egui::Window::new("Accounts").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
                ui.heading("Microsoft account");
                if self.login_in_progress {
                    if self.login_code.is_empty() {
                        ui.label("Contacting Microsoft…");
                    } else {
                        ui.label("1. Open this page (it should have opened in your browser):");
                        ui.hyperlink(&self.login_uri);
                        ui.label("2. Enter this code:");
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&self.login_code).size(26.0).strong().monospace());
                            if ui.button("Copy").clicked() {
                                ctx.copy_text(self.login_code.clone());
                            }
                        });
                        ui.label("3. Sign in with the Microsoft account that owns Minecraft. This window updates automatically.");
                    }
                    ui.spinner();
                } else if ui.button("Sign in with Microsoft").clicked() {
                    start_login = true;
                }
                ui.add_space(8.0);
                ui.heading("Offline account");
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.offline_name).hint_text("player name").desired_width(160.0));
                    let valid = (3..=16).contains(&self.offline_name.trim().len());
                    if ui.add_enabled(valid, egui::Button::new("Add")).clicked() {
                        let acc = Account::offline(self.offline_name.trim());
                        self.accounts.upsert(acc);
                        let _ = self.accounts.save(&self.paths);
                        self.offline_name.clear();
                    }
                });
                ui.label(RichText::new("Offline accounts work in singleplayer and on servers with online-mode=false.").weak());
                ui.add_space(8.0);
                ui.heading("Saved accounts");
                for a in &self.accounts.accounts {
                    ui.horizontal(|ui| {
                        let sel = self.accounts.selected.as_deref() == Some(&a.id);
                        let kind = match a.kind {
                            crate::auth::AccountKind::Microsoft => "Microsoft",
                            crate::auth::AccountKind::Offline => "offline",
                        };
                        ui.label(format!("{}{}  ({kind}, {})", if sel { "● " } else { "○ " }, a.username, a.uuid_dashed()));
                        if ui.small_button("remove").clicked() {
                            remove = Some(a.id.clone());
                        }
                    });
                }
            });
            if let Some(id) = remove {
                self.accounts.remove(&id);
                let _ = self.accounts.save(&self.paths);
            }
            if start_login {
                self.start_login(ctx);
            }
            self.show_accounts = open;
        }

        // Settings
        if self.show_settings {
            let mut open = true;
            let mut changed = false;
            let mut detect = false;
            egui::Window::new("Settings").open(&mut open).collapsible(false).default_width(520.0).show(ctx, |ui| {
                egui::Grid::new("settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Game directory");
                    ui.label(self.paths.root.to_string_lossy());
                    ui.end_row();
                    ui.label("Microsoft client id").on_hover_text("Azure app registration id approved for the Minecraft API. See README → Microsoft login.");
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.settings.msa_client_id).desired_width(320.0).hint_text("00000000-0000-0000-0000-000000000000")).changed();
                    ui.end_row();
                    ui.label("Parallel downloads");
                    changed |= ui.add(egui::Slider::new(&mut self.settings.parallel_downloads, 1..=64)).changed();
                    ui.end_row();
                    ui.label("Keep launcher open");
                    changed |= ui.checkbox(&mut self.settings.keep_open, "show the game log while Minecraft runs").changed();
                    ui.end_row();
                    ui.label("Verify every file");
                    changed |= ui.checkbox(&mut self.settings.verify_on_launch, "hash all files on each launch (slow; use if the game is corrupted)").changed();
                    ui.end_row();
                    ui.label("Download Java");
                    changed |= ui.checkbox(&mut self.settings.auto_java, "fetch Mojang's Java runtime when the right version is missing").changed();
                    ui.end_row();
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading("Java installations");
                    if ui.button(if self.java_loading { "Detecting…" } else { "Detect" }).clicked() {
                        detect = true;
                    }
                });
                for j in &self.java_list {
                    ui.label(format!("Java {:<3} {:<16} {}", j.major, j.version, j.path.display()));
                }
                ui.separator();
                ui.label(RichText::new(format!("Launcher started in {} ms", self.started.elapsed().as_millis().min(99_999))).weak());
            });
            if changed {
                let _ = self.settings.save(&self.paths);
            }
            if detect {
                self.detect_java(ctx);
            }
            self.show_settings = open;
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            log::info!(
                "first frame after {} ms",
                self.started.elapsed().as_millis()
            );
        }
        self.poll(ctx);
        self.sync_editors();
        self.top_bar(ctx);
        self.side_panel(ctx);
        self.bottom(ctx);
        self.central(ctx);
        self.dialogs(ctx);
        self.save_if_due();
        if self.busy || self.login_in_progress || self.dirty_at.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty_at.is_some() {
            let _ = self.profiles.save(&self.paths);
        }
    }
}

/// A little grass block, drawn procedurally so we need no image decoder.
fn icon() -> egui::IconData {
    const N: usize = 64;
    let mut rgba = vec![0u8; N * N * 4];
    let mut seed: u32 = 0x9e37_79b9;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 24) as i32 - 128
    };
    for y in 0..N {
        for x in 0..N {
            let i = (y * N + x) * 4;
            let n = noise() / 10;
            let grass = y < 22 || (y < 26 && (x / 4 + y) % 3 != 0);
            let (r, g, b) = if grass {
                (0x5d + n, 0x9a + n, 0x3c + n / 2)
            } else {
                (0x79 + n, 0x55 + n, 0x3a + n / 2)
            };
            let edge = x < 2 || y < 2 || x >= N - 2 || y >= N - 2;
            let (r, g, b) = if edge {
                (r / 2, g / 2, b / 2)
            } else {
                (r, g, b)
            };
            rgba[i] = r.clamp(0, 255) as u8;
            rgba[i + 1] = g.clamp(0, 255) as u8;
            rgba[i + 2] = b.clamp(0, 255) as u8;
            rgba[i + 3] = 255;
        }
    }
    egui::IconData {
        rgba,
        width: N as u32,
        height: N as u32,
    }
}

/// Tiny grass block glyph used next to the title.
fn paint_block(painter: &egui::Painter, rect: egui::Rect) {
    let r = rect.shrink(2.0);
    painter.rect_filled(r, 3.0, Color32::from_rgb(0x79, 0x55, 0x3a));
    let top = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), r.height() * 0.38));
    painter.rect_filled(
        top,
        egui::CornerRadius {
            nw: 3,
            ne: 3,
            sw: 0,
            se: 0,
        },
        ACCENT_HOVER,
    );
}

fn parse_res(s: &str) -> Option<Resolution> {
    let lower = s.to_ascii_lowercase();
    let (w, h) = lower.split_once('x')?;
    Some(Resolution {
        width: w.trim().parse().ok()?,
        height: h.trim().parse().ok()?,
    })
}

fn style(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.selection.bg_fill = ACCENT.gamma_multiply(0.7);
    visuals.widgets.hovered.bg_fill = ACCENT_HOVER.gamma_multiply(0.3);
    visuals.window_corner_radius = 8.0.into();
    visuals.widgets.noninteractive.corner_radius = 6.0.into();
    visuals.widgets.inactive.corner_radius = 6.0.into();
    visuals.widgets.hovered.corner_radius = 6.0.into();
    visuals.widgets.active.corner_radius = 6.0.into();
    ctx.set_visuals(visuals);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(10.0, 5.0);
    ctx.set_style(style);
}
