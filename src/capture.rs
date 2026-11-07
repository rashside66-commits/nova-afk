//! Screen capture of the already running Fortnite window (plain GDI, no
//! interaction with the game process). Fortnite must run in windowed or
//! "windowed fullscreen" mode so the desktop compositor contains its image.

use anyhow::{bail, Result};
use windows::core::BOOL;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub struct Frame {
    pub w: usize,
    pub h: usize,
    /// BGRA, top-down.
    pub data: Vec<u8>,
}

#[derive(Clone, Copy)]
pub struct GameWindow {
    hwnd: isize,
    /// Client area in virtual-screen pixels.
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl GameWindow {
    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    pub fn is_hung(&self) -> bool {
        unsafe { IsHungAppWindow(self.hwnd()).as_bool() }
    }

    pub fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd() }
    }

    /// Bring Fortnite to the front. Windows refuses SetForegroundWindow to a
    /// background process unless it just received input, so Alt is held
    /// during the call (the usual workaround; released into Fortnite).
    pub fn try_focus(&self) {
        unsafe {
            crate::input::key_down(0x38);
            let _ = SetForegroundWindow(self.hwnd());
            let _ = BringWindowToTop(self.hwnd());
            crate::input::key_up(0x38);
        }
    }

    /// Screen position of a normalised point inside the client area.
    pub fn screen_point(&self, nx: f32, ny: f32) -> (i32, i32) {
        (self.x + (nx * self.w as f32) as i32, self.y + (ny * self.h as f32) as i32)
    }
}

unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let found = &mut *(lparam.0 as *mut Option<isize>);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut cls = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut cls);
    let cls = String::from_utf16_lossy(&cls[..n.max(0) as usize]);
    let mut title = [0u16; 128];
    let n = GetWindowTextW(hwnd, &mut title);
    let title = String::from_utf16_lossy(&title[..n.max(0) as usize]);
    if cls == "UnrealWindow" && title.trim().eq_ignore_ascii_case("fortnite") {
        *found = Some(hwnd.0 as isize);
        return BOOL(0);
    }
    BOOL(1)
}

/// Locate the running Fortnite window (class `UnrealWindow`, title "Fortnite").
pub fn find_fortnite() -> Option<GameWindow> {
    let mut found: Option<isize> = None;
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut found as *mut _ as isize));
    }
    let hwnd = HWND(found? as *mut _);
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return None;
        }
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc).ok()?;
        let mut pt = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut pt).as_bool() {
            return None;
        }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        if w < 320 || h < 240 {
            return None;
        }
        Some(GameWindow { hwnd: hwnd.0 as isize, x: pt.x, y: pt.y, w, h })
    }
}

unsafe extern "system" fn hide_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let count = &mut *(lparam.0 as *mut u32);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, Some(&mut pid));
    if pid == windows::Win32::System::Threading::GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() {
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok() {
            *count += 1;
        }
    }
    BOOL(1)
}

/// Make our own windows invisible to screen capture, so the NOVA AFK window
/// lying on top of Fortnite does not hide the game from recognition.
/// Returns the number of windows excluded.
pub fn exclude_own_windows() -> u32 {
    let mut count = 0u32;
    unsafe {
        let _ = EnumWindows(Some(hide_cb), LPARAM(&mut count as *mut _ as isize));
    }
    count
}

pub fn grab(win: &GameWindow) -> Result<Frame> {
    unsafe {
        let screen = GetDC(None);
        if screen.is_invalid() {
            bail!("GetDC failed");
        }
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, win.w, win.h);
        let old = SelectObject(mem, bmp.into());
        let blt = BitBlt(mem, 0, 0, win.w, win.h, Some(screen), win.x, win.y, SRCCOPY | CAPTUREBLT);

        let mut bi = BITMAPINFO::default();
        bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.bmiHeader.biWidth = win.w;
        bi.bmiHeader.biHeight = -win.h; // top-down
        bi.bmiHeader.biPlanes = 1;
        bi.bmiHeader.biBitCount = 32;
        bi.bmiHeader.biCompression = BI_RGB.0;
        let mut data = vec![0u8; (win.w * win.h * 4) as usize];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            win.h as u32,
            Some(data.as_mut_ptr() as *mut _),
            &mut bi,
            DIB_RGB_COLORS,
        );

        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);

        if blt.is_err() || lines == 0 {
            bail!("BitBlt/GetDIBits failed");
        }
        Ok(Frame { w: win.w as usize, h: win.h as usize, data })
    }
}
