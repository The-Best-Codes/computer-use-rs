use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, RECT, TRUE};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
use windows::Win32::System::Threading::{
    GetProcessId, OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Shell::{
    IShellLinkW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW, ShellLink,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SW_SHOWNOACTIVATE,
    WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, HSTRING, PWSTR, w};

use super::WinResultExt as _;
use crate::app::AppInfo;
use crate::error::{Error, Result};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn list_apps() -> Vec<AppInfo> {
    let mut apps = running_apps();
    apps.extend(installed_apps());
    apps
}

fn running_apps() -> Vec<AppInfo> {
    let foreground = foreground_window().map(window_pid);
    let mut by_pid: BTreeMap<u32, AppInfo> = BTreeMap::new();
    for hwnd in top_level_windows() {
        if !is_app_window(hwnd) {
            continue;
        }
        let pid = window_pid(hwnd);
        if pid == 0 || by_pid.contains_key(&pid) {
            continue;
        }
        let path = process_path(pid);
        let stem = path.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().into_owned());
        let name = stem.unwrap_or_else(|| window_title(hwnd));
        let mut app = AppInfo::new(name)
            .with_pid(pid)
            .with_active(foreground == Some(pid))
            .with_window_title(main_window(pid).map(window_title).unwrap_or_default());
        if let Some(path) = path {
            if let Some(file) = path.file_name() {
                app = app.with_bundle_id(file.to_string_lossy());
            }
            app = app.with_path(path);
        }
        by_pid.insert(pid, app);
    }
    by_pid.into_values().collect()
}

fn installed_apps() -> Vec<AppInfo> {
    let mut shortcuts = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            let dir = PathBuf::from(base).join(r"Microsoft\Windows\Start Menu\Programs");
            collect_shortcuts(&dir, 0, &mut shortcuts);
        }
    }
    let mut seen = HashSet::new();
    shortcuts
        .into_iter()
        .filter_map(|path| {
            let name = path.file_stem()?.to_string_lossy().into_owned();
            let key = name.to_lowercase();
            (!key.starts_with("uninstall") && seen.insert(key)).then(|| AppInfo::new(name).with_path(path))
        })
        .collect()
}

fn collect_shortcuts(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    const MAX_DEPTH: usize = 4;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() && depth < MAX_DEPTH {
            collect_shortcuts(&path, depth + 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
            out.push(path);
        }
    }
}

pub(super) fn launch(app: &AppInfo) -> Result<AppInfo> {
    let target: OsString = app
        .path
        .as_ref()
        .map(|p| p.as_os_str().to_owned())
        .or_else(|| app.bundle_id.as_ref().map(OsString::from))
        .unwrap_or_else(|| OsString::from(&app.name));
    let expected = expected_executable(app);
    let before: HashSet<u32> = running_apps().iter().filter_map(|a| a.pid).collect();
    let launched = shell_execute(&HSTRING::from(target.as_os_str()))?;

    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    loop {
        let running = running_apps();
        let found = running
            .iter()
            .find(|a| launched.is_some() && a.pid == launched)
            .or_else(|| {
                let exe = expected.as_deref()?;
                running.iter().find(|a| a.bundle_id.as_deref().is_some_and(|b| b.eq_ignore_ascii_case(exe)))
            })
            .or_else(|| {
                running
                    .iter()
                    .find(|a| a.pid.is_some_and(|pid| !before.contains(&pid)) && a.match_score(&app.name).is_some())
            });
        if let Some(found) = found {
            return Ok(found.clone());
        }
        if Instant::now() >= deadline {
            return Err(Error::action(format!("{} did not open a window within {LAUNCH_TIMEOUT:?}", app.name)));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn expected_executable(app: &AppInfo) -> Option<String> {
    let from_path = app.path.as_deref().and_then(|path| {
        let is_link = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
        let exe = if is_link { shortcut_target(path)? } else { path.to_path_buf() };
        Some(exe.file_name()?.to_string_lossy().into_owned())
    });
    from_path.or_else(|| app.bundle_id.clone()).filter(|name| name.to_ascii_lowercase().ends_with(".exe"))
}

fn shell_execute(file: &HSTRING) -> Result<Option<u32>> {
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_u32::<SHELLEXECUTEINFOW>(),
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpVerb: w!("open"),
        lpFile: windows::core::PCWSTR(file.as_ptr()),
        nShow: SW_SHOWNOACTIVATE.0,
        ..Default::default()
    };
    // SAFETY: `info` is fully initialized with its size and points at strings that outlive the call.
    unsafe { ShellExecuteExW(&raw mut info) }.or_action("ShellExecuteExW failed")?;
    if info.hProcess.is_invalid() {
        return Ok(None);
    }
    let process = OwnedHandle(info.hProcess);
    // SAFETY: `process` is a live process handle returned by the shell.
    let pid = unsafe { GetProcessId(process.0) };
    Ok((pid != 0).then_some(pid))
}

fn shortcut_target(link: &Path) -> Option<PathBuf> {
    // SAFETY: COM is initialized by `WindowsBackend::new`; `ShellLink` is an in-process server.
    let shell_link: IShellLinkW = unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let file: IPersistFile = windows::core::Interface::cast(&shell_link).ok()?;
    com!(file.Load(&HSTRING::from(link.as_os_str()), STGM_READ)).ok()?;
    let mut buffer = [0u16; 1024];
    com!(shell_link.GetPath(&mut buffer, std::ptr::null_mut(), 0)).ok()?;
    let len = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    (len > 0).then(|| PathBuf::from(OsString::from_wide(&buffer[..len])))
}

pub(super) fn main_window(pid: u32) -> Option<HWND> {
    if let Some(fg) = foreground_window()
        && window_pid(fg) == pid
        && is_visible_window(fg)
    {
        return Some(fg);
    }
    let candidates: Vec<HWND> =
        top_level_windows().into_iter().filter(|&h| window_pid(h) == pid && is_visible_window(h)).collect();
    let area = |h: &HWND| window_frame(*h).map_or(0, |r| i64::from(r.right - r.left) * i64::from(r.bottom - r.top));
    candidates
        .iter()
        .copied()
        .filter(|&h| !has_owner(h))
        .max_by_key(area)
        .or_else(|| candidates.iter().copied().max_by_key(area))
}

/// The process's visible top-level windows, front to back.
pub(super) fn app_windows(pid: u32) -> Vec<HWND> {
    top_level_windows().into_iter().filter(|&h| window_pid(h) == pid && is_visible_window(h)).collect()
}

pub(super) fn window_frame(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a writable RECT of exactly the size passed, as this attribute requires.
    let dwm =
        unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, (&raw mut rect).cast(), size_u32::<RECT>()) };
    if dwm.is_err() || is_empty(&rect) {
        rect = window_rect(hwnd)?;
    }
    (!is_empty(&rect)).then_some(rect)
}

pub(super) fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect = RECT::default();
    // SAFETY: `rect` is a writable RECT.
    unsafe { GetWindowRect(hwnd, &raw mut rect) }.ok()?;
    Some(rect)
}

pub(super) fn window_title(hwnd: HWND) -> String {
    // SAFETY: plain FFI call; any HWND value is accepted.
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    let Ok(len) = usize::try_from(len) else {
        return String::new();
    };
    if len == 0 {
        return String::new();
    }
    let mut buffer = vec![0u16; len + 1];
    // SAFETY: the slice is writable and its length is passed along with it.
    let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
    let copied = usize::try_from(copied).unwrap_or(0).min(buffer.len());
    String::from_utf16_lossy(&buffer[..copied])
}

pub(super) fn window_pid(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: `pid` is a writable u32.
    unsafe { GetWindowThreadProcessId(hwnd, Some(&raw mut pid)) };
    pid
}

pub(super) fn foreground_window() -> Option<HWND> {
    // SAFETY: plain FFI call with no arguments.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.is_invalid()).then_some(hwnd)
}

pub(super) fn top_level_windows() -> Vec<HWND> {
    let mut windows: Vec<HWND> = Vec::new();
    let lparam = LPARAM(std::ptr::from_mut(&mut windows).expose_provenance().cast_signed());
    // SAFETY: the callback runs only during this call and only touches `windows` through `lparam`.
    let _ = unsafe { EnumWindows(Some(collect_window), lparam) };
    windows
}

unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let windows = std::ptr::with_exposed_provenance_mut::<Vec<HWND>>(lparam.0.cast_unsigned());
    // SAFETY: `lparam` carries the exclusive `&mut Vec<HWND>` from `top_level_windows`, which
    // outlives the enumeration.
    unsafe { (*windows).push(hwnd) };
    TRUE
}

fn is_visible_window(hwnd: HWND) -> bool {
    // SAFETY: plain FFI call; any HWND value is accepted.
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || is_cloaked(hwnd) {
        return false;
    }
    // SAFETY: plain FFI call; any HWND value is accepted.
    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    if isize::try_from(WS_EX_TOOLWINDOW.0).is_ok_and(|flag| ex_style & flag != 0) {
        return false;
    }
    window_frame(hwnd).is_some()
}

fn is_app_window(hwnd: HWND) -> bool {
    is_visible_window(hwnd) && !has_owner(hwnd) && !window_title(hwnd).is_empty()
}

fn has_owner(hwnd: HWND) -> bool {
    // SAFETY: plain FFI call; any HWND value is accepted.
    unsafe { GetWindow(hwnd, GW_OWNER) }.is_ok_and(|owner| !owner.is_invalid())
}

fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: `cloaked` is a writable u32 of exactly the size passed, as this attribute requires.
    let result = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, (&raw mut cloaked).cast(), size_u32::<u32>()) };
    result.is_ok() && cloaked != 0
}

fn process_path(pid: u32) -> Option<PathBuf> {
    // SAFETY: plain FFI call; failure is reported through the result.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let handle = OwnedHandle(handle);
    let mut buffer = vec![0u16; 4096];
    let mut size = u32::try_from(buffer.len()).ok()?;
    // SAFETY: `buffer` holds `size` writable UTF-16 units and `size` is a writable u32.
    unsafe { QueryFullProcessImageNameW(handle.0, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &raw mut size) }
        .ok()?;
    let len = usize::try_from(size).ok()?.min(buffer.len());
    Some(PathBuf::from(OsString::from_wide(&buffer[..len])))
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this guard and closed exactly once.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

const fn is_empty(rect: &RECT) -> bool {
    rect.right <= rect.left || rect.bottom <= rect.top
}

pub(super) fn size_u32<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}
