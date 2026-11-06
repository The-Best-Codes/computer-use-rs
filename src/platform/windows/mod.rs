macro_rules! com {
    ($call:expr) => {{
        // SAFETY: the receiver is a live, reference-counted COM interface borrowed for the
        // duration of the call, every argument is a valid value or reference that outlives
        // it, and the method has no further preconditions.
        unsafe { $call }
    }};
}

mod apps;
mod capture;
mod input;
mod keys;
mod uia;

use std::cell::Cell;
use std::ops::Range;

use windows::Win32::Foundation::{HWND, RECT, RPC_E_CHANGED_MODE};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Accessibility::IUIAutomationElement;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};

use crate::app::AppInfo;
use crate::backend::{Backend, Capture, CaptureRequest, InputTarget, MouseButton};
use crate::element::RawNode;
use crate::error::{Error, Result};
use crate::geometry::{Point, Rect};
use crate::keys::KeyChord;
use crate::state::WindowInfo;

trait WinResultExt<T> {
    fn or_platform(self, what: &str) -> Result<T>;
    fn or_action(self, what: &str) -> Result<T>;
}

impl<T> WinResultExt<T> for windows::core::Result<T> {
    fn or_platform(self, what: &str) -> Result<T> {
        self.map_err(|e| Error::platform(format!("{what}: {e}")))
    }

    fn or_action(self, what: &str) -> Result<T> {
        self.map_err(|e| Error::action(format!("{what}: {e}")))
    }
}

/// The Windows implementation of [`Backend`], built on UI Automation.
///
/// Element handles are [`WindowsElement`]s.
///
/// [`WindowsBackend::new`] makes the process per-monitor DPI aware (unless the host already chose
/// a mode), so all coordinates are physical pixels in the virtual-desktop space. Every call joins
/// the calling thread to the COM multithreaded apartment first, so the backend (and a
/// [`Computer`](crate::Computer) over it) can move between threads. Use it from threads that have
/// not entered a single-threaded apartment.
pub struct WindowsBackend {
    uia: uia::Uia,
}

impl std::fmt::Debug for WindowsBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsBackend").finish_non_exhaustive()
    }
}

/// A live UI Automation element: the [`WindowsBackend`] handle type.
///
/// Unlike a bare `IUIAutomationElement`, it is `Send` and `Sync`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowsElement(IUIAutomationElement);

#[allow(clippy::non_send_fields_in_send_ty)] // see the SAFETY comment
// SAFETY: UI Automation client objects are free-threaded: they aggregate the free-threaded
// marshaler and may be called from any thread in any apartment. The backend also joins every
// thread it runs on to the multithreaded apartment before touching an element.
unsafe impl Send for WindowsElement {}
// SAFETY: as above; COM reference counting is atomic and the interface has no `&self` state.
unsafe impl Sync for WindowsElement {}

impl WindowsElement {
    /// The underlying `IUIAutomationElement`.
    #[must_use]
    pub const fn as_uia(&self) -> &IUIAutomationElement {
        &self.0
    }
}

fn wrap(nodes: Vec<RawNode<IUIAutomationElement>>) -> Vec<RawNode<WindowsElement>> {
    nodes.into_iter().map(|n| RawNode { handle: WindowsElement(n.handle), parent: n.parent, data: n.data }).collect()
}

thread_local! {
    static COM_READY: Cell<bool> = const { Cell::new(false) };
}

/// Joins the calling thread to the COM multithreaded apartment, once per thread.
fn ensure_com() -> Result<()> {
    COM_READY.with(|ready| {
        if ready.get() {
            return Ok(());
        }
        // SAFETY: plain FFI call with no reserved pointer. COM stays initialized for the life of
        // the thread because element handles may outlive any one call.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        // An existing single-threaded apartment still lets UI Automation work.
        if hr.is_err() && hr != RPC_E_CHANGED_MODE {
            return Err(Error::platform(format!("CoInitializeEx failed: {}", windows::core::Error::from(hr))));
        }
        ready.set(true);
        Ok(())
    })
}

impl WindowsBackend {
    /// Initializes COM for this thread, opts into per-monitor DPI awareness, and creates the
    /// UI Automation client.
    ///
    /// # Errors
    ///
    /// Fails if COM cannot be initialized or UI Automation is unavailable.
    pub fn new() -> Result<Self> {
        // SAFETY: plain FFI call; a failure (awareness already set by the host) is harmless.
        let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        ensure_com()?;
        Ok(Self { uia: uia::Uia::new()? })
    }
}

impl Backend for WindowsBackend {
    type Handle = WindowsElement;

    fn list_apps(&self) -> Result<Vec<AppInfo>> {
        Ok(apps::list_apps())
    }

    fn launch_app(&self, app: &AppInfo) -> Result<AppInfo> {
        ensure_com()?;
        apps::launch(app)
    }

    fn list_windows(&self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let pid = app.pid.ok_or_else(|| Error::AppNotFound(format!("{} is not running", app.name)))?;
        Ok(apps::app_windows(pid)
            .into_iter()
            .filter_map(|hwnd| {
                let frame = apps::window_frame(hwnd)?;
                let title = Some(apps::window_title(hwnd)).filter(|t| !t.is_empty());
                Some(WindowInfo::new(Some(hwnd_id(hwnd)), title, rect_from(frame)))
            })
            .collect())
    }

    fn capture(&self, app: &AppInfo, request: &CaptureRequest) -> Result<Capture<Self::Handle>> {
        ensure_com()?;
        let pid = app.pid.ok_or_else(|| Error::AppNotFound(format!("{} is not running", app.name)))?;
        let hwnd = match request.window {
            // An `HWND` from another process, or a stale one, must not be captured instead.
            Some(id) => hwnd_from_id(id)
                .filter(|&h| apps::window_pid(h) == pid)
                .ok_or_else(|| Error::WindowNotFound(format!("{} (no window with ID {id})", app.name)))?,
            None => apps::main_window(pid).ok_or_else(|| Error::WindowNotFound(app.name.clone()))?,
        };
        let bounds = apps::window_frame(hwnd).ok_or_else(|| Error::WindowNotFound(app.name.clone()))?;
        let title = Some(apps::window_title(hwnd)).filter(|t| !t.is_empty());
        let mut capture = Capture::new(WindowInfo::new(Some(hwnd_id(hwnd)), title, rect_from(bounds)));
        if request.include_tree {
            let walk = self.uia.walk(hwnd, request.max_nodes, request.max_depth)?;
            let (focused, selected_text) = self.uia.focus(pid, &walk.nodes);
            capture.nodes = wrap(walk.nodes);
            capture.truncated = walk.truncated;
            capture.focused = focused;
            capture.selected_text = selected_text;
        }
        if request.include_screenshot {
            capture.screenshot = Some(capture::screenshot(hwnd, bounds)?);
        }
        Ok(capture)
    }

    fn perform_action(&self, element: &Self::Handle, action: &str) -> Result<()> {
        ensure_com()?;
        match uia::perform_action(&element.0, action)? {
            uia::ActionOutcome::Done => Ok(()),
            uia::ActionOutcome::RightClick { pid, point } => {
                let target = input::Target { pid, window: None, delivery: crate::backend::Delivery::Background };
                input::click(&target, point, MouseButton::Right, 1)
            }
        }
    }

    fn set_value(&self, element: &Self::Handle, value: &str) -> Result<()> {
        ensure_com()?;
        uia::set_value(&element.0, value)
    }

    fn value(&self, element: &Self::Handle) -> Result<Option<String>> {
        ensure_com()?;
        uia::value(&element.0)
    }

    fn select_text(&self, element: &Self::Handle, value: &str, range: Range<usize>) -> Result<()> {
        ensure_com()?;
        uia::select_text(&element.0, value, range)
    }

    fn insert_text(&self, app: &AppInfo, text: &str) -> Result<bool> {
        ensure_com()?;
        let Some(pid) = app.pid else {
            return Ok(false);
        };
        self.uia.focused_native_window(pid).map_or(Ok(false), |hwnd| input::replace_selection(hwnd, pid, text))
    }

    fn click(&self, target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()> {
        ensure_com()?;
        input::click(&input::Target::from_input(target)?, point, button, count)
    }

    fn scroll(&self, target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()> {
        ensure_com()?;
        input::scroll(&input::Target::from_input(target)?, point, dx, dy)
    }

    fn drag(&self, target: InputTarget<'_>, from: Point, to: Point) -> Result<()> {
        ensure_com()?;
        input::drag(&input::Target::from_input(target)?, from, to)
    }

    fn type_text(&self, target: InputTarget<'_>, text: &str) -> Result<()> {
        ensure_com()?;
        input::type_text(&input::Target::from_input(target)?, text)
    }

    fn press_key(&self, target: InputTarget<'_>, chord: &KeyChord) -> Result<()> {
        ensure_com()?;
        input::press_key(&input::Target::from_input(target)?, chord)
    }
}

fn hwnd_id(hwnd: HWND) -> u64 {
    u64::try_from(hwnd.0.addr()).unwrap_or_default()
}

fn hwnd_from_id(id: u64) -> Option<HWND> {
    let addr = usize::try_from(id).ok().filter(|&a| a != 0)?;
    Some(HWND(std::ptr::without_provenance_mut(addr)))
}

fn rect_from(rect: RECT) -> Rect {
    Rect::new(
        f64::from(rect.left),
        f64::from(rect.top),
        f64::from(rect.right - rect.left),
        f64::from(rect.bottom - rect.top),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rects_and_ids() {
        let rect = rect_from(RECT { left: -10, top: 20, right: 90, bottom: 70 });
        assert_eq!(rect, Rect::new(-10.0, 20.0, 100.0, 50.0));
        assert_eq!(hwnd_from_id(0), None);
        assert_eq!(hwnd_from_id(0x1234).map(hwnd_id), Some(0x1234));
    }
}
