//! Anti-AFK movement during a match: run, strafe, jump, turn the camera and
//! swing the pickaxe in random order. Runs on its own thread and acts only
//! while the engine raises the `enabled` flag (match HUD visible, Fortnite in
//! focus, automation running); every key is released as soon as it drops.

use crate::input;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

// Hardware scan codes (layout independent).
const W: u16 = 0x11;
const A: u16 = 0x1E;
const S: u16 = 0x1F;
const D: u16 = 0x20;
const SPACE: u16 = 0x39;
const SHIFT: u16 = 0x2A;
const SLOT_PICKAXE: u16 = 0x02; // "1"

const ALL_KEYS: [u16; 6] = [W, A, S, D, SPACE, SHIFT];

/// Small xorshift generator: no dependency needed for "random enough".
struct Rng(u64);

impl Rng {
    fn new() -> Self {
        let seed = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
        Rng(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        lo + self.next() % (hi - lo).max(1)
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.range(0, 100) < percent
    }
}

struct Mover {
    enabled: Arc<AtomicBool>,
    rng: Rng,
    held: Vec<u16>,
    lmb: bool,
}

impl Mover {
    fn on(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    fn hold(&mut self, k: u16) {
        if !self.held.contains(&k) {
            input::key_down(k);
            self.held.push(k);
        }
    }

    fn release_all(&mut self) {
        for k in self.held.drain(..) {
            input::key_up(k);
        }
        if self.lmb {
            input::mouse_left(false);
            self.lmb = false;
        }
    }

    /// Wait up to `ms`, returning false early if movement got disabled.
    fn wait(&self, ms: u64) -> bool {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            if !self.on() {
                return false;
            }
            sleep(Duration::from_millis(30));
        }
        true
    }

    fn tap(&mut self, k: u16) {
        input::key_down(k);
        sleep(Duration::from_millis(60));
        input::key_up(k);
    }

    /// Smooth camera turn of `dx` pixels over `ms`.
    fn turn(&mut self, dx: i32, dy: i32, ms: u64) -> bool {
        let steps = 12;
        for _ in 0..steps {
            if !self.on() {
                return false;
            }
            input::mouse_rel(dx / steps, dy / steps);
            sleep(Duration::from_millis(ms / steps as u64));
        }
        true
    }

    fn sign(&mut self) -> i32 {
        if self.rng.chance(50) {
            1
        } else {
            -1
        }
    }

    /// Run forward (sometimes sprinting / strafing), jumping now and then.
    fn run(&mut self) {
        self.hold(W);
        if self.rng.chance(50) {
            self.hold(SHIFT);
        }
        if self.rng.chance(30) {
            let side = if self.rng.chance(50) { A } else { D };
            self.hold(side);
        }
        let total = self.rng.range(1200, 3500);
        let start = Instant::now();
        while start.elapsed().as_millis() < total as u128 {
            if self.rng.chance(15) {
                self.tap(SPACE);
            }
            if self.rng.chance(20) {
                let dx = self.sign() * self.rng.range(40, 200) as i32;
                if !self.turn(dx, 0, 200) {
                    break;
                }
            }
            if !self.wait(250) {
                break;
            }
        }
        self.release_all();
    }

    /// Swing the pickaxe: breaks whatever is in front.
    fn swing(&mut self) {
        self.tap(SLOT_PICKAXE);
        if !self.wait(200) {
            return;
        }
        input::mouse_left(true);
        self.lmb = true;
        let total = self.rng.range(1000, 2600);
        if self.rng.chance(50) {
            self.hold(W);
        }
        self.wait(total);
        self.release_all();
    }

    fn look_around(&mut self) {
        let dx = self.sign() * self.rng.range(250, 900) as i32;
        let dy = self.sign() * self.rng.range(0, 120) as i32;
        let ms = self.rng.range(300, 700);
        if self.turn(dx, dy, ms) {
            // Level the view out again.
            self.turn(0, -dy, 200);
        }
    }

    fn strafe_back(&mut self) {
        let k = match self.rng.range(0, 3) {
            0 => S,
            1 => A,
            _ => D,
        };
        self.hold(k);
        let ms = self.rng.range(500, 1500);
        self.wait(ms);
        self.release_all();
    }

    fn step(&mut self) {
        match self.rng.range(0, 100) {
            0..=39 => self.run(),
            40..=64 => self.swing(),
            65..=84 => self.look_around(),
            85..=94 => self.strafe_back(),
            _ => {
                self.tap(SPACE);
            }
        }
        self.release_all();
        let pause = self.rng.range(100, 600);
        self.wait(pause);
    }
}

/// Start the movement thread. It idles while `enabled` is false.
pub fn spawn(enabled: Arc<AtomicBool>) {
    std::thread::Builder::new()
        .name("nova-mover".into())
        .spawn(move || {
            let mut m = Mover { enabled, rng: Rng::new(), held: Vec::new(), lmb: false };
            loop {
                if m.on() {
                    m.step();
                } else {
                    m.release_all();
                    sleep(Duration::from_millis(100));
                }
            }
        })
        .expect("mover thread");
}

/// Release every key the mover may use (e.g. on exit).
pub fn release_everything() {
    for k in ALL_KEYS {
        input::key_up(k);
    }
    input::mouse_left(false);
}
