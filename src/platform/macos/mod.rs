mod apps;
mod ax;
mod input;
mod keys;
mod menu;
mod screenshot;
mod skylight;
mod tree;
mod window;

use std::ops::Range;

use objc2_application_services::{AXIsProcessTrusted, AXIsProcessTrustedWithOptions, kAXTrustedCheckOptionPrompt};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFNumber, CFRange, CFString, CFType};
use objc2_core_graphics::{CGPreflightScreenCaptureAccess, CGRequestScreenCaptureAccess};

use self::ax::AxElement;
use crate::app::AppInfo;
use crate::backend::{Backend, Capture, CaptureRequest, Delivery, InputTarget, MouseButton, Permissions};
use crate::error::{Error, Permission, Result};
use crate::geometry::Point;
use crate::keys::KeyChord;
use crate::state::WindowInfo;
use crate::text::utf16_offset;

const INSERTABLE_ROLES: [&str; 3] = ["AXTextField", "AXTextArea", "AXComboBox"];

/// The macOS [`Backend`].
///
/// Requires macOS 14 or later, and the Accessibility and Screen Recording permissions for
/// the host process (usually the terminal or app that runs your binary).
#[derive(Debug, Default)]
pub struct MacBackend {
    installed: apps::InstalledCache,
}

impl MacBackend {
    /// Creates the backend. Never fails; missing permissions surface on first use.
    ///
    /// # Errors
    ///
    /// Infallible today; the `Result` keeps the signature uniform across platforms.
    pub fn new() -> Result<Self> {
        Ok(Self::default())
    }
}

fn require_accessibility() -> Result<()> {
    // SAFETY: AXIsProcessTrusted has no preconditions.
    if unsafe { AXIsProcessTrusted() } { Ok(()) } else { Err(Error::PermissionDenied(Permission::Accessibility)) }
}

fn pid_of(app: &AppInfo) -> Result<i32> {
    app.pid.and_then(|p| i32::try_from(p).ok()).ok_or_else(|| Error::invalid(format!("{} is not running", app.name)))
}

impl Backend for MacBackend {
    type Handle = AxElement;

    fn list_apps(&self) -> Result<Vec<AppInfo>> {
        let mut apps = apps::running_apps();
        let installed: Vec<AppInfo> = self
            .installed
            .get()
            .into_iter()
            .filter(|candidate| {
                !apps.iter().any(|running| {
                    (candidate.bundle_id.is_some() && running.bundle_id == candidate.bundle_id)
                        || (candidate.path.is_some() && running.path == candidate.path)
                })
            })
            .collect();
        apps.extend(installed);
        Ok(apps)
    }

    fn launch_app(&self, app: &AppInfo) -> Result<AppInfo> {
        apps::launch(app)
    }

    fn permissions(&self) -> Permissions {
        // SAFETY: AXIsProcessTrusted has no preconditions.
        let accessibility = unsafe { AXIsProcessTrusted() };
        Permissions::new(accessibility, CGPreflightScreenCaptureAccess())
    }

    fn request_permissions(&self) -> Permissions {
        // SAFETY: reading an immutable framework constant.
        let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
        let options = CFDictionary::<CFString, CFType>::from_slices(&[key], &[CFBoolean::new(true).as_ref()]);
        // SAFETY: `options` is a valid dictionary with the documented key.
        let accessibility = unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
        let screen = CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess();
        Permissions::new(accessibility, screen)
    }

    fn capture(&self, app: &AppInfo, request: &CaptureRequest) -> Result<Capture<AxElement>> {
        require_accessibility()?;
        let pid = pid_of(app)?;
        let app_element = ax::application(pid);
        // Chromium and Electron only build their tree once an assistive client asks for it.
        let _ = ax::set_bool(&app_element, "AXManualAccessibility", true);

        let mut notes = Vec::new();
        let target = match window::main_window(&app_element, &app.name) {
            Ok(target) => target,
            // Some apps expose no AX window for a visible one; use the window server's geometry.
            Err(Error::WindowNotFound(name)) => {
                let server = window::server_main_window(pid).ok_or(Error::WindowNotFound(name))?;
                notes.push("Accessibility exposes no window for this app; the tree is rooted at the app.".to_owned());
                window::TargetWindow { element: app_element.clone(), frame: server.frame, title: server.title }
            }
            Err(e) => return Err(e),
        };
        let window_id = skylight::get()
            .window_id(&target.element)
            .filter(|id| window::server_windows(pid).iter().any(|w| w.id == *id))
            .or_else(|| window::window_id(pid, target.frame, target.title.as_deref()));
        let mut capture = Capture::new(WindowInfo::new(window_id.map(u64::from), target.title.clone(), target.frame));
        capture.notes = notes;

        if request.include_tree {
            let focused = ax::element(&app_element, "AXFocusedUIElement");
            let walk =
                tree::walk(&target.element, target.frame, focused.as_ref(), request.max_nodes, request.max_depth);
            capture.nodes = walk.nodes;
            capture.truncated = walk.truncated;
            capture.focused = walk.focused;
            capture.selected_text =
                focused.as_ref().and_then(|f| ax::string(f, "AXSelectedText")).filter(|s| !s.is_empty());
        }

        if request.include_screenshot {
            match window_id.map(screenshot::capture_window) {
                Some(Ok(image)) => capture.screenshot = Some(image),
                Some(Err(e @ Error::PermissionDenied(_))) => return Err(e),
                Some(Err(e)) => capture.notes.push(format!("Screenshot omitted: {e}")),
                None => capture.notes.push("Screenshot omitted: the window has no window-server ID".to_owned()),
            }
        }
        Ok(capture)
    }

    fn perform_action(&self, element: &AxElement, action: &str) -> Result<()> {
        ax::perform(element, action)
    }

    fn set_value(&self, element: &AxElement, value: &str) -> Result<()> {
        if !ax::is_settable(element, "AXValue") {
            return Err(Error::action("this element's value is not settable"));
        }
        let numeric = ax::attribute(element, "AXValue").is_some_and(|v| v.downcast_ref::<CFNumber>().is_some());
        if numeric {
            let number: f64 = value.trim().parse().map_err(|_| Error::invalid(format!("{value:?} is not a number")))?;
            ax::set(element, "AXValue", &CFNumber::new_f64(number))
        } else {
            ax::set(element, "AXValue", &CFString::from_str(value))
        }
    }

    fn value(&self, element: &AxElement) -> Result<Option<String>> {
        Ok(ax::string(element, "AXValue"))
    }

    fn select_text(&self, element: &AxElement, value: &str, range: Range<usize>) -> Result<()> {
        let start = utf16_offset(value, range.start);
        let len = utf16_offset(value, range.end) - start;
        let cf_range = CFRange::new(
            isize::try_from(start).map_err(|_| Error::invalid("selection start is out of range"))?,
            isize::try_from(len).map_err(|_| Error::invalid("selection length is out of range"))?,
        );
        let _ = ax::set_bool(element, "AXFocused", true);
        ax::set_range(element, "AXSelectedTextRange", cf_range)?;
        // Verify, since some apps accept the write but ignore it.
        let applied = ax::attribute(element, "AXSelectedTextRange").as_deref().and_then(ax::as_range);
        match applied {
            Some(r) if r.location == cf_range.location && r.length == cf_range.length => Ok(()),
            Some(_) => Err(Error::action("the app did not apply the requested selection")),
            None => Ok(()),
        }
    }

    fn insert_text(&self, app: &AppInfo, text: &str) -> Result<bool> {
        let app_element = ax::application(pid_of(app)?);
        let Some(focused) = ax::element(&app_element, "AXFocusedUIElement") else { return Ok(false) };
        let role = ax::string(&focused, "AXRole").unwrap_or_default();
        if !INSERTABLE_ROLES.contains(&role.as_str()) || !ax::is_settable(&focused, "AXSelectedText") {
            return Ok(false);
        }
        let before = ax::string(&focused, "AXValue");
        ax::set(&focused, "AXSelectedText", &CFString::from_str(text))?;
        // Some editors accept the write without changing anything; let the caller fall back.
        Ok(ax::string(&focused, "AXValue") != before)
    }

    fn click(&self, target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()> {
        if target.delivery == Delivery::FocusedBackground
            && button == MouseButton::Left
            && let Some((pid, window)) = focus_target(target)
        {
            let local = Point::new(point.x - window.frame.min_x(), point.y - window.frame.min_y());
            let id = window_server_id(window).unwrap_or_default();
            if skylight::get().click(pid, id, point, local, count)? {
                return Ok(());
            }
        }
        input::click(target, point, button, count)
    }

    fn scroll(&self, target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()> {
        input::scroll(target, point, dx, dy)
    }

    fn drag(&self, target: InputTarget<'_>, from: Point, to: Point) -> Result<()> {
        input::drag(target, from, to)
    }

    fn type_text(&self, target: InputTarget<'_>, text: &str) -> Result<()> {
        let _focus = match (target.delivery, focus_target(target)) {
            (Delivery::FocusedBackground, Some((pid, window))) => focus(pid, window)?,
            _ => None,
        };
        input::type_text(target, text)
    }

    fn press_key(&self, target: InputTarget<'_>, chord: &KeyChord) -> Result<()> {
        let shortcut = menu::shortcut(chord);
        // Background apps ignore key equivalents, so Command chords always get synthetic focus.
        let wants_focus = target.delivery == Delivery::FocusedBackground
            || (target.delivery == Delivery::Background && shortcut.is_some());
        let Some((pid, window)) = focus_target(target).filter(|_| wants_focus) else {
            return input::press_key(target, chord);
        };
        let _focus = focus(pid, window)?;
        if let Some(item) = shortcut.and_then(|wanted| menu::find_item(&ax::application(pid), &wanted)) {
            ax::perform(&item, "AXPress")?;
            std::thread::sleep(std::time::Duration::from_millis(100));
            return Ok(());
        }
        input::press_key(target, chord)
    }
}

fn focus_target(target: InputTarget<'_>) -> Option<(i32, &WindowInfo)> {
    let pid = pid_of(target.app).ok()?;
    let window = target.window?;
    window_server_id(window)?;
    let frontmost = ax::element(&ax::system_wide(), "AXFocusedApplication").and_then(|app| ax::pid(&app));
    (frontmost != Some(pid)).then_some((pid, window))
}

fn window_server_id(window: &WindowInfo) -> Option<u32> {
    window.id.and_then(|id| u32::try_from(id).ok())
}

fn focus(pid: i32, window: &WindowInfo) -> Result<Option<skylight::FocusGuard<'static>>> {
    window_server_id(window).map_or(Ok(None), |id| skylight::get().focus(pid, id))
}
