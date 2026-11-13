use std::ops::Range;

use crate::app::AppInfo;
use crate::element::RawNode;
use crate::error::{Error, Result};
use crate::geometry::Point;
use crate::keys::KeyChord;
use crate::screenshot::RawImage;
use crate::state::WindowInfo;

/// A mouse button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum MouseButton {
    /// Primary button.
    #[default]
    Left,
    /// Secondary button.
    Right,
    /// Middle button / wheel click.
    Middle,
}

impl std::str::FromStr for MouseButton {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "left" | "primary" => Ok(Self::Left),
            "right" | "secondary" => Ok(Self::Right),
            "middle" | "center" => Ok(Self::Middle),
            other => Err(Error::invalid(format!("unknown mouse button {other:?}"))),
        }
    }
}

/// A scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ScrollDirection {
    /// Reveal content above.
    Up,
    /// Reveal content below.
    Down,
    /// Reveal content to the left.
    Left,
    /// Reveal content to the right.
    Right,
}

impl ScrollDirection {
    /// Lowercase name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

impl std::str::FromStr for ScrollDirection {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "up" => Ok(Self::Up),
            "down" => Ok(Self::Down),
            "left" => Ok(Self::Left),
            "right" => Ok(Self::Right),
            other => Err(Error::invalid(format!("unknown scroll direction {other:?}"))),
        }
    }
}

/// How synthetic input reaches the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Delivery {
    /// Post events directly to the target app without moving the user's pointer or
    /// changing focus, where the platform allows it. Backends that cannot target an app
    /// may fall back to [`Delivery::Global`]. Linux instead returns [`Error::Unsupported`]
    /// for synthesized input unless `Global` is explicitly selected. Accessibility actions
    /// remain available without global input.
    #[default]
    Background,
    /// Like [`Delivery::Background`], but the target window is also made active and key
    /// *inside its own process* for the duration of the input, without changing the real
    /// frontmost app, raising the window, or moving the pointer.
    ///
    /// On macOS this uses private `SkyLight` window-server calls and is what Chromium/Electron
    /// apps need to accept background clicks and typing. Command-key shortcuts use it automatically.
    /// Other platforms treat it as [`Delivery::Background`].
    FocusedBackground,
    /// Post events through the system input stream. May move the pointer and activate the app.
    /// On Linux this does not activate or verify the target: keys go to the current focus,
    /// and pointer events go to the supplied screen coordinates, possibly in another app.
    Global,
}

/// Where synthetic input should be delivered.
#[derive(Debug, Clone, Copy)]
pub struct InputTarget<'a> {
    /// The receiving app (must be running).
    pub app: &'a AppInfo,
    /// The receiving window, when known.
    pub window: Option<&'a WindowInfo>,
    /// Delivery path.
    pub delivery: Delivery,
}

/// Which OS permissions the current process holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Permissions {
    /// Accessibility / UI automation access.
    pub accessibility: bool,
    /// Screen capture access.
    pub screen_recording: bool,
}

impl Permissions {
    /// Every permission granted.
    pub const GRANTED: Self = Self { accessibility: true, screen_recording: true };

    /// Creates a permission report.
    #[must_use]
    pub const fn new(accessibility: bool, screen_recording: bool) -> Self {
        Self { accessibility, screen_recording }
    }

    /// True when nothing is missing.
    #[must_use]
    pub const fn all_granted(&self) -> bool {
        self.accessibility && self.screen_recording
    }
}

/// Parameters for [`Backend::capture`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct CaptureRequest {
    /// Walk the accessibility tree.
    pub include_tree: bool,
    /// Capture window pixels.
    pub include_screenshot: bool,
    /// Maximum raw nodes to visit.
    pub max_nodes: usize,
    /// Maximum depth to visit.
    pub max_depth: usize,
    /// Capture the window with this [`WindowInfo::id`] instead of the focused or main window.
    /// Backends that cannot honor it must fail rather than capture another window.
    pub window: Option<u64>,
    /// Also walk the app's menu bar when it lives outside the window (macOS). Its nodes follow
    /// the window's, with `parent: None` on the menu bar itself. Counts toward `max_nodes`.
    pub include_menu_bar: bool,
}

impl CaptureRequest {
    /// Creates a request.
    #[must_use]
    pub const fn new(include_tree: bool, include_screenshot: bool, max_nodes: usize, max_depth: usize) -> Self {
        Self { include_tree, include_screenshot, max_nodes, max_depth, window: None, include_menu_bar: false }
    }

    /// Sets [`window`](Self::window).
    #[must_use]
    pub const fn with_window(mut self, window: Option<u64>) -> Self {
        self.window = window;
        self
    }

    /// Sets [`include_menu_bar`](Self::include_menu_bar).
    #[must_use]
    pub const fn with_menu_bar(mut self, yes: bool) -> Self {
        self.include_menu_bar = yes;
        self
    }
}

/// What a backend returns from [`Backend::capture`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Capture<H> {
    /// The captured window.
    pub window: WindowInfo,
    /// Accessibility nodes in depth-first pre-order, rooted at the window.
    pub nodes: Vec<RawNode<H>>,
    /// Whether limits cut the walk short.
    pub truncated: bool,
    /// Index into `nodes` of the focused element.
    pub focused: Option<usize>,
    /// Selected text in the focused element.
    pub selected_text: Option<String>,
    /// Window pixels; the image must cover exactly `window.frame`.
    pub screenshot: Option<RawImage>,
    /// Non-fatal problems worth surfacing, e.g. why a screenshot is missing.
    pub notes: Vec<String>,
}

impl<H> Capture<H> {
    /// A capture of `window` with nothing else filled in.
    #[must_use]
    pub const fn new(window: WindowInfo) -> Self {
        Self {
            window,
            nodes: Vec::new(),
            truncated: false,
            focused: None,
            selected_text: None,
            screenshot: None,
            notes: Vec::new(),
        }
    }
}

/// OS primitives for observing and driving apps.
///
/// All coordinates are screen points with a top-left origin. Methods with default
/// implementations return [`Error::Unsupported`].
pub trait Backend {
    /// Handle to a live accessibility element (e.g. `AXUIElement`, `IUIAutomationElement`).
    type Handle: Clone;

    /// Running apps plus installed apps the user can launch.
    ///
    /// # Errors
    ///
    /// Fails if the platform's process or app registry cannot be read.
    fn list_apps(&self) -> Result<Vec<AppInfo>>;

    /// Starts `app` without bringing it to the foreground and returns its running record.
    ///
    /// # Errors
    ///
    /// Fails if the app cannot be started or does not appear in time.
    fn launch_app(&self, app: &AppInfo) -> Result<AppInfo> {
        Err(Error::Unsupported(format!("launching {} is not supported by this backend", app.name)))
    }

    /// Permissions currently held by this process.
    fn permissions(&self) -> Permissions {
        Permissions::GRANTED
    }

    /// Asks the OS to prompt the user for missing permissions, then reports the current state.
    fn request_permissions(&self) -> Permissions {
        self.permissions()
    }

    /// The app's windows, front to back where the platform knows the order. Each one with an
    /// [`id`](WindowInfo::id) can be passed back as [`CaptureRequest::window`].
    ///
    /// Unsupported by default.
    ///
    /// # Errors
    ///
    /// Fails if the app is not running or permissions are missing.
    fn list_windows(&self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let _ = app;
        Err(Error::Unsupported("listing windows".into()))
    }

    /// Captures the app's current window, or the one [`CaptureRequest::window`] names.
    ///
    /// # Errors
    ///
    /// Fails if the app has no usable window (or not the requested one) or permissions are
    /// missing.
    fn capture(&self, app: &AppInfo, request: &CaptureRequest) -> Result<Capture<Self::Handle>>;

    /// Invokes a named accessibility action (as listed in [`Action::name`](crate::Action::name)).
    ///
    /// # Errors
    ///
    /// Fails if the element is gone or rejects the action.
    fn perform_action(&self, element: &Self::Handle, action: &str) -> Result<()>;

    /// Writes an element's value.
    ///
    /// # Errors
    ///
    /// Fails if the value is not settable.
    fn set_value(&self, element: &Self::Handle, value: &str) -> Result<()>;

    /// Reads an element's full, untruncated value.
    ///
    /// # Errors
    ///
    /// Fails if the element is gone.
    fn value(&self, element: &Self::Handle) -> Result<Option<String>> {
        let _ = element;
        Err(Error::Unsupported("reading element values".into()))
    }

    /// Selects `range` (byte offsets into `value`, the element's current value).
    /// An empty range places the caret.
    ///
    /// # Errors
    ///
    /// Fails if the element does not support text selection.
    fn select_text(&self, element: &Self::Handle, value: &str, range: Range<usize>) -> Result<()> {
        let _ = (element, value, range);
        Err(Error::Unsupported("text selection".into()))
    }

    /// Inserts text at the caret of the focused element without synthesizing keystrokes.
    /// Returns `Ok(false)` when the focused element does not support it, so the caller can
    /// fall back to [`Backend::type_text`].
    ///
    /// # Errors
    ///
    /// Fails if insertion was attempted and rejected.
    fn insert_text(&self, app: &AppInfo, text: &str) -> Result<bool> {
        let _ = (app, text);
        Ok(false)
    }

    /// Clicks `count` times at a screen point.
    ///
    /// # Errors
    ///
    /// Fails if the events cannot be posted.
    fn click(&self, target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()>;

    /// Scrolls at a screen point by `(dx, dy)` points; positive `dy` reveals content below.
    ///
    /// # Errors
    ///
    /// Fails if the events cannot be posted.
    fn scroll(&self, target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()>;

    /// Presses the primary button at `from`, moves to `to`, and releases.
    ///
    /// # Errors
    ///
    /// Fails if the events cannot be posted.
    fn drag(&self, target: InputTarget<'_>, from: Point, to: Point) -> Result<()>;

    /// Types literal text.
    ///
    /// # Errors
    ///
    /// Fails if the events cannot be posted.
    fn type_text(&self, target: InputTarget<'_>, text: &str) -> Result<()>;

    /// Presses and releases a key chord.
    ///
    /// # Errors
    ///
    /// Fails if the key has no mapping on this platform or events cannot be posted.
    fn press_key(&self, target: InputTarget<'_>, chord: &KeyChord) -> Result<()>;
}
