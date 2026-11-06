use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFString, CFType};
use objc2_core_graphics::{
    CGWindowListCopyWindowInfo, CGWindowListOption, kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen,
    kCGWindowLayer, kCGWindowName, kCGWindowNumber, kCGWindowOwnerPID,
};

use super::ax::{self, AxElement};
use crate::error::{Error, Result};
use crate::geometry::Rect;
use crate::state::WindowInfo;

pub(super) struct TargetWindow {
    pub(super) element: AxElement,
    pub(super) frame: Rect,
    pub(super) title: Option<String>,
}

/// The app's AX windows that are real windows (Finder reports its desktop, an `AXScrollArea`,
/// as the focused window), focused and main first, without duplicates.
fn ax_windows(app: &AxElement) -> Vec<AxElement> {
    let mut out: Vec<AxElement> = Vec::new();
    let candidates = ["AXFocusedWindow", "AXMainWindow"]
        .into_iter()
        .filter_map(|attr| ax::element(app, attr))
        .chain(ax::elements(app, "AXWindows"));
    for window in candidates {
        if ax::string(&window, "AXRole").as_deref() == Some("AXWindow") && !out.contains(&window) {
            out.push(window);
        }
    }
    out
}

fn target(element: AxElement) -> Option<TargetWindow> {
    let frame = ax::frame(&element).filter(|f| !f.is_empty())?;
    let title = ax::string(&element, "AXTitle").filter(|t| !t.is_empty());
    Some(TargetWindow { element, frame, title })
}

/// Restores a minimized window so it can be captured and driven.
fn unminimize(window: AxElement) -> Result<Option<TargetWindow>> {
    ax::set_bool(&window, "AXMinimized", false)?;
    std::thread::sleep(std::time::Duration::from_millis(400));
    Ok(target(window))
}

pub(super) fn main_window(app: &AxElement, app_name: &str) -> Result<TargetWindow> {
    let mut minimized = None;
    for window in ax_windows(app) {
        if ax::boolean(&window, "AXMinimized") == Some(true) {
            minimized.get_or_insert(window);
            continue;
        }
        if let Some(found) = target(window) {
            return Ok(found);
        }
    }
    if let Some(window) = minimized
        && let Some(found) = unminimize(window)?
    {
        return Ok(found);
    }
    Err(Error::WindowNotFound(app_name.to_owned()))
}

/// The window-server ID of an AX window.
pub(super) fn server_id(element: &AxElement, pid: i32, servers: &[ServerWindow]) -> Option<u32> {
    let frame = ax::frame(element)?;
    let title = ax::string(element, "AXTitle").filter(|t| !t.is_empty());
    super::skylight::get()
        .window_id(element)
        .filter(|id| servers.iter().any(|w| w.id == *id))
        .or_else(|| window_id(pid, frame, title.as_deref()))
}

/// Every window of the app: AX windows first, then window-server windows AX does not expose.
pub(super) fn list(app: &AxElement, pid: i32) -> Vec<WindowInfo> {
    let servers = server_windows(pid);
    let mut out: Vec<WindowInfo> = ax_windows(app)
        .iter()
        .filter_map(|element| {
            let frame = ax::frame(element).filter(|f| !f.is_empty())?;
            let title = ax::string(element, "AXTitle").filter(|t| !t.is_empty());
            Some(WindowInfo::new(server_id(element, pid, &servers).map(u64::from), title, frame))
        })
        .collect();
    for server in servers.into_iter().filter(|w| w.on_screen && is_window_sized(w)) {
        if !out.iter().any(|w| w.id == Some(u64::from(server.id))) {
            out.push(WindowInfo::new(Some(u64::from(server.id)), server.title, server.frame));
        }
    }
    out
}

/// What [`by_id`] found.
pub(super) enum Selected {
    /// An AX window (restored if it was minimized).
    Ax(TargetWindow),
    /// A window only the window server knows about.
    ServerOnly(ServerWindow),
}

/// The window with window-server ID `id`.
pub(super) fn by_id(app: &AxElement, pid: i32, id: u64, app_name: &str) -> Result<Selected> {
    let servers = server_windows(pid);
    let not_found = || Error::WindowNotFound(format!("{app_name} (no window with ID {id})"));
    let id = u32::try_from(id).map_err(|_| not_found())?;
    for window in ax_windows(app) {
        if server_id(&window, pid, &servers) != Some(id) {
            continue;
        }
        let found =
            if ax::boolean(&window, "AXMinimized") == Some(true) { unminimize(window)? } else { target(window) };
        return found.map(Selected::Ax).ok_or_else(not_found);
    }
    servers.into_iter().find(|w| w.id == id).map(Selected::ServerOnly).ok_or_else(not_found)
}

pub(super) struct ServerWindow {
    pub(super) id: u32,
    pub(super) frame: Rect,
    pub(super) title: Option<String>,
    pub(super) on_screen: bool,
}

pub(super) fn server_windows(pid: i32) -> Vec<ServerWindow> {
    let options = CGWindowListOption::OptionAll | CGWindowListOption::ExcludeDesktopElements;
    let Some(list) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else { return Vec::new() };
    // SAFETY: CGWindowListCopyWindowInfo returns an array of CFDictionary<CFString, CFType>.
    let list: &CFArray<CFDictionary<CFString, CFType>> = unsafe { list.cast_unchecked() };
    // SAFETY: these are immutable framework constants.
    let (owner_key, layer_key, number_key, bounds_key, name_key, onscreen_key) = unsafe {
        (kCGWindowOwnerPID, kCGWindowLayer, kCGWindowNumber, kCGWindowBounds, kCGWindowName, kCGWindowIsOnscreen)
    };
    list.iter()
        .filter_map(|info| {
            let number =
                |key: &CFString| info.get(key).and_then(|v| v.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64));
            if number(owner_key) != Some(i64::from(pid)) || number(layer_key) != Some(0) {
                return None;
            }
            Some(ServerWindow {
                id: number(number_key).and_then(|n| u32::try_from(n).ok())?,
                frame: info.get(bounds_key).and_then(|b| bounds_rect(&b))?,
                title: info.get(name_key).and_then(|n| ax::as_string(&n)).filter(|t| !t.is_empty()),
                on_screen: info.get(onscreen_key).and_then(|v| ax::as_bool(&v)).unwrap_or(false),
            })
        })
        .collect()
}

const fn is_window_sized(w: &ServerWindow) -> bool {
    w.frame.size.width >= 50.0 && w.frame.size.height >= 50.0
}

pub(super) fn server_main_window(pid: i32) -> Option<ServerWindow> {
    server_windows(pid).into_iter().find(|w| w.on_screen && is_window_sized(w))
}

pub(super) fn window_id(pid: i32, frame: Rect, title: Option<&str>) -> Option<u32> {
    let distance = |w: &ServerWindow| {
        (w.frame.min_x() - frame.min_x()).abs()
            + (w.frame.min_y() - frame.min_y()).abs()
            + (w.frame.size.width - frame.size.width).abs()
            + (w.frame.size.height - frame.size.height).abs()
    };
    server_windows(pid)
        .into_iter()
        .map(|w| (distance(&w), title.is_none() || w.title.as_deref() != title, w.id))
        .filter(|(d, _, _)| *d < 8.0)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, _, id)| id)
}

fn bounds_rect(value: &CFType) -> Option<Rect> {
    let dict = value.downcast_ref::<CFDictionary>()?;
    // SAFETY: window bounds dictionaries map CFString keys to CFNumber values.
    let dict: &CFDictionary<CFString, CFNumber> = unsafe { dict.cast_unchecked() };
    let get = |key: &'static str| dict.get(&CFString::from_static_str(key)).and_then(|n| n.as_f64());
    Some(Rect::new(get("X")?, get("Y")?, get("Width")?, get("Height")?))
}
