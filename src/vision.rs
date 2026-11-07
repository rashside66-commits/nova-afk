//! Screen understanding. Pure image analysis, no game access.
//!
//! Every signature is recognised in one of two ways:
//!  * a user captured template (`templates/<name>.png`, matched by normalised
//!    cross-correlation inside a small region, resolution independent), or
//!  * a built-in colour heuristic in HSV space (fallback, where one exists).
//! Templates always take priority, so calibration makes recognition exact for
//! the current season / language / UI scale.

use crate::capture::Frame;
use crate::config::{templates_dir, Config, Roi};
use std::collections::HashMap;

pub const TW: usize = 96;
pub const TH: usize = 48;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Sig {
    Hud,
    Results,
    Lobby,
    Ready,
    Searching,
    Loading,
}

impl Sig {
    pub const ALL: [Sig; 6] = [Sig::Hud, Sig::Results, Sig::Lobby, Sig::Ready, Sig::Searching, Sig::Loading];

    pub fn file(self) -> &'static str {
        match self {
            Sig::Hud => "hud",
            Sig::Results => "results",
            Sig::Lobby => "lobby",
            Sig::Ready => "ready",
            Sig::Searching => "searching",
            Sig::Loading => "loading",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Sig::Hud => "Матч (HUD)",
            Sig::Results => "Экран результатов",
            Sig::Lobby => "Лобби загружено",
            Sig::Ready => "Кнопка ГОТОВО",
            Sig::Searching => "Поиск матча",
            Sig::Loading => "Загрузка",
        }
    }
}

/// Coarse classification of a single frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Raw {
    Sleep,
    Hud,
    Results,
    Panel,
    Discover,
    Searching,
    Ready,
    Lobby,
    Loading,
    Unknown,
}

#[derive(Clone, Copy, Default)]
pub struct Det {
    pub hit: bool,
    pub score: f32,
    pub template: bool,
}

pub struct Observation {
    pub hud: Det,
    pub results: Det,
    pub lobby: Det,
    pub ready: Det,
    pub searching: Det,
    pub loading: Det,
    /// Fortnite's own "FORTNITE IS IN SLEEP MODE" screen.
    pub sleep: Det,
    /// Centre of its KEEP PLAYING button (normalised).
    pub sleep_button: Option<(f32, f32)>,
    /// Lobby mode settings panel ("Team Fill" etc.).
    pub panel: Det,
    /// Mode menu (featured mode + tile row).
    pub discover: Det,
    /// "Team Fill" switch is on (only meaningful while `panel.hit`).
    pub fill_on: Det,
    /// Featured mode title vs "BATTLE ROYALE" (only while `discover.hit`).
    pub hero_br: f32,
    /// Lobby button is yellow (whether or not it looks idle).
    pub button_yellow: bool,
    /// Grayscale patch of the button region (for stability checks).
    pub button_patch: Vec<f32>,
    /// Same for the PLAY AGAIN button of the recap screen.
    pub play_again_patch: Vec<f32>,
    /// Mean luminance of a coarse sample of the whole frame.
    pub mean_luma: f32,
}

impl Observation {
    pub fn classify(&self) -> Raw {
        // Screens with their own distinctive yellow buttons come first, so the
        // dark mode-menu background can never be mistaken for sleep mode.
        if self.hud.hit {
            Raw::Hud
        } else if self.results.hit {
            Raw::Results
        } else if self.panel.hit {
            Raw::Panel
        } else if self.discover.hit {
            Raw::Discover
        } else if self.sleep.hit {
            Raw::Sleep
        } else if self.searching.hit {
            Raw::Searching
        } else if self.ready.hit {
            Raw::Ready
        } else if self.lobby.hit {
            Raw::Lobby
        } else if self.loading.hit {
            Raw::Loading
        } else {
            Raw::Unknown
        }
    }

    pub fn summary(&self) -> String {
        let f = |n: &str, d: &Det| format!("{n}={:.2}{}{}", d.score, if d.template { "T" } else { "h" }, if d.hit { "*" } else { "" });
        format!(
            "{} {} {} {} {} {} {} {} {} br={:.2}",
            f("hud", &self.hud),
            f("res", &self.results),
            f("lob", &self.lobby),
            f("rdy", &self.ready),
            f("srch", &self.searching),
            f("load", &self.loading),
            f("pan", &self.panel),
            f("fill", &self.fill_on),
            f("disc", &self.discover),
            self.hero_br
        )
    }
}

struct Template {
    gray: Vec<f32>,
}

/// References cut from real 1920x1080 screenshots, compiled into the exe.
struct Builtin {
    /// "BATTLE ROYALE" title of the featured mode (bright-pixel mask).
    br_title: Vec<f32>,
    /// Battle Royale tile of the bottom row.
    br_tile: Vec<f32>,
    /// Lobby PLAY button while idle (not searching).
    idle_play: Vec<f32>,
}

impl Builtin {
    fn load() -> Self {
        let g = |bytes: &[u8]| image::load_from_memory(bytes).map(|i| to_gray(&i)).unwrap_or_default();
        Self {
            br_title: bright_mask(&g(include_bytes!("../assets/br_hero_title.png"))),
            br_tile: g(include_bytes!("../assets/br_tile.png")),
            idle_play: g(include_bytes!("../assets/idle_play.png")),
        }
    }
}

pub struct Vision {
    cfg: Config,
    templates: HashMap<Sig, Template>,
    builtin: Builtin,
}

struct Hsv {
    purple: f32,
    yellow: f32,
    green: f32,
    blue: f32,
    luma: f32,
    navy: f32,
}

impl Vision {
    pub fn new(cfg: Config) -> Self {
        let mut v = Self { cfg, templates: HashMap::new(), builtin: Builtin::load() };
        v.reload_templates();
        v
    }

    pub fn reload_templates(&mut self) {
        self.templates.clear();
        for s in Sig::ALL {
            let p = templates_dir().join(format!("{}.png", s.file()));
            if let Ok(img) = image::open(&p) {
                self.templates.insert(s, Template { gray: to_gray(&img) });
            }
        }
    }

    pub fn has_template(&self, s: Sig) -> bool {
        self.templates.contains_key(&s)
    }

    fn roi_of(&self, s: Sig) -> Roi {
        match s {
            Sig::Hud => self.cfg.hud_roi,
            Sig::Results => self.cfg.results_roi,
            Sig::Lobby => self.cfg.lobby_roi,
            Sig::Ready | Sig::Searching => self.cfg.button_roi,
            Sig::Loading => Roi::FULL,
        }
    }

    /// Save the current region of `s` as its template (as RGB, 2x working size).
    pub fn save_template(&mut self, f: &Frame, s: Sig) -> anyhow::Result<()> {
        let (w, h) = (TW * 2, TH * 2);
        let rgb = crop_rgb(f, self.roi_of(s), w, h);
        let mut img = image::RgbImage::new(w as u32, h as u32);
        for y in 0..h {
            for x in 0..w {
                let [r, g, b] = rgb[y * w + x];
                img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
            }
        }
        std::fs::create_dir_all(templates_dir())?;
        img.save(templates_dir().join(format!("{}.png", s.file())))?;
        self.reload_templates();
        Ok(())
    }

    pub fn delete_template(&mut self, s: Sig) {
        let _ = std::fs::remove_file(templates_dir().join(format!("{}.png", s.file())));
        self.reload_templates();
    }

    fn by_template(&self, f: &Frame, s: Sig) -> Option<Det> {
        let t = self.templates.get(&s)?;
        let score = ncc_best(f, self.roi_of(s), &t.gray);
        Some(Det { hit: score >= self.cfg.template_threshold, score, template: true })
    }

    pub fn observe(&self, f: &Frame) -> Observation {
        let c = &self.cfg;
        let btn = hsv_stats(f, c.button_roi);
        let hud = hsv_stats(f, c.hud_roi);
        let map = hsv_stats(f, c.minimap_roi);
        let again = hsv_stats(f, c.play_again_roi);
        let whole = hsv_stats(f, Roi::FULL);

        let hero = hsv_stats(f, c.hero_play_roi);
        let panel_play = hsv_stats(f, c.panel_play_roi);
        let panel_done = hsv_stats(f, c.panel_done_roi);
        let fill = hsv_stats(f, c.fill_switch_roi);

        // Heuristics -------------------------------------------------------
        // Yellow PLAY that also looks like the idle PLAY button (a search in
        // progress changes the button, and pressing it then would cancel).
        let idle_score = ncc_best(f, c.button_roi, &self.builtin.idle_play);
        let ready_h = Det {
            hit: btn.yellow >= c.yellow_fraction_ready && idle_score >= 0.75,
            score: idle_score,
            template: false,
        };
        // Mode settings panel: yellow PLAY + purple DONE (bottom-right).
        let panel = Det {
            hit: panel_play.yellow >= 0.5 && panel_done.purple >= 0.3,
            score: panel_play.yellow.min(panel_done.purple),
            template: false,
        };
        let fill_on = Det { hit: fill.yellow >= 0.3, score: fill.yellow, template: false };
        // Mode menu: yellow PLAY under the featured mode.
        let discover = Det { hit: hero.yellow >= 0.5 && !panel.hit, score: hero.yellow, template: false };
        let hero_br = if discover.hit { self.hero_is_br(f) } else { 0.0 };
        // Sleep mode: almost the whole screen dark navy + a light lavender
        // KEEP PLAYING button near the centre (white title is excluded).
        let sleep_button = if whole.navy >= 0.55 {
            let (x, y, frac) = find_blob(f, c.sleep_button_search_roi, |h, s, v| {
                (200.0..270.0).contains(&h) && (0.08..0.5).contains(&s) && v > 0.75
            });
            (frac >= 0.005).then_some((x, y))
        } else {
            None
        };
        let sleep = Det { hit: sleep_button.is_some(), score: whole.navy, template: false };
        // No colour heuristic for "searching": it fired on transition frames.
        // A search is recognised by the button changing after our press (in
        // the engine) or by a captured "searching" template.
        let searching_h = Det::default();
        // Own bars (bottom-left) + minimap (water around the island).
        let hud_h = Det {
            hit: hud.green >= c.hud_green_fraction
                && (hud.blue >= c.hud_blue_fraction || hud.green >= c.hud_green_fraction * 3.0)
                && map.blue >= c.minimap_blue_fraction,
            score: ((hud.green + hud.blue) / (c.hud_green_fraction * 2.0)).min(1.0) * 0.99,
            template: false,
        };
        // Match recap: big yellow PLAY AGAIN on the right.
        let results_h = Det {
            hit: again.yellow >= c.results_yellow_fraction,
            score: (again.yellow / c.results_yellow_fraction).min(1.0) * 0.99,
            template: false,
        };
        let loading_h = Det { hit: whole.luma < c.black_luma, score: 1.0 - whole.luma, template: false };

        let ready = self.by_template(f, Sig::Ready).unwrap_or(ready_h);
        let searching = self.by_template(f, Sig::Searching).unwrap_or(searching_h);
        let hud = self.by_template(f, Sig::Hud).unwrap_or(hud_h);
        let results = self.by_template(f, Sig::Results).unwrap_or(results_h);
        let loading = self.by_template(f, Sig::Loading).unwrap_or(loading_h);
        // Without a lobby template the lobby counts as loaded once a lobby
        // button (Ready / Searching) is on screen.
        let lobby = self.by_template(f, Sig::Lobby).unwrap_or(Det {
            hit: ready.hit || searching.hit,
            score: ready.score.max(searching.score),
            template: false,
        });

        // A black screen never counts as a match / lobby.
        let dark = whole.luma < c.black_luma * 0.5;
        let mask = |d: Det| if dark { Det { hit: false, ..d } } else { d };

        Observation {
            hud: mask(hud),
            results: mask(results),
            lobby: mask(lobby),
            ready: mask(ready),
            searching: mask(searching),
            loading,
            sleep,
            sleep_button,
            panel: mask(panel),
            discover: mask(discover),
            fill_on,
            hero_br,
            button_yellow: btn.yellow >= c.yellow_fraction_ready,
            button_patch: crop_gray(f, c.button_roi, TW, TH),
            play_again_patch: crop_gray(f, c.play_again_roi, TW, TH),
            mean_luma: whole.luma,
        }
    }

    /// How much the featured mode title looks like "BATTLE ROYALE" (0..1).
    pub fn hero_is_br(&self, f: &Frame) -> f32 {
        let r = self.cfg.hero_title_roi;
        let (sx, sy) = (r.w / TW as f32, r.h / TH as f32);
        let mut best = -1f32;
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let s = Roi::new(r.x + dx as f32 * sx, r.y + dy as f32 * sy, r.w, r.h);
                best = best.max(ncc(&bright_mask(&crop_gray(f, s, TW, TH)), &self.builtin.br_title));
            }
        }
        best
    }

    /// Find the Battle Royale tile in the bottom row of the mode menu.
    pub fn find_br_tile(&self, f: &Frame) -> Option<(Roi, f32)> {
        let c = &self.cfg;
        (0..c.tile_count)
            .map(|i| {
                let r = Roi::new(c.tile_roi.x + i as f32 * c.tile_step, c.tile_roi.y, c.tile_roi.w, c.tile_roi.h);
                (r, ncc_best(f, r, &self.builtin.br_tile))
            })
            .filter(|(r, _)| r.x + r.w <= 1.0)
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|(_, s)| *s >= 0.6)
    }
}

fn to_gray(img: &image::DynamicImage) -> Vec<f32> {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut gray = vec![0f32; TW * TH];
    for y in 0..TH {
        for x in 0..TW {
            let p = rgb.get_pixel(((x * w / TW).min(w - 1)) as u32, ((y * h / TH).min(h - 1)) as u32);
            gray[y * TW + x] = luma(p[0], p[1], p[2]);
        }
    }
    gray
}

/// Keeps only near-white pixels: big white titles survive any background.
fn bright_mask(g: &[f32]) -> Vec<f32> {
    g.iter().map(|v| if *v > 0.8 { 1.0 } else { 0.0 }).collect()
}

fn luma(r: u8, g: u8, b: u8) -> f32 {
    (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0
}

fn px(f: &Frame, x: usize, y: usize) -> (u8, u8, u8) {
    let i = (y * f.w + x) * 4;
    (f.data[i + 2], f.data[i + 1], f.data[i])
}

fn rect_px(f: &Frame, r: Roi) -> (usize, usize, usize, usize) {
    let x0 = ((r.x.clamp(0.0, 1.0)) * f.w as f32) as usize;
    let y0 = ((r.y.clamp(0.0, 1.0)) * f.h as f32) as usize;
    let x1 = (((r.x + r.w).clamp(0.0, 1.0)) * f.w as f32) as usize;
    let y1 = (((r.y + r.h).clamp(0.0, 1.0)) * f.h as f32) as usize;
    (x0.min(f.w - 1), y0.min(f.h - 1), x1.clamp(x0 + 1, f.w), y1.clamp(y0 + 1, f.h))
}

fn rgb_to_hsv(r: u8, g: u8, b: u8) -> (f32, f32, f32) {
    let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let h = if d == 0.0 {
        0.0
    } else if mx == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if mx == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (if h < 0.0 { h + 360.0 } else { h }, if mx == 0.0 { 0.0 } else { d / mx }, mx)
}

/// Colour statistics over a sampled grid of the region.
fn hsv_stats(f: &Frame, roi: Roi) -> Hsv {
    let (x0, y0, x1, y1) = rect_px(f, roi);
    let gx = 64usize.min(x1 - x0).max(1);
    let gy = 32usize.min(y1 - y0).max(1);
    let (mut yel, mut grn, mut blu, mut pur, mut lum, mut navy) = (0f32, 0f32, 0f32, 0f32, 0f32, 0f32);
    let mut n = 0f32;
    for j in 0..gy {
        let y = y0 + j * (y1 - y0) / gy;
        for i in 0..gx {
            let x = x0 + i * (x1 - x0) / gx;
            let (r, g, b) = px(f, x, y);
            let (h, s, v) = rgb_to_hsv(r, g, b);
            // Dark navy background of the sleep-mode screen.
            if (195.0..250.0).contains(&h) && s > 0.3 && (0.05..0.5).contains(&v) {
                navy += 1.0;
            }
            if (38.0..65.0).contains(&h) && s > 0.55 && v > 0.65 {
                yel += 1.0;
            }
            // Health bar is a lime-to-green gradient.
            if (68.0..150.0).contains(&h) && s > 0.5 && v > 0.45 {
                grn += 1.0;
            }
            if (185.0..235.0).contains(&h) && s > 0.5 && v > 0.45 {
                blu += 1.0;
            }
            if (240.0..295.0).contains(&h) && s > 0.35 && v > 0.35 {
                pur += 1.0;
            }
            lum += luma(r, g, b);
            n += 1.0;
        }
    }
    Hsv { purple: pur / n, yellow: yel / n, green: grn / n, blue: blu / n, luma: lum / n, navy: navy / n }
}

/// Centroid (normalised window coordinates) and area fraction of the pixels
/// inside `roi` that satisfy `pred(h, s, v)`.
fn find_blob(f: &Frame, roi: Roi, pred: impl Fn(f32, f32, f32) -> bool) -> (f32, f32, f32) {
    let (x0, y0, x1, y1) = rect_px(f, roi);
    let (gx, gy) = (96usize.min(x1 - x0).max(1), 64usize.min(y1 - y0).max(1));
    let (mut sx, mut sy, mut hits) = (0f32, 0f32, 0f32);
    for j in 0..gy {
        let y = y0 + j * (y1 - y0) / gy;
        for i in 0..gx {
            let x = x0 + i * (x1 - x0) / gx;
            let (r, g, b) = px(f, x, y);
            let (h, s, v) = rgb_to_hsv(r, g, b);
            if pred(h, s, v) {
                sx += x as f32;
                sy += y as f32;
                hits += 1.0;
            }
        }
    }
    if hits == 0.0 {
        return (0.0, 0.0, 0.0);
    }
    (sx / hits / f.w as f32, sy / hits / f.h as f32, hits / (gx * gy) as f32)
}

/// Box-sampled RGB crop of a region to `w`x`h`.
fn crop_rgb(f: &Frame, roi: Roi, w: usize, h: usize) -> Vec<[u8; 3]> {
    let (x0, y0, x1, y1) = rect_px(f, roi);
    let (sw, sh) = ((x1 - x0) as f32, (y1 - y0) as f32);
    let mut out = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let (mut r, mut g, mut b) = (0u32, 0u32, 0u32);
            let mut n = 0u32;
            for sy in 0..2 {
                for sx in 0..2 {
                    let fx = (i as f32 + 0.25 + 0.5 * sx as f32) / w as f32;
                    let fy = (j as f32 + 0.25 + 0.5 * sy as f32) / h as f32;
                    let x = (x0 + (fx * sw) as usize).min(f.w - 1);
                    let y = (y0 + (fy * sh) as usize).min(f.h - 1);
                    let (pr, pg, pb) = px(f, x, y);
                    r += pr as u32;
                    g += pg as u32;
                    b += pb as u32;
                    n += 1;
                }
            }
            out.push([(r / n) as u8, (g / n) as u8, (b / n) as u8]);
        }
    }
    out
}

fn crop_gray(f: &Frame, roi: Roi, w: usize, h: usize) -> Vec<f32> {
    crop_rgb(f, roi, w, h).iter().map(|p| luma(p[0], p[1], p[2])).collect()
}

fn ncc(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len() as f32;
    let (ma, mb) = (a.iter().sum::<f32>() / n, b.iter().sum::<f32>() / n);
    let (mut num, mut da, mut db) = (0f32, 0f32, 0f32);
    for (x, y) in a.iter().zip(b) {
        let (p, q) = (x - ma, y - mb);
        num += p * q;
        da += p * p;
        db += q * q;
    }
    let den = (da * db).sqrt();
    if den < 1e-4 {
        0.0
    } else {
        num / den
    }
}

/// Best NCC over small translations (tolerates minor layout shifts).
fn ncc_best(f: &Frame, roi: Roi, tpl: &[f32]) -> f32 {
    let mut best = -1f32;
    let (sx, sy) = (roi.w / TW as f32, roi.h / TH as f32);
    for dy in -2i32..=2 {
        for dx in -2i32..=2 {
            let r = Roi::new(roi.x + dx as f32 * sx, roi.y + dy as f32 * sy, roi.w, roi.h);
            best = best.max(ncc(&crop_gray(f, r, TW, TH), tpl));
        }
    }
    best
}

/// Mean absolute difference of two equally sized patches.
pub fn patch_diff(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 1.0;
    }
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len() as f32
}

/// Coarse fingerprint of the entire frame, used to detect a frozen picture.
pub fn fingerprint(f: &Frame) -> Vec<f32> {
    crop_gray(f, Roi::FULL, 32, 18)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(fill: impl Fn(usize, usize) -> (u8, u8, u8)) -> Frame {
        let (w, h) = (1280, 720);
        let mut data = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let (r, g, b) = fill(x, y);
                let i = (y * w + x) * 4;
                data[i] = b;
                data[i + 1] = g;
                data[i + 2] = r;
            }
        }
        Frame { w, h, data }
    }

    fn in_roi(x: usize, y: usize, r: Roi) -> bool {
        let (fx, fy) = (x as f32 / 1280.0, y as f32 / 720.0);
        fx >= r.x && fx < r.x + r.w && fy >= r.y && fy < r.y + r.h
    }

    /// A yellow plate without the PLAY lettering (e.g. a changed button
    /// while searching) must not be pressed.
    #[test]
    fn blank_yellow_button_is_not_ready() {
        let cfg = Config::default();
        let r = cfg.button_roi;
        let f = frame(|x, y| if in_roi(x, y, r) { (255, 214, 0) } else { (40, 60, 90) });
        let o = Vision::new(cfg).observe(&f);
        assert!(o.button_yellow);
        assert_ne!(o.classify(), Raw::Ready);
    }

    #[test]
    fn health_and_shield_bars_are_hud() {
        let cfg = Config::default();
        let r = cfg.hud_roi;
        let map = cfg.minimap_roi;
        let f = frame(|x, y| {
            if in_roi(x, y, map) {
                (30, 110, 210)
            } else if in_roi(x, y, r) {
                if (y as f32 / 720.0) < r.y + r.h * 0.5 { (60, 220, 60) } else { (40, 150, 255) }
            } else {
                (90, 110, 70)
            }
        });
        assert_eq!(Vision::new(cfg).observe(&f).classify(), Raw::Hud);
    }

    #[test]
    fn black_screen_is_loading() {
        let f = frame(|_, _| (0, 0, 0));
        assert_eq!(Vision::new(Config::default()).observe(&f).classify(), Raw::Loading);
    }

    /// Real 1920x1080 screenshots, if present (set NOVA_SHOTS to their folder).
    fn shot(name: &str) -> Option<Frame> {
        let dir = std::env::var("NOVA_SHOTS").ok()?;
        let img = image::open(std::path::Path::new(&dir).join(name)).ok()?.to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        let mut data = img.into_raw();
        for p in data.chunks_mut(4) {
            p.swap(0, 2);
        }
        Some(Frame { w, h, data })
    }

    /// Cut the game picture out of a screenshot with a border around it.
    fn inner(f: Frame, x: usize, y: usize, w: usize, h: usize) -> Frame {
        let mut data = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            data.extend_from_slice(&f.data[(row * f.w + x) * 4..(row * f.w + x + w) * 4]);
        }
        Frame { w, h, data }
    }

    #[test]
    fn real_screenshots() {
        let v = Vision::new(Config::default());
        let shots = [
            ("1.webp", None, Raw::Ready),
            ("2.webp", None, Raw::Ready),
            ("4.webp", None, Raw::Hud),
            // Elimination screen: the killer's bars must not count as our HUD.
            ("5.png", Some((12, 30, 541, 290)), Raw::Unknown),
            ("6.png", Some((17, 25, 534, 308)), Raw::Results),
            ("7.webp", None, Raw::Discover),
            ("8.webp", None, Raw::Ready),
            ("9.webp", None, Raw::Panel),
            // Lobby right after choosing Battle Royale in the mode menu.
            ("10.webp", None, Raw::Ready),
            // Sleep mode (phone photo: game area without the black bars).
            ("11.png", Some((0, 200, 335, 290)), Raw::Sleep),
        ];
        for (name, crop, want) in shots {
            let Some(f) = shot(name) else { continue };
            let f = match crop {
                Some((x, y, w, h)) => inner(f, x, y, w, h),
                None => f,
            };
            let o = v.observe(&f);
            println!("{name}: {:?} {}", o.classify(), o.summary());
            assert_eq!(o.classify(), want, "{name}");
            if want == Raw::Sleep {
                // KEEP PLAYING is horizontally centred, under the title.
                let (x, _) = o.sleep_button.unwrap();
                assert!((0.4..0.6).contains(&x), "button x {x}");
            }
        }
        // Mode menu: featured is Battle Royale and its tile is the first one.
        if let Some(f) = shot("7.webp") {
            assert!(v.hero_is_br(&f) > 0.8);
            let (tile, _) = v.find_br_tile(&f).expect("BR tile");
            assert!(tile.x < 0.05);
        }
        // Lobby / panel do not look like the featured BR title.
        for name in ["2.webp", "8.webp", "9.webp"] {
            if let Some(f) = shot(name) {
                println!("{name}: hero_br={:.2}", v.hero_is_br(&f));
                assert!(v.hero_is_br(&f) < 0.5, "{name}");
            }
        }
        // Team Fill on in the panel screenshot.
        if let Some(f) = shot("9.webp") {
            assert!(v.observe(&f).fill_on.hit);
        }
    }

    /// Regenerates the built-in references in `assets/` from screenshots:
    /// `cargo test make_assets -- --ignored` with NOVA_SHOTS set.
    #[test]
    #[ignore]
    fn make_assets() {
        let c = Config::default();
        let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
        std::fs::create_dir_all(&out).unwrap();
        let save = |file: &str, roi: Roi, name: &str| {
            let f = shot(file).expect(file);
            let (w, h) = (TW * 2, TH * 2);
            let rgb = crop_rgb(&f, roi, w, h);
            let mut img = image::RgbImage::new(w as u32, h as u32);
            for (i, p) in rgb.iter().enumerate() {
                img.put_pixel((i % w) as u32, (i / w) as u32, image::Rgb(*p));
            }
            img.save(out.join(name)).unwrap();
        };
        save("7.webp", c.hero_title_roi, "br_hero_title.png");
        save("7.webp", c.tile_roi, "br_tile.png");
        save("8.webp", c.button_roi, "idle_play.png");
    }

    #[test]
    fn ncc_matches_itself_and_rejects_noise() {
        let a: Vec<f32> = (0..TW * TH).map(|i| ((i * 7919) % 255) as f32 / 255.0).collect();
        let b: Vec<f32> = (0..TW * TH).map(|i| ((i * 104729 + 13) % 251) as f32 / 255.0).collect();
        assert!(ncc(&a, &a) > 0.999);
        assert!(ncc(&a, &b) < 0.3);
    }
}
