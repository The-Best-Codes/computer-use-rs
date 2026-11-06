use std::collections::HashMap;
use std::ops::Range;

use atspi::proxy::accessible::AccessibleProxyBlocking;
use atspi::proxy::action::ActionProxyBlocking;
use atspi::proxy::bus::{BusProxyBlocking, StatusProxyBlocking};
use atspi::proxy::component::ComponentProxyBlocking;
use atspi::proxy::editable_text::EditableTextProxyBlocking;
use atspi::proxy::text::TextProxyBlocking;
use atspi::proxy::value::ValueProxyBlocking;
use atspi::{CoordType, Interface, InterfaceSet, ObjectRefOwned, State, StateSet};
use zbus::blocking::Connection;
use zbus::blocking::proxy::Builder;
use zbus::proxy::CacheProperties;
use zbus::proxy::Defaults;

use super::LinuxElement;
use super::session::SessionEnv;
use crate::element::{Action, ElementData, ElementStates, RawNode};
use crate::error::{Error, Result};
use crate::geometry::Rect;
use crate::state::WindowInfo;

pub(crate) const REGISTRY: &str = "org.a11y.atspi.Registry";
pub(crate) const ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";

const CAPTURE_TEXT_LIMIT: i32 = 10_000;

const TEXT_VALUE_ROLES: [&str; 7] =
    ["entry", "password text", "text", "terminal", "document text", "editbar", "combo box"];

const NUMERIC_VALUE_ROLES: [&str; 7] =
    ["slider", "spin button", "scroll bar", "progress bar", "level bar", "dial", "ruler"];

const WINDOW_ROLES: [&str; 5] = ["frame", "window", "dialog", "alert", "file chooser"];

#[allow(clippy::needless_pass_by_value)]
pub(crate) fn dbus(err: zbus::Error) -> Error {
    Error::Platform(format!("AT-SPI: {err}"))
}

pub(crate) fn connect(session: &SessionEnv) -> Result<Connection> {
    let address = match session.atspi_bus() {
        Some(address) => address.to_owned(),
        None => accessibility_bus_address(session)?,
    };
    zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(zbus::blocking::connection::Builder::build)
        .map_err(|e| Error::Platform(format!("cannot connect to the AT-SPI bus at {address}: {e}")))
}

fn session_bus(session: &SessionEnv) -> Result<Connection> {
    session
        .session_bus()
        .map_or_else(Connection::session, |address| {
            zbus::blocking::connection::Builder::address(address).and_then(zbus::blocking::connection::Builder::build)
        })
        .map_err(|e| Error::Platform(format!("cannot connect to the D-Bus session bus: {e}")))
}

/// Turns on `org.a11y.Status.IsEnabled`, the flag screen readers set.
///
/// GTK apps always register on the accessibility bus, but some apps only do so if this flag is on when they start.
/// Best effort: failures are ignored, since GTK apps work either way.
pub(crate) fn enable_accessibility(session: &SessionEnv) {
    let Ok(bus) = session_bus(session) else { return };
    let Ok(status) = StatusProxyBlocking::new(&bus) else { return };
    if status.is_enabled() != Ok(true) {
        let _ = status.set_is_enabled(true);
    }
}

fn accessibility_bus_address(session: &SessionEnv) -> Result<String> {
    let session_bus = session_bus(session)?;
    BusProxyBlocking::new(&session_bus).and_then(|bus| bus.get_address()).map_err(|e| {
        Error::Platform(format!("the AT-SPI accessibility bus is unavailable (is at-spi2-core running?): {e}"))
    })
}

pub(crate) fn proxy_at<'a, T>(conn: &Connection, bus: &'a str, path: &'a str) -> Result<T>
where
    T: Defaults + From<zbus::Proxy<'a>>,
{
    Builder::<T>::new(conn)
        .destination(bus)
        .and_then(|b| b.path(path))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(Builder::build)
        .map_err(dbus)
}

pub(crate) fn proxy<'a, T>(conn: &Connection, element: &'a LinuxElement) -> Result<T>
where
    T: Defaults + From<zbus::Proxy<'a>>,
{
    proxy_at(conn, element.bus_name(), element.object_path())
}

pub(crate) fn element_from_ref(object: &ObjectRefOwned) -> Option<LinuxElement> {
    if object.is_null() {
        return None;
    }
    let bus = object.name_as_str()?;
    Some(LinuxElement::new(bus, object.path_as_str()))
}

pub(crate) fn children(conn: &Connection, element: &LinuxElement) -> Result<Vec<LinuxElement>> {
    let accessible: AccessibleProxyBlocking<'_> = proxy(conn, element)?;
    let refs = accessible.get_children().map_err(dbus)?;
    Ok(refs.iter().filter_map(element_from_ref).collect())
}

fn extents(conn: &Connection, element: &LinuxElement) -> Option<Rect> {
    let component: ComponentProxyBlocking<'_> = proxy(conn, element).ok()?;
    let (x, y, w, h) = component.get_extents(CoordType::Screen).ok()?;
    ((1..=100_000).contains(&w) && (1..=100_000).contains(&h))
        .then(|| Rect::new(f64::from(x), f64::from(y), f64::from(w), f64::from(h)))
}

#[derive(Debug)]
pub(crate) struct MainWindow {
    pub(crate) element: LinuxElement,
    pub(crate) info: WindowInfo,
}

/// An opaque, stable ID for a window's accessible object, used as its [`WindowInfo::id`].
fn window_id(element: &LinuxElement) -> u64 {
    // FNV-1a: stable across runs and Rust versions, unlike `DefaultHasher`.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in element.bus_name().bytes().chain([0]).chain(element.object_path().bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

struct Candidate {
    window: MainWindow,
    active: bool,
}

/// The app's top-level windows that are active or showing, in the app's child order.
fn windows(conn: &Connection, app: &LinuxElement) -> Result<Vec<Candidate>> {
    let mut out = Vec::new();
    for child in children(conn, app)? {
        let Ok(accessible) = proxy::<AccessibleProxyBlocking<'_>>(conn, &child) else { continue };
        let role = accessible.get_role_name().unwrap_or_default();
        let frame = extents(conn, &child);
        if !WINDOW_ROLES.contains(&role.as_str()) && frame.is_none() {
            continue;
        }
        let Ok(states) = accessible.get_state() else { continue };
        let active = states.contains(State::Active);
        if !active && !states.contains(State::Showing) {
            continue;
        }
        let title = accessible.name().ok().filter(|s| !s.is_empty());
        let info = WindowInfo::new(Some(window_id(&child)), title, frame.unwrap_or_default());
        drop(accessible);
        out.push(Candidate { window: MainWindow { element: child, info }, active });
    }
    Ok(out)
}

/// The app's windows, the active one first.
pub(crate) fn list_windows(conn: &Connection, app: &LinuxElement) -> Result<Vec<WindowInfo>> {
    let mut found = windows(conn, app)?;
    found.sort_by_key(|c| !c.active);
    Ok(found.into_iter().map(|c| c.window.info).collect())
}

/// The window with [`WindowInfo::id`] `id`, or the active (else first showing) window.
pub(crate) fn main_window(
    conn: &Connection,
    app: &LinuxElement,
    app_name: &str,
    id: Option<u64>,
) -> Result<MainWindow> {
    let found = windows(conn, app)?;
    let chosen = if let Some(id) = id {
        found
            .into_iter()
            .find(|c| c.window.info.id == Some(id))
            .ok_or_else(|| Error::WindowNotFound(format!("{app_name} (no window with ID {id})")))?
    } else {
        let index = found.iter().position(|c| c.active).unwrap_or(0);
        found.into_iter().nth(index).ok_or_else(|| Error::WindowNotFound(app_name.to_owned()))?
    };
    Ok(chosen.window)
}

pub(crate) fn has_active_window(conn: &Connection, app: &LinuxElement) -> bool {
    children(conn, app).unwrap_or_default().iter().any(|child| {
        proxy::<AccessibleProxyBlocking<'_>>(conn, child)
            .and_then(|a| a.get_state().map_err(dbus))
            .is_ok_and(|states| states.contains(State::Active))
    })
}

#[derive(Debug, Default)]
pub(crate) struct Walk {
    pub(crate) nodes: Vec<RawNode<LinuxElement>>,
    pub(crate) truncated: bool,
    pub(crate) focused: Option<usize>,
    pub(crate) selected_text: Option<String>,
}

struct NodeRead {
    data: ElementData,
    interfaces: InterfaceSet,
    hidden: bool,
}

pub(crate) fn walk(conn: &Connection, root: &LinuxElement, max_nodes: usize, max_depth: usize) -> Result<Walk> {
    let mut walk = Walk::default();
    let mut focused_has_text = false;
    let mut stack: Vec<(LinuxElement, Option<usize>, usize)> = vec![(root.clone(), None, 0)];
    while let Some((element, parent, depth)) = stack.pop() {
        if walk.nodes.len() >= max_nodes {
            walk.truncated = true;
            break;
        }
        let read = match read_node(conn, &element) {
            Ok(read) => read,
            Err(err) if parent.is_none() => return Err(err),
            Err(_) => continue,
        };
        let index = walk.nodes.len();
        if walk.focused.is_none() && read.data.states.contains(ElementStates::FOCUSED) {
            walk.focused = Some(index);
            focused_has_text = read.interfaces.contains(Interface::Text);
        }
        let descend = !read.hidden || parent.is_none();
        walk.nodes.push(RawNode { handle: element, parent, data: read.data });
        if !descend {
            continue;
        }
        let kids = children(conn, &walk.nodes[index].handle).unwrap_or_default();
        if kids.is_empty() {
            continue;
        }
        if depth >= max_depth {
            walk.truncated = true;
            continue;
        }
        stack.extend(kids.into_iter().rev().map(|kid| (kid, Some(index), depth + 1)));
    }
    if let Some(index) = walk.focused.filter(|_| focused_has_text) {
        walk.selected_text = selected_text(conn, &walk.nodes[index].handle);
    }
    Ok(walk)
}

fn read_node(conn: &Connection, element: &LinuxElement) -> Result<NodeRead> {
    let accessible: AccessibleProxyBlocking<'_> = proxy(conn, element)?;
    let interfaces = accessible.get_interfaces().map_err(dbus)?;
    let platform_role = accessible.get_role_name().map_err(dbus)?;
    let state_set = accessible.get_state().unwrap_or_else(|_| StateSet::empty());
    let mut data = ElementData::new(normalize_role(&platform_role), platform_role.clone());
    data.title = accessible.name().ok().filter(|s| !s.trim().is_empty());
    data.description = accessible.description().ok().filter(|d| !d.trim().is_empty() && Some(d) != data.title.as_ref());
    let attributes = accessible.get_attributes().unwrap_or_default();
    data.identifier = identifier(&accessible, &attributes);
    data.placeholder = attributes.get("placeholder-text").filter(|s| !s.is_empty()).cloned();
    data.states = element_states(state_set);
    if interfaces.contains(Interface::Component) {
        data.frame = extents(conn, element);
    }
    if interfaces.contains(Interface::Action) {
        data.actions = actions(conn, element);
    }
    data.value = read_value(conn, element, &platform_role, interfaces, Some(CAPTURE_TEXT_LIMIT));
    if interfaces.contains(Interface::EditableText) && !data.states.contains(ElementStates::DISABLED) {
        data.states |= ElementStates::SETTABLE;
    }
    if interfaces.contains(Interface::Value) && NUMERIC_VALUE_ROLES.contains(&platform_role.as_str()) {
        data.states |= ElementStates::SETTABLE;
    }
    let hidden = !state_set.contains(State::Showing) && !state_set.contains(State::Visible);
    Ok(NodeRead { data, interfaces, hidden })
}

fn identifier(accessible: &AccessibleProxyBlocking<'_>, attributes: &HashMap<String, String>) -> Option<String> {
    attributes
        .get("id")
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| accessible.accessible_id().ok().filter(|s| !s.is_empty()))
}

fn read_value(
    conn: &Connection,
    element: &LinuxElement,
    role: &str,
    interfaces: InterfaceSet,
    limit: Option<i32>,
) -> Option<String> {
    if interfaces.contains(Interface::Value) && NUMERIC_VALUE_ROLES.contains(&role) {
        let value: ValueProxyBlocking<'_> = proxy(conn, element).ok()?;
        return value.current_value().ok().map(format_number);
    }
    if interfaces.contains(Interface::Text)
        && (interfaces.contains(Interface::EditableText) || TEXT_VALUE_ROLES.contains(&role))
    {
        let text: TextProxyBlocking<'_> = proxy(conn, element).ok()?;
        return text.get_text(0, limit.unwrap_or(-1)).ok().filter(|s| !s.is_empty());
    }
    None
}

fn selected_text(conn: &Connection, element: &LinuxElement) -> Option<String> {
    let text: TextProxyBlocking<'_> = proxy(conn, element).ok()?;
    if text.get_n_selections().ok()? < 1 {
        return None;
    }
    let (start, end) = text.get_selection(0).ok()?;
    (end > start).then(|| text.get_text(start, end).ok()).flatten().filter(|s| !s.is_empty())
}

/// Reads an element's action names, indexed as `DoAction` expects.
///
/// `GetActions` reports each action's localized name, which some toolkits (Chromium among them)
/// leave empty; `GetName` still returns the real name, so it fills any gaps.
fn action_names(action: &ActionProxyBlocking<'_>) -> zbus::Result<Vec<String>> {
    let listed = match action.get_actions() {
        Ok(list) => list.into_iter().map(|a| a.name).collect(),
        Err(err) => {
            let count = action.n_actions().map_err(|_| err)?;
            vec![String::new(); usize::try_from(count).unwrap_or(0)]
        }
    };
    Ok(listed
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            if !name.trim().is_empty() {
                return name;
            }
            i32::try_from(i).ok().and_then(|i| action.get_name(i).ok()).unwrap_or_default()
        })
        .collect())
}

fn actions(conn: &Connection, element: &LinuxElement) -> Vec<Action> {
    let Ok(action) = proxy::<ActionProxyBlocking<'_>>(conn, element) else { return Vec::new() };
    let names = action_names(&action);
    let primary = names.as_ref().ok().and_then(|names| names.iter().position(|n| is_primary_action(n)));
    names
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
        .map(|(i, name)| Action::new(action_name(&name), primary == Some(i)))
        .collect()
}

/// Whether an action is what a left click on the element would do.
///
/// Besides the common verbs, this covers Chromium's: it names each element's default action after
/// what it does (`open`, `check`, `select`, ...), uses `doDefault` when it has no better verb, and
/// `clickAncestor` for content inside a clickable parent.
pub(crate) fn is_primary_action(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "click"
            | "press"
            | "activate"
            | "jump"
            | "default.activate"
            | "invoke"
            | "dodefault"
            | "open"
            | "check"
            | "uncheck"
            | "select"
            | "toggle"
            | "clickancestor"
    )
}

pub(crate) fn action_name(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "menu" | "popup" | "popupmenu" | "popup menu" | "popup_menu" | "context menu" | "contextmenu" | "showmenu"
        | "show menu" | "show_menu" | "showcontextmenu" | "show context menu" => "showMenu".to_owned(),
        _ => raw.to_owned(),
    }
}

pub(crate) fn normalize_role(role_name: &str) -> String {
    let lower = role_name.trim().to_lowercase();
    let mapped = match lower.as_str() {
        "push button" | "button" => "button",
        "entry" => "text field",
        "password text" => "secure text field",
        "frame" => "window",
        "check box" => "checkbox",
        "check menu item" => "checkbox menu item",
        "label" | "static" => "text",
        "text" => "text area",
        "page tab" => "tab",
        "page tab list" => "tab group",
        "document frame" | "document web" | "document text" => "document",
        "scroll pane" => "scroll area",
        "table cell" => "cell",
        "invalid" | "redundant object" | "" => "unknown",
        "layered pane" | "root pane" | "viewport" | "glass pane" | "option pane" => "pane",
        other => other,
    };
    mapped.to_owned()
}

pub(crate) fn element_states(set: StateSet) -> ElementStates {
    let mut states = ElementStates::empty();
    let has = |state: State| set.contains(state);
    states.set(ElementStates::FOCUSED, has(State::Focused));
    states.set(ElementStates::SELECTED, has(State::Selected));
    states.set(ElementStates::DISABLED, !has(State::Enabled) && !has(State::Sensitive));
    states.set(ElementStates::EXPANDED, has(State::Expanded));
    states.set(ElementStates::COLLAPSED, has(State::Collapsed) || (has(State::Expandable) && !has(State::Expanded)));
    states.set(ElementStates::CHECKED, has(State::Checked));
    states.set(ElementStates::EDITABLE | ElementStates::SETTABLE, has(State::Editable) && !has(State::ReadOnly));
    states.set(ElementStates::OFFSCREEN, !has(State::Showing));
    states
}

pub(crate) fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{value:.0}") } else { format!("{value}") }
}

pub(crate) fn perform_action(conn: &Connection, element: &LinuxElement, name: &str) -> Result<()> {
    let action: ActionProxyBlocking<'_> = proxy(conn, element)?;
    let names = action_names(&action).map_err(dbus)?;
    let index = names
        .iter()
        .position(|n| {
            !n.is_empty() && (n.eq_ignore_ascii_case(name) || Action::new(action_name(n), false).matches(name))
        })
        .ok_or_else(|| Error::action(format!("element has no action {name:?}")))?;
    let index = i32::try_from(index).map_err(|_| Error::action("action index out of range"))?;
    if action.do_action(index).map_err(dbus)? {
        Ok(())
    } else {
        Err(Error::action(format!("the app rejected action {name:?}")))
    }
}

pub(crate) fn set_value(conn: &Connection, element: &LinuxElement, value: &str) -> Result<()> {
    let accessible: AccessibleProxyBlocking<'_> = proxy(conn, element)?;
    let interfaces = accessible.get_interfaces().map_err(dbus)?;
    if interfaces.contains(Interface::EditableText) {
        let editable: EditableTextProxyBlocking<'_> = proxy(conn, element)?;
        return if editable.set_text_contents(value).map_err(dbus)? {
            Ok(())
        } else {
            Err(Error::action("the app rejected the new text"))
        };
    }
    if interfaces.contains(Interface::Value) {
        let number: f64 = value.trim().parse().map_err(|_| Error::invalid(format!("{value:?} is not a number")))?;
        let proxy: ValueProxyBlocking<'_> = proxy(conn, element)?;
        return proxy.set_current_value(number).map_err(dbus);
    }
    Err(Error::Unsupported("this element's value cannot be set".into()))
}

pub(crate) fn value(conn: &Connection, element: &LinuxElement) -> Result<Option<String>> {
    let accessible: AccessibleProxyBlocking<'_> = proxy(conn, element)?;
    let interfaces = accessible.get_interfaces().map_err(dbus)?;
    if interfaces.contains(Interface::Text) {
        let text: TextProxyBlocking<'_> = proxy(conn, element)?;
        return text.get_text(0, -1).map(Some).map_err(dbus);
    }
    let role = accessible.get_role_name().map_err(dbus)?;
    Ok(read_value(conn, element, &role, interfaces, None))
}

pub(crate) fn select_text(conn: &Connection, element: &LinuxElement, value: &str, range: Range<usize>) -> Result<()> {
    let offset = |byte: usize| {
        i32::try_from(crate::text::char_offset(value, byte)).map_err(|_| Error::invalid("text offset out of range"))
    };
    let (start, end) = (offset(range.start)?, offset(range.end)?);
    let text: TextProxyBlocking<'_> = proxy(conn, element)?;
    let ok = if start == end {
        text.set_caret_offset(start).map_err(dbus)?
    } else if text.get_n_selections().map_err(dbus)? > 0 {
        text.set_selection(0, start, end).map_err(dbus)?
    } else {
        text.add_selection(start, end).map_err(dbus)?
    };
    if ok { Ok(()) } else { Err(Error::action("the app rejected the text selection")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_roles() {
        assert_eq!(normalize_role("push button"), "button");
        assert_eq!(normalize_role("entry"), "text field");
        assert_eq!(normalize_role("frame"), "window");
        assert_eq!(normalize_role("check box"), "checkbox");
        assert_eq!(normalize_role("label"), "text");
        assert_eq!(normalize_role("menu item"), "menu item");
        assert_eq!(normalize_role("Filler"), "filler");
    }

    #[test]
    fn maps_actions() {
        assert!(is_primary_action("click"));
        assert!(is_primary_action("Activate"));
        assert!(is_primary_action("doDefault"));
        assert!(is_primary_action("clickAncestor"));
        assert!(!is_primary_action("showMenu"));
        assert!(!is_primary_action("showContextMenu"));
        assert!(Action::new(action_name("menu"), false).matches("Show Menu"));
        assert!(Action::new(action_name("showMenu"), false).matches("Show Menu"));
        assert!(Action::new(action_name("showContextMenu"), false).matches("Show Menu"));
        assert_eq!(action_name("toggle"), "toggle");
    }

    #[test]
    fn maps_states() {
        let set = StateSet::new(State::Focused | State::Editable | State::Enabled | State::Sensitive);
        let states = element_states(set);
        assert!(states.contains(ElementStates::FOCUSED | ElementStates::EDITABLE | ElementStates::SETTABLE));
        assert!(states.contains(ElementStates::OFFSCREEN));
        assert!(!states.contains(ElementStates::DISABLED));
        let states = element_states(StateSet::new(State::Showing | State::Expandable));
        assert!(states.contains(ElementStates::DISABLED | ElementStates::COLLAPSED));
        assert!(!states.contains(ElementStates::OFFSCREEN));
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(0.5), "0.5");
    }
}
