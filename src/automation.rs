//! The AFK engine: observation -> confirmation -> state flow -> guarded click.
//! Runs on its own thread and publishes a snapshot for the UI through `Shared`.

use crate::capture::{self, Frame, GameWindow};
use crate::config::{Config, Roi};
use crate::input;
use crate::logger::Logger;
use crate::mover;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::vision::{self, Observation, Raw, Sig, Vision};
use parking_lot::Mutex;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GameState {
    NoGame,
    Unknown,
    InMatch,
    MatchEnded,
    Results,
    Exiting,
    LobbyLoading,
    LobbyReady,
    Searching,
    LoadingMatch,
    MatchStarted,
    ModeSelect,
    ModeSettings,
    Sleep,
}

const SCAN_ESC: u16 = 0x01;

impl GameState {
    pub fn label(self) -> &'static str {
        match self {
            GameState::NoGame => "Fortnite не найден",
            GameState::Unknown => "Не определено",
            GameState::InMatch => "Идёт матч",
            GameState::MatchEnded => "Матч завершён",
            GameState::Results => "Экран результатов",
            GameState::Exiting => "Выход из матча",
            GameState::LobbyLoading => "Загрузка лобби",
            GameState::LobbyReady => "Лобби: ГОТОВО доступна",
            GameState::Searching => "Поиск матча",
            GameState::LoadingMatch => "Загрузка новой катки",
            GameState::MatchStarted => "Матч запущен",
            GameState::ModeSelect => "Меню выбора режима",
            GameState::ModeSettings => "Настройки режима (Fill)",
            GameState::Sleep => "Fortnite в спящем режиме",
        }
    }
}

#[derive(Clone, Copy, Default)]
pub struct Stats {
    pub matches_ended: u32,
    pub launches: u32,
    pub cycles: u32,
    pub clicks: u32,
    pub retries: u32,
}

pub struct Shared {
    pub game: GameState,
    pub window_found: bool,
    pub running: bool,
    pub paused: Option<String>,
    pub status: String,
    pub stats: Stats,
    pub debug: String,
    pub run_started: Option<Instant>,
    pub templates: Vec<(Sig, bool)>,
    pub anti_afk: bool,
    pub moving: bool,
}

impl Shared {
    pub fn new() -> Self {
        Self {
            game: GameState::NoGame,
            window_found: false,
            running: false,
            paused: None,
            status: "Остановлено".into(),
            stats: Stats::default(),
            debug: String::new(),
            run_started: None,
            templates: Vec::new(),
            anti_afk: true,
            moving: false,
        }
    }
}

pub enum Cmd {
    Start,
    Stop,
    ResetStats,
    Capture(Sig),
    Delete(Sig),
    SetAntiAfk(bool),
}

/// Which start button a press goes to.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Lobby,
    PlayAgain,
}

#[derive(Clone, Copy, PartialEq)]
enum Click {
    Idle,
    Clicked(Instant),
    Done,
}

pub struct Engine {
    cfg: Config,
    vision: Vision,
    shared: Arc<Mutex<Shared>>,
    log: Logger,
    rx: Receiver<Cmd>,

    history: VecDeque<Raw>,
    confirmed: Raw,
    last_frame: Option<Frame>,

    // health
    no_window: u32,
    grab_fail: u32,
    prev_fp: Vec<f32>,
    static_since: Option<Instant>,
    healthy_ticks: u32,
    pause_auto: bool,

    // flow
    running: bool,
    in_match: bool,
    post_match: bool,
    end_counted: bool,
    launch_pending: bool,
    cycle_armed: bool,
    click: Click,
    attempts: u32,
    ready_stable: u32,
    prev_patch: Vec<f32>,
    /// Button region right before our press: the idle "PLAY" look.
    pre_press_patch: Vec<f32>,
    press_target: Target,
    play_again_failed: bool,
    results_since: Option<Instant>,
    post_match_since: Option<Instant>,
    continue_clicks: u32,
    last_continue: Option<Instant>,
    ready_seen_since: Option<Instant>,
    // menu navigation (mode menu, Team Fill panel)
    fill_checked: bool,
    fill_clicks: u32,
    panel_opens: u32,
    nav_last: Option<Instant>,
    nav_tries: u32,
    esc_backouts: u32,
    lobby_yellow_since: Option<Instant>,
    // recovery, sleep mode, anti-AFK movement
    recoveries: u32,
    sleep_last: Option<Instant>,
    anti_afk: bool,
    move_flag: Arc<AtomicBool>,
    own_hidden: bool,
    started_until: Option<Instant>,
    focus_warned: bool,
    display: GameState,
    display_since: Instant,
    stats: Stats,
}

impl Engine {
    pub fn new(cfg: Config, shared: Arc<Mutex<Shared>>, log: Logger, rx: Receiver<Cmd>) -> Self {
        let vision = Vision::new(cfg.clone());
        let cfg_anti_afk = cfg.anti_afk;
        shared.lock().anti_afk = cfg_anti_afk;
        let move_flag = Arc::new(AtomicBool::new(false));
        mover::spawn(move_flag.clone());
        let e = Self {
            cfg,
            vision,
            shared,
            log,
            rx,
            history: VecDeque::new(),
            confirmed: Raw::Unknown,
            last_frame: None,
            no_window: 0,
            grab_fail: 0,
            prev_fp: Vec::new(),
            static_since: None,
            healthy_ticks: 0,
            pause_auto: false,
            running: false,
            in_match: false,
            post_match: false,
            end_counted: false,
            launch_pending: false,
            cycle_armed: false,
            click: Click::Idle,
            attempts: 0,
            ready_stable: 0,
            prev_patch: Vec::new(),
            pre_press_patch: Vec::new(),
            press_target: Target::Lobby,
            play_again_failed: false,
            results_since: None,
            post_match_since: None,
            continue_clicks: 0,
            last_continue: None,
            ready_seen_since: None,
            fill_checked: false,
            fill_clicks: 0,
            panel_opens: 0,
            nav_last: None,
            nav_tries: 0,
            esc_backouts: 0,
            lobby_yellow_since: None,
            recoveries: 0,
            sleep_last: None,
            anti_afk: cfg_anti_afk,
            move_flag: move_flag.clone(),
            own_hidden: false,
            started_until: None,
            focus_warned: false,
            display: GameState::NoGame,
            display_since: Instant::now(),
            stats: Stats::default(),
        };
        e.publish_templates();
        e
    }

    fn publish_templates(&self) {
        let t = Sig::ALL.iter().map(|s| (*s, self.vision.has_template(*s))).collect();
        self.shared.lock().templates = t;
    }

    pub fn run(mut self) {
        self.log.info("NOVA AFK запущен. Остановка автоматизации — клавиша F10.");
        let poll = Duration::from_millis(self.cfg.poll_ms);
        let mut next_tick = Instant::now();
        let mut f10_prev = false;
        loop {
            match self.rx.recv_timeout(Duration::from_millis(40)) {
                Ok(c) => self.command(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let f10 = input::stop_hotkey_down();
            if f10 && !f10_prev && self.running {
                self.log.action("F10: автоматизация остановлена пользователем");
                self.stop();
            }
            f10_prev = f10;
            if Instant::now() >= next_tick {
                self.tick();
                next_tick = Instant::now() + poll;
            }
        }
    }

    fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Start => {
                if !self.running {
                    self.running = true;
                    self.reset_flow();
                    let mut s = self.shared.lock();
                    s.running = true;
                    s.paused = None;
                    s.run_started = Some(Instant::now());
                    drop(s);
                    input::keep_awake(true);
                    self.log.action("Старт автоматизации (спящий режим Windows отключён до остановки)");
                }
            }
            Cmd::Stop => {
                if self.running {
                    self.log.action("Стоп автоматизации");
                    self.stop();
                }
            }
            Cmd::ResetStats => {
                self.stats = Stats::default();
                self.shared.lock().stats = self.stats;
                self.log.info("Счётчики сброшены");
            }
            Cmd::Capture(sig) => match self.last_frame.as_ref() {
                Some(f) => match self.vision.save_template(f, sig) {
                    Ok(()) => {
                        self.log.action(format!("Эталон «{}» сохранён (templates/{}.png)", sig.title(), sig.file()));
                        self.publish_templates();
                    }
                    Err(e) => self.log.error(format!("Не удалось сохранить эталон: {e}")),
                },
                None => self.log.warn("Нет кадра Fortnite для сохранения эталона"),
            },
            Cmd::SetAntiAfk(on) => {
                self.anti_afk = on;
                self.shared.lock().anti_afk = on;
                self.log.action(if on { "Движение в матче включено" } else { "Движение в матче выключено" });
                if !on {
                    self.set_moving(false);
                }
            }
            Cmd::Delete(sig) => {
                self.vision.delete_template(sig);
                self.log.action(format!("Эталон «{}» удалён", sig.title()));
                self.publish_templates();
            }
        }
    }

    fn stop(&mut self) {
        self.running = false;
        self.set_moving(false);
        input::keep_awake(false);
        let mut s = self.shared.lock();
        s.running = false;
        s.paused = None;
        s.status = "Остановлено".into();
    }

    fn reset_flow(&mut self) {
        self.history.clear();
        self.in_match = false;
        self.post_match = false;
        self.end_counted = false;
        self.launch_pending = false;
        self.cycle_armed = false;
        self.click = Click::Idle;
        self.attempts = 0;
        self.ready_stable = 0;
        self.ready_seen_since = None;
        self.fill_checked = false;
        self.fill_clicks = 0;
        self.panel_opens = 0;
        self.nav_last = None;
        self.nav_tries = 0;
        self.esc_backouts = 0;
        self.lobby_yellow_since = None;
        self.recoveries = 0;
        self.static_since = None;
        self.healthy_ticks = 0;
        self.display_since = Instant::now();
    }

    // ------------------------------------------------------------------ tick

    fn tick(&mut self) {
        if self.cfg.hide_from_capture && !self.own_hidden && capture::exclude_own_windows() > 0 {
            self.own_hidden = true;
            self.log.info("Окно NOVA AFK скрыто от захвата — оно не мешает распознаванию");
        }
        let Some(win) = capture::find_fortnite() else {
            self.set_moving(false);
            self.no_window += 1;
            self.last_frame = None;
            self.healthy_ticks = 0;
            self.set_display(GameState::NoGame);
            if self.no_window >= self.cfg.no_window_limit {
                self.unhealthy("Окно Fortnite не найдено (или свёрнуто)");
            }
            self.publish("Ожидание окна Fortnite", false, String::new());
            return;
        };
        self.no_window = 0;

        if win.is_hung() {
            self.set_moving(false);
            self.healthy_ticks = 0;
            self.unhealthy("Fortnite не отвечает (зависание)");
            self.publish("Fortnite не отвечает", true, String::new());
            return;
        }

        let frame = match capture::grab(&win) {
            Ok(f) => f,
            Err(e) => {
                self.set_moving(false);
                self.grab_fail += 1;
                self.healthy_ticks = 0;
                if self.grab_fail >= self.cfg.capture_fail_limit {
                    self.unhealthy(&format!("Потеряно изображение: {e}"));
                }
                self.publish("Нет изображения", true, String::new());
                return;
            }
        };
        self.grab_fail = 0;

        // Frozen picture detection.
        let fp = vision::fingerprint(&frame);
        let frozen = if vision::patch_diff(&fp, &self.prev_fp) < 0.0008 {
            let since = *self.static_since.get_or_insert_with(Instant::now);
            let limit = if self.in_match {
                self.cfg.static_frame_timeout_in_match_s
            } else {
                self.cfg.static_frame_timeout_s
            };
            since.elapsed().as_secs() >= limit
        } else {
            self.static_since = None;
            false
        };
        self.prev_fp = fp;
        if frozen {
            self.healthy_ticks = 0;
            self.unhealthy("Изображение не меняется (игра зависла?)");
        } else {
            self.healthy_ticks += 1;
        }

        let obs = self.vision.observe(&frame);
        self.last_frame = Some(frame);
        let raw = obs.classify();
        self.history.push_back(raw);
        while self.history.len() > self.cfg.confirm_window {
            self.history.pop_front();
        }
        let votes = self.history.iter().filter(|r| **r == raw).count();
        if votes >= self.cfg.confirm_needed.min(self.history.len().max(1))
            && self.history.len() >= self.cfg.confirm_needed
            && raw != self.confirmed
        {
            self.log.info(format!("Экран: {:?} -> {:?}  [{}]", self.confirmed, raw, obs.summary()));
            self.confirmed = raw;
        }
        let debug = format!("{:?} | {} | luma {:.2}", self.confirmed, obs.summary(), obs.mean_luma);

        // Auto-resume after a recoverable problem.
        let paused = self.shared.lock().paused.is_some();
        if paused && self.pause_auto && self.healthy_ticks >= 3 {
            self.log.action("Состояние восстановлено — автоматизация продолжена");
            self.shared.lock().paused = None;
            self.click = if self.click == Click::Done { Click::Done } else { Click::Idle };
        }
        let paused = self.shared.lock().paused.is_some();

        if !self.running {
            let g = match self.confirmed {
                Raw::Hud => GameState::InMatch,
                Raw::Results => GameState::Results,
                Raw::Searching => GameState::Searching,
                Raw::Ready => GameState::LobbyReady,
                Raw::Lobby => GameState::LobbyLoading,
                Raw::Loading => GameState::LoadingMatch,
                Raw::Unknown => GameState::Unknown,
                Raw::Panel => GameState::ModeSettings,
                Raw::Discover => GameState::ModeSelect,
                Raw::Sleep => GameState::Sleep,
            };
            self.set_display(g);
            self.set_moving(false);
            self.publish("Остановлено", false, debug);
            return;
        }

        // Fortnite's sleep mode is static and would also trip the frozen-
        // picture pause, so it is handled even while paused.
        if self.confirmed == Raw::Sleep {
            self.set_moving(false);
            let status = self.wake_up(&obs, &win);
            self.publish(&status, false, debug);
            return;
        }

        if paused {
            self.set_moving(false);
            self.publish_status_only(debug);
            return;
        }

        let status = self.advance(self.confirmed, &obs, &win);

        // No-progress watchdog (not a detector, only a safety net): reset the
        // cycle instead of stopping, so a night of farming does not end here.
        let in_match = matches!(self.display, GameState::InMatch | GameState::MatchStarted);
        if !in_match && self.display_since.elapsed().as_secs() >= self.cfg.stuck_timeout_s {
            self.recover(&format!("нет прогресса в состоянии «{}»", self.display.label()), &win);
        }

        let move_now = self.anti_afk && in_match && win.is_foreground();
        self.set_moving(move_now);
        self.publish(&status, false, debug);
    }

    /// Press KEEP PLAYING on Fortnite's sleep-mode screen.
    fn wake_up(&mut self, obs: &Observation, win: &GameWindow) -> String {
        self.set_display(GameState::Sleep);
        let due = self.sleep_last.map_or(true, |t| t.elapsed().as_secs() >= 4);
        if due && self.ensure_focus(win) {
            self.sleep_last = Some(Instant::now());
            if let Some((x, y)) = obs.sleep_button {
                self.click_point(win, Roi::new(x - 0.001, y - 0.001, 0.002, 0.002), "KEEP PLAYING (выход из спящего режима)");
            }
            // Keep the watchdog from counting the sleep as "no progress".
            self.display_since = Instant::now();
        }
        "Fortnite в спящем режиме — нажимаю KEEP PLAYING".into()
    }

    /// Reset the cycle after a failure; pause only if that keeps failing.
    fn recover(&mut self, reason: &str, win: &GameWindow) {
        self.recoveries += 1;
        if self.recoveries > 3 {
            self.pause_auto = false;
            self.pause(&format!("{reason} (3 попытки восстановления не помогли)"));
            return;
        }
        self.log.warn(format!("Восстановление {}/3: {reason} — начинаю цикл заново", self.recoveries));
        if matches!(self.display, GameState::Unknown | GameState::ModeSettings | GameState::Exiting) && self.ensure_focus(win) {
            input::press_key(SCAN_ESC);
            self.log.action("Esc — выход с непонятного экрана");
        }
        self.click = Click::Idle;
        self.attempts = 0;
        self.launch_pending = false;
        self.fill_checked = false;
        self.fill_clicks = 0;
        self.panel_opens = 0;
        self.nav_tries = 0;
        self.esc_backouts = 0;
        self.play_again_failed = false;
        self.ready_stable = 0;
        self.ready_seen_since = None;
        self.lobby_yellow_since = None;
        self.display_since = Instant::now();
    }

    fn set_moving(&mut self, on: bool) {
        if self.move_flag.swap(on, Ordering::Relaxed) != on {
            self.log.info(if on { "Движение персонажа: старт" } else { "Движение персонажа: стоп" });
            self.shared.lock().moving = on;
        }
    }

    // ------------------------------------------------------------------ flow

    fn advance(&mut self, raw: Raw, obs: &Observation, win: &GameWindow) -> String {
        let now = Instant::now();
        let mut lost_hud = false;
        let display;
        let status;
        if raw != Raw::Results {
            self.results_since = None;
        }
        if raw != Raw::Lobby {
            self.lobby_yellow_since = None;
        }
        if raw != Raw::Unknown {
            self.esc_backouts = 0;
        }

        if raw == Raw::Hud {
            if !self.in_match {
                self.in_match = true;
                if self.post_match && !self.end_counted {
                    self.log.info("HUD вернулся — завершение матча не подтверждено");
                }
                self.post_match = false;
                self.end_counted = false;
                self.play_again_failed = false;
                // Team Fill is verified again before the next start.
                self.fill_checked = false;
                self.recoveries = 0;
                self.panel_opens = 0;
                self.nav_tries = 0;
                self.click = Click::Idle;
                self.attempts = 0;
                self.ready_stable = 0;
                self.ready_seen_since = None;
                if self.launch_pending {
                    self.launch_pending = false;
                    self.stats.launches += 1;
                    if self.cycle_armed {
                        self.stats.cycles += 1;
                        self.cycle_armed = false;
                    }
                    self.started_until = Some(now + Duration::from_secs(5));
                    self.log.action(format!("Новая катка запущена (успешных запусков: {})", self.stats.launches));
                } else {
                    self.log.info("Идёт матч — режим ожидания");
                }
            }
            display = if self.started_until.map_or(false, |t| now < t) { GameState::MatchStarted } else { GameState::InMatch };
            status = "Ожидание завершения матча".into();
        } else {
            if self.in_match {
                self.in_match = false;
                self.post_match = true;
                self.post_match_since = Some(now);
                self.continue_clicks = 0;
                self.last_continue = None;
                self.end_counted = false;
                lost_hud = true;
                self.log.info("HUD матча пропал — проверяю завершение");
            }
            match raw {
                Raw::Results => {
                    self.count_end();
                    self.results_since.get_or_insert(now);
                    let (d, s) = self.on_button(Target::PlayAgain, obs, win, now);
                    display = d;
                    status = s;
                }
                Raw::Searching => {
                    self.count_end();
                    self.post_match = false;
                    self.on_search(now);
                    display = GameState::Searching;
                    status = "Поиск матча идёт — повторных нажатий нет".into();
                }
                Raw::Ready => {
                    self.count_end();
                    self.post_match = false;
                    self.play_again_failed = false;
                    let (d, s) = self.on_button(Target::Lobby, obs, win, now);
                    display = d;
                    status = s;
                }
                Raw::Panel => {
                    self.count_end();
                    self.post_match = false;
                    let (d, s) = self.on_panel(obs, win, now);
                    display = d;
                    status = s;
                }
                Raw::Discover => {
                    self.count_end();
                    self.post_match = false;
                    let (d, s) = self.on_discover(obs, win, now);
                    display = d;
                    status = s;
                }
                Raw::Lobby => {
                    self.count_end();
                    self.post_match = false;
                    self.ready_stable = 0;
                    if let Click::Clicked(_) = self.click {
                        // The PLAY button changed after our press.
                        self.log.info("Кнопка изменилась после нажатия — поиск начался");
                        self.on_search(now);
                    }
                    display = GameState::LobbyLoading;
                    status = "Лобби: жду обычную кнопку PLAY".into();
                    // Yellow button that never looks like the idle PLAY: after a
                    // long while try to start anyway instead of waiting forever.
                    if obs.button_yellow && self.click == Click::Idle {
                        let since = *self.lobby_yellow_since.get_or_insert(now);
                        if since.elapsed().as_secs() >= 240 {
                            self.log.warn("Кнопка PLAY 4 минуты не похожа на обычную — пробую запуск");
                            self.lobby_yellow_since = None;
                            self.ready_stable = self.cfg.ready_stable_needed as u32;
                            let (d, s) = self.on_button(Target::Lobby, obs, win, now);
                            self.set_display(d);
                            return s;
                        }
                    }
                }
                Raw::Loading | Raw::Unknown => {
                    if matches!(self.click, Click::Clicked(_)) && !self.launch_pending {
                        self.log.info("Кнопка исчезла после нажатия — считаю, что поиск начался");
                        self.on_search(now);
                    }
                    self.ready_stable = 0;
                    if lost_hud {
                        display = GameState::MatchEnded;
                        status = "Матч завершён".into();
                    } else if self.launch_pending {
                        display = GameState::LoadingMatch;
                        status = "Загрузка новой катки".into();
                    } else if self.post_match {
                        display = GameState::Exiting;
                        status = "Выход из матча / загрузка лобби".into();
                        // Elimination screen waits for CONTINUE; press it if
                        // nothing else shows up for a while.
                        if raw == Raw::Unknown
                            && self.post_match_since.map_or(false, |t| t.elapsed().as_secs() >= self.cfg.continue_after_s)
                            && self.last_continue.map_or(true, |t| t.elapsed().as_secs() >= 10)
                            && self.continue_clicks < 3
                            && self.ensure_focus(win)
                        {
                            self.continue_clicks += 1;
                            self.last_continue = Some(now);
                            self.click_point(win, self.cfg.continue_roi, "CONTINUE");
                        }
                    } else {
                        display = if raw == Raw::Loading { GameState::LoadingMatch } else { GameState::Unknown };
                        status = "Определяю состояние".into();
                        // Our menu click led to an unknown screen (e.g. a mode
                        // details page): back out with Esc.
                        if raw == Raw::Unknown
                            && self.nav_last.map_or(false, |t| (10..90).contains(&t.elapsed().as_secs()))
                            && self.esc_backouts < 3
                            && self.ensure_focus(win)
                        {
                            self.esc_backouts += 1;
                            self.nav_last = Some(now);
                            input::press_key(SCAN_ESC);
                            self.log.action("Неизвестный экран после навигации — Esc");
                        }
                    }
                }
                Raw::Hud => unreachable!(),
                // Handled in `tick` before the flow runs.
                Raw::Sleep => {
                    display = GameState::Sleep;
                    status = "Fortnite в спящем режиме".into();
                }
            }
        }
        self.set_display(display);
        status
    }

    fn count_end(&mut self) {
        if self.post_match && !self.end_counted {
            self.end_counted = true;
            self.cycle_armed = true;
            self.stats.matches_ended += 1;
            self.log.action(format!("Матч завершён (всего завершений: {})", self.stats.matches_ended));
        }
    }

    fn on_search(&mut self, _now: Instant) {
        if !self.launch_pending {
            self.launch_pending = true;
            self.log.action("Поиск матча начался");
        }
        if matches!(self.click, Click::Clicked(_)) {
            self.log.info("Начало поиска подтверждено после нажатия");
        }
        self.click = Click::Done;
        self.attempts = 0;
        self.ready_seen_since = None;
    }

    /// Handles a start button that is on screen: the lobby PLAY button or the
    /// PLAY AGAIN button of the match recap.
    fn on_button(&mut self, t: Target, obs: &Observation, win: &GameWindow, now: Instant) -> (GameState, String) {
        let patch = match t {
            Target::Lobby => &obs.button_patch,
            Target::PlayAgain => &obs.play_again_patch,
        };
        let shown = match t {
            Target::Lobby => GameState::LobbyReady,
            Target::PlayAgain => GameState::Results,
        };
        match self.click {
            Click::Done => {
                // Re-arm only when the button looks exactly like before our
                // press again; a search in progress changes it, and pressing
                // then would cancel the search.
                let same_button = self.press_target == t && !self.pre_press_patch.is_empty();
                let idle_look = same_button && vision::patch_diff(patch, &self.pre_press_patch) <= self.cfg.ready_stable_diff;
                if same_button && !idle_look {
                    self.ready_seen_since = None;
                    return (GameState::Searching, "Поиск матча идёт — повторных нажатий нет".into());
                }
                // The idle PLAY (or PLAY AGAIN) is on screen and unchanged:
                // nothing is running, start again after a short check.
                let wait = self.cfg.rearm_after_s;
                let since = *self.ready_seen_since.get_or_insert(now);
                if since.elapsed().as_secs() >= wait {
                    self.log.warn("Кнопка запуска снова в исходном виде — поиск не идёт; перезапускаю цикл");
                    self.click = Click::Idle;
                    self.launch_pending = false;
                    self.ready_stable = 0;
                    self.ready_seen_since = None;
                }
                (shown, "Проверяю, идёт ли поиск".into())
            }
            Click::Clicked(_) if self.press_target != t => {
                // The press moved us to another screen: it worked.
                self.log.info("После нажатия открылся другой экран");
                self.on_search(now);
                (GameState::Searching, "Поиск матча идёт — повторных нажатий нет".into())
            }
            Click::Clicked(at) => {
                if vision::patch_diff(patch, &self.pre_press_patch) > self.cfg.press_change_diff {
                    self.log.info("Кнопка изменилась после нажатия — поиск начался");
                    self.on_search(now);
                    return (GameState::Searching, "Поиск матча идёт — повторных нажатий нет".into());
                }
                if at.elapsed().as_secs() < self.cfg.click_verify_timeout_s {
                    return (shown, "Проверяю, что поиск начался".into());
                }
                if self.attempts >= self.cfg.max_click_retries {
                    if t == Target::PlayAgain {
                        self.log.warn("PLAY AGAIN не сработала — возвращаюсь в лобби");
                        self.click_point(win, self.cfg.return_lobby_roi, "RETURN TO LOBBY");
                        self.play_again_failed = true;
                        self.click = Click::Idle;
                        self.attempts = 0;
                        return (GameState::Exiting, "Возврат в лобби".into());
                    }
                    self.recover("поиск не запустился после нескольких нажатий", win);
                    return (shown, "Восстановление: нет реакции на нажатие".into());
                }
                self.stats.retries += 1;
                self.log.warn(format!("Поиск не начался за {} с — повторная попытка", self.cfg.click_verify_timeout_s));
                self.click = Click::Idle;
                self.ready_stable = 0;
                (shown, "Повторная проверка кнопки".into())
            }
            Click::Idle => {
                if t == Target::PlayAgain && self.play_again_failed {
                    return (GameState::Exiting, "Жду возврата в лобби".into());
                }
                // The interface must be settled: button visible and unchanged.
                let calm = vision::patch_diff(patch, &self.prev_patch) <= self.cfg.ready_stable_diff;
                self.prev_patch = patch.clone();
                self.ready_stable = if calm { self.ready_stable + 1 } else { 0 };
                let settling = t == Target::PlayAgain
                    && self.results_since.map_or(true, |s| s.elapsed().as_secs() < self.cfg.results_settle_s);
                if settling || (self.ready_stable as usize) < self.cfg.ready_stable_needed {
                    let s = if t == Target::Lobby { GameState::LobbyLoading } else { GameState::Results };
                    return (s, "Жду полной загрузки интерфейса".into());
                }
                // Before starting from the lobby, make sure "Team Fill" is on.
                if t == Target::Lobby && self.cfg.enforce_fill && !self.fill_checked {
                    if self.panel_opens >= 3 {
                        self.log.warn("Меню настроек режима не открылось — запускаю без проверки Fill");
                        self.fill_checked = true;
                    } else {
                        if self.nav_ready() && self.ensure_focus(win) {
                            self.panel_opens += 1;
                            self.nav_last = Some(now);
                            self.click_point(win, self.cfg.mode_button_roi, "настройки режима (проверка Team Fill)");
                        }
                        return (GameState::LobbyReady, "Проверяю Team Fill".into());
                    }
                }
                if self.try_press(t, win) {
                    (shown, "Кнопка нажата, проверяю поиск".into())
                } else {
                    (shown, "Кнопка доступна, жду возможности нажать".into())
                }
            }
        }
    }

    /// Menu actions are spaced out so each one can take effect.
    fn nav_ready(&self) -> bool {
        self.nav_last.map_or(true, |t| t.elapsed() >= Duration::from_millis(2500))
    }

    /// Mode settings panel: switch "Team Fill" on, then close the panel.
    fn on_panel(&mut self, obs: &Observation, win: &GameWindow, now: Instant) -> (GameState, String) {
        self.panel_opens = 0;
        if !self.nav_ready() || !self.ensure_focus(win) {
            return (GameState::ModeSettings, "Жду реакции меню".into());
        }
        let fill_ok = obs.fill_on.hit || !self.cfg.enforce_fill;
        if !fill_ok && self.fill_clicks < 3 {
            self.fill_clicks += 1;
            self.nav_last = Some(now);
            self.click_point(win, self.cfg.fill_toggle_roi, "Team Fill (включаю заполнение)");
            return (GameState::ModeSettings, "Включаю Team Fill".into());
        }
        if fill_ok {
            if !self.fill_checked {
                self.log.info("Team Fill включён");
            }
        } else {
            self.log.warn("Не удалось включить Team Fill — продолжаю без него");
        }
        self.fill_checked = true;
        self.fill_clicks = 0;
        self.nav_last = Some(now);
        // Close: Esc first, the DONE button if Esc did not work.
        self.nav_tries += 1;
        if self.nav_tries % 2 == 1 {
            input::press_key(SCAN_ESC);
            self.log.action("Закрываю настройки режима (Esc)");
        } else {
            self.click_point(win, self.cfg.panel_done_roi, "DONE");
        }
        (GameState::ModeSettings, "Team Fill проверен, закрываю меню".into())
    }

    /// Mode menu: make sure Battle Royale is selected, then press its PLAY.
    fn on_discover(&mut self, obs: &Observation, win: &GameWindow, now: Instant) -> (GameState, String) {
        self.click = Click::Idle;
        if !self.nav_ready() || !self.ensure_focus(win) {
            return (GameState::ModeSelect, "Жду реакции меню".into());
        }
        if obs.hero_br >= 0.6 {
            self.nav_last = Some(now);
            self.nav_tries = 0;
            self.fill_checked = false;
            self.log.info(format!("В меню режимов выбран Battle Royale ({:.2})", obs.hero_br));
            self.click_point(win, self.cfg.hero_play_roi, "PLAY (Battle Royale)");
            return (GameState::ModeSelect, "Battle Royale выбран".into());
        }
        if self.nav_tries >= 4 {
            self.pause_auto = false;
            self.pause("Не удалось выбрать Battle Royale в меню режимов");
            return (GameState::ModeSelect, "Пауза: Battle Royale не найден".into());
        }
        self.nav_tries += 1;
        self.nav_last = Some(now);
        let tile = self.last_frame.as_ref().and_then(|f| self.vision.find_br_tile(f));
        match tile {
            Some((roi, score)) => {
                self.click_point(win, roi, &format!("плитка Battle Royale ({score:.2})"));
            }
            None => {
                self.log.warn("Плитка Battle Royale не опознана — кликаю первую в ряду");
                self.click_point(win, self.cfg.tile_roi, "первая плитка");
            }
        }
        (GameState::ModeSelect, "Выбираю Battle Royale".into())
    }

    /// Make sure Fortnite has focus before any input.
    fn ensure_focus(&mut self, win: &GameWindow) -> bool {
        if input::stop_hotkey_down() {
            return false;
        }
        if !win.is_foreground() {
            win.try_focus();
            std::thread::sleep(Duration::from_millis(350));
            if !win.is_foreground() {
                if !self.focus_warned {
                    self.log.warn("Fortnite не в фокусе — нажатие отложено");
                    self.focus_warned = true;
                }
                return false;
            }
        }
        self.focus_warned = false;
        true
    }

    /// Click the centre of a region and park the cursor away from it (so the
    /// hover highlight does not look like a changed button).
    fn click_point(&mut self, win: &GameWindow, roi: Roi, name: &str) -> bool {
        let (nx, ny) = roi.center();
        let (x, y) = win.screen_point(nx, ny);
        let ok = input::click_at(x, y);
        let (px, py) = win.screen_point(0.5, 0.3);
        input::move_to(px, py);
        if ok {
            self.stats.clicks += 1;
            self.log.action(format!("Клик: {name} (экран {x},{y})"));
        } else {
            self.log.error(format!("SendInput не выполнил клик: {name}"));
        }
        ok
    }

    /// One guarded press: focus check, fresh re-verification, single press.
    fn try_press(&mut self, t: Target, win: &GameWindow) -> bool {
        if !self.ensure_focus(win) {
            return false;
        }
        // Re-verify on a fresh frame right before pressing.
        let Ok(fresh) = capture::grab(win) else { return false };
        let obs = self.vision.observe(&fresh);
        let still = match t {
            Target::Lobby => obs.ready.hit && !obs.searching.hit && !obs.results.hit,
            Target::PlayAgain => obs.results.hit,
        };
        if !still || obs.hud.hit {
            self.log.warn("Перед нажатием кнопка уже не подтверждена — нажатие отменено");
            self.ready_stable = 0;
            return false;
        }
        self.press_target = t;
        self.pre_press_patch = match t {
            Target::Lobby => obs.button_patch.clone(),
            Target::PlayAgain => obs.play_again_patch.clone(),
        };
        // Lobby: even attempts use the P shortcut, odd ones a mouse click.
        let ok = if t == Target::Lobby && self.cfg.use_start_key && self.attempts % 2 == 0 {
            let ok = input::press_key(self.cfg.start_key_scancode);
            if ok {
                self.stats.clicks += 1;
                self.log.action(format!("Нажата клавиша P — старт поиска (попытка {})", self.attempts + 1));
            }
            ok
        } else {
            let (roi, name) = match t {
                Target::Lobby => (self.cfg.button_roi, "PLAY"),
                Target::PlayAgain => (self.cfg.play_again_roi, "PLAY AGAIN"),
            };
            self.click_point(win, roi, &format!("{name} (попытка {})", self.attempts + 1))
        };
        if ok {
            self.attempts += 1;
            self.click = Click::Clicked(Instant::now());
        }
        ok
    }

    // --------------------------------------------------------------- helpers

    fn unhealthy(&mut self, reason: &str) {
        if self.running && self.shared.lock().paused.is_none() {
            self.pause_auto = true;
            self.pause(reason);
        }
    }

    fn pause(&mut self, reason: &str) {
        let mut s = self.shared.lock();
        if s.paused.is_none() {
            s.paused = Some(reason.to_string());
            drop(s);
            self.log.warn(format!("ПАУЗА: {reason}"));
        }
    }

    fn set_display(&mut self, g: GameState) {
        if g != self.display {
            self.display = g;
            self.display_since = Instant::now();
        }
    }

    fn publish(&self, status: &str, _problem: bool, debug: String) {
        let mut s = self.shared.lock();
        s.game = self.display;
        s.window_found = self.no_window == 0;
        s.stats = self.stats;
        if self.running {
            s.status = match &s.paused {
                Some(r) => format!("Пауза: {r}"),
                None => status.to_string(),
            };
        }
        if !debug.is_empty() {
            s.debug = debug;
        }
    }

    fn publish_status_only(&self, debug: String) {
        let mut s = self.shared.lock();
        s.window_found = true;
        s.debug = debug;
        if let Some(r) = &s.paused {
            s.status = format!("Пауза: {r}");
        }
    }
}
