use super::browser::{pill, Action as BrowserAction, Browser};
use super::theme::{self, Palette, ACCENTS, THEMES};
use crate::auth::{Account, AccountKind, Accounts};
use crate::java::JavaInfo;
use crate::manifest::VersionManifest;
use crate::paths::Paths;
use crate::perf::PerfPreset;
use crate::profiles::{Appearance, Loader, Profile, Profiles, QuickPlay, Resolution, Settings};
use crate::session::{self, Overrides};
use crate::Progress;
use anyhow::Result;
use eframe::egui::{self, Color32, RichText};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

pub fn run(paths: Paths) -> Result<()> {
    let settings = Settings::load(&paths);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([860.0, 560.0])
            .with_title(crate::APP_TITLE)
            .with_decorations(!settings.appearance.custom_frame)
            .with_icon(std::sync::Arc::new(icon())),
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native(
        crate::APP_TITLE,
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

#[derive(PartialEq, Clone, Copy, Debug)]
enum Page {
    Play,
    Installations,
    Mods,
    Looks,
    Settings,
}

impl Page {
    const ALL: [Page; 5] = [
        Page::Play,
        Page::Installations,
        Page::Mods,
        Page::Looks,
        Page::Settings,
    ];
    fn label(self) -> &'static str {
        match self {
            Page::Play => "Play",
            Page::Installations => "Installations",
            Page::Mods => "Mods",
            Page::Looks => "Looks",
            Page::Settings => "Settings",
        }
    }
    fn glyph(self) -> &'static str {
        match self {
            Page::Play => "▶",
            Page::Installations => "☰",
            Page::Mods => "⬇",
            Page::Looks => "🎨",
            Page::Settings => "⚙",
        }
    }
}

struct App {
    paths: Paths,
    settings: Settings,
    profiles: Profiles,
    accounts: Accounts,
    manifest: Option<VersionManifest>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pal: Palette,
    applied: Appearance,
    browser: Browser,
    page: Page,

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
    show_new_profile: bool,
    new_profile_name: String,
    login_in_progress: bool,
    login_uri: String,
    login_code: String,
    offline_name: String,
    java_list: Vec<JavaInfo>,
    java_loading: bool,

    dirty_at: Option<Instant>,
    settings_dirty_at: Option<Instant>,
    started: Instant,
    first_frame_logged: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, paths: Paths) -> Self {
        let started = Instant::now();
        egui_extras::install_image_loaders(&cc.egui_ctx);
        theme::install_fonts(&cc.egui_ctx);
        let _ = std::fs::create_dir_all(paths.cache_dir());
        let settings = Settings::load(&paths);
        let pal = theme::apply(&cc.egui_ctx, &settings.appearance);
        let mut profiles = Profiles::load(&paths).unwrap_or_default();
        if profiles.ensure_defaults() {
            let _ = profiles.save(&paths);
        }
        let accounts = Accounts::load(&paths);
        let manifest = VersionManifest::load_cached(&paths);
        let (tx, rx) = mpsc::channel();
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
        let applied = settings.appearance.clone();
        let mut app = Self {
            paths,
            settings,
            profiles,
            accounts,
            manifest,
            tx,
            rx,
            pal,
            applied,
            browser: Browser::default(),
            page: Page::Play,
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
            show_new_profile: false,
            new_profile_name: String::new(),
            login_in_progress: false,
            login_uri: String::new(),
            login_code: String::new(),
            offline_name: String::new(),
            java_list: Vec::new(),
            java_loading: false,
            dirty_at: None,
            settings_dirty_at: None,
            started,
            first_frame_logged: false,
        };
        app.sync_editors();
        // Undocumented: open a page at start-up (used for screenshots and UI checks).
        match std::env::var("ELO_OPEN_PAGE").as_deref() {
            Ok("installations") => app.page = Page::Installations,
            Ok("mods") => app.page = Page::Mods,
            Ok("looks") => app.page = Page::Looks,
            Ok("settings") => app.page = Page::Settings,
            _ => {}
        }
        app
    }

    // ------------------------------------------------------------ helpers

    fn selected_profile(&self) -> Option<&Profile> {
        self.profiles.selected()
    }

    fn game_version_of(&self, p: &Profile) -> String {
        self.manifest
            .as_ref()
            .map(|m| m.resolve_alias(&p.official.last_version_id).to_string())
            .unwrap_or_else(|| p.official.last_version_id.clone())
    }

    fn has_bg_image(&self) -> bool {
        !self.settings.appearance.background_image.trim().is_empty()
    }

    fn surface(&self, base: Color32) -> Color32 {
        theme::surface(&self.settings.appearance, self.has_bg_image(), base)
    }

    fn card(&self) -> egui::Frame {
        theme::card(
            &self.pal,
            &self.settings.appearance,
            self.surface(self.pal.panel),
        )
    }

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

    fn save_if_due(&mut self, ctx: &egui::Context) {
        if let Some(t) = self.dirty_at {
            if t.elapsed() > Duration::from_millis(400) {
                self.dirty_at = None;
                if let Err(e) = self.profiles.save(&self.paths) {
                    self.error = Some(format!("Could not save profiles: {e:#}"));
                }
            }
        }
        if self.applied != self.settings.appearance {
            if self.applied.custom_frame != self.settings.appearance.custom_frame {
                ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(
                    !self.settings.appearance.custom_frame,
                ));
            }
            self.pal = theme::apply(ctx, &self.settings.appearance);
            self.applied = self.settings.appearance.clone();
            self.settings_dirty_at = Some(Instant::now());
        }
        if let Some(t) = self.settings_dirty_at {
            if t.elapsed() > Duration::from_millis(600) {
                self.settings_dirty_at = None;
                let _ = self.settings.save(&self.paths);
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
                    if self.status.is_empty() {
                        self.status = "Done".into();
                    }
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
                }
                Msg::Manifest(Err(e)) => {
                    self.push_log(format!("version manifest: {e:#}"));
                    if self.manifest.is_none() {
                        self.error = Some("Could not fetch the version list (offline?). Installed versions still launch; see the log.".into());
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

    fn forwarder(&self, ctx: &egui::Context) -> (crate::ProgressSink, std::thread::JoinHandle<()>) {
        let (ptx, prx) = mpsc::channel::<Progress>();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let h = std::thread::spawn(move || {
            for p in prx {
                let _ = tx.send(Msg::Progress(p));
                ctx.request_repaint();
            }
        });
        (Some(ptx), h)
    }

    fn play(&mut self, ctx: &egui::Context) {
        let Some(profile) = self.selected_profile().cloned() else {
            self.error = Some("Select or create a profile first.".into());
            return;
        };
        let Some(account_id) = self.accounts.selected.clone() else {
            self.error = Some(
                "Add an account first: Settings → Accounts (sign in, or play offline).".into(),
            );
            self.page = Page::Settings;
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
        let ctx2 = ctx.clone();
        let (sink, fwd) = self.forwarder(ctx);
        std::thread::spawn(move || {
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
                    ctx2.request_repaint();
                    let code = session::pump_output(child, sink.clone()).unwrap_or(-1);
                    drop(sink);
                    let _ = fwd.join();
                    let _ = tx.send(Msg::GameExited(code));
                }
                Err(e) => {
                    drop(sink);
                    let _ = fwd.join();
                    let _ = tx.send(Msg::Progress(Progress::Failed(format!("{e:#}"))));
                }
            }
            ctx2.request_repaint();
        });
    }

    fn install_mods_now(&mut self, ctx: &egui::Context) {
        let Some(profile) = self.selected_profile().cloned() else {
            return;
        };
        if !profile.extra.loader.is_modded() {
            self.error = Some("Pick a mod loader for this profile first.".into());
            return;
        }
        let _ = self.profiles.save(&self.paths);
        self.busy = true;
        self.error = None;
        self.status.clear();
        self.phase = "Resolving mods".into();
        let paths = self.paths.clone();
        let settings = self.settings.clone();
        let game = self.game_version_of(&profile);
        let tx = self.tx.clone();
        let ctx2 = ctx.clone();
        let (sink, fwd) = self.forwarder(ctx);
        std::thread::spawn(move || {
            let mut refs: Vec<String> = Vec::new();
            if profile.extra.perf_mods {
                refs.extend(crate::mods::PERF_MODS.iter().map(|(s, _)| s.to_string()));
            }
            refs.extend(profile.extra.extra_mods.iter().cloned());
            let r = crate::mods::install(
                &paths,
                &profile.game_dir(&paths),
                &refs,
                &game,
                profile.extra.loader,
                &settings.curseforge_api_key,
                false,
                settings.parallel_downloads,
                &sink,
            );
            let msg = match r {
                Ok(files) => Progress::Log(format!("{} file(s) in place", files.len())),
                Err(e) => Progress::Failed(format!("{e:#}")),
            };
            let ok = !matches!(msg, Progress::Failed(_));
            crate::report(&sink, msg);
            if ok {
                crate::report(&sink, Progress::Done);
            }
            drop(sink);
            let _ = fwd.join();
            let _ = tx.send(Msg::Progress(Progress::Log(String::new())));
            ctx2.request_repaint();
        });
    }

    fn start_login(&mut self, ctx: &egui::Context) {
        let client = match crate::auth::msa_client_id(Some(&self.settings.msa_client_id)) {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
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

    // ------------------------------------------------------------ chrome

    fn paint_background(&self, ctx: &egui::Context) {
        let rect = ctx.screen_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        theme::gradient_v(&painter, rect, self.pal.bg2, self.pal.bg);
        theme::glow(
            &painter,
            rect.left_top() + egui::vec2(rect.width() * 0.75, -40.0),
            rect.width() * 0.45,
            self.pal.accent,
            if self.pal.dark { 70 } else { 40 },
        );
        if !self.has_bg_image() {
            return;
        }
        let path = self.settings.appearance.background_image.trim();
        let uri = if path.starts_with("file://") {
            path.to_string()
        } else {
            format!("file://{path}")
        };
        if let Ok(egui::load::TexturePoll::Ready { texture }) = ctx.try_load_texture(
            &uri,
            egui::TextureOptions::LINEAR,
            egui::load::SizeHint::Size(2048, 2048),
        ) {
            let img = texture.size;
            let scale = (rect.width() / img.x).max(rect.height() / img.y);
            let shown = egui::vec2(
                rect.width() / (img.x * scale),
                rect.height() / (img.y * scale),
            );
            let uv_min = egui::pos2((1.0 - shown.x) / 2.0, (1.0 - shown.y) / 2.0);
            let uv = egui::Rect::from_min_size(uv_min, shown);
            painter.image(texture.id, rect, uv, Color32::WHITE);
            let dim = (self.settings.appearance.background_dim as f32 / 100.0 * 255.0) as u8;
            let veil = if self.pal.dark {
                Color32::from_black_alpha(dim)
            } else {
                Color32::from_white_alpha(dim)
            };
            painter.rect_filled(rect, 0.0, veil);
        }
    }

    /// Custom title bar (when OS decorations are off) with drag, maximize and close.
    fn title_bar(&mut self, ctx: &egui::Context) {
        let custom = self.settings.appearance.custom_frame;
        let frame = egui::Frame::new()
            .fill(Color32::TRANSPARENT)
            .inner_margin(egui::Margin {
                left: 14,
                right: 10,
                top: 8,
                bottom: 4,
            });
        egui::TopBottomPanel::top("title")
            .frame(frame)
            .show_separator_line(false)
            .show(ctx, |ui| {
                let height = if custom { 30.0 } else { 26.0 };
                let (rect, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), height),
                    if custom {
                        egui::Sense::click_and_drag()
                    } else {
                        egui::Sense::hover()
                    },
                );
                let painter = ui.painter();
                paint_block(
                    painter,
                    egui::Rect::from_min_size(
                        rect.left_top() + egui::vec2(0.0, 3.0),
                        egui::vec2(24.0, 24.0),
                    ),
                    self.pal.accent_hover,
                );
                painter.text(
                    rect.left_top() + egui::vec2(32.0, height / 2.0),
                    egui::Align2::LEFT_CENTER,
                    "Elo",
                    theme::bold(19.0),
                    self.pal.text,
                );
                painter.text(
                    rect.left_top() + egui::vec2(70.0, height / 2.0 + 1.0),
                    egui::Align2::LEFT_CENTER,
                    "Launcher",
                    theme::semibold(15.0),
                    self.pal.weak,
                );
                if custom {
                    if resp.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if resp.double_clicked() {
                        let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                    let mut x = rect.right();
                    for (glyph, action) in [("×", 0), ("☐", 1), ("–", 2)] {
                        let r = egui::Rect::from_min_size(
                            egui::pos2(x - 36.0, rect.top()),
                            egui::vec2(34.0, height),
                        );
                        x -= 38.0;
                        let id = ui.id().with(glyph);
                        let br = ui.interact(r, id, egui::Sense::click());
                        let t = ctx.animate_bool_with_time(id, br.hovered(), 0.12);
                        let fill = if action == 0 {
                            theme::mix(Color32::TRANSPARENT, self.pal.danger, t)
                        } else {
                            theme::mix(Color32::TRANSPARENT, self.pal.card, t)
                        };
                        ui.painter().rect_filled(r, 6.0, fill);
                        ui.painter().text(
                            r.center(),
                            egui::Align2::CENTER_CENTER,
                            glyph,
                            egui::FontId::proportional(13.0),
                            self.pal.text,
                        );
                        if br.clicked() {
                            match action {
                                0 => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                1 => {
                                    let max =
                                        ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                                }
                                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                            }
                        }
                    }
                }
            });
    }

    /// Edge resizing for the undecorated window.
    fn handle_resize(&self, ctx: &egui::Context) {
        if !self.settings.appearance.custom_frame {
            return;
        }
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized {
            return;
        }
        let rect = ctx.screen_rect();
        let Some(pos) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let m = 6.0;
        let l = pos.x < rect.left() + m;
        let r = pos.x > rect.right() - m;
        let t = pos.y < rect.top() + m;
        let b = pos.y > rect.bottom() - m;
        use egui::viewport::ResizeDirection as D;
        let dir = match (l, r, t, b) {
            (true, _, true, _) => Some(D::NorthWest),
            (_, true, true, _) => Some(D::NorthEast),
            (true, _, _, true) => Some(D::SouthWest),
            (_, true, _, true) => Some(D::SouthEast),
            (true, _, _, _) => Some(D::West),
            (_, true, _, _) => Some(D::East),
            (_, _, true, _) => Some(D::North),
            (_, _, _, true) => Some(D::South),
            _ => None,
        };
        if let Some(d) = dir {
            let icon = match d {
                D::North | D::South => egui::CursorIcon::ResizeVertical,
                D::East | D::West => egui::CursorIcon::ResizeHorizontal,
                D::NorthEast | D::SouthWest => egui::CursorIcon::ResizeNeSw,
                _ => egui::CursorIcon::ResizeNwSe,
            };
            ctx.set_cursor_icon(icon);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d));
            }
        }
    }

    fn nav_rail(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::new()
            .fill(Color32::TRANSPARENT)
            .inner_margin(egui::Margin {
                left: 12,
                right: 6,
                top: 6,
                bottom: 12,
            });
        egui::SidePanel::left("nav")
            .frame(frame)
            .resizable(false)
            .exact_width(176.0)
            .show_separator_line(false)
            .show(ctx, |ui| {
                let r = self.settings.appearance.rounding as f32 + 2.0;
                for page in Page::ALL {
                    let active = self.page == page;
                    let enabled = page != Page::Mods
                        || self
                            .selected_profile()
                            .map(|p| p.extra.loader.is_modded())
                            .unwrap_or(false);
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 40.0),
                        egui::Sense::click(),
                    );
                    let t = ctx.animate_bool_with_time(resp.id, resp.hovered() && enabled, 0.12);
                    let fill = if active {
                        self.pal
                            .accent
                            .gamma_multiply(if self.pal.dark { 0.9 } else { 1.0 })
                    } else {
                        theme::mix(Color32::TRANSPARENT, self.pal.card, t)
                    };
                    ui.painter().rect_filled(rect, r, fill);
                    let color = if !enabled {
                        self.pal.weak.gamma_multiply(0.5)
                    } else if active {
                        self.pal.on_accent
                    } else {
                        self.pal.text
                    };
                    ui.painter().text(
                        rect.left_center() + egui::vec2(14.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        page.glyph(),
                        egui::FontId::proportional(15.0),
                        color,
                    );
                    ui.painter().text(
                        rect.left_center() + egui::vec2(42.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        page.label(),
                        theme::semibold(14.5),
                        color,
                    );
                    if resp.clicked() && enabled {
                        self.page = page;
                    }
                    if resp.hovered() && enabled {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if !enabled {
                        resp.on_hover_text("Pick a mod loader in Installations first");
                    }
                    ui.add_space(2.0);
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    // Account chip
                    let name = self
                        .accounts
                        .selected()
                        .map(|a| a.username.clone())
                        .unwrap_or_else(|| "No account".into());
                    let kind = match self.accounts.selected().map(|a| a.kind.clone()) {
                        Some(AccountKind::Microsoft) => "Microsoft",
                        Some(AccountKind::Offline) => "offline",
                        None => "sign in",
                    };
                    let (rect, resp) = ui.allocate_exact_size(
                        egui::vec2(ui.available_width(), 48.0),
                        egui::Sense::click(),
                    );
                    let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    ui.painter().rect_filled(
                        rect,
                        r,
                        theme::mix(
                            self.surface(self.pal.card),
                            self.pal.accent.gamma_multiply(0.5),
                            t * 0.5,
                        ),
                    );
                    ui.painter().rect_stroke(
                        rect,
                        r,
                        egui::Stroke::new(1.0_f32, self.pal.stroke),
                        egui::StrokeKind::Inside,
                    );
                    let av = egui::Rect::from_center_size(
                        rect.left_center() + egui::vec2(24.0, 0.0),
                        egui::vec2(28.0, 28.0),
                    );
                    ui.painter().rect_filled(av, 7.0, self.pal.accent);
                    let initial: String =
                        name.chars().next().unwrap_or('?').to_uppercase().collect();
                    ui.painter().text(
                        av.center(),
                        egui::Align2::CENTER_CENTER,
                        initial,
                        theme::bold(14.0),
                        self.pal.on_accent,
                    );
                    ui.painter().text(
                        rect.left_top() + egui::vec2(46.0, 15.0),
                        egui::Align2::LEFT_CENTER,
                        &name,
                        theme::semibold(13.5),
                        self.pal.text,
                    );
                    ui.painter().text(
                        rect.left_top() + egui::vec2(46.0, 32.0),
                        egui::Align2::LEFT_CENTER,
                        kind,
                        egui::FontId::proportional(11.5),
                        self.pal.weak,
                    );
                    if resp.clicked() {
                        self.page = Page::Settings;
                    }
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(format!("v{}", crate::LAUNCHER_VERSION))
                            .small()
                            .color(self.pal.weak),
                    );
                });
            });
    }

    // ------------------------------------------------------------ pages

    fn play_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(p) = self.selected_profile().cloned() else {
            ui.label("Create a profile in Installations to get started.");
            return;
        };
        let a = self.settings.appearance.clone();
        let r = a.rounding as f32 + 6.0;

        // Hero
        let hero_h = if a.compact { 210.0 } else { 250.0 };
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), hero_h),
            egui::Sense::hover(),
        );
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, r, self.pal.accent_dark);
        let inner = rect.shrink(1.0);
        // gradient inside rounded rect: clip then paint
        let clip = painter.with_clip_rect(inner);
        theme::gradient_h(
            &clip,
            inner,
            theme::mix(self.pal.accent_dark, self.pal.accent, 0.35),
            theme::mix(self.pal.panel, self.pal.accent, 0.15),
        );
        theme::gradient_v(
            &clip,
            inner,
            Color32::TRANSPARENT,
            Color32::from_black_alpha(if self.pal.dark { 120 } else { 40 }),
        );
        theme::glow(
            &clip,
            inner.right_top() + egui::vec2(-80.0, 30.0),
            220.0,
            self.pal.accent_hover,
            110,
        );
        // decorative blocks
        let mut seed: u32 = 0x1234_5678;
        for i in 0..14 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let x = inner.right() - 40.0 - (i as f32 * 29.0) - (seed % 20) as f32;
            let y = inner.bottom() - 14.0 - ((seed >> 8) % 60) as f32;
            let s = 10.0 + ((seed >> 16) % 14) as f32;
            let c = if (seed >> 20) % 3 == 0 {
                self.pal.accent
            } else {
                theme::mix(self.pal.accent, self.pal.panel, 0.5)
            };
            clip.rect_filled(
                egui::Rect::from_min_size(egui::pos2(x, y - s), egui::vec2(s, s)),
                2.0,
                c.gamma_multiply(0.35),
            );
        }
        painter.rect_stroke(
            rect,
            r,
            egui::Stroke::new(1.0_f32, self.pal.accent.gamma_multiply(0.5)),
            egui::StrokeKind::Inside,
        );
        // text
        let gv = self.game_version_of(&p);
        let x0 = rect.left() + 28.0;
        painter.text(
            egui::pos2(x0, rect.top() + 34.0),
            egui::Align2::LEFT_CENTER,
            "READY TO PLAY",
            theme::semibold(12.0),
            Color32::from_white_alpha(170),
        );
        painter.text(
            egui::pos2(x0, rect.top() + 72.0),
            egui::Align2::LEFT_CENTER,
            p.display_name(),
            theme::bold(34.0),
            Color32::WHITE,
        );
        let mut chips: Vec<(String, Color32)> = vec![
            (gv.clone(), Color32::WHITE),
            (
                p.extra.loader.label().to_string(),
                loader_color(p.extra.loader, &self.pal),
            ),
            (
                p.extra
                    .perf
                    .label()
                    .split(' ')
                    .next()
                    .unwrap_or("")
                    .to_string(),
                Color32::WHITE,
            ),
        ];
        match &p.extra.quick_play {
            QuickPlay::None => {}
            QuickPlay::Singleplayer { world } => chips.push((format!("▶ {world}"), Color32::WHITE)),
            QuickPlay::Multiplayer { address } => {
                chips.push((format!("▶ {address}"), Color32::WHITE))
            }
            QuickPlay::Realms { .. } => chips.push(("▶ Realm".into(), Color32::WHITE)),
        }
        let mut cx = x0;
        for (text, color) in chips {
            let galley = painter.layout_no_wrap(text, theme::semibold(12.5), color);
            let w = galley.size().x + 20.0;
            let crect =
                egui::Rect::from_min_size(egui::pos2(cx, rect.top() + 100.0), egui::vec2(w, 24.0));
            painter.rect_filled(
                crect,
                999.0,
                Color32::from_black_alpha(if self.pal.dark { 90 } else { 60 }),
            );
            painter.rect_stroke(
                crect,
                999.0,
                egui::Stroke::new(1.0_f32, Color32::from_white_alpha(40)),
                egui::StrokeKind::Inside,
            );
            painter.galley(
                crect.left_center() + egui::vec2(10.0, -galley.size().y / 2.0),
                galley,
                color,
            );
            cx += w + 8.0;
        }
        // Play button inside the hero
        let can_play = !self.busy;
        let label = a.play_label.trim();
        let label = if label.is_empty() { "PLAY" } else { label };
        let text = if self.busy {
            "…".to_string()
        } else if self.game_pid.is_some() {
            format!("▶  {label} AGAIN")
        } else {
            format!("▶  {label}")
        };
        let brect = egui::Rect::from_min_size(
            egui::pos2(x0, rect.bottom() - 84.0),
            egui::vec2(260.0, 60.0),
        );
        let bid = ui.id().with("play");
        let bresp = ui.interact(brect, bid, egui::Sense::click());
        let t = ctx.animate_bool_with_time(bid, bresp.hovered() && can_play, 0.12);
        let grow = brect.expand(t * 2.0);
        let fill = if can_play {
            theme::mix(self.pal.accent, self.pal.accent_hover, t)
        } else {
            self.pal.accent.gamma_multiply(0.45)
        };
        painter.rect_filled(
            grow.translate(egui::vec2(0.0, 4.0)),
            r,
            Color32::from_black_alpha(90),
        );
        painter.rect_filled(grow, r, fill);
        painter.rect_stroke(
            grow,
            r,
            egui::Stroke::new(1.0_f32, Color32::from_white_alpha(60)),
            egui::StrokeKind::Inside,
        );
        painter.text(
            grow.center(),
            egui::Align2::CENTER_CENTER,
            text,
            theme::bold(21.0),
            self.pal.on_accent,
        );
        if bresp.hovered() && can_play {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if bresp.clicked() && can_play {
            self.play(ctx);
        }
        // Edit shortcut
        let erect = egui::Rect::from_min_size(
            egui::pos2(x0 + 272.0, rect.bottom() - 84.0 + 12.0),
            egui::vec2(120.0, 36.0),
        );
        let eid = ui.id().with("edit");
        let eresp = ui.interact(erect, eid, egui::Sense::click());
        let te = ctx.animate_bool_with_time(eid, eresp.hovered(), 0.12);
        painter.rect_filled(
            erect,
            r - 2.0,
            Color32::from_white_alpha((25.0 + 40.0 * te) as u8),
        );
        painter.text(
            erect.center(),
            egui::Align2::CENTER_CENTER,
            "⚙ Edit profile",
            theme::semibold(13.0),
            Color32::WHITE,
        );
        if eresp.clicked() {
            self.page = Page::Installations;
        }
        ui.add_space(12.0);

        // Status line
        ui.horizontal(|ui| {
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
                ui.add_sized(
                    [ui.available_width() - 160.0, 20.0],
                    egui::ProgressBar::new(frac)
                        .text(text)
                        .animate(total == 0)
                        .fill(self.pal.accent),
                );
            } else if let Some(e) = &self.error {
                ui.colored_label(self.pal.danger, format!("⚠ {e}"));
                if ui.small_button("dismiss").clicked() {
                    self.error = None;
                }
            } else if !self.status.is_empty() {
                ui.label(&self.status);
            } else {
                let hint = match &p.extra.quick_play {
                    QuickPlay::Singleplayer { world } => {
                        format!("Opens world “{world}” directly, no menus.")
                    }
                    QuickPlay::Multiplayer { address } => {
                        format!("Connects to {address} directly, no menus.")
                    }
                    QuickPlay::Realms { .. } => "Joins the Realm directly.".into(),
                    QuickPlay::None => {
                        "Tip: set a Quick Play target in Installations to skip every menu.".into()
                    }
                };
                ui.label(RichText::new(hint).color(self.pal.weak));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.toggle_value(&mut self.show_log, "Log");
                if ui.button("Open game folder").clicked() {
                    let _ = open::that(&self.paths.root);
                }
            });
        });
        if self.show_log {
            egui::ScrollArea::vertical()
                .max_height(140.0)
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
                            .desired_rows(6),
                    );
                });
        }
        ui.add_space(10.0);

        // Profile chips
        ui.label(
            RichText::new("Your installations")
                .font(theme::semibold(15.0))
                .color(self.pal.weak),
        );
        ui.add_space(4.0);
        let mut pick: Option<String> = None;
        let card_fill = self.surface(self.pal.card);
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for prof in &self.profiles.profiles {
                    let selected = self.profiles.selected.as_deref() == Some(&prof.key);
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(200.0, 70.0), egui::Sense::click());
                    let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                    let fill = if selected {
                        theme::mix(card_fill, self.pal.accent, 0.35)
                    } else {
                        theme::mix(card_fill, self.pal.accent, t * 0.15)
                    };
                    let pr = ui.painter();
                    pr.rect_filled(
                        rect.translate(egui::vec2(0.0, 3.0)),
                        r,
                        Color32::from_black_alpha(if self.pal.dark { 60 } else { 20 }),
                    );
                    pr.rect_filled(rect, r, fill);
                    pr.rect_stroke(
                        rect,
                        r,
                        egui::Stroke::new(
                            1.0_f32,
                            if selected {
                                self.pal.accent
                            } else {
                                self.pal.stroke
                            },
                        ),
                        egui::StrokeKind::Inside,
                    );
                    let strip =
                        egui::Rect::from_min_size(rect.left_top(), egui::vec2(5.0, rect.height()));
                    pr.with_clip_rect(rect).rect_filled(
                        strip,
                        0.0,
                        loader_color(prof.extra.loader, &self.pal),
                    );
                    pr.text(
                        rect.left_top() + egui::vec2(16.0, 24.0),
                        egui::Align2::LEFT_CENTER,
                        prof.display_name(),
                        theme::semibold(14.5),
                        self.pal.text,
                    );
                    let v = self
                        .manifest
                        .as_ref()
                        .map(|m| m.resolve_alias(&prof.official.last_version_id).to_string())
                        .unwrap_or_else(|| prof.official.last_version_id.clone());
                    pr.text(
                        rect.left_top() + egui::vec2(16.0, 46.0),
                        egui::Align2::LEFT_CENTER,
                        format!("{v} · {}", prof.extra.loader.label()),
                        egui::FontId::proportional(12.0),
                        self.pal.weak,
                    );
                    if resp.clicked() {
                        pick = Some(prof.key.clone());
                    }
                    if resp.hovered() {
                        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
                let (rect, resp) =
                    ui.allocate_exact_size(egui::vec2(70.0, 70.0), egui::Sense::click());
                let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                ui.painter().rect_stroke(
                    rect,
                    r,
                    egui::Stroke::new(1.0_f32, theme::mix(self.pal.stroke, self.pal.accent, t)),
                    egui::StrokeKind::Inside,
                );
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "+",
                    theme::bold(26.0),
                    theme::mix(self.pal.weak, self.pal.accent, t),
                );
                if resp.clicked() {
                    self.show_new_profile = true;
                    self.new_profile_name.clear();
                }
            });
        });
        if let Some(k) = pick {
            self.profiles.selected = Some(k);
            self.editing_key = None;
            self.sync_editors();
            self.mark_dirty();
        }
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
                                .filter(|n| {
                                    !n.starts_with("fabric-loader-")
                                        && !n.starts_with("quilt-loader-")
                                        && !n.contains("forge")
                                })
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

    fn installations_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let r = self.settings.appearance.rounding as f32 + 4.0;
        let card_fill = self.surface(self.pal.card);
        ui.horizontal_top(|ui| {
            // Profile list
            ui.vertical(|ui| {
                ui.set_width(250.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Installations").font(theme::bold(22.0)));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("+ New").color(self.pal.on_accent))
                                    .fill(self.pal.accent),
                            )
                            .clicked()
                        {
                            self.show_new_profile = true;
                            self.new_profile_name.clear();
                        }
                    });
                });
                ui.add_space(6.0);
                let mut pick: Option<String> = None;
                egui::ScrollArea::vertical()
                    .id_salt("profiles")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for p in &self.profiles.profiles {
                            let selected = self.profiles.selected.as_deref() == Some(&p.key);
                            let (rect, resp) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), 58.0),
                                egui::Sense::click(),
                            );
                            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), 0.12);
                            let fill = if selected {
                                theme::mix(card_fill, self.pal.accent, 0.4)
                            } else {
                                theme::mix(card_fill, self.pal.accent, t * 0.15)
                            };
                            let pr = ui.painter();
                            pr.rect_filled(rect, r, fill);
                            pr.rect_stroke(
                                rect,
                                r,
                                egui::Stroke::new(
                                    1.0_f32,
                                    if selected {
                                        self.pal.accent
                                    } else {
                                        self.pal.stroke
                                    },
                                ),
                                egui::StrokeKind::Inside,
                            );
                            pr.circle_filled(
                                rect.left_center() + egui::vec2(16.0, 0.0),
                                5.0,
                                loader_color(p.extra.loader, &self.pal),
                            );
                            pr.text(
                                rect.left_top() + egui::vec2(30.0, 20.0),
                                egui::Align2::LEFT_CENTER,
                                p.display_name(),
                                theme::semibold(14.5),
                                self.pal.text,
                            );
                            let v = self
                                .manifest
                                .as_ref()
                                .map(|m| m.resolve_alias(&p.official.last_version_id).to_string())
                                .unwrap_or_else(|| p.official.last_version_id.clone());
                            let qp = match &p.extra.quick_play {
                                QuickPlay::None => String::new(),
                                QuickPlay::Singleplayer { world } => format!(" · ▶ {world}"),
                                QuickPlay::Multiplayer { address } => format!(" · ▶ {address}"),
                                QuickPlay::Realms { .. } => " · ▶ Realm".into(),
                            };
                            pr.text(
                                rect.left_top() + egui::vec2(30.0, 40.0),
                                egui::Align2::LEFT_CENTER,
                                format!("{v}{qp}"),
                                egui::FontId::proportional(12.0),
                                self.pal.weak,
                            );
                            if resp.clicked() {
                                pick = Some(p.key.clone());
                            }
                            if resp.hovered() {
                                ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            ui.add_space(4.0);
                        }
                    });
                if let Some(k) = pick {
                    self.profiles.selected = Some(k);
                    self.editing_key = None;
                    self.sync_editors();
                    self.mark_dirty();
                }
            });
            ui.add_space(14.0);
            // Editor
            ui.vertical(|ui| {
                let card = self.card();
                egui::ScrollArea::vertical()
                    .id_salt("editor")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        card.show(ui, |ui| {
                            ui.set_min_width(ui.available_width());
                            self.profile_editor(ui, ctx);
                        });
                    });
            });
        });
    }

    fn profile_editor(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(key) = self.profiles.selected.clone() else {
            ui.label("Select or create an installation.");
            return;
        };
        let Some(mut p) = self.profiles.get(&key).cloned() else {
            return;
        };
        let mut changed = false;
        let hints = self.settings.appearance.show_hints;
        let accent = self.pal.accent_hover;
        let pal = self.pal;

        ui.horizontal(|ui| {
            let mut name = p.display_name();
            let r = ui.add(
                egui::TextEdit::singleline(&mut name)
                    .font(theme::bold(22.0))
                    .desired_width(340.0)
                    .margin(egui::Margin::symmetric(8, 6)),
            );
            if r.changed() {
                p.official.name = name;
                changed = true;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add(
                        egui::Button::new(RichText::new("Delete").color(Color32::WHITE))
                            .fill(pal.danger.gamma_multiply(0.75)),
                    )
                    .clicked()
                {
                    self.profiles.remove(&key);
                    self.editing_key = None;
                    self.mark_dirty();
                }
                if ui
                    .add(
                        egui::Button::new(RichText::new("▶ Play").color(pal.on_accent))
                            .fill(pal.accent),
                    )
                    .clicked()
                {
                    self.page = Page::Play;
                    self.play(ctx);
                }
            });
        });
        if !self.profiles.profiles.iter().any(|x| x.key == key) {
            return;
        }
        ui.add_space(10.0);

        section(ui, "Version", accent);
        ui.horizontal(|ui| {
            let mut v = p.official.last_version_id.clone();
            if self.version_picker(ui, &mut v) {
                p.official.last_version_id = v;
                changed = true;
            }
        });
        ui.add_space(8.0);

        section(ui, "Mod loader", accent);
        ui.horizontal_wrapped(|ui| {
            for l in Loader::ALL {
                let resp = pill(ui, &pal, p.extra.loader == l, l.label()).on_hover_text(l.hint());
                if resp.clicked() && p.extra.loader != l {
                    p.extra.loader = l;
                    changed = true;
                }
            }
        });
        if hints {
            ui.label(RichText::new(p.extra.loader.hint()).color(pal.weak));
        }
        if p.extra.loader.is_modded() {
            ui.indent("modded", |ui| {
                changed |= ui
                    .checkbox(
                        &mut p.extra.perf_mods,
                        "Performance pack (Sodium, Lithium, C2ME, ModernFix, …)",
                    )
                    .on_hover_text(format!(
                        "Mods without a {} build are skipped automatically.\n\n{}",
                        p.extra.loader.label(),
                        crate::mods::PERF_MODS
                            .iter()
                            .map(|(s, d)| format!("{s}: {d}"))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ))
                    .changed();
                ui.horizontal(|ui| {
                    let n = p.extra.extra_mods.len();
                    ui.label(format!("{n} from the catalog"));
                    if ui
                        .add(
                            egui::Button::new(RichText::new("⬇ Browse mods").color(pal.on_accent))
                                .fill(pal.accent),
                        )
                        .clicked()
                    {
                        self.page = Page::Mods;
                    }
                    ui.menu_button("by id…", |ui| {
                        ui.label("Modrinth slugs or curseforge:mod:ID, comma separated");
                        let r = ui.add(
                            egui::TextEdit::singleline(&mut self.extra_mods_text)
                                .desired_width(320.0),
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
            });
        }
        ui.add_space(8.0);

        section(ui, "Performance", accent);
        ui.horizontal_wrapped(|ui| {
            for pr in PerfPreset::ALL {
                if pill(ui, &pal, p.extra.perf == pr, pr.label()).clicked() && p.extra.perf != pr {
                    p.extra.perf = pr;
                    changed = true;
                }
            }
        });
        ui.horizontal(|ui| {
            ui.label("Memory");
            let auto = crate::perf::heap_mb(p.extra.perf, None, p.extra.loader.is_modded());
            let total = crate::perf::total_memory_mb();
            let r = ui.add(
                egui::Slider::new(&mut p.extra.memory_mb, 0..=total.min(32768))
                    .step_by(256.0)
                    .custom_formatter(move |v, _| {
                        if v < 512.0 {
                            format!("Auto ({auto} MiB)")
                        } else {
                            format!("{v:.0} MiB")
                        }
                    })
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

        section(ui, "Quick Play", accent);
        if hints {
            ui.label(
                RichText::new(
                    "Skip every menu: the game opens straight into a world, a server or a Realm.",
                )
                .color(pal.weak),
            );
        }
        ui.horizontal(|ui| {
            let mut k = self.qp_kind;
            for (kind, label) in [
                (QpKind::None, "Main menu"),
                (QpKind::World, "Singleplayer world"),
                (QpKind::Server, "Server"),
                (QpKind::Realm, "Realm"),
            ] {
                if pill(ui, &pal, k == kind, label).clicked() {
                    k = kind;
                }
            }
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
                        .selected_text(if self.qp_text.is_empty() {
                            "choose a world…"
                        } else {
                            &self.qp_text
                        })
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
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.qp_text)
                                .hint_text("or type the save folder name")
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
            }
            QpKind::Server => {
                ui.horizontal(|ui| {
                    ui.label("Server:");
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.qp_text)
                                .hint_text("play.example.net:25565")
                                .desired_width(260.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    if !self.servers.is_empty() {
                        egui::ComboBox::from_id_salt("servers")
                            .selected_text("saved servers")
                            .show_ui(ui, |ui| {
                                for (name, ip) in self.servers.clone() {
                                    if ui
                                        .selectable_label(false, format!("{name}  ({ip})"))
                                        .clicked()
                                    {
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
                    if ui
                        .add(egui::TextEdit::singleline(&mut self.qp_text).desired_width(200.0))
                        .changed()
                    {
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

        egui::CollapsingHeader::new(RichText::new("Advanced").font(theme::semibold(14.0))).show(
            ui,
            |ui| {
                egui::Grid::new("adv")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Game directory");
                        let mut gd = p.official.game_dir.clone().unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut gd)
                                    .hint_text(self.paths.root.to_string_lossy())
                                    .desired_width(420.0),
                            )
                            .changed()
                        {
                            p.official.game_dir =
                                if gd.trim().is_empty() { None } else { Some(gd) };
                            self.worlds_dir = None;
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Java executable");
                        let mut jd = p.official.java_dir.clone().unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut jd)
                                    .hint_text("automatic (Mojang runtime)")
                                    .desired_width(420.0),
                            )
                            .changed()
                        {
                            p.official.java_dir =
                                if jd.trim().is_empty() { None } else { Some(jd) };
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Extra JVM arguments");
                        let mut ja = p.official.java_args.clone().unwrap_or_default();
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut ja)
                                    .hint_text("-Dfoo=bar")
                                    .desired_width(420.0),
                            )
                            .changed()
                        {
                            p.official.java_args =
                                if ja.trim().is_empty() { None } else { Some(ja) };
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Window size");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.resolution_text)
                                    .hint_text("1920x1080")
                                    .desired_width(140.0),
                            )
                            .changed()
                        {
                            p.official.resolution = parse_res(&self.resolution_text);
                            changed = true;
                        }
                        ui.end_row();
                        ui.label("Loader version");
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut p.extra.loader_version)
                                    .hint_text("latest stable / recommended")
                                    .desired_width(200.0),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                        ui.end_row();
                    });
            },
        );

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
    }

    fn mods_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let Some(key) = self.profiles.selected.clone() else {
            return;
        };
        let gv = self
            .selected_profile()
            .map(|p| self.game_version_of(p))
            .unwrap_or_default();
        let cf = self.settings.curseforge_api_key.clone();
        let pal = self.pal;
        let mut install_now = false;
        let card = self.card();
        card.show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height(ui.available_height());
            if let Some(p) = self.profiles.get_mut(&key) {
                match self.browser.page(ui, &pal, p, &gv, &cf) {
                    BrowserAction::None => {}
                    BrowserAction::Changed => {
                        self.extra_mods_text = p.extra.extra_mods.join(", ");
                        self.dirty_at = Some(Instant::now());
                    }
                    BrowserAction::InstallNow => install_now = true,
                }
            }
        });
        if install_now {
            self.install_mods_now(ctx);
        }
    }

    fn looks_page(&mut self, ui: &mut egui::Ui) {
        let pal = self.pal;
        let card = self.card();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                card.show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    let a = &mut self.settings.appearance;
                    ui.label(RichText::new("Looks").font(theme::bold(24.0)));
                    ui.label(
                        RichText::new("Everything applies instantly and is saved.").color(pal.weak),
                    );
                    ui.add_space(10.0);
                    section(ui, "Theme", pal.accent_hover);
                    ui.horizontal_wrapped(|ui| {
                        for (id, label) in THEMES {
                            let preview = theme::palette(&Appearance {
                                theme: id.to_string(),
                                ..a.clone()
                            });
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(118.0, 64.0), egui::Sense::click());
                            let pr = ui.painter();
                            pr.rect_filled(rect, 10.0, preview.bg);
                            pr.rect_filled(
                                egui::Rect::from_min_size(
                                    rect.left_top() + egui::vec2(8.0, 8.0),
                                    egui::vec2(102.0, 30.0),
                                ),
                                6.0,
                                preview.panel,
                            );
                            pr.rect_filled(
                                egui::Rect::from_min_size(
                                    rect.left_top() + egui::vec2(14.0, 14.0),
                                    egui::vec2(40.0, 18.0),
                                ),
                                5.0,
                                preview.accent,
                            );
                            pr.text(
                                rect.left_bottom() + egui::vec2(10.0, -12.0),
                                egui::Align2::LEFT_CENTER,
                                *label,
                                theme::semibold(12.5),
                                preview.text,
                            );
                            let sel = a.theme == *id;
                            pr.rect_stroke(
                                rect,
                                10.0,
                                egui::Stroke::new(
                                    if sel { 2.0_f32 } else { 1.0_f32 },
                                    if sel { pal.accent } else { pal.stroke },
                                ),
                                egui::StrokeKind::Inside,
                            );
                            if resp.clicked() {
                                a.theme = id.to_string();
                            }
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                        }
                    });
                    ui.add_space(10.0);
                    section(ui, "Accent", pal.accent_hover);
                    ui.horizontal_wrapped(|ui| {
                        let mut c = Color32::from_rgb(a.accent[0], a.accent[1], a.accent[2]);
                        if egui::color_picker::color_edit_button_srgba(
                            ui,
                            &mut c,
                            egui::color_picker::Alpha::Opaque,
                        )
                        .changed()
                        {
                            a.accent = [c.r(), c.g(), c.b()];
                        }
                        for (name, col) in ACCENTS {
                            let (rect, resp) = ui
                                .allocate_exact_size(egui::vec2(26.0, 26.0), egui::Sense::click());
                            let cc = Color32::from_rgb(col[0], col[1], col[2]);
                            ui.painter().circle_filled(rect.center(), 11.0, cc);
                            if a.accent == *col {
                                ui.painter().circle_stroke(
                                    rect.center(),
                                    13.0,
                                    egui::Stroke::new(2.0_f32, pal.text),
                                );
                            }
                            if resp.on_hover_text(*name).clicked() {
                                a.accent = *col;
                            }
                        }
                    });
                    if a.theme == "custom" {
                        egui::Grid::new("custom-colors")
                            .num_columns(2)
                            .spacing([12.0, 8.0])
                            .show(ui, |ui| {
                                for (label, slot) in [
                                    ("Background", &mut a.background),
                                    ("Panels", &mut a.panel),
                                    ("Text", &mut a.text),
                                ] {
                                    ui.label(label);
                                    let mut c = Color32::from_rgb(slot[0], slot[1], slot[2]);
                                    if egui::color_picker::color_edit_button_srgba(
                                        ui,
                                        &mut c,
                                        egui::color_picker::Alpha::Opaque,
                                    )
                                    .changed()
                                    {
                                        *slot = [c.r(), c.g(), c.b()];
                                    }
                                    ui.end_row();
                                }
                            });
                    }
                    ui.add_space(10.0);
                    section(ui, "Layout", pal.accent_hover);
                    egui::Grid::new("appearance")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("UI size");
                            ui.add(
                                egui::Slider::new(&mut a.zoom, 0.7..=1.6)
                                    .step_by(0.05)
                                    .custom_formatter(|v, _| format!("{:.0}%", v * 100.0)),
                            );
                            ui.end_row();
                            ui.label("Corner rounding");
                            ui.add(egui::Slider::new(&mut a.rounding, 0..=16));
                            ui.end_row();
                            ui.label("Options");
                            ui.horizontal(|ui| {
                                ui.checkbox(&mut a.compact, "Compact");
                                ui.checkbox(&mut a.show_hints, "Hints");
                                ui.checkbox(&mut a.custom_frame, "Custom window frame");
                                ui.checkbox(&mut a.glass, "Glass panels");
                            });
                            ui.end_row();
                            ui.label("Play button text");
                            ui.add(
                                egui::TextEdit::singleline(&mut a.play_label)
                                    .desired_width(160.0)
                                    .hint_text("PLAY"),
                            );
                            ui.end_row();
                            ui.label("Background picture");
                            ui.vertical(|ui| {
                                ui.add(
                                    egui::TextEdit::singleline(&mut a.background_image)
                                        .desired_width(320.0)
                                        .hint_text("path to a .png or .jpg"),
                                );
                                ui.horizontal(|ui| {
                                    ui.label("Darken");
                                    ui.add(
                                        egui::Slider::new(&mut a.background_dim, 0..=100)
                                            .suffix("%"),
                                    );
                                });
                            });
                            ui.end_row();
                        });
                    ui.add_space(8.0);
                    if ui.button("Reset to defaults").clicked() {
                        *a = Appearance::default();
                    }
                });
            });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let pal = self.pal;
        let card = self.card();
        let mut changed = false;
        let mut detect = false;
        let mut start_login = false;
        let mut remove: Option<String> = None;
        let mut select: Option<String> = None;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            card.show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new("Accounts").font(theme::bold(24.0)));
                ui.add_space(6.0);
                for a in &self.accounts.accounts {
                    ui.horizontal(|ui| {
                        let sel = self.accounts.selected.as_deref() == Some(&a.id);
                        let kind = match a.kind {
                            AccountKind::Microsoft => "Microsoft",
                            AccountKind::Offline => "offline",
                        };
                        if pill(ui, &pal, sel, &a.username).clicked() {
                            select = Some(a.id.clone());
                        }
                        ui.label(RichText::new(format!("{kind} · {}", a.uuid_dashed())).color(pal.weak));
                        if ui.small_button("remove").clicked() {
                            remove = Some(a.id.clone());
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.login_in_progress {
                        ui.spinner();
                        if self.login_code.is_empty() {
                            ui.label("Contacting Microsoft…");
                        } else {
                            ui.label("Enter this code at");
                            ui.hyperlink(&self.login_uri);
                            ui.label(RichText::new(&self.login_code).font(theme::bold(22.0)));
                            if ui.button("Copy").clicked() {
                                ctx.copy_text(self.login_code.clone());
                            }
                        }
                    } else if ui.add(egui::Button::new(RichText::new("Sign in with Microsoft").color(pal.on_accent)).fill(pal.accent)).clicked() {
                        start_login = true;
                    }
                    ui.add_space(12.0);
                    ui.add(egui::TextEdit::singleline(&mut self.offline_name).hint_text("offline name").desired_width(150.0));
                    let valid = (3..=16).contains(&self.offline_name.trim().len());
                    if ui.add_enabled(valid, egui::Button::new("Add offline account")).clicked() {
                        let acc = Account::offline(self.offline_name.trim());
                        self.accounts.upsert(acc);
                        let _ = self.accounts.save(&self.paths);
                        self.offline_name.clear();
                    }
                });
                if self.settings.appearance.show_hints {
                    ui.label(RichText::new("Offline accounts work in singleplayer and on servers with online-mode=false.").color(pal.weak));
                }
            });
            ui.add_space(12.0);
            card.show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.label(RichText::new("Settings").font(theme::bold(24.0)));
                ui.add_space(6.0);
                egui::Grid::new("settings").num_columns(2).spacing([14.0, 10.0]).show(ui, |ui| {
                    ui.label("Game directory");
                    ui.horizontal(|ui| {
                        ui.add_sized([ui.available_width().min(520.0), 18.0], egui::Label::new(RichText::new(self.paths.root.to_string_lossy()).color(pal.weak)).truncate());
                        if ui.small_button("open").clicked() {
                            let _ = open::that(&self.paths.root);
                        }
                    });
                    ui.end_row();
                    ui.label("Microsoft client id").on_hover_text("Only needed when this build has no id baked in. See README → Microsoft login.");
                    let hint = if crate::auth::BUILT_IN_CLIENT_ID.is_some() { "built in (leave empty)" } else { "00000000-0000-0000-0000-000000000000" };
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.settings.msa_client_id).desired_width(340.0).hint_text(hint)).changed();
                    ui.end_row();
                    ui.label("CurseForge API key").on_hover_text("Free: console.curseforge.com → API keys. Enables the CurseForge catalog in Mods.");
                    changed |= ui.add(egui::TextEdit::singleline(&mut self.settings.curseforge_api_key).desired_width(340.0).password(true).hint_text("optional")).changed();
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
                    ui.label("Java installations");
                    ui.vertical(|ui| {
                        if ui.button(if self.java_loading { "Detecting…" } else { "Detect" }).clicked() {
                            detect = true;
                        }
                        for j in &self.java_list {
                            ui.label(RichText::new(format!("Java {:<3} {:<16} {}", j.major, j.version, j.path.display())).monospace());
                        }
                    });
                    ui.end_row();
                });
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Elo Launcher v{} · started in {} ms", crate::LAUNCHER_VERSION, self.started.elapsed().as_millis().min(99_999))).color(pal.weak).small());
            });
        });
        if let Some(id) = select {
            self.accounts.selected = Some(id);
            let _ = self.accounts.save(&self.paths);
        }
        if let Some(id) = remove {
            self.accounts.remove(&id);
            let _ = self.accounts.save(&self.paths);
        }
        if changed {
            let _ = self.settings.save(&self.paths);
        }
        if detect {
            self.detect_java(ctx);
        }
        if start_login {
            self.start_login(ctx);
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        if self.show_new_profile {
            let mut open = true;
            let mut create = false;
            egui::Window::new("New installation")
                .open(&mut open)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("Name");
                        let r = ui.text_edit_singleline(&mut self.new_profile_name);
                        r.request_focus();
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            create = true;
                        }
                    });
                    if ui.button("Create").clicked() {
                        create = true;
                    }
                });
            if create {
                let name = if self.new_profile_name.trim().is_empty() {
                    "New installation".to_string()
                } else {
                    self.new_profile_name.trim().to_string()
                };
                let p = Profile::new(&name, "latest-release");
                self.profiles.selected = Some(p.key.clone());
                self.profiles.profiles.push(p);
                self.editing_key = None;
                self.sync_editors();
                self.mark_dirty();
                self.page = Page::Installations;
                open = false;
            }
            self.show_new_profile = open;
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
        self.paint_background(ctx);
        self.handle_resize(ctx);
        self.title_bar(ctx);
        self.nav_rail(ctx);
        let frame = egui::Frame::new()
            .fill(Color32::TRANSPARENT)
            .inner_margin(egui::Margin {
                left: 6,
                right: 16,
                top: 4,
                bottom: 14,
            });
        egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| match self.page {
                Page::Play => self.play_page(ui, ctx),
                Page::Installations => self.installations_page(ui, ctx),
                Page::Mods => self.mods_page(ui, ctx),
                Page::Looks => self.looks_page(ui),
                Page::Settings => self.settings_page(ui, ctx),
            });
        self.dialogs(ctx);
        self.save_if_due(ctx);
        if self.busy
            || self.login_in_progress
            || self.dirty_at.is_some()
            || self.settings_dirty_at.is_some()
        {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = self.pal.bg;
        [
            c.r() as f32 / 255.0,
            c.g() as f32 / 255.0,
            c.b() as f32 / 255.0,
            1.0,
        ]
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty_at.is_some() {
            let _ = self.profiles.save(&self.paths);
        }
        if self.settings_dirty_at.is_some() {
            let _ = self.settings.save(&self.paths);
        }
    }
}

fn section(ui: &mut egui::Ui, title: &str, accent: Color32) {
    ui.label(
        RichText::new(title.to_uppercase())
            .font(theme::semibold(12.0))
            .color(accent),
    );
}

fn loader_color(l: Loader, pal: &Palette) -> Color32 {
    match l {
        Loader::Vanilla => pal.accent_hover,
        Loader::Fabric | Loader::LegacyFabric => Color32::from_rgb(0xdb, 0xb7, 0x6c),
        Loader::Quilt => Color32::from_rgb(0xc7, 0x7d, 0xff),
        Loader::Forge => Color32::from_rgb(0x5c, 0x8a, 0xd6),
        Loader::NeoForge => Color32::from_rgb(0xf1, 0x8a, 0x3c),
    }
}

/// A little grass block, drawn procedurally so we need no image decoder at start-up.
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
                (0x4a + n, 0xb0 + n, 0x5a + n / 2)
            } else {
                (0x6b + n, 0x4b + n, 0x34 + n / 2)
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
fn paint_block(painter: &egui::Painter, rect: egui::Rect, grass: Color32) {
    let r = rect.shrink(2.0);
    painter.rect_filled(r, 5.0, Color32::from_rgb(0x6b, 0x4b, 0x34));
    let top = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), r.height() * 0.4));
    painter.rect_filled(
        top,
        egui::CornerRadius {
            nw: 5,
            ne: 5,
            sw: 0,
            se: 0,
        },
        grass,
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
