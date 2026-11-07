//! Ordinary user input only: a synthetic mouse click via SendInput and a
//! hotkey poll. Nothing here touches the game process.

use std::thread::sleep;
use std::time::Duration;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT { dx, dy, mouseData: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn send(i: INPUT) -> bool {
    unsafe { SendInput(&[i], std::mem::size_of::<INPUT>() as i32) == 1 }
}

fn to_absolute(x: i32, y: i32) -> (i32, i32) {
    unsafe {
        let (vx, vy) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (vw, vh) = (GetSystemMetrics(SM_CXVIRTUALSCREEN).max(2), GetSystemMetrics(SM_CYVIRTUALSCREEN).max(2));
        (
            ((x - vx) as i64 * 65535 / (vw - 1) as i64) as i32,
            ((y - vy) as i64 * 65535 / (vh - 1) as i64) as i32,
        )
    }
}

/// Move the cursor to a virtual-screen pixel without clicking.
pub fn move_to(x: i32, y: i32) {
    let (ax, ay) = to_absolute(x, y);
    let _ = send(mouse(ax, ay, MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK | MOUSEEVENTF_MOVE));
}

/// Move the cursor to a virtual-screen pixel and left-click once.
pub fn click_at(x: i32, y: i32) -> bool {
    {
        let (ax, ay) = to_absolute(x, y);
        let abs = MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
        if !send(mouse(ax, ay, abs | MOUSEEVENTF_MOVE)) {
            return false;
        }
        sleep(Duration::from_millis(250));
        // Wiggle a little so the game registers the hover (the button
        // highlights) before the press; the game ignores a "teleported" click.
        for d in [12, -12, 6, 0] {
            let _ = send(mouse(ax + d, ay, abs | MOUSEEVENTF_MOVE));
            sleep(Duration::from_millis(70));
        }
        sleep(Duration::from_millis(300));
        let down = send(mouse(0, 0, MOUSEEVENTF_LEFTDOWN));
        sleep(Duration::from_millis(110));
        let up = send(mouse(0, 0, MOUSEEVENTF_LEFTUP));
        sleep(Duration::from_millis(150));
        down && up
    }
}

fn key(scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: scan, dwFlags: KEYEVENTF_SCANCODE | flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

pub fn key_down(scan: u16) {
    let _ = send(key(scan, KEYBD_EVENT_FLAGS(0)));
}

pub fn key_up(scan: u16) {
    let _ = send(key(scan, KEYEVENTF_KEYUP));
}

/// Relative mouse movement (turns the camera in game).
pub fn mouse_rel(dx: i32, dy: i32) {
    let _ = send(mouse(dx, dy, MOUSEEVENTF_MOVE));
}

pub fn mouse_left(down: bool) {
    let _ = send(mouse(0, 0, if down { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }));
}

/// Press and release one key by hardware scan code (layout independent:
/// 0x19 is the physical P key even with the Russian layout active).
pub fn press_key(scan: u16) -> bool {
    let down = send(key(scan, KEYBD_EVENT_FLAGS(0)));
    sleep(Duration::from_millis(80));
    let up = send(key(scan, KEYEVENTF_KEYUP));
    down && up
}

/// Keep Windows (and the display, which capture needs) from going to sleep
/// while automation runs; `false` restores normal power behaviour.
pub fn keep_awake(on: bool) {
    use windows::Win32::System::Power::*;
    unsafe {
        let flags = if on { ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED } else { ES_CONTINUOUS };
        SetThreadExecutionState(flags);
    }
}

/// True while the stop hotkey (F10) is held.
pub fn stop_hotkey_down() -> bool {
    unsafe { GetAsyncKeyState(VK_F10.0 as i32) < 0 }
}
