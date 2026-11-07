//! Action journal: in-memory ring buffer for the UI + append-only file on disk.

use parking_lot::Mutex;
use std::collections::VecDeque;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use windows::Win32::System::SystemInformation::GetLocalTime;

const MAX_LINES: usize = 1500;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Action,
    Warn,
    Error,
}

#[derive(Clone)]
pub struct Line {
    pub level: Level,
    pub text: String,
}

#[derive(Clone)]
pub struct Logger {
    inner: Arc<Mutex<VecDeque<Line>>>,
    path: Arc<PathBuf>,
}

impl Logger {
    pub fn new(path: PathBuf) -> Self {
        Self { inner: Arc::new(Mutex::new(VecDeque::new())), path: Arc::new(path) }
    }

    pub fn log(&self, level: Level, msg: impl AsRef<str>) {
        let t = unsafe { GetLocalTime() };
        let text = format!("[{:02}:{:02}:{:02}] {}", t.wHour, t.wMinute, t.wSecond, msg.as_ref());
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&*self.path) {
            let _ = writeln!(
                f,
                "{:04}-{:02}-{:02} {}",
                t.wYear,
                t.wMonth,
                t.wDay,
                text
            );
        }
        let mut q = self.inner.lock();
        if q.len() >= MAX_LINES {
            q.pop_front();
        }
        q.push_back(Line { level, text });
    }

    pub fn info(&self, m: impl AsRef<str>) {
        self.log(Level::Info, m)
    }
    pub fn action(&self, m: impl AsRef<str>) {
        self.log(Level::Action, m)
    }
    pub fn warn(&self, m: impl AsRef<str>) {
        self.log(Level::Warn, m)
    }
    pub fn error(&self, m: impl AsRef<str>) {
        self.log(Level::Error, m)
    }

    pub fn snapshot(&self) -> Vec<Line> {
        self.inner.lock().iter().cloned().collect()
    }
}
