//! Tunable settings. Stored next to the executable as `nova_afk.json`; defaults
//! are written on first start so every ROI and threshold can be adjusted.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Region of interest in normalised window coordinates (0..1).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Roi {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Roi {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub const FULL: Roi = Roi::new(0.0, 0.0, 1.0, 1.0);
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

/// Bumped whenever default regions change; an older file is replaced.
pub const CONFIG_VERSION: u32 = 7;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub config_version: u32,
    // ---- regions ----
    /// The big yellow PLAY / "ГОТОВО" button (bottom-left of the lobby).
    pub button_roi: Roi,
    /// Health / shield bars of the in-match HUD (bottom-left).
    pub hud_roi: Roi,
    /// Minimap (top-right): required together with the bars, so the
    /// killer's bars on the elimination screen do not count as our HUD.
    pub minimap_roi: Roi,
    /// Area that is characteristic for the post-match results screen
    /// (template only; the heuristic uses `play_again_roi`).
    pub results_roi: Roi,
    /// Yellow "PLAY AGAIN" button on the match recap screen.
    pub play_again_roi: Roi,
    /// "RETURN TO LOBBY" button above it (fallback).
    pub return_lobby_roi: Roi,
    /// "CONTINUE" button of the elimination screen (bottom-right).
    pub continue_roi: Roi,

    // ---- mode menu (Discover) ----
    /// Yellow PLAY under the featured mode.
    pub hero_play_roi: Roi,
    /// Big title of the featured mode ("BATTLE ROYALE").
    pub hero_title_roi: Roi,
    /// First tile of the bottom row; the others follow every `tile_step`.
    pub tile_roi: Roi,
    pub tile_step: f32,
    pub tile_count: usize,

    // ---- lobby mode settings ("Build, Squad, Fill") ----
    pub mode_button_roi: Roi,
    pub panel_play_roi: Roi,
    pub panel_done_roi: Roi,
    /// Inner switch of "Team Fill": yellow when enabled.
    pub fill_switch_roi: Roi,
    /// Whole "Team Fill" toggle (click target).
    pub fill_toggle_roi: Roi,
    /// Check "Team Fill" before every matchmaking start.
    pub enforce_fill: bool,

    // ---- sleep mode / anti-AFK ----
    /// Where to look for the KEEP PLAYING button of Fortnite's sleep mode.
    pub sleep_button_search_roi: Roi,
    /// Keep the character moving (run, jump, turn, swing the pickaxe) while
    /// a match is running.
    pub anti_afk: bool,
    /// Hide the NOVA AFK window from screen capture so it can sit on top of
    /// the game (also hides it from OBS / screenshots).
    pub hide_from_capture: bool,
    /// Area that is characteristic for a fully loaded lobby (top navigation).
    pub lobby_roi: Roi,

    // ---- recognition ----
    /// Minimum normalised cross-correlation for a user captured template.
    pub template_threshold: f32,
    pub yellow_fraction_ready: f32,
    pub hud_green_fraction: f32,
    pub hud_blue_fraction: f32,
    pub minimap_blue_fraction: f32,
    pub results_yellow_fraction: f32,
    pub black_luma: f32,

    // ---- input ----
    /// Start matchmaking with the keyboard shortcut first (P), mouse click on retry.
    pub use_start_key: bool,
    /// Hardware scan code of the start key (0x19 = P).
    pub start_key_scancode: u16,
    /// Button region must change at least this much after a press to count
    /// as "search started" (prevents a second press cancelling the search).
    pub press_change_diff: f32,

    // ---- reliability ----
    pub poll_ms: u64,
    /// A state is confirmed when it is the newest observation and appears at
    /// least `confirm_needed` times among the last `confirm_window` observations.
    pub confirm_window: usize,
    pub confirm_needed: usize,
    /// Consecutive confirmed "Ready" observations with an unchanged button
    /// before it is clicked (interface is fully loaded and settled).
    pub ready_stable_needed: usize,
    pub ready_stable_diff: f32,
    /// Safety nets (not used to detect states, only to catch failures).
    pub click_verify_timeout_s: u64,
    /// After the HUD is gone and nothing known is on screen for this long,
    /// press CONTINUE on the elimination screen.
    pub continue_after_s: u64,
    /// The recap screen slides in and ignores clicks for a moment: press
    /// PLAY AGAIN only after it has been on screen this long.
    pub results_settle_s: u64,
    pub max_click_retries: u32,
    pub rearm_after_s: u64,
    pub static_frame_timeout_s: u64,
    pub static_frame_timeout_in_match_s: u64,
    pub stuck_timeout_s: u64,
    pub capture_fail_limit: u32,
    pub no_window_limit: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            // Measured on a 1920x1080 lobby: PLAY at x 96..424, y 872..952.
            button_roi: Roi::new(0.055, 0.812, 0.16, 0.065),
            // Shield + health bars at x 63..368, y 962..1020.
            hud_roi: Roi::new(0.03, 0.885, 0.17, 0.07),
            minimap_roi: Roi::new(0.87, 0.03, 0.11, 0.2),
            results_roi: Roi::new(0.25, 0.04, 0.50, 0.18),
            play_again_roi: Roi::new(0.735, 0.46, 0.23, 0.055),
            return_lobby_roi: Roi::new(0.735, 0.39, 0.23, 0.04),
            continue_roi: Roi::new(0.86, 0.935, 0.125, 0.045),
            // Measured on 1920x1080 screenshots of the mode menu and panel.
            hero_play_roi: Roi::new(0.045, 0.648, 0.13, 0.042),
            hero_title_roi: Roi::new(0.052, 0.335, 0.186, 0.153),
            tile_roi: Roi::new(0.0354, 0.907, 0.122, 0.09),
            tile_step: 0.1344,
            tile_count: 7,
            mode_button_roi: Roi::new(0.05, 0.637, 0.171, 0.0685),
            panel_play_roi: Roi::new(0.621, 0.852, 0.181, 0.074),
            panel_done_roi: Roi::new(0.811, 0.852, 0.119, 0.074),
            fill_switch_roi: Roi::new(0.861, 0.4537, 0.037, 0.0296),
            fill_toggle_roi: Roi::new(0.8297, 0.437, 0.1, 0.063),
            enforce_fill: true,
            sleep_button_search_roi: Roi::new(0.25, 0.3, 0.5, 0.5),
            anti_afk: true,
            hide_from_capture: true,
            lobby_roi: Roi::new(0.05, 0.0, 0.90, 0.08),
            template_threshold: 0.82,
            yellow_fraction_ready: 0.30,
            hud_green_fraction: 0.025,
            hud_blue_fraction: 0.025,
            minimap_blue_fraction: 0.08,
            results_yellow_fraction: 0.35,
            black_luma: 0.05,
            use_start_key: true,
            start_key_scancode: 0x19,
            press_change_diff: 0.05,
            poll_ms: 300,
            confirm_window: 5,
            confirm_needed: 3,
            ready_stable_needed: 4,
            ready_stable_diff: 0.04,
            click_verify_timeout_s: 8,
            continue_after_s: 10,
            results_settle_s: 4,
            max_click_retries: 3,
            rearm_after_s: 12,
            static_frame_timeout_s: 120,
            static_frame_timeout_in_match_s: 600,
            stuck_timeout_s: 300,
            capture_fail_limit: 5,
            no_window_limit: 5,
        }
    }
}

pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    app_dir().join("nova_afk.json")
}

pub fn templates_dir() -> PathBuf {
    app_dir().join("templates")
}

impl Config {
    pub fn load() -> (Self, Option<String>) {
        let path = config_path();
        let outdated = |s: &str| {
            serde_json::from_str::<serde_json::Value>(s)
                .ok()
                .and_then(|v| v.get("config_version")?.as_u64())
                .map_or(true, |v| v < CONFIG_VERSION as u64)
        };
        match std::fs::read_to_string(&path) {
            Ok(s) if outdated(&s) => {
                let c = Config::default();
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&c).unwrap_or_default());
                (c, Some("nova_afk.json устарел — записаны новые области по умолчанию".into()))
            }
            Ok(s) => match serde_json::from_str::<Config>(&s) {
                Ok(mut c) => {
                    c.sanitize();
                    (c, None)
                }
                Err(e) => (Config::default(), Some(format!("nova_afk.json: {e} — использую значения по умолчанию"))),
            },
            Err(_) => {
                let c = Config::default();
                let _ = std::fs::write(&path, serde_json::to_string_pretty(&c).unwrap_or_default());
                (c, None)
            }
        }
    }

    fn sanitize(&mut self) {
        self.confirm_window = self.confirm_window.clamp(1, 30);
        self.confirm_needed = self.confirm_needed.clamp(1, self.confirm_window);
        self.poll_ms = self.poll_ms.clamp(100, 5000);
        self.ready_stable_needed = self.ready_stable_needed.max(1);
    }
}
