use std::collections::HashMap;
use std::thread;
use std::time::{Duration, SystemTime};

use crate::app::{self, AppInfo};
use crate::backend::{Backend, CaptureRequest, Delivery, InputTarget, MouseButton, Permissions, ScrollDirection};
use crate::element::{Element, build_tree};
use crate::error::{Error, Result};
use crate::geometry::{Point, Rect};
use crate::keys::KeyChord;
use crate::screenshot::Screenshot;
use crate::state::{AppState, CaptureOptions, ElementQuery, WindowInfo, WindowSelector};
use crate::text::TextSelection;

/// What to click.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub enum ClickTarget {
    /// An element index from the latest [`AppState`].
    Element(usize),
    /// A pixel position in the latest screenshot.
    Pixel {
        /// Horizontal pixel.
        x: f64,
        /// Vertical pixel.
        y: f64,
    },
    /// A point in screen coordinates (top-left origin). Needs no earlier capture.
    Screen {
        /// Horizontal screen point.
        x: f64,
        /// Vertical screen point.
        y: f64,
    },
}

/// How a click is performed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum ClickMethod {
    /// Use the element's accessibility action when that is equivalent, otherwise synthesize
    /// pointer events with the computer's default [`Delivery`].
    #[default]
    Auto,
    /// Only use accessibility actions; requires an element target.
    Accessibility,
    /// Synthesize pointer events delivered to the app in the background.
    Background,
    /// Like `Background`, with the target window given synthetic focus first
    /// (see [`Delivery::FocusedBackground`]).
    FocusedBackground,
    /// Synthesize pointer events through the global input stream.
    Global,
}

impl std::str::FromStr for ClickMethod {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "accessibility" | "ax" => Ok(Self::Accessibility),
            "background" => Ok(Self::Background),
            "focused_background" | "focused-background" => Ok(Self::FocusedBackground),
            "global" => Ok(Self::Global),
            other => Err(Error::invalid(format!("unknown click method {other:?}"))),
        }
    }
}

/// Options for [`Computer::click`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClickOptions {
    /// Button to press.
    pub button: MouseButton,
    /// Number of clicks (2 for a double click).
    pub count: u32,
    /// Strategy.
    pub method: ClickMethod,
}

impl Default for ClickOptions {
    fn default() -> Self {
        Self { button: MouseButton::Left, count: 1, method: ClickMethod::Auto }
    }
}

impl ClickOptions {
    /// Sets the button.
    #[must_use]
    pub const fn button(mut self, button: MouseButton) -> Self {
        self.button = button;
        self
    }

    /// Sets the click count.
    #[must_use]
    pub const fn count(mut self, count: u32) -> Self {
        self.count = count;
        self
    }

    /// Sets the method.
    #[must_use]
    pub const fn method(mut self, method: ClickMethod) -> Self {
        self.method = method;
        self
    }
}

/// How a click was ultimately delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ClickOutcome {
    /// An accessibility action was invoked.
    Action(String),
    /// Pointer events were synthesized.
    Pointer(Delivery),
}

struct Session<H> {
    state: AppState,
    handles: Vec<H>,
    /// The window explicitly requested for this state, if any.
    window: Option<u64>,
}

/// A computer-use session over a [`Backend`].
///
/// Observe an app with [`get_app_state`](Self::get_app_state), then act on it by element index or by
/// screenshot pixel. It remembers the latest state per app so indexes stay valid until the
/// next capture of that app.
///
/// ```no_run
/// # #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
/// # fn main() -> computer_use::Result<()> {
/// use computer_use::{CaptureOptions, ClickOptions, ClickTarget, Computer};
///
/// let mut computer = Computer::new()?;
/// let state = computer.get_app_state("TextEdit", &CaptureOptions::default())?;
/// println!("{}", state.render());
///
/// computer.click("TextEdit", ClickTarget::Element(3), ClickOptions::default())?;
/// computer.type_text("TextEdit", "Hello from Rust")?;
/// computer.press_key("TextEdit", "super+s")?;
/// # Ok(())
/// # }
/// # #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
/// # fn main() {}
/// ```
pub struct Computer<B: Backend> {
    backend: B,
    sessions: HashMap<String, Session<B::Handle>>,
    delivery: Delivery,
    settle_delay: Duration,
}

impl<B: Backend> std::fmt::Debug for Computer<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Computer")
            .field("sessions", &self.sessions.keys().collect::<Vec<_>>())
            .field("delivery", &self.delivery)
            .field("settle_delay", &self.settle_delay)
            .finish_non_exhaustive()
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
impl Computer<crate::NativeBackend> {
    /// Creates a session over this platform's native backend.
    ///
    /// # Errors
    ///
    /// Fails if the platform's accessibility stack cannot be initialized.
    pub fn new() -> Result<Self> {
        Ok(Self::with_backend(crate::NativeBackend::new()?))
    }
}

impl<B: Backend> Computer<B> {
    /// Creates a session over a custom backend.
    #[must_use]
    pub fn with_backend(backend: B) -> Self {
        Self { backend, sessions: HashMap::new(), delivery: Delivery::Background, settle_delay: Duration::ZERO }
    }

    /// Sets the default input delivery for synthesized events.
    #[must_use]
    pub const fn delivery(mut self, delivery: Delivery) -> Self {
        self.delivery = delivery;
        self
    }

    /// Sleeps this long after every action so the UI can settle before the next capture.
    #[must_use]
    pub const fn settle_delay(mut self, delay: Duration) -> Self {
        self.settle_delay = delay;
        self
    }

    /// The underlying backend, for platform-specific calls.
    pub const fn backend(&self) -> &B {
        &self.backend
    }

    /// Permissions currently held by this process.
    pub fn permissions(&self) -> Permissions {
        self.backend.permissions()
    }

    /// Prompts for missing permissions where the OS supports it.
    pub fn request_permissions(&self) -> Permissions {
        self.backend.request_permissions()
    }

    /// Lists running apps first (frontmost first), then installed apps, by name.
    ///
    /// # Errors
    ///
    /// Fails if the platform cannot enumerate apps.
    pub fn list_apps(&self) -> Result<Vec<AppInfo>> {
        let mut apps = self.backend.list_apps()?;
        apps.sort_by(|a, b| {
            (!a.is_running, !a.is_active, a.name.to_lowercase()).cmp(&(
                !b.is_running,
                !b.is_active,
                b.name.to_lowercase(),
            ))
        });
        Ok(apps)
    }

    /// Resolves an app query (name, bundle ID, path, or `pid:<n>`), preferring running apps.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AppNotFound`] when nothing matches, or [`Error::AmbiguousApp`] when several
    /// running apps match equally well.
    pub fn resolve_app(&self, query: &str) -> Result<AppInfo> {
        app::resolve(&self.backend.list_apps()?, query)
    }

    /// Captures the app's current window, replacing any earlier state (and element indexes)
    /// for that app.
    ///
    /// # Errors
    ///
    /// Fails if the app cannot be found or launched, has no window, or permissions are missing.
    pub fn get_app_state(&mut self, app: &str, options: &CaptureOptions) -> Result<&AppState> {
        let info = self.running_app(app, options.launch_if_needed)?;
        let window = match &options.window {
            None => None,
            Some(WindowSelector::Id(id)) => Some(*id),
            Some(WindowSelector::Title(title)) => Some(self.window_by_title(&info, title)?),
        };
        let request =
            CaptureRequest::new(options.include_tree, options.include_screenshot, options.max_nodes, options.max_depth)
                .with_window(window)
                .with_menu_bar(options.include_menu_bar);
        let capture = self.backend.capture(&info, &request)?;
        let (elements, handles, raw_to_element) = build_tree(capture.nodes, options.text_limit);
        let focused = capture.focused.and_then(|raw| raw_to_element.get(raw).copied().flatten());
        let screenshot = capture
            .screenshot
            .map(|image| Screenshot::encode(image, capture.window.frame, options.max_screenshot_dimension))
            .transpose()?;
        let state = AppState {
            app: info,
            window: capture.window,
            elements,
            truncated: capture.truncated,
            tree_included: options.include_tree,
            focused,
            selected_text: capture.selected_text.map(|s| crate::element::sanitize(&s, options.text_limit)),
            screenshot,
            captured_at: SystemTime::now(),
            notes: capture.notes,
        };
        let key = state.app.pid.map_or_else(|| state.app.reference().to_lowercase(), |pid| format!("pid:{pid}"));
        let session = Session { state, handles, window };
        Ok(&self.sessions.entry(key).insert_entry(session).into_mut().state)
    }

    /// The app's windows, front to back where the platform knows the order. Pass one to
    /// [`CaptureOptions::window`] to capture it instead of the focused or main window.
    ///
    /// # Errors
    ///
    /// Fails if the app is not running or the backend cannot list windows.
    pub fn list_windows(&self, app: &str) -> Result<Vec<WindowInfo>> {
        let info = self.running_app(app, false)?;
        self.backend.list_windows(&info)
    }

    /// The latest captured state of an app, if any.
    #[must_use]
    pub fn state(&self, app: &str) -> Option<&AppState> {
        self.session_for(app).ok().map(|s| &s.state)
    }

    /// Discards the remembered state of one app.
    pub fn forget(&mut self, app: &str) {
        if let Ok(key) = self.session_key(app) {
            self.sessions.remove(&key);
        }
    }

    /// Discards all remembered state.
    pub fn clear(&mut self) {
        self.sessions.clear();
    }

    /// Finds elements in the app's current window without taking a screenshot.
    ///
    /// The fresh tree replaces the app's remembered state, so the returned indexes can be used
    /// with element actions. A previous screenshot is kept when the window has not moved.
    ///
    /// # Errors
    ///
    /// Fails like [`get_app_state`](Self::get_app_state) or if the query is empty.
    pub fn query(&mut self, app: &str, query: &ElementQuery, max_nodes: usize) -> Result<Vec<Element>> {
        if query.text.as_deref().is_none_or(str::is_empty) && query.role.as_deref().is_none_or(str::is_empty) {
            return Err(Error::invalid("query requires text and/or role"));
        }
        let previous = self.session_for(app).ok().map(|s| (s.state.window.frame, s.state.screenshot.clone()));
        let mut options =
            CaptureOptions::default().include_screenshot(false).max_nodes(max_nodes).launch_if_needed(false);
        // Stay on the window the last capture chose, if it was one other than the default.
        options.window = self.session_for(app).ok().and_then(|s| s.window.map(WindowSelector::Id));
        let state = self.get_app_state(app, &options)?;
        let matches: Vec<Element> = state.find(query).cloned().collect();
        let frame = state.window.frame;
        if let Some((prev_frame, Some(shot))) = previous
            && prev_frame == frame
            && let Ok(session) = self.session_for_mut(app)
        {
            session.state.screenshot = Some(shot);
        }
        Ok(matches)
    }

    /// Clicks an element, a screenshot pixel, or a screen point.
    ///
    /// # Errors
    ///
    /// Fails if there is no state for the app, the target is invalid, or delivery fails.
    pub fn click(&mut self, app: &str, target: ClickTarget, options: ClickOptions) -> Result<ClickOutcome> {
        if options.count == 0 {
            return Err(Error::invalid("click count must be at least 1"));
        }
        let delivery = match options.method {
            ClickMethod::Global => Delivery::Global,
            ClickMethod::Background => Delivery::Background,
            ClickMethod::FocusedBackground => Delivery::FocusedBackground,
            ClickMethod::Auto | ClickMethod::Accessibility => self.delivery,
        };
        let index = match target {
            ClickTarget::Element(index) => index,
            ClickTarget::Pixel { x, y } | ClickTarget::Screen { x, y } => {
                if options.method == ClickMethod::Accessibility {
                    return Err(Error::invalid("accessibility clicks require an element target"));
                }
                let screen = matches!(target, ClickTarget::Screen { .. });
                self.with_pointer(app, (x, y), screen, delivery, |backend, input, point| {
                    backend.click(input, point, options.button, options.count)
                })?;
                return Ok(ClickOutcome::Pointer(delivery));
            }
        };
        let session = self.session_for(app)?;
        let element = element(session, app, index)?;
        if matches!(options.method, ClickMethod::Auto | ClickMethod::Accessibility)
            && let Some(action) = equivalent_action(element, options)
        {
            let name = action.name.clone();
            self.backend.perform_action(&session.handles[index], &name)?;
            self.settle();
            return Ok(ClickOutcome::Action(name));
        }
        if options.method == ClickMethod::Accessibility {
            return Err(Error::action(format!(
                "element {index} has no accessibility action for a {:?} click",
                options.button
            )));
        }
        let point = element
            .frame
            .filter(|f| !f.is_empty())
            .map(|f| f.center())
            .ok_or_else(|| Error::StateUnavailable(format!("element {index} has no on-screen frame")))?;
        let input = input_target(&session.state, delivery);
        self.backend.click(input, point, options.button, options.count)?;
        self.settle();
        Ok(ClickOutcome::Pointer(delivery))
    }

    /// Invokes a named accessibility action on an element (e.g. `Show Menu`, `AXRaise`).
    ///
    /// # Errors
    ///
    /// Fails if the element does not expose the action.
    pub fn perform_secondary_action(&mut self, app: &str, index: usize, action: &str) -> Result<()> {
        let session = self.session_for(app)?;
        let element = element(session, app, index)?;
        let found = element.action(action).ok_or_else(|| {
            let available: Vec<&str> = element.actions.iter().map(|a| a.label.as_str()).collect();
            Error::invalid(format!(
                "{action:?} is not a valid action for element {index}; available: [{}]",
                available.join(", ")
            ))
        })?;
        self.backend.perform_action(&session.handles[index], &found.name)?;
        self.settle();
        Ok(())
    }

    /// Scrolls an element by a number of pages (fractions allowed).
    ///
    /// Whole pages use the element's own scroll-by-page action when it has one; otherwise
    /// wheel events are posted at the element's center.
    ///
    /// # Errors
    ///
    /// Fails if `pages` is not positive or the element has no frame.
    pub fn scroll(&mut self, app: &str, index: usize, direction: ScrollDirection, pages: f64) -> Result<()> {
        if !pages.is_finite() || pages <= 0.0 {
            return Err(Error::invalid("pages must be a positive number"));
        }
        let session = self.session_for(app)?;
        let element = element(session, app, index)?;
        let by_page = format!("Scroll {} By Page", direction.as_str());
        if pages.fract() == 0.0
            && pages <= 100.0
            && let Some(action) = element.action(&by_page)
        {
            for _ in 0..whole_pages(pages) {
                self.backend.perform_action(&session.handles[index], &action.name)?;
            }
            self.settle();
            return Ok(());
        }
        let frame = element
            .frame
            .filter(|f| !f.is_empty())
            .ok_or_else(|| Error::StateUnavailable(format!("element {index} has no scrollable frame")))?;
        let (dx, dy) = wheel_delta(frame, direction, pages);
        let input = input_target(&session.state, self.delivery);
        self.backend.scroll(input, frame.center(), dx, dy)?;
        self.settle();
        Ok(())
    }

    /// Scrolls at an element, a screenshot pixel, or a screen point, by a number of pages
    /// (fractions allowed).
    ///
    /// An element target behaves like [`scroll`](Self::scroll). For a point, wheel events are
    /// posted there; a page is 80% of the scroll area under the point when the latest state
    /// has one, else of the captured window, else 400 points.
    ///
    /// # Errors
    ///
    /// Fails if `pages` is not positive, a pixel lies outside the screenshot, or delivery fails.
    pub fn scroll_at(&mut self, app: &str, target: ClickTarget, direction: ScrollDirection, pages: f64) -> Result<()> {
        let (x, y, screen) = match target {
            ClickTarget::Element(index) => return self.scroll(app, index, direction, pages),
            ClickTarget::Pixel { x, y } => (x, y, false),
            ClickTarget::Screen { x, y } => (x, y, true),
        };
        if !pages.is_finite() || pages <= 0.0 {
            return Err(Error::invalid("pages must be a positive number"));
        }
        let state = self.session_for(app).ok().map(|s| &s.state);
        let page_frame = |point: Point| {
            state.and_then(|s| scroll_area_at(s, point).or(Some(s.window.frame))).filter(|f| !f.is_empty())
        };
        let delivery = self.delivery;
        self.with_pointer(app, (x, y), screen, delivery, |backend, input, point| {
            let frame = page_frame(point).unwrap_or(Rect::new(0.0, 0.0, FALLBACK_PAGE, FALLBACK_PAGE));
            let (dx, dy) = wheel_delta(frame, direction, pages);
            backend.scroll(input, point, dx, dy)
        })
    }

    /// Drags between two screenshot pixels.
    ///
    /// # Errors
    ///
    /// Fails if there is no screenshot or a point lies outside it.
    pub fn drag(&mut self, app: &str, from: (f64, f64), to: (f64, f64)) -> Result<()> {
        let session = self.session_for(app)?;
        let start = screenshot_point(&session.state, from.0, from.1)?;
        let end = screenshot_point(&session.state, to.0, to.1)?;
        let input = input_target(&session.state, self.delivery);
        self.backend.drag(input, start, end)?;
        self.settle();
        Ok(())
    }

    /// Drags between two screen points. Needs no earlier capture.
    ///
    /// # Errors
    ///
    /// Fails if the app is not running or delivery fails.
    pub fn drag_screen(&mut self, app: &str, from: Point, to: Point) -> Result<()> {
        let delivery = self.delivery;
        self.with_pointer(app, (from.x, from.y), true, delivery, |backend, input, start| backend.drag(input, start, to))
    }

    /// Types literal text into the app's focused element.
    ///
    /// With background delivery, text is inserted through accessibility when the focused
    /// element supports it, and synthesized as keystrokes otherwise.
    ///
    /// # Errors
    ///
    /// Fails if the app is not running or delivery fails.
    pub fn type_text(&mut self, app: &str, text: &str) -> Result<()> {
        if text.is_empty() {
            return Ok(());
        }
        let (info, window) = self.input_context(app)?;
        if self.delivery != Delivery::Global && self.backend.insert_text(&info, text)? {
            self.settle();
            return Ok(());
        }
        let target = InputTarget { app: &info, window: window.as_ref(), delivery: self.delivery };
        self.backend.type_text(target, text)?;
        self.settle();
        Ok(())
    }

    /// Presses a key chord in xdotool syntax, e.g. `Return`, `super+c`, `KP_0`.
    ///
    /// # Errors
    ///
    /// Fails if the chord cannot be parsed or delivered.
    pub fn press_key(&mut self, app: &str, key: &str) -> Result<()> {
        let chord: KeyChord = key.parse()?;
        let (info, window) = self.input_context(app)?;
        let target = InputTarget { app: &info, window: window.as_ref(), delivery: self.delivery };
        self.backend.press_key(target, &chord)?;
        self.settle();
        Ok(())
    }

    /// Sets the value of a settable element.
    ///
    /// # Errors
    ///
    /// Fails if the element's value is read-only.
    pub fn set_value(&mut self, app: &str, index: usize, value: &str) -> Result<()> {
        let session = self.session_for(app)?;
        element(session, app, index)?;
        self.backend.set_value(&session.handles[index], value)?;
        self.settle();
        Ok(())
    }

    /// Selects text inside a text element, or places the caret before or after it.
    ///
    /// # Errors
    ///
    /// Fails if the text is missing or ambiguous, or the element does not support selection.
    pub fn select_text(&mut self, app: &str, index: usize, selection: &TextSelection) -> Result<()> {
        let session = self.session_for(app)?;
        let cached = element(session, app, index)?;
        let handle = &session.handles[index];
        let value = match self.backend.value(handle) {
            Ok(Some(value)) => value,
            Ok(None) => return Err(Error::StateUnavailable(format!("element {index} has no text value"))),
            Err(Error::Unsupported(_)) => cached
                .value
                .clone()
                .ok_or_else(|| Error::StateUnavailable(format!("element {index} has no text value")))?,
            Err(e) => return Err(e),
        };
        let range = selection.resolve(&value)?;
        self.backend.select_text(handle, &value, range)?;
        self.settle();
        Ok(())
    }

    /// Resolves a screenshot pixel (or a screen point when `screen`) and runs a pointer action
    /// there, then settles. Screen points use the remembered window when there is one, and
    /// need no capture otherwise.
    fn with_pointer(
        &self,
        app: &str,
        (x, y): (f64, f64),
        screen: bool,
        delivery: Delivery,
        act: impl FnOnce(&B, InputTarget<'_>, Point) -> Result<()>,
    ) -> Result<()> {
        if screen {
            if !x.is_finite() || !y.is_finite() {
                return Err(Error::invalid(format!("({x}, {y}) is not a screen point")));
            }
            let (info, window) = self.input_context(app)?;
            act(&self.backend, InputTarget { app: &info, window: window.as_ref(), delivery }, Point::new(x, y))?;
        } else {
            let session = self.session_for(app)?;
            let point = screenshot_point(&session.state, x, y)?;
            act(&self.backend, input_target(&session.state, delivery), point)?;
        }
        self.settle();
        Ok(())
    }

    fn window_by_title(&self, app: &AppInfo, title: &str) -> Result<u64> {
        let wanted = title.trim().to_lowercase();
        let windows = self.backend.list_windows(app)?;
        let titled = |w: &&WindowInfo| w.id.is_some() && w.title.is_some();
        let lower = |w: &WindowInfo| w.title.as_deref().unwrap_or_default().to_lowercase();
        let mut matches: Vec<&WindowInfo> = windows.iter().filter(titled).filter(|w| lower(w) == wanted).collect();
        if matches.is_empty() {
            matches = windows.iter().filter(titled).filter(|w| lower(w).contains(&wanted)).collect();
        }
        match matches.as_slice() {
            [] => Err(Error::WindowNotFound(format!("{} (no window titled {title:?})", app.name))),
            [window] => window.id.ok_or_else(|| Error::WindowNotFound(app.name.clone())),
            _ => Err(Error::AmbiguousWindow {
                title: title.to_owned(),
                candidates: matches.into_iter().cloned().collect(),
            }),
        }
    }

    fn running_app(&self, query: &str, launch: bool) -> Result<AppInfo> {
        let info = self.resolve_app(query)?;
        if info.is_running {
            Ok(info)
        } else if launch {
            self.backend.launch_app(&info)
        } else {
            Err(Error::AppNotFound(format!("{query} is not running")))
        }
    }

    fn input_context(&self, app: &str) -> Result<(AppInfo, Option<WindowInfo>)> {
        match self.session_for(app) {
            Ok(session) => Ok((session.state.app.clone(), Some(session.state.window.clone()))),
            Err(Error::NoAppState(_)) => Ok((self.running_app(app, false)?, None)),
            Err(error) => Err(error),
        }
    }

    /// The observed app that best matches `app`; several equally good matches are ambiguous.
    fn session_key(&self, app: &str) -> Result<String> {
        let mut matches: Vec<_> = self
            .sessions
            .iter()
            .filter_map(|(key, s)| Some((s.state.app.match_score(app)?, key, &s.state.app)))
            .collect();
        matches.sort_by_key(|(score, _, info)| (*score, info.pid));
        let best: Vec<_> = matches.iter().take_while(|(score, ..)| *score == matches[0].0).collect();
        match best.as_slice() {
            [] => Err(Error::NoAppState(app.to_owned())),
            [(_, key, _)] => Ok((*key).clone()),
            _ => Err(Error::AmbiguousApp {
                query: app.to_owned(),
                candidates: best.iter().map(|(_, _, info)| (*info).clone()).collect(),
            }),
        }
    }

    fn session_for(&self, app: &str) -> Result<&Session<B::Handle>> {
        let key = self.session_key(app)?;
        Ok(&self.sessions[&key])
    }

    fn session_for_mut(&mut self, app: &str) -> Result<&mut Session<B::Handle>> {
        let key = self.session_key(app)?;
        self.sessions.get_mut(&key).ok_or_else(|| Error::NoAppState(app.to_owned()))
    }

    fn settle(&self) {
        if !self.settle_delay.is_zero() {
            thread::sleep(self.settle_delay);
        }
    }
}

fn element<'a, H>(session: &'a Session<H>, app: &str, index: usize) -> Result<&'a Element> {
    session.state.element(index).ok_or_else(|| Error::ElementNotFound { app: app.to_owned(), index })
}

fn equivalent_action(element: &Element, options: ClickOptions) -> Option<&crate::element::Action> {
    match (options.button, options.count) {
        (MouseButton::Left, 1) => element.primary_action(),
        (MouseButton::Right, 1) => element.action("Show Menu"),
        _ => None,
    }
}

fn screenshot_point(state: &AppState, x: f64, y: f64) -> Result<Point> {
    let shot = state.screenshot.as_ref().ok_or_else(|| {
        Error::StateUnavailable("no screenshot in the latest state; capture one before coordinate actions".into())
    })?;
    if !shot.contains_pixel(x, y) {
        return Err(Error::invalid(format!("({x}, {y}) is outside the {}x{} screenshot", shot.width(), shot.height())));
    }
    Ok(shot.to_screen_point(x, y))
}

/// Points per page when nothing tells how large the scrolled area is.
const FALLBACK_PAGE: f64 = 500.0;

/// The wheel delta for scrolling `frame` by `pages`: a page is 80% of the frame (at least 40).
fn wheel_delta(frame: Rect, direction: ScrollDirection, pages: f64) -> (f64, f64) {
    let page_x = (frame.size.width * 0.8).max(40.0);
    let page_y = (frame.size.height * 0.8).max(40.0);
    match direction {
        ScrollDirection::Up => (0.0, -page_y * pages),
        ScrollDirection::Down => (0.0, page_y * pages),
        ScrollDirection::Left => (-page_x * pages, 0.0),
        ScrollDirection::Right => (page_x * pages, 0.0),
    }
}

/// The frame of the innermost scroll container under `point`.
fn scroll_area_at(state: &AppState, point: Point) -> Option<Rect> {
    state
        .elements
        .iter()
        .filter(|e| e.role.contains("scroll") && !e.role.contains("bar"))
        .filter_map(|e| e.frame)
        .filter(|f| !f.is_empty() && f.contains(point))
        .min_by(|a, b| a.area().total_cmp(&b.area()))
}

const fn input_target(state: &AppState, delivery: Delivery) -> InputTarget<'_> {
    InputTarget { app: &state.app, window: Some(&state.window), delivery }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
const fn whole_pages(pages: f64) -> u32 {
    pages as u32
}
