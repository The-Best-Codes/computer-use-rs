mod a11y;
mod apps;
mod input;
mod keys;
mod screenshot;
mod session;

use std::ops::Range;
use std::sync::Arc;

use zbus::blocking::Connection;

use self::screenshot::Shot;
use self::session::SessionEnv;

use crate::app::AppInfo;
use crate::backend::{Backend, Capture, CaptureRequest, InputTarget, MouseButton};
use crate::error::Result;
use crate::geometry::Point;
use crate::keys::KeyChord;
use crate::state::WindowInfo;

/// A live AT-SPI accessible object: the owning app's bus name plus the object path.
///
/// Proxies for the object's interfaces are built on demand from this pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LinuxElement {
    bus: Arc<str>,
    path: Arc<str>,
}

impl LinuxElement {
    pub(crate) fn new(bus: &str, path: &str) -> Self {
        Self { bus: bus.into(), path: path.into() }
    }

    /// The bus name of the app that owns the object.
    #[must_use]
    pub fn bus_name(&self) -> &str {
        &self.bus
    }

    /// The object path of the accessible.
    #[must_use]
    pub fn object_path(&self) -> &str {
        &self.path
    }
}

/// The Linux implementation of [`Backend`], built on AT-SPI.
#[derive(Debug, Clone)]
pub struct LinuxBackend {
    conn: Connection,
    session: Arc<SessionEnv>,
}

impl LinuxBackend {
    /// Connects to the AT-SPI accessibility bus.
    ///
    /// Session variables this process lacks (`DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`,
    /// `XAUTHORITY`, `XDG_RUNTIME_DIR`, `AT_SPI_BUS_ADDRESS`, ...) are discovered from the
    /// user's running desktop processes, so the backend also works from services, cron jobs and SSH sessions.
    ///
    /// Also turns on the desktop's accessibility flag (`org.a11y.Status.IsEnabled`),
    /// which some apps need in order to expose their UI. Apps of that kind that were
    /// already running before the flag was on must be restarted to show up.
    ///
    /// # Errors
    ///
    /// Fails if there is no D-Bus session or the accessibility bus is not running.
    pub fn new() -> Result<Self> {
        let session = SessionEnv::discover();
        a11y::enable_accessibility(&session);
        Ok(Self { conn: a11y::connect(&session)?, session: Arc::new(session) })
    }
}

impl Backend for LinuxBackend {
    type Handle = LinuxElement;

    fn list_apps(&self) -> Result<Vec<AppInfo>> {
        apps::list_apps(&self.conn)
    }

    fn launch_app(&self, app: &AppInfo) -> Result<AppInfo> {
        apps::launch_app(&self.conn, &self.session, app)
    }

    fn list_windows(&self, app: &AppInfo) -> Result<Vec<WindowInfo>> {
        let root = apps::find_running(&self.conn, app)?;
        a11y::list_windows(&self.conn, &root)
    }

    fn capture(&self, app: &AppInfo, request: &CaptureRequest) -> Result<Capture<Self::Handle>> {
        let root = apps::find_running(&self.conn, app)?;
        let window = a11y::main_window(&self.conn, &root, &app.name, request.window)?;
        let mut capture = Capture::new(window.info);
        if request.include_tree {
            let walk = a11y::walk(&self.conn, &window.element, request.max_nodes, request.max_depth)?;
            capture.nodes = walk.nodes;
            capture.truncated = walk.truncated;
            capture.focused = walk.focused;
            capture.selected_text = walk.selected_text;
        }
        if request.include_screenshot {
            match screenshot::capture(&self.session, capture.window.frame)? {
                Shot::Captured(frame, image) => {
                    capture.window.frame = frame;
                    capture.screenshot = Some(image);
                }
                Shot::Omitted(note) => capture.notes.push(note),
            }
        }
        Ok(capture)
    }

    fn perform_action(&self, element: &Self::Handle, action: &str) -> Result<()> {
        a11y::perform_action(&self.conn, element, action)
    }

    fn set_value(&self, element: &Self::Handle, value: &str) -> Result<()> {
        a11y::set_value(&self.conn, element, value)
    }

    fn value(&self, element: &Self::Handle) -> Result<Option<String>> {
        a11y::value(&self.conn, element)
    }

    fn select_text(&self, element: &Self::Handle, value: &str, range: Range<usize>) -> Result<()> {
        a11y::select_text(&self.conn, element, value, range)
    }

    fn click(&self, _target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()> {
        input::click(&self.conn, point, button, count)
    }

    fn scroll(&self, _target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()> {
        input::scroll(&self.conn, point, dx, dy)
    }

    fn drag(&self, _target: InputTarget<'_>, from: Point, to: Point) -> Result<()> {
        input::drag(&self.conn, from, to)
    }

    fn type_text(&self, _target: InputTarget<'_>, text: &str) -> Result<()> {
        input::type_text(&self.conn, text)
    }

    fn press_key(&self, _target: InputTarget<'_>, chord: &KeyChord) -> Result<()> {
        input::press_key(&self.conn, chord)
    }
}
