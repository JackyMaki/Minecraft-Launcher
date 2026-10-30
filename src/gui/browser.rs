//! The mod browser: search Modrinth / CurseForge, add to the profile.

use super::theme::Palette;
use crate::mods::{Kind, SearchPage, SearchQuery, Source};
use crate::profiles::Profile;
use eframe::egui::{self, RichText};
use std::sync::mpsc::{self, Receiver};

pub enum Action {
    None,
    /// The profile's mod list changed (save it).
    Changed,
    /// Download the profile's mods now.
    InstallNow,
}

pub struct Browser {
    pub open: bool,
    query: String,
    source: Source,
    kind: Kind,
    page: Option<SearchPage>,
    loading: bool,
    error: Option<String>,
    rx: Option<Receiver<anyhow::Result<SearchPage>>>,
    offset: u32,
    searched_for: String,
}

impl Default for Browser {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            source: Source::Modrinth,
            kind: Kind::Mod,
            page: None,
            loading: false,
            error: None,
            rx: None,
            offset: 0,
            searched_for: String::new(),
        }
    }
}

const PAGE: u32 = 20;

impl Browser {
    fn search(&mut self, ctx: &egui::Context, profile: &Profile, game_version: &str, cf_key: &str) {
        let q = SearchQuery {
            text: self.query.trim().to_string(),
            kind: self.kind,
            loader: profile.extra.loader,
            game_version: game_version.to_string(),
            offset: self.offset,
            limit: PAGE,
        };
        self.searched_for = format!(
            "{:?}/{:?}/{}/{}/{}",
            self.source,
            self.kind,
            game_version,
            profile.extra.loader.id(),
            q.text
        );
        self.loading = true;
        self.error = None;
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        let source = self.source;
        let key = cf_key.to_string();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let r = match source {
                Source::Modrinth => crate::mods::search_modrinth(&q),
                Source::CurseForge => crate::mods::search_curseforge(&q, &key),
            };
            let _ = tx.send(r);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        if let Some(rx) = &self.rx {
            if let Ok(r) = rx.try_recv() {
                self.loading = false;
                self.rx = None;
                match r {
                    Ok(p) => self.page = Some(p),
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        pal: &Palette,
        profile: &mut Profile,
        game_version: &str,
        cf_key: &str,
    ) -> Action {
        if !self.open {
            return Action::None;
        }
        self.poll();
        let mut action = Action::None;
        let mut open = self.open;
        let mut do_search = false;
        let mut install_now = false;
        egui::Window::new("Mods, resource packs and shaders")
            .open(&mut open)
            .default_size([760.0, 500.0])
            .min_size([560.0, 360.0])
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("for {}  ·  {}  ·  {}", profile.display_name(), game_version, profile.extra.loader.label())).weak());
                });
                ui.horizontal(|ui| {
                    for s in [Source::Modrinth, Source::CurseForge] {
                        if ui.selectable_label(self.source == s, s.label()).clicked() && self.source != s {
                            self.source = s;
                            self.offset = 0;
                            do_search = true;
                        }
                    }
                    ui.separator();
                    for k in Kind::ALL {
                        if ui.selectable_label(self.kind == k, k.label()).clicked() && self.kind != k {
                            self.kind = k;
                            self.offset = 0;
                            do_search = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.query)
                            .hint_text("search the whole catalog… (empty = most popular)")
                            .desired_width(ui.available_width() - 110.0),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.offset = 0;
                        do_search = true;
                    }
                    if ui.add_sized([96.0, 24.0], egui::Button::new("Search")).clicked() {
                        self.offset = 0;
                        do_search = true;
                    }
                });
                if self.source == Source::CurseForge && cf_key.trim().is_empty() {
                    ui.colored_label(pal.danger, "CurseForge needs a free API key: Settings → CurseForge API key (console.curseforge.com).");
                }
                ui.separator();

                // Installed list
                let perf_on = profile.extra.perf_mods && profile.extra.loader.is_modded();
                egui::CollapsingHeader::new(format!("In this profile ({}{})", profile.extra.extra_mods.len(), if perf_on { " + performance pack" } else { "" }))
                    .default_open(false)
                    .show(ui, |ui| {
                        if perf_on {
                            ui.label(RichText::new(format!("Performance pack: {}", crate::mods::PERF_MODS.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(", "))).weak());
                        }
                        let mut remove: Option<usize> = None;
                        for (i, m) in profile.extra.extra_mods.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.label(m);
                                if ui.small_button("remove").clicked() {
                                    remove = Some(i);
                                }
                            });
                        }
                        if let Some(i) = remove {
                            profile.extra.extra_mods.remove(i);
                            action = Action::Changed;
                        }
                        if ui.button("⬇ Download / update now").on_hover_text("Otherwise this happens when you press Play").clicked() {
                            install_now = true;
                        }
                    });
                ui.separator();

                // Results
                if self.loading {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Searching…");
                    });
                }
                if let Some(e) = &self.error {
                    ui.colored_label(pal.danger, e);
                }
                let page = self.page.clone();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    if let Some(page) = &page {
                        if page.hits.is_empty() && !self.loading {
                            ui.label("Nothing found for this version and loader.");
                        }
                        for h in &page.hits {
                            let installed = profile.extra.extra_mods.iter().any(|m| m == &h.reference)
                                || (h.source == Source::Modrinth && perf_on && crate::mods::PERF_MODS.iter().any(|(s, _)| *s == h.reference));
                            let frame = egui::Frame::new()
                                .fill(pal.card)
                                .corner_radius(8.0)
                                .inner_margin(egui::Margin::symmetric(10, 8))
                                .stroke(egui::Stroke::new(1.0_f32, pal.stroke));
                            frame.show(ui, |ui| {
                                ui.set_min_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    // Initial badge instead of a network icon: instant and offline-safe.
                                    let (rect, _) = ui.allocate_exact_size(egui::vec2(36.0, 36.0), egui::Sense::hover());
                                    let hue = h.title.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) % 360;
                                    let color = egui::ecolor::Hsva::new(hue as f32 / 360.0, 0.45, if pal.dark { 0.55 } else { 0.75 }, 1.0);
                                    ui.painter().rect_filled(rect, 8.0, color);
                                    let initial: String = h.title.chars().next().unwrap_or('?').to_uppercase().collect();
                                    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, initial, egui::FontId::proportional(18.0), egui::Color32::WHITE);
                                    ui.vertical(|ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(RichText::new(&h.title).strong().size(15.0));
                                            if !h.author.is_empty() {
                                                ui.label(RichText::new(format!("by {}", h.author)).weak());
                                            }
                                            ui.label(RichText::new(format!("{} downloads", human(h.downloads))).weak());
                                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                                let label = if installed { "✔ Added" } else { "+ Add" };
                                                let btn = egui::Button::new(RichText::new(label).color(if installed { pal.text } else { pal.on_accent }))
                                                    .fill(if installed { pal.panel } else { pal.accent });
                                                if ui.add(btn).clicked() {
                                                    if installed {
                                                        profile.extra.extra_mods.retain(|m| m != &h.reference);
                                                    } else {
                                                        profile.extra.extra_mods.push(h.reference.clone());
                                                    }
                                                    action = Action::Changed;
                                                }
                                                if !h.page_url.is_empty() && ui.small_button("page").clicked() {
                                                    let _ = open::that(&h.page_url);
                                                }
                                            });
                                        });
                                        ui.label(RichText::new(short(&h.description, 160)).color(pal.weak));
                                    });
                                });
                            });
                            ui.add_space(2.0);
                        }
                        if page.total > PAGE as u64 {
                            ui.horizontal(|ui| {
                                if self.offset > 0 && ui.button("◀ Previous").clicked() {
                                    self.offset = self.offset.saturating_sub(PAGE);
                                    do_search = true;
                                }
                                ui.label(RichText::new(format!("{}–{} of {}", self.offset + 1, (self.offset + PAGE).min(page.total as u32), page.total)).weak());
                                if (self.offset + PAGE) < page.total as u32 && ui.button("Next ▶").clicked() {
                                    self.offset += PAGE;
                                    do_search = true;
                                }
                            });
                        }
                    } else if !self.loading && self.error.is_none() {
                        ui.label(RichText::new("Press Search to list the most popular projects for this version, or type a name.").weak());
                    }
                });
            });
        self.open = open;
        if do_search && !self.loading {
            self.search(ctx, profile, game_version, cf_key);
        }
        if install_now {
            action = Action::InstallNow;
        }
        action
    }

    /// Run a first search automatically when the window opens.
    pub fn open_for(
        &mut self,
        ctx: &egui::Context,
        profile: &Profile,
        game_version: &str,
        cf_key: &str,
    ) {
        self.open = true;
        let key = format!(
            "{:?}/{:?}/{}/{}/{}",
            self.source,
            self.kind,
            game_version,
            profile.extra.loader.id(),
            self.query.trim()
        );
        if self.page.is_none() || self.searched_for != key {
            self.offset = 0;
            self.search(ctx, profile, game_version, cf_key);
        }
    }
}

fn human(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn short(s: &str, n: usize) -> String {
    let s = s.replace('\n', " ");
    if s.chars().count() <= n {
        s
    } else {
        format!("{}…", s.chars().take(n).collect::<String>().trim_end())
    }
}
