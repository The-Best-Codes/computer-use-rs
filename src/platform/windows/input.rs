use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_UNICODE, MAPVK_VK_TO_VSC, MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MapVirtualKeyW,
    SendInput, VIRTUAL_KEY, VK_CONTROL, VK_MENU, VK_RETURN, VK_TAB,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CWP_SKIPDISABLED, CWP_SKIPINVISIBLE, CWP_SKIPTRANSPARENT, ChildWindowFromPointEx, ES_READONLY, GUITHREADINFO,
    GWL_STYLE, GetClassNameW, GetGUIThreadInfo, GetSystemMetrics, GetWindowLongPtrW, GetWindowThreadProcessId,
    IsWindow, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    SMTO_ABORTIFHUNG, SMTO_NORMAL, SendMessageTimeoutW, SetForegroundWindow, WHEEL_DELTA, WM_CHAR, WM_KEYDOWN,
    WM_KEYUP, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MBUTTONDBLCLK, WM_MBUTTONDOWN, WM_MBUTTONUP,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDBLCLK, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
    WM_SYSKEYUP,
};

use super::apps::{foreground_window, main_window, size_u32, window_pid};
use super::hwnd_from_id;
use super::keys::{self, Stroke, VirtualKey};
use crate::backend::{Delivery, InputTarget, MouseButton};
use crate::error::{Error, Result};
use crate::geometry::Point;
use crate::keys::KeyChord;

const POINTS_PER_NOTCH: f64 = 60.0;
const MK_LBUTTON: usize = 0x0001;
const MK_RBUTTON: usize = 0x0002;
const MK_MBUTTON: usize = 0x0010;
const DRAG_STEPS: u32 = 12;

#[derive(Debug, Clone, Copy)]
pub(super) struct Target {
    pub(super) pid: u32,
    pub(super) window: Option<HWND>,
    pub(super) delivery: Delivery,
}

impl Target {
    pub(super) fn from_input(target: InputTarget<'_>) -> Result<Self> {
        let pid = target.app.pid.ok_or_else(|| Error::AppNotFound(format!("{} is not running", target.app.name)))?;
        let window = target.window.and_then(|w| w.id).and_then(hwnd_from_id);
        Ok(Self { pid, window, delivery: target.delivery })
    }

    fn root(&self) -> Option<HWND> {
        self.window
            .filter(|&hwnd| {
                // SAFETY: plain FFI call; any HWND value is accepted.
                unsafe { IsWindow(Some(hwnd)) }.as_bool() && window_pid(hwnd) == self.pid
            })
            .or_else(|| main_window(self.pid))
    }

    fn background_root(&self) -> Result<Option<HWND>> {
        if matches!(self.delivery, Delivery::Global) {
            return Ok(None);
        }
        self.root()
            .map(Some)
            .ok_or_else(|| Error::WindowNotFound(format!("process {} has no window to post input to", self.pid)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Message {
    msg: u32,
    wparam: usize,
    lparam: isize,
}

impl Message {
    const fn new(msg: u32, wparam: usize, lparam: isize) -> Self {
        Self { msg, wparam, lparam }
    }
}

fn post(hwnd: HWND, messages: &[Message]) -> Result<()> {
    for m in messages {
        // SAFETY: plain FFI call; the parameters are plain integers and no pointers are passed.
        unsafe { PostMessageW(Some(hwnd), m.msg, WPARAM(m.wparam), LPARAM(m.lparam)) }
            .map_err(|e| Error::action(format!("posting input to the window failed: {e}")))?;
    }
    Ok(())
}

pub(super) fn click(target: &Target, point: Point, button: MouseButton, count: u32) -> Result<()> {
    let screen = to_pixel(point);
    if let Some(root) = target.background_root()? {
        let hwnd = child_at(root, screen);
        let client = to_client(hwnd, screen);
        let mut messages = vec![Message::new(WM_MOUSEMOVE, 0, make_lparam(client.x, client.y))];
        messages.extend(click_messages(button, count, client));
        return post(hwnd, &messages);
    }
    let (down, up) = match button {
        MouseButton::Left => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
    };
    let mut inputs = vec![move_input(point)];
    for _ in 0..count {
        inputs.push(mouse_input(0, 0, 0, down));
        inputs.push(mouse_input(0, 0, 0, up));
    }
    send(&inputs)
}

fn click_messages(button: MouseButton, count: u32, client: POINT) -> Vec<Message> {
    let (down, double, up, held) = match button {
        MouseButton::Left => (WM_LBUTTONDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONUP, MK_LBUTTON),
        MouseButton::Right => (WM_RBUTTONDOWN, WM_RBUTTONDBLCLK, WM_RBUTTONUP, MK_RBUTTON),
        MouseButton::Middle => (WM_MBUTTONDOWN, WM_MBUTTONDBLCLK, WM_MBUTTONUP, MK_MBUTTON),
    };
    let lparam = make_lparam(client.x, client.y);
    (0..count)
        .flat_map(|i| {
            let press = if i % 2 == 1 { double } else { down };
            [Message::new(press, held, lparam), Message::new(up, 0, lparam)]
        })
        .collect()
}

pub(super) fn scroll(target: &Target, point: Point, dx: f64, dy: f64) -> Result<()> {
    let vertical: Vec<i32> = wheel_steps(dy).into_iter().map(|d| -d).collect();
    let horizontal = wheel_steps(dx);
    let screen = to_pixel(point);
    if let Some(root) = target.background_root()? {
        let hwnd = child_at(root, screen);
        let lparam = make_lparam(screen.x, screen.y);
        let messages: Vec<Message> = vertical
            .iter()
            .map(|&d| Message::new(WM_MOUSEWHEEL, wheel_wparam(d), lparam))
            .chain(horizontal.iter().map(|&d| Message::new(WM_MOUSEHWHEEL, wheel_wparam(d), lparam)))
            .collect();
        return post(hwnd, &messages);
    }
    let mut inputs = vec![move_input(point)];
    inputs.extend(vertical.iter().map(|&d| mouse_input(0, 0, d.cast_unsigned(), MOUSEEVENTF_WHEEL)));
    inputs.extend(horizontal.iter().map(|&d| mouse_input(0, 0, d.cast_unsigned(), MOUSEEVENTF_HWHEEL)));
    send(&inputs)
}

fn wheel_steps(points: f64) -> Vec<i32> {
    let notch = f64::from(WHEEL_DELTA);
    let total = (points / POINTS_PER_NOTCH * notch).round().clamp(-1e6, 1e6);
    let notch_units = i32::try_from(WHEEL_DELTA).unwrap_or(120);
    let mut remaining = round_to_i32(total);
    let mut steps = Vec::new();
    while remaining != 0 {
        let step = remaining.clamp(-notch_units, notch_units);
        steps.push(step);
        remaining -= step;
    }
    steps
}

fn wheel_wparam(delta: i32) -> usize {
    let delta = i16::try_from(delta).unwrap_or(if delta < 0 { i16::MIN } else { i16::MAX });
    usize::from(delta.cast_unsigned()) << 16
}

pub(super) fn drag(target: &Target, from: Point, to: Point) -> Result<()> {
    let start = to_pixel(from);
    if let Some(root) = target.background_root()? {
        let hwnd = child_at(root, start);
        let held = MK_LBUTTON;
        let at = |p: Point| {
            let client = to_client(hwnd, to_pixel(p));
            make_lparam(client.x, client.y)
        };
        let mut messages = vec![Message::new(WM_MOUSEMOVE, 0, at(from)), Message::new(WM_LBUTTONDOWN, held, at(from))];
        messages.extend(
            (1..=DRAG_STEPS)
                .map(|i| Message::new(WM_MOUSEMOVE, held, at(from.lerp(to, f64::from(i) / f64::from(DRAG_STEPS))))),
        );
        messages.push(Message::new(WM_LBUTTONUP, 0, at(to)));
        return post(hwnd, &messages);
    }
    let pause = Duration::from_millis(15);
    send(&[move_input(from)])?;
    std::thread::sleep(pause);
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTDOWN)])?;
    for i in 1..=DRAG_STEPS {
        std::thread::sleep(pause);
        send(&[move_input(from.lerp(to, f64::from(i) / f64::from(DRAG_STEPS)))])?;
    }
    std::thread::sleep(pause);
    send(&[mouse_input(0, 0, 0, MOUSEEVENTF_LEFTUP)])
}

pub(super) fn type_text(target: &Target, text: &str) -> Result<()> {
    let text = text.replace("\r\n", "\n");
    if let Some(root) = target.background_root()? {
        let hwnd = focus_window(root);
        let messages: Vec<Message> = text
            .encode_utf16()
            .map(|unit| {
                Message::new(WM_CHAR, usize::from(if unit == u16::from(b'\n') { u16::from(b'\r') } else { unit }), 1)
            })
            .collect();
        return post(hwnd, &messages);
    }
    activate(target)?;
    for unit in text.encode_utf16() {
        let inputs = match unit {
            0x0A => key_inputs(&[], VirtualKey { vk: VK_RETURN.0, extended: false }),
            0x09 => key_inputs(&[], VirtualKey { vk: VK_TAB.0, extended: false }),
            _ => unicode_inputs(unit).to_vec(),
        };
        send(&inputs)?;
    }
    Ok(())
}

pub(super) fn press_key(target: &Target, chord: &KeyChord) -> Result<()> {
    let stroke = keys::resolve(chord)?;
    if let Some(root) = target.background_root()? {
        let hwnd = focus_window(root);
        let messages = match &stroke {
            Stroke::Keys { modifiers, text: Some(c), .. } if !modifiers.is_empty() => char_messages(*c),
            Stroke::Keys { modifiers, key, .. } => key_messages(modifiers, *key, scan_code),
            Stroke::Unicode(c) => char_messages(*c),
        };
        return post(hwnd, &messages);
    }
    activate(target)?;
    let inputs = match &stroke {
        Stroke::Keys { modifiers, key, .. } => key_inputs(modifiers, *key),
        Stroke::Unicode(c) => {
            let mut units = [0u16; 2];
            c.encode_utf16(&mut units).iter().flat_map(|&u| unicode_inputs(u)).collect()
        }
    };
    send(&inputs)
}

fn char_messages(c: char) -> Vec<Message> {
    let mut units = [0u16; 2];
    c.encode_utf16(&mut units).iter().map(|&u| Message::new(WM_CHAR, usize::from(u), 1)).collect()
}

fn key_messages(modifiers: &[VirtualKey], key: VirtualKey, scan: impl Fn(VirtualKey) -> u16) -> Vec<Message> {
    let is = |vk: VIRTUAL_KEY| modifiers.iter().any(|m| m.vk == vk.0);
    let alt = is(VK_MENU) || key.vk == VK_MENU.0;
    let system = alt && !is(VK_CONTROL);
    let (down, up) = if system { (WM_SYSKEYDOWN, WM_SYSKEYUP) } else { (WM_KEYDOWN, WM_KEYUP) };
    let event = |k: VirtualKey, msg: u32, released: bool| {
        // The context-code bit is set only while Alt is down, so not on Alt's own events.
        let context = alt && k.vk != VK_MENU.0;
        Message::new(msg, usize::from(k.vk), key_lparam(scan(k), k.extended, context, released))
    };
    let mut messages: Vec<Message> = modifiers.iter().map(|&m| event(m, down, false)).collect();
    messages.push(event(key, down, false));
    messages.push(event(key, up, true));
    messages.extend(modifiers.iter().rev().map(|&m| event(m, up, true)));
    messages
}

const EM_REPLACESEL: u32 = 0x00C2;
const SEND_TIMEOUT_MS: u32 = 2000;

pub(super) fn replace_selection(hwnd: HWND, pid: u32, text: &str) -> Result<bool> {
    if window_pid(hwnd) != pid || !is_edit_class(&class_name(hwnd)) {
        return Ok(false);
    }
    // SAFETY: plain FFI call; any HWND value is accepted.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    if isize::try_from(ES_READONLY).is_ok_and(|flag| style & flag != 0) {
        return Ok(false);
    }
    let wide: Vec<u16> = text.replace("\r\n", "\n").replace('\n', "\r\n").encode_utf16().chain([0]).collect();
    let mut result = 0usize;
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call (the system marshals
    // it to the target process), and `result` is a writable usize.
    let sent = unsafe {
        SendMessageTimeoutW(
            hwnd,
            EM_REPLACESEL,
            WPARAM(1),
            LPARAM(wide.as_ptr().expose_provenance().cast_signed()),
            SMTO_ABORTIFHUNG | SMTO_NORMAL,
            SEND_TIMEOUT_MS,
            Some(&raw mut result),
        )
    };
    if sent.0 == 0 {
        return Err(Error::action("the text control did not respond to EM_REPLACESEL"));
    }
    Ok(true)
}

fn class_name(hwnd: HWND) -> String {
    let mut buffer = [0u16; 256];
    // SAFETY: the slice is writable and its length is passed along with it.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    String::from_utf16_lossy(&buffer[..usize::try_from(len).unwrap_or(0).min(buffer.len())])
}

fn is_edit_class(class: &str) -> bool {
    class.split('.').any(|part| {
        let part = part.to_ascii_lowercase();
        part == "edit" || part.starts_with("richedit")
    })
}

fn key_lparam(scan: u16, extended: bool, alt: bool, up: bool) -> isize {
    let mut value = 1u32 | (u32::from(scan & 0xFF) << 16);
    if extended {
        value |= 1 << 24;
    }
    if alt {
        value |= 1 << 29;
    }
    if up {
        value |= (1 << 30) | (1 << 31);
    }
    value.cast_signed() as isize
}

fn focus_window(root: HWND) -> HWND {
    // SAFETY: plain FFI call; a null process ID pointer is allowed.
    let thread = unsafe { GetWindowThreadProcessId(root, None) };
    let mut info = GUITHREADINFO { cbSize: size_u32::<GUITHREADINFO>(), ..Default::default() };
    // SAFETY: `info` is writable and its size field is initialized.
    let ok = unsafe { GetGUIThreadInfo(thread, &raw mut info) }.is_ok();
    if ok && !info.hwndFocus.is_invalid() { info.hwndFocus } else { root }
}

fn activate(target: &Target) -> Result<()> {
    let is_foreground = || foreground_window().is_some_and(|fg| window_pid(fg) == target.pid);
    if is_foreground() {
        return Ok(());
    }
    let root = target
        .root()
        .ok_or_else(|| Error::WindowNotFound(format!("process {} has no window to activate", target.pid)))?;
    // SAFETY: plain FFI call; Windows may refuse, which is checked below.
    let _ = unsafe { SetForegroundWindow(root) };
    if !wait_for(is_foreground) {
        // Sending input lifts the foreground lock; a zero-distance move has no side effects.
        send(&[mouse_input(0, 0, 0, MOUSEEVENTF_MOVE)])?;
        // SAFETY: plain FFI call, checked below.
        let _ = unsafe { SetForegroundWindow(root) };
    }
    if wait_for(is_foreground) { Ok(()) } else { Err(Error::action("could not bring the app to the foreground")) }
}

fn wait_for(condition: impl Fn() -> bool) -> bool {
    for _ in 0..10 {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

fn child_at(root: HWND, screen: POINT) -> HWND {
    let mut current = root;
    for _ in 0..32 {
        let client = to_client(current, screen);
        let flags = CWP_SKIPINVISIBLE | CWP_SKIPDISABLED | CWP_SKIPTRANSPARENT;
        // SAFETY: plain FFI call with values only.
        let child = unsafe { ChildWindowFromPointEx(current, client, flags) };
        if child.is_invalid() || child == current {
            break;
        }
        current = child;
    }
    current
}

fn to_client(hwnd: HWND, screen: POINT) -> POINT {
    let mut point = screen;
    // SAFETY: `point` is a writable POINT.
    let _ = unsafe { ScreenToClient(hwnd, &raw mut point) };
    point
}

fn make_lparam(x: i32, y: i32) -> isize {
    let low = u32::from(clamp_i16(x).cast_unsigned());
    let high = u32::from(clamp_i16(y).cast_unsigned());
    isize::try_from((high << 16) | low).unwrap_or_default()
}

fn clamp_i16(value: i32) -> i16 {
    i16::try_from(value).unwrap_or(if value < 0 { i16::MIN } else { i16::MAX })
}

fn to_pixel(point: Point) -> POINT {
    POINT { x: round_to_i32(point.x), y: round_to_i32(point.y) }
}

#[allow(clippy::cast_possible_truncation)] // Rounded and clamped to the i32 range first.
fn round_to_i32(value: f64) -> i32 {
    if value.is_nan() { 0 } else { value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32 }
}

// Windows maps back with `normalized * extent / 65536` rounded down, so aim at the pixel center.
fn normalize(value: f64, origin: i32, extent: i32) -> i32 {
    let extent = extent.max(1);
    let pixel = round_to_i32(value).saturating_sub(origin).clamp(0, extent - 1);
    round_to_i32(((f64::from(pixel) + 0.5) * 65536.0 / f64::from(extent)).floor().min(65535.0))
}

fn move_input(point: Point) -> INPUT {
    let metric = |index| {
        // SAFETY: plain FFI call with a metric index.
        unsafe { GetSystemMetrics(index) }
    };
    let (left, top) = (metric(SM_XVIRTUALSCREEN), metric(SM_YVIRTUALSCREEN));
    let (width, height) = (metric(SM_CXVIRTUALSCREEN), metric(SM_CYVIRTUALSCREEN));
    mouse_input(
        normalize(point.x, left, width),
        normalize(point.y, top, height),
        0,
        MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
    )
}

const fn mouse_input(dx: i32, dy: i32, data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

const fn keyboard_input(vk: u16, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn key_inputs(modifiers: &[VirtualKey], key: VirtualKey) -> Vec<INPUT> {
    let event = |k: VirtualKey, up: bool| {
        let mut flags = if k.extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
        if up {
            flags |= KEYEVENTF_KEYUP;
        }
        keyboard_input(k.vk, scan_code(k), flags)
    };
    let mut inputs: Vec<INPUT> = modifiers.iter().map(|&m| event(m, false)).collect();
    inputs.push(event(key, false));
    inputs.push(event(key, true));
    inputs.extend(modifiers.iter().rev().map(|&m| event(m, true)));
    inputs
}

const fn unicode_inputs(unit: u16) -> [INPUT; 2] {
    [
        keyboard_input(0, unit, KEYEVENTF_UNICODE),
        keyboard_input(0, unit, KEYBD_EVENT_FLAGS(KEYEVENTF_UNICODE.0 | KEYEVENTF_KEYUP.0)),
    ]
}

fn scan_code(key: VirtualKey) -> u16 {
    // SAFETY: plain FFI call with values only.
    let scan = unsafe { MapVirtualKeyW(u32::from(key.vk), MAPVK_VK_TO_VSC) };
    u16::try_from(scan).unwrap_or_default()
}

fn send(inputs: &[INPUT]) -> Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    let size = i32::try_from(size_of::<INPUT>()).map_err(|_| Error::platform("INPUT size does not fit in an i32"))?;
    // SAFETY: `inputs` is a slice of initialized INPUT structs and `size` is their size.
    let sent = unsafe { SendInput(inputs, size) };
    if usize::try_from(sent).is_ok_and(|n| n == inputs.len()) {
        Ok(())
    } else {
        Err(Error::action("SendInput was blocked; the target may run elevated or the desktop may be locked"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_to_virtual_desktop() {
        for pixel in 0..1920 {
            assert_eq!(normalize(f64::from(pixel), 0, 1920) * 1920 / 65536, pixel);
        }
        assert_eq!(normalize(-1920.0, -1920, 3840), 8);
        assert_eq!(normalize(5000.0, 0, 1920), normalize(1919.0, 0, 1920));
    }

    #[test]
    fn splits_wheel_deltas() {
        assert_eq!(wheel_steps(0.0), Vec::<i32>::new());
        assert_eq!(wheel_steps(60.0), vec![120]);
        assert_eq!(wheel_steps(150.0), vec![120, 120, 60]);
        assert_eq!(wheel_steps(-30.0), vec![-60]);
        assert_eq!(wheel_wparam(-120), 0xFF88_0000);
        assert_eq!(wheel_wparam(120), 0x0078_0000);
    }

    #[test]
    fn packs_lparams() {
        assert_eq!(make_lparam(10, 20), 0x0014_000A);
        assert_eq!(make_lparam(-1, 0), 0xFFFF);
        assert_eq!(key_lparam(0x1E, false, false, false), 0x001E_0001);
        assert_eq!(key_lparam(0x48, true, false, true) & 0xFFFF_FFFF, 0xC148_0001);
        assert_eq!(key_lparam(0x3E, false, true, false), 0x203E_0001);
    }

    #[test]
    fn recognizes_edit_classes() {
        for class in ["Edit", "RichEdit20W", "RICHEDIT50W", "WindowsForms10.EDIT.app.0.141b42a_r6_ad1"] {
            assert!(is_edit_class(class), "{class}");
        }
        for class in ["Button", "Chrome_RenderWidgetHostHWND", "Scintilla", "EditorWindow"] {
            assert!(!is_edit_class(class), "{class}");
        }
    }

    #[test]
    fn builds_key_messages() {
        let ctrl = VirtualKey { vk: VK_CONTROL.0, extended: false };
        let alt = VirtualKey { vk: VK_MENU.0, extended: false };
        let t = VirtualKey { vk: u16::from(b'T'), extended: false };
        let kinds = |m: &[Message]| m.iter().map(|m| (m.msg, m.wparam)).collect::<Vec<_>>();
        let ctrl_t = key_messages(&[ctrl], t, |_| 0x14);
        assert_eq!(kinds(&ctrl_t), vec![(WM_KEYDOWN, 0x11), (WM_KEYDOWN, 0x54), (WM_KEYUP, 0x54), (WM_KEYUP, 0x11)]);
        let alt_t = key_messages(&[alt], t, |_| 0x14);
        assert_eq!(
            kinds(&alt_t),
            vec![(WM_SYSKEYDOWN, 0x12), (WM_SYSKEYDOWN, 0x54), (WM_SYSKEYUP, 0x54), (WM_SYSKEYUP, 0x12)]
        );
        assert_eq!(alt_t[1].lparam & 0x2000_0000, 0x2000_0000);
        assert_eq!(alt_t[0].lparam & 0x2000_0000, 0);
    }

    #[test]
    fn builds_click_messages() {
        let messages = click_messages(MouseButton::Left, 2, POINT { x: 5, y: 7 });
        let kinds: Vec<u32> = messages.iter().map(|m| m.msg).collect();
        assert_eq!(kinds, vec![WM_LBUTTONDOWN, WM_LBUTTONUP, WM_LBUTTONDBLCLK, WM_LBUTTONUP]);
        assert_eq!(messages[0].wparam, 1);
        assert_eq!(messages[0].lparam, 0x0007_0005);
    }
}
