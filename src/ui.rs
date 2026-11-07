//! Minimal matte-black interface.

use crate::automation::{Cmd, GameState, Shared};
use crate::logger::{Level, Logger};
use crate::vision::Sig;
use eframe::egui::{self, Color32, RichText, Stroke};
use parking_lot::Mutex;
use std::sync::mpsc::Sender;
use std::sync::Arc;

const BG: Color32 = Color32::from_rgb(10, 10, 10);
const PANEL: Color32 = Color32::from_rgb(18, 18, 18);
const LINE: Color32 = Color32::from_rgb(40, 40, 40);
const TEXT: Color32 = Color32::from_rgb(214, 214, 214);
const DIM: Color32 = Color32::from_rgb(120, 120, 120);
const OK: Color32 = Color32::from_rgb(92, 190, 120);
const WARN: Color32 = Color32::from_rgb(222, 170, 70);
const BAD: Color32 = Color32::from_rgb(210, 84, 84);

pub struct App {
    shared: Arc<Mutex<Shared>>,
    log: Logger,
    tx: Sender<Cmd>,
    show_calib: bool,
    show_debug: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, shared: Arc<Mutex<Shared>>, log: Logger, tx: Sender<Cmd>) -> Self {
        apply_style(&cc.egui_ctx);
        Self { shared, log, tx, show_calib: false, show_debug: false }
    }
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = Color32::from_rgb(6, 6, 6);
    v.override_text_color = Some(TEXT);
    v.widgets.noninteractive.bg_fill = PANEL;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.bg_fill = PANEL;
    v.widgets.inactive.weak_bg_fill = PANEL;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.hovered.bg_fill = Color32::from_rgb(30, 30, 30);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(30, 30, 30);
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(90, 90, 90));
    v.widgets.active.bg_fill = Color32::from_rgb(44, 44, 44);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(44, 44, 44);
    v.selection.bg_fill = Color32::from_rgb(60, 60, 60);
    ctx.set_visuals(v);
}

fn state_color(g: GameState) -> Color32 {
    match g {
        GameState::NoGame | GameState::Unknown => BAD,
        GameState::InMatch | GameState::MatchStarted => OK,
        GameState::LobbyReady => OK,
        _ => WARN,
    }
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn uptime(s: &Shared) -> String {
    match s.run_started {
        Some(t) if s.running => {
            let secs = t.elapsed().as_secs();
            format!("{:02}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
        }
        _ => "--:--:--".into(),
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(std::time::Duration::from_millis(250));

        let (game, found, running, paused, status, stats, debug, up, templates, anti_afk, moving) = {
            let s = self.shared.lock();
            (
                s.game,
                s.window_found,
                s.running,
                s.paused.clone(),
                s.status.clone(),
                s.stats,
                s.debug.clone(),
                uptime(&s),
                s.templates.clone(),
                s.anti_afk,
                s.moving,
            )
        };

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(16))).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("NOVA AFK").size(22.0).strong().color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("F10 — стоп   |   {up}")).color(DIM).size(12.0));
                });
            });
            ui.add_space(10.0);

            card(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(12.0, 12.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 6.0, if found { state_color(game) } else { BAD });
                    ui.vertical(|ui| {
                        ui.label(RichText::new("FORTNITE").color(DIM).size(11.0));
                        ui.label(RichText::new(game.label()).size(16.0));
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.add_space(6.0);
                let (txt, col) = if let Some(r) = &paused {
                    (format!("ПАУЗА — {r}"), WARN)
                } else if running {
                    ("АВТОМАТИЗАЦИЯ ВКЛЮЧЕНА".to_string(), OK)
                } else {
                    ("АВТОМАТИЗАЦИЯ ВЫКЛЮЧЕНА".to_string(), DIM)
                };
                ui.label(RichText::new(txt).color(col).size(13.0).strong());
                ui.label(RichText::new(status).color(DIM).size(12.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let mut on = anti_afk;
                    if ui.checkbox(&mut on, "Движение в матче (бег, прыжки, кирка)").changed() {
                        let _ = self.tx.send(Cmd::SetAntiAfk(on));
                    }
                    if moving {
                        ui.label(RichText::new("● двигается").color(OK).size(11.0));
                    }
                });
            });
            ui.add_space(8.0);

            card(ui, |ui| {
                ui.columns(4, |c| {
                    let item = |ui: &mut egui::Ui, n: u32, t: &str| {
                        ui.vertical_centered(|ui| {
                            ui.label(RichText::new(n.to_string()).size(22.0).strong().color(Color32::WHITE));
                            ui.label(RichText::new(t).size(11.0).color(DIM));
                        });
                    };
                    item(&mut c[0], stats.cycles, "ЦИКЛЫ");
                    item(&mut c[1], stats.matches_ended, "МАТЧЕЙ ЗАВЕРШЕНО");
                    item(&mut c[2], stats.launches, "ЗАПУСКОВ");
                    item(&mut c[3], stats.clicks, "НАЖАТИЙ");
                });
            });
            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let btn = |t: &str| egui::Button::new(RichText::new(t).size(14.0).strong()).min_size(egui::vec2(110.0, 32.0));
                if ui.add_enabled(!running, btn("START")).clicked() {
                    let _ = self.tx.send(Cmd::Start);
                    // Get out of the way: an overlapping window would end up in
                    // the screen capture and hide game buttons. F10 still stops.
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                }
                if ui.add_enabled(running, btn("STOP")).clicked() {
                    let _ = self.tx.send(Cmd::Stop);
                }
                if ui.add(egui::Button::new("Сброс счётчиков").min_size(egui::vec2(0.0, 32.0))).clicked() {
                    let _ = self.tx.send(Cmd::ResetStats);
                }
                ui.checkbox(&mut self.show_calib, "Калибровка");
                ui.checkbox(&mut self.show_debug, "Отладка");
            });

            if self.show_calib {
                ui.add_space(8.0);
                card(ui, |ui| {
                    ui.label(RichText::new("Эталоны (откройте нужный экран в Fortnite и нажмите «Захват»)").color(DIM).size(11.0));
                    for (sig, have) in &templates {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if *have { "●" } else { "○" }).color(if *have { OK } else { DIM }));
                            ui.label(sig.title());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if *have && ui.small_button("Удалить").clicked() {
                                    let _ = self.tx.send(Cmd::Delete(*sig));
                                }
                                if ui.small_button("Захват").clicked() {
                                    let _ = self.tx.send(Cmd::Capture(*sig));
                                }
                            });
                        });
                    }
                    ui.label(RichText::new("Без эталона используется цветовая эвристика.").color(DIM).size(11.0));
                });
            }
            if self.show_debug {
                ui.add_space(6.0);
                ui.label(RichText::new(debug).monospace().size(10.5).color(DIM));
            }

            ui.add_space(8.0);
            ui.label(RichText::new("ЖУРНАЛ").color(DIM).size(11.0));
            egui::Frame::new().fill(Color32::from_rgb(6, 6, 6)).stroke(Stroke::new(1.0, LINE)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    for l in self.log.snapshot() {
                        let col = match l.level {
                            Level::Info => DIM,
                            Level::Action => TEXT,
                            Level::Warn => WARN,
                            Level::Error => BAD,
                        };
                        ui.label(RichText::new(l.text).monospace().size(11.5).color(col));
                    }
                });
            });
        });
    }
}

#[allow(dead_code)]
fn _sig_used(_: Sig) {}
