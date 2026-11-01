use crate::geometry::Rect;

bitflags::bitflags! {
    /// Boolean accessibility states of an element.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
    pub struct ElementStates: u16 {
        /// Has keyboard focus.
        const FOCUSED = 1 << 0;
        /// Is selected (rows, tabs, list items).
        const SELECTED = 1 << 1;
        /// Is disabled and will ignore input.
        const DISABLED = 1 << 2;
        /// Is expanded (disclosure triangles, combo boxes, tree rows).
        const EXPANDED = 1 << 3;
        /// Is collapsed but can be expanded.
        const COLLAPSED = 1 << 4;
        /// Is checked or toggled on.
        const CHECKED = 1 << 5;
        /// Its value can be written with `set_value`.
        const SETTABLE = 1 << 6;
        /// Accepts text input.
        const EDITABLE = 1 << 7;
        /// Is not visible on screen (off-screen or scrolled away).
        const OFFSCREEN = 1 << 8;
    }
}

impl ElementStates {
    /// Short labels used when rendering the tree.
    #[must_use]
    pub fn labels(self) -> Vec<&'static str> {
        const LABELS: [(ElementStates, &str); 9] = [
            (ElementStates::FOCUSED, "focused"),
            (ElementStates::SELECTED, "selected"),
            (ElementStates::DISABLED, "disabled"),
            (ElementStates::EXPANDED, "expanded"),
            (ElementStates::COLLAPSED, "collapsed"),
            (ElementStates::CHECKED, "checked"),
            (ElementStates::SETTABLE, "settable"),
            (ElementStates::EDITABLE, "editable"),
            (ElementStates::OFFSCREEN, "offscreen"),
        ];
        LABELS.iter().filter(|(flag, _)| self.contains(*flag)).map(|(_, label)| *label).collect()
    }
}

/// An accessibility action an element exposes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    /// The platform's raw action name, e.g. `AXPress`, `Invoke`, `click`.
    pub name: String,
    /// Human-readable label, e.g. `Press`, `Show Menu`.
    pub label: String,
    /// Whether this is the element's default "click" action.
    pub primary: bool,
}

impl Action {
    /// Creates an action; the label is derived from the raw name.
    #[must_use]
    pub fn new(name: impl Into<String>, primary: bool) -> Self {
        let name = name.into();
        let label = humanize_action_name(&name);
        Self { name, label, primary }
    }

    /// True when `query` names this action by raw name or label, ignoring case,
    /// spaces, and an `AX` prefix.
    #[must_use]
    pub fn matches(&self, query: &str) -> bool {
        let q = normalize_action(query);
        !q.is_empty() && (normalize_action(&self.name) == q || normalize_action(&self.label) == q)
    }
}

fn normalize_action(value: &str) -> String {
    let lowered: String = value.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
    lowered.strip_prefix("ax").map(str::to_owned).unwrap_or(lowered)
}

fn humanize_action_name(name: &str) -> String {
    let trimmed = name.strip_prefix("AX").unwrap_or(name);
    let mut out = String::with_capacity(trimmed.len() + 4);
    let mut prev_lower = false;
    let mut start_word = true;
    for c in trimmed.chars() {
        if c == '_' || c == '-' || c == ' ' {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
            start_word = true;
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower {
            out.push(' ');
            start_word = true;
        }
        if start_word {
            out.extend(c.to_uppercase());
        } else {
            out.push(c);
        }
        start_word = false;
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
    }
    out
}

/// Raw attributes of an element, as reported by a [`Backend`](crate::Backend).
///
/// Backends fill this in; the crate assigns indexes, prunes uninteresting containers, and
/// truncates text to produce [`Element`]s.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ElementData {
    /// Display role, lowercase and human-readable (`button`, `text field`).
    pub role: String,
    /// The platform's raw role (`AXButton`, `ControlType.Button`, `push button`).
    pub platform_role: String,
    /// Primary label or name.
    pub title: Option<String>,
    /// Current value (text contents, slider position, checkbox state).
    pub value: Option<String>,
    /// Accessibility description, when different from the title.
    pub description: Option<String>,
    /// Tooltip or help text.
    pub help: Option<String>,
    /// Placeholder text of empty text fields.
    pub placeholder: Option<String>,
    /// Developer-assigned identifier.
    pub identifier: Option<String>,
    /// Link target.
    pub url: Option<String>,
    /// Frame in screen points (not window-relative).
    pub frame: Option<Rect>,
    /// States.
    pub states: ElementStates,
    /// Available actions.
    pub actions: Vec<Action>,
}

impl ElementData {
    /// Creates data for an element with the given display and platform role.
    #[must_use]
    pub fn new(role: impl Into<String>, platform_role: impl Into<String>) -> Self {
        Self { role: role.into(), platform_role: platform_role.into(), ..Self::default() }
    }

    fn is_noise(&self) -> bool {
        const CONTAINER_ROLES: [&str; 8] =
            ["group", "unknown", "generic", "filler", "panel", "pane", "section", "layout table"];
        CONTAINER_ROLES.contains(&self.role.as_str())
            && self.title.is_none()
            && self.value.is_none()
            && self.description.is_none()
            && self.identifier.is_none()
            && self.url.is_none()
            && !self.actions.iter().any(|a| !a.primary)
            && !self.states.intersects(ElementStates::FOCUSED | ElementStates::SELECTED)
    }
}

/// A node in a captured accessibility tree.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Element {
    /// Index of this element within its [`AppState`](crate::AppState); pass it to element actions.
    pub index: usize,
    /// Index of the nearest rendered ancestor.
    pub parent: Option<usize>,
    /// Indexes of rendered children.
    pub children: Vec<usize>,
    /// Nesting depth below the window.
    pub depth: usize,
    /// Display role.
    pub role: String,
    /// Raw platform role.
    pub platform_role: String,
    /// Primary label.
    pub title: Option<String>,
    /// Current value.
    pub value: Option<String>,
    /// Accessibility description.
    pub description: Option<String>,
    /// Help text.
    pub help: Option<String>,
    /// Placeholder text.
    pub placeholder: Option<String>,
    /// Developer identifier.
    pub identifier: Option<String>,
    /// Link target.
    pub url: Option<String>,
    /// Frame in screen points.
    pub frame: Option<Rect>,
    /// States.
    pub states: ElementStates,
    /// Available actions.
    pub actions: Vec<Action>,
}

impl Element {
    /// The default click action, if any.
    #[must_use]
    pub fn primary_action(&self) -> Option<&Action> {
        self.actions.iter().find(|a| a.primary)
    }

    /// Finds an action by raw name or label.
    #[must_use]
    pub fn action(&self, query: &str) -> Option<&Action> {
        self.actions.iter().find(|a| a.matches(query))
    }

    /// Text that best describes the element: title, then description, then value.
    #[must_use]
    pub fn label(&self) -> Option<&str> {
        self.title.as_deref().or(self.description.as_deref()).or(self.value.as_deref())
    }

    pub(crate) fn text_fields(&self) -> impl Iterator<Item = &str> {
        [
            self.title.as_deref(),
            self.description.as_deref(),
            self.value.as_deref(),
            self.placeholder.as_deref(),
            self.identifier.as_deref(),
            self.help.as_deref(),
        ]
        .into_iter()
        .flatten()
    }
}

/// A raw node returned by a backend, in depth-first pre-order.
#[derive(Debug, Clone)]
pub struct RawNode<H> {
    /// Backend handle used to act on the element later.
    pub handle: H,
    /// Index of the parent within the same `Vec`, `None` for the root.
    pub parent: Option<usize>,
    /// The element's attributes.
    pub data: ElementData,
}

pub(crate) fn build_tree<H>(
    nodes: Vec<RawNode<H>>,
    text_limit: Option<usize>,
) -> (Vec<Element>, Vec<H>, Vec<Option<usize>>) {
    let mut elements: Vec<Element> = Vec::with_capacity(nodes.len());
    let mut handles = Vec::with_capacity(nodes.len());
    let mut kept_ancestor: Vec<Option<usize>> = Vec::with_capacity(nodes.len());
    let mut raw_to_element: Vec<Option<usize>> = Vec::with_capacity(nodes.len());

    for (raw_index, node) in nodes.into_iter().enumerate() {
        let parent = node.parent.filter(|&p| p < raw_index).and_then(|p| kept_ancestor[p]);
        let is_root = node.parent.is_none();
        if !is_root && node.data.is_noise() {
            kept_ancestor.push(parent);
            raw_to_element.push(None);
            continue;
        }
        let index = elements.len();
        let depth = parent.map_or(0, |p| elements[p].depth + 1);
        if let Some(p) = parent {
            elements[p].children.push(index);
        }
        let d = node.data;
        let clip = |s: Option<String>| s.map(|s| sanitize(&s, text_limit)).filter(|s| !s.is_empty());
        elements.push(Element {
            index,
            parent,
            children: Vec::new(),
            depth,
            role: d.role,
            platform_role: d.platform_role,
            title: clip(d.title),
            value: clip(d.value),
            description: clip(d.description),
            help: clip(d.help),
            placeholder: clip(d.placeholder),
            identifier: clip(d.identifier),
            url: clip(d.url),
            frame: d.frame,
            states: d.states,
            actions: d.actions,
        });
        handles.push(node.handle);
        kept_ancestor.push(Some(index));
        raw_to_element.push(Some(index));
    }
    (elements, handles, raw_to_element)
}

pub(crate) fn sanitize(text: &str, limit: Option<usize>) -> String {
    let mut out = String::with_capacity(text.len().min(limit.unwrap_or(usize::MAX)));
    let mut count = 0usize;
    let mut pending_space = false;
    for c in text.chars() {
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
            continue;
        }
        let needed = usize::from(pending_space) + 1;
        if limit.is_some_and(|l| count + needed > l) {
            out.push('…');
            return out;
        }
        if pending_space {
            out.push(' ');
            count += 1;
            pending_space = false;
        }
        out.push(c);
        count += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(parent: Option<usize>, role: &str, title: Option<&str>) -> RawNode<u32> {
        let mut data = ElementData::new(role, role);
        data.title = title.map(str::to_owned);
        RawNode { handle: 0, parent, data }
    }

    #[test]
    fn humanizes_actions() {
        assert_eq!(humanize_action_name("AXShowMenu"), "Show Menu");
        assert_eq!(humanize_action_name("show_menu"), "Show Menu");
        assert_eq!(humanize_action_name("AXScrollDownByPage"), "Scroll Down By Page");
        assert!(Action::new("AXShowMenu", false).matches("show menu"));
        assert!(Action::new("AXShowMenu", false).matches("ShowMenu"));
        assert!(!Action::new("AXPress", true).matches("show menu"));
    }

    #[test]
    fn prunes_anonymous_groups_and_reparents() {
        let nodes = vec![
            node(None, "window", Some("Main")),
            node(Some(0), "group", None),
            node(Some(1), "button", Some("OK")),
            node(Some(0), "group", Some("Named")),
            node(Some(3), "text", Some("hi")),
        ];
        let (elements, _, map) = build_tree(nodes, None);
        let roles: Vec<_> = elements.iter().map(|e| (e.index, e.parent, e.depth, e.role.as_str())).collect();
        assert_eq!(
            roles,
            vec![(0, None, 0, "window"), (1, Some(0), 1, "button"), (2, Some(0), 1, "group"), (3, Some(2), 2, "text"),]
        );
        assert_eq!(map, vec![Some(0), None, Some(1), Some(2), Some(3)]);
        assert_eq!(elements[0].children, vec![1, 2]);
    }

    #[test]
    fn sanitize_collapses_and_truncates() {
        assert_eq!(sanitize("  a \n\t b  ", None), "a b");
        assert_eq!(sanitize("hello world", Some(5)), "hello…");
        assert_eq!(sanitize("héllo", Some(10)), "héllo");
    }
}
