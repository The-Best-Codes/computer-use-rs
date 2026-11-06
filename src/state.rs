use std::fmt::{self, Write as _};
use std::time::SystemTime;

use crate::app::AppInfo;
use crate::element::{Element, ElementStates};
use crate::geometry::Rect;
use crate::screenshot::Screenshot;

/// The window an [`AppState`] was captured from.
#[derive(Debug, Clone, PartialEq, Default)]
#[non_exhaustive]
pub struct WindowInfo {
    /// Platform window ID, when known: the `CGWindowID` on macOS, the `HWND` on Windows, and an
    /// opaque ID derived from the window's accessibility object on Linux. Pass it to
    /// [`WindowSelector::Id`] to capture this window again.
    pub id: Option<u64>,
    /// Window title.
    pub title: Option<String>,
    /// Frame in screen points.
    pub frame: Rect,
}

impl WindowInfo {
    /// Creates window info.
    #[must_use]
    pub const fn new(id: Option<u64>, title: Option<String>, frame: Rect) -> Self {
        Self { id, title, frame }
    }
}

/// Which of an app's windows to capture.
///
/// [`Computer::list_windows`](crate::Computer::list_windows) lists the candidates.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum WindowSelector {
    /// A [`WindowInfo::id`].
    Id(u64),
    /// A window title, matched case-insensitively: an exact match wins, then the first window
    /// whose title contains it.
    Title(String),
}

/// Limits and toggles for [`Computer::get_app_state`](crate::Computer::get_app_state).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // independent toggles, each with a builder
pub struct CaptureOptions {
    /// Walk the accessibility tree. Disabling this invalidates element indexes.
    pub include_tree: bool,
    /// Capture a screenshot. Disabling this prevents coordinate-based actions.
    pub include_screenshot: bool,
    /// Maximum characters kept per text attribute; `None` keeps everything.
    pub text_limit: Option<usize>,
    /// Maximum number of raw accessibility nodes visited.
    pub max_nodes: usize,
    /// Maximum tree depth visited.
    pub max_depth: usize,
    /// Screenshots larger than this on either side are downscaled; `None` keeps full resolution.
    pub max_screenshot_dimension: Option<u32>,
    /// Launch the app (without activating it) if it is installed but not running.
    pub launch_if_needed: bool,
    /// The window to capture; `None` picks the focused or main window.
    pub window: Option<WindowSelector>,
    /// Also capture the app's menu bar, as a second root after the window's tree.
    ///
    /// Only macOS keeps the menu bar outside the window; on Windows and Linux it is already
    /// part of the window's tree, so this has no effect there.
    pub include_menu_bar: bool,
}

impl Default for CaptureOptions {
    fn default() -> Self {
        Self {
            include_tree: true,
            include_screenshot: true,
            text_limit: Some(500),
            max_nodes: 1200,
            max_depth: 64,
            max_screenshot_dimension: Some(1280),
            launch_if_needed: true,
            window: None,
            include_menu_bar: false,
        }
    }
}

impl CaptureOptions {
    /// Sets [`include_tree`](Self::include_tree).
    #[must_use]
    pub const fn include_tree(mut self, yes: bool) -> Self {
        self.include_tree = yes;
        self
    }

    /// Sets [`include_screenshot`](Self::include_screenshot).
    #[must_use]
    pub const fn include_screenshot(mut self, yes: bool) -> Self {
        self.include_screenshot = yes;
        self
    }

    /// Sets [`text_limit`](Self::text_limit).
    #[must_use]
    pub const fn text_limit(mut self, limit: Option<usize>) -> Self {
        self.text_limit = limit;
        self
    }

    /// Sets [`max_nodes`](Self::max_nodes).
    #[must_use]
    pub const fn max_nodes(mut self, max: usize) -> Self {
        self.max_nodes = max;
        self
    }

    /// Sets [`max_depth`](Self::max_depth).
    #[must_use]
    pub const fn max_depth(mut self, max: usize) -> Self {
        self.max_depth = max;
        self
    }

    /// Sets [`max_screenshot_dimension`](Self::max_screenshot_dimension).
    #[must_use]
    pub const fn max_screenshot_dimension(mut self, max: Option<u32>) -> Self {
        self.max_screenshot_dimension = max;
        self
    }

    /// Sets [`launch_if_needed`](Self::launch_if_needed).
    #[must_use]
    pub const fn launch_if_needed(mut self, yes: bool) -> Self {
        self.launch_if_needed = yes;
        self
    }

    /// Sets [`window`](Self::window).
    #[must_use]
    pub fn window(mut self, window: WindowSelector) -> Self {
        self.window = Some(window);
        self
    }

    /// Sets [`include_menu_bar`](Self::include_menu_bar).
    #[must_use]
    pub const fn include_menu_bar(mut self, yes: bool) -> Self {
        self.include_menu_bar = yes;
        self
    }
}

/// A snapshot of one app window.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AppState {
    /// The app.
    pub app: AppInfo,
    /// The captured window.
    pub window: WindowInfo,
    /// Accessibility elements in depth-first order; `elements[i].index == i`.
    pub elements: Vec<Element>,
    /// Whether node or depth limits cut the tree short.
    pub truncated: bool,
    /// Whether the tree was requested at all.
    pub tree_included: bool,
    /// Index of the focused element, if it is part of the tree.
    pub focused: Option<usize>,
    /// Currently selected text in the focused element.
    pub selected_text: Option<String>,
    /// Screenshot of the window.
    pub screenshot: Option<Screenshot>,
    /// When the state was captured.
    pub captured_at: SystemTime,
    /// Non-fatal problems encountered while capturing, e.g. why the screenshot is missing.
    pub notes: Vec<String>,
}

impl AppState {
    /// Looks up an element by index.
    #[must_use]
    pub fn element(&self, index: usize) -> Option<&Element> {
        self.elements.get(index)
    }

    /// The focused element.
    #[must_use]
    pub fn focused_element(&self) -> Option<&Element> {
        self.focused.and_then(|i| self.element(i))
    }

    /// Elements matching `query`, in tree order.
    pub fn find(&self, query: &ElementQuery) -> impl Iterator<Item = &Element> + use<'_> {
        let query = query.clone();
        let limit = query.limit;
        self.elements.iter().filter(move |e| query.matches(e)).take(limit)
    }

    /// The deepest element whose frame contains the screen point, preferring actionable ones.
    #[must_use]
    pub fn element_at(&self, point: crate::geometry::Point) -> Option<&Element> {
        self.elements.iter().filter(|e| e.frame.is_some_and(|f| f.contains(point))).min_by(|a, b| {
            let rank = |e: &Element| (e.primary_action().is_none(), e.frame.map_or(f64::MAX, |f| f.area()));
            let (ra, rb) = (rank(a), rank(b));
            ra.0.cmp(&rb.0).then(ra.1.total_cmp(&rb.1))
        })
    }

    /// Renders the state as compact, human-readable text (also handy as language-model input).
    ///
    /// Each element is one tab-indented line beginning with its index:
    ///
    /// ```text
    /// App=com.apple.TextEdit (pid 812)
    /// Window: "Untitled", App: TextEdit.
    /// 0 standard window Untitled
    ///     1 text area (focused, settable, editable) Value: Hello
    ///     2 button Close Secondary Actions: Show Menu
    /// ```
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let pid = self.app.pid.map_or_else(String::new, |p| format!(" (pid {p})"));
        let _ = writeln!(out, "App={}{pid}", self.app.reference());
        let title = self.window.title.as_deref().filter(|t| !t.is_empty()).unwrap_or(&self.app.name);
        let _ = write!(out, "Window: {title:?}, App: {}.", self.app.name);
        for element in &self.elements {
            out.push('\n');
            for _ in 0..element.depth {
                out.push('\t');
            }
            render_element(&mut out, element);
        }
        if !self.tree_included {
            out.push_str("\nAccessibility tree omitted (include_tree = false); element indexes are unavailable.");
        } else if self.truncated {
            out.push_str("\n… tree truncated by node or depth limits.");
        }
        for note in &self.notes {
            let _ = write!(out, "\n{note}");
        }
        if let Some(selected) = self.selected_text.as_deref().filter(|s| !s.is_empty()) {
            let _ = write!(out, "\n\nSelected text: [{selected}]");
        } else if let Some(focused) = self.focused_element() {
            out.push_str("\n\nThe focused UI element is ");
            render_element(&mut out, focused);
            out.push('.');
        }
        out
    }
}

impl fmt::Display for AppState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

fn render_element(out: &mut String, e: &Element) {
    let _ = write!(out, "{} {}", e.index, e.role);
    let states = (e.states - ElementStates::OFFSCREEN).labels();
    if !states.is_empty() {
        let _ = write!(out, " ({})", states.join(", "));
    }
    if let Some(title) = &e.title {
        let _ = write!(out, " {title}");
    }
    let mut field = |name: &str, value: Option<&String>| {
        if let Some(v) = value.filter(|v| Some(*v) != e.title.as_ref()) {
            let _ = write!(out, " {name}: {v}");
        }
    };
    field("Description", e.description.as_ref());
    field("Help", e.help.as_ref().filter(|h| Some(*h) != e.description.as_ref()));
    field("URL", e.url.as_ref());
    field("ID", e.identifier.as_ref());
    field("Value", e.value.as_ref());
    if e.value.is_none() {
        field("Placeholder", e.placeholder.as_ref());
    }
    let secondary: Vec<&str> = e.actions.iter().filter(|a| !a.primary).map(|a| a.label.as_str()).collect();
    if !secondary.is_empty() {
        let _ = write!(out, " Secondary Actions: {}", secondary.join(", "));
    }
}

/// Criteria for [`Computer::query`](crate::Computer::query) and [`AppState::find`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ElementQuery {
    /// Text matched against title, description, value, placeholder, identifier, and help.
    pub text: Option<String>,
    /// Role matched against the display role or platform role (an `AX` prefix is optional).
    pub role: Option<String>,
    /// Require an exact (case-insensitive) text match instead of a substring.
    pub exact: bool,
    /// Maximum number of matches.
    pub limit: usize,
}

impl Default for ElementQuery {
    fn default() -> Self {
        Self { text: None, role: None, exact: false, limit: 20 }
    }
}

impl ElementQuery {
    /// Matches elements containing `text`.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self { text: Some(text.into()), ..Self::default() }
    }

    /// Matches elements with `role`.
    #[must_use]
    pub fn role(role: impl Into<String>) -> Self {
        Self { role: Some(role.into()), ..Self::default() }
    }

    /// Adds a role constraint.
    #[must_use]
    pub fn with_role(mut self, role: impl Into<String>) -> Self {
        self.role = Some(role.into());
        self
    }

    /// Requires exact text matches.
    #[must_use]
    pub const fn exact(mut self, exact: bool) -> Self {
        self.exact = exact;
        self
    }

    /// Caps the number of results.
    #[must_use]
    pub const fn limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Whether `element` satisfies the query.
    #[must_use]
    pub fn matches(&self, element: &Element) -> bool {
        if let Some(role) = &self.role {
            let want = normalize_role(role);
            if normalize_role(&element.role) != want && normalize_role(&element.platform_role) != want {
                return false;
            }
        }
        if let Some(text) = &self.text {
            let needle = text.to_lowercase();
            let hit = element.text_fields().any(|field| {
                let field = field.to_lowercase();
                if self.exact { field == needle } else { field.contains(&needle) }
            });
            if !hit {
                return false;
            }
        }
        true
    }
}

fn normalize_role(role: &str) -> String {
    let lowered: String = role.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    lowered.strip_prefix("ax").map(str::to_owned).unwrap_or(lowered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::element::{Action, ElementData, RawNode, build_tree};

    pub(crate) fn sample_state() -> AppState {
        let mut window = ElementData::new("standard window", "AXWindow");
        window.title = Some("Doc".into());
        let mut text = ElementData::new("text area", "AXTextArea");
        text.value = Some("Hello world".into());
        text.states = ElementStates::FOCUSED | ElementStates::SETTABLE;
        text.frame = Some(Rect::new(0.0, 20.0, 200.0, 100.0));
        let mut button = ElementData::new("button", "AXButton");
        button.title = Some("Save".into());
        button.actions = vec![Action::new("AXPress", true), Action::new("AXShowMenu", false)];
        button.frame = Some(Rect::new(10.0, 130.0, 50.0, 20.0));
        let nodes = vec![
            RawNode { handle: (), parent: None, data: window },
            RawNode { handle: (), parent: Some(0), data: text },
            RawNode { handle: (), parent: Some(0), data: button },
        ];
        let (elements, _, _) = build_tree(nodes, Some(500));
        AppState {
            app: AppInfo::new("TextEdit").with_bundle_id("com.apple.TextEdit").with_pid(9),
            window: WindowInfo::new(Some(1), Some("Doc".into()), Rect::new(0.0, 0.0, 300.0, 200.0)),
            elements,
            truncated: false,
            tree_included: true,
            focused: Some(1),
            selected_text: None,
            screenshot: None,
            captured_at: SystemTime::UNIX_EPOCH,
            notes: Vec::new(),
        }
    }

    #[test]
    fn renders_tree() {
        let text = sample_state().render();
        let expected = "App=com.apple.TextEdit (pid 9)\n\
            Window: \"Doc\", App: TextEdit.\n\
            0 standard window Doc\n\
            \t1 text area (focused, settable) Value: Hello world\n\
            \t2 button Save Secondary Actions: Show Menu\n\
            \n\
            The focused UI element is 1 text area (focused, settable) Value: Hello world.";
        assert_eq!(text, expected);
    }

    #[test]
    fn queries_by_text_and_role() {
        let state = sample_state();
        let q = ElementQuery::text("save");
        assert_eq!(state.find(&q).map(|e| e.index).collect::<Vec<_>>(), vec![2]);
        let q = ElementQuery::role("AXTextArea");
        assert_eq!(state.find(&q).map(|e| e.index).collect::<Vec<_>>(), vec![1]);
        let q = ElementQuery::role("button").exact(true);
        assert_eq!(state.find(&q).count(), 1);
        let q = ElementQuery::text("hello").exact(true);
        assert_eq!(state.find(&q).count(), 0);
    }

    #[test]
    fn hit_tests_prefer_actionable_elements() {
        let state = sample_state();
        let hit = state.element_at(crate::geometry::Point::new(20.0, 140.0));
        assert_eq!(hit.map(|e| e.index), Some(2));
    }
}
