use std::collections::HashSet;

use objc2_core_foundation::{CFArray, CFRetained, CFString, CFType};

use super::ax::{self, AxElement};
use crate::element::{Action, ElementData, ElementStates, RawNode};
use crate::geometry::Rect;

const ATTRIBUTES: [&str; 15] = [
    "AXRole",
    "AXSubrole",
    "AXRoleDescription",
    "AXTitle",
    "AXValue",
    "AXDescription",
    "AXHelp",
    "AXIdentifier",
    "AXPlaceholderValue",
    "AXURL",
    "AXPosition",
    "AXSize",
    "AXEnabled",
    "AXFocused",
    "AXSelected",
];

const AX_EXPANDED: &str = "AXExpanded";

const TEXT_ROLES: [&str; 3] = ["AXTextField", "AXTextArea", "AXComboBox"];
const TOGGLE_ROLES: [&str; 3] = ["AXCheckBox", "AXRadioButton", "AXSwitch"];

pub(super) struct Walk {
    pub(super) nodes: Vec<RawNode<AxElement>>,
    pub(super) truncated: bool,
    pub(super) focused: Option<usize>,
}

pub(super) fn walk(
    root: &AxElement,
    window_frame: Rect,
    focused: Option<&AxElement>,
    max_nodes: usize,
    max_depth: usize,
) -> Walk {
    let names: CFRetained<CFArray<CFString>> = ax::attribute_names(&ATTRIBUTES);
    let mut nodes = Vec::new();
    let mut seen: HashSet<AxElement> = HashSet::new();
    let mut truncated = false;
    let mut focused_index = None;
    let mut stack: Vec<(AxElement, Option<usize>, usize)> = vec![(root.clone(), None, 0)];

    while let Some((element, parent, depth)) = stack.pop() {
        if nodes.len() >= max_nodes {
            truncated = true;
            break;
        }
        if !seen.insert(element.clone()) {
            continue;
        }
        let values = ax::attributes(&element, &names);
        let data = element_data(&element, &values, window_frame);
        let index = nodes.len();
        if focused.is_some_and(|f| *f == element) {
            focused_index = Some(index);
        }
        let children = children(&element, &data.platform_role);
        if depth + 1 > max_depth {
            truncated |= !children.is_empty();
        } else {
            stack.extend(children.into_iter().rev().map(|child| (child, Some(index), depth + 1)));
        }
        nodes.push(RawNode { handle: element, parent, data });
    }
    Walk { nodes, truncated: truncated || !stack.is_empty(), focused: focused_index }
}

fn children(element: &AxElement, role: &str) -> Vec<AxElement> {
    // Tables and outlines expose rows separately from their (sometimes empty) children.
    if matches!(role, "AXTable" | "AXOutline") {
        let rows = ax::elements(element, "AXRows");
        if !rows.is_empty() {
            let mut kids = ax::elements(element, "AXChildren");
            kids.retain(|k| !rows.contains(k));
            kids.extend(rows);
            return kids;
        }
    }
    ax::elements(element, "AXChildren")
}

fn element_data(element: &AxElement, v: &[Option<CFRetained<CFType>>], window_frame: Rect) -> ElementData {
    let get = |i: usize| v.get(i).and_then(Option::as_deref);
    let text = |i: usize| get(i).and_then(ax::as_string).filter(|s| !s.trim().is_empty());

    let platform_role = text(0).unwrap_or_else(|| "AXUnknown".to_owned());
    let subrole = text(1);
    let role = text(2).map_or_else(|| humanize_role(&platform_role), |r| r.to_lowercase());

    let mut data = ElementData::new(role, platform_role.clone());
    data.title = text(3);
    data.description = text(5);
    data.help = text(6);
    data.identifier = text(7);
    data.placeholder = text(8);
    data.url = text(9);
    data.frame = ax::rect(get(10).and_then(ax::as_point), get(11).and_then(ax::as_size));

    let mut states = ElementStates::empty();
    if get(12).and_then(ax::as_bool) == Some(false) {
        states |= ElementStates::DISABLED;
    }
    if get(13).and_then(ax::as_bool) == Some(true) {
        states |= ElementStates::FOCUSED;
    }
    if get(14).and_then(ax::as_bool) == Some(true) {
        states |= ElementStates::SELECTED;
    }
    if data.frame.is_some_and(|f| !f.is_empty() && f.intersection(&window_frame).is_none()) {
        states |= ElementStates::OFFSCREEN;
    }

    let role_str = platform_role.as_str();
    let raw_value = get(4);
    if TOGGLE_ROLES.contains(&role_str) {
        if raw_value.and_then(ax::as_bool) == Some(true) {
            states |= ElementStates::CHECKED;
        }
    } else if let Some(value) = raw_value {
        data.value = ax::as_string(value).or_else(|| ax::as_number(value).map(format_number)).filter(|s| !s.is_empty());
    }

    let is_text = TEXT_ROLES.contains(&role_str) || subrole.as_deref() == Some("AXSearchField");
    if (is_text || raw_value.is_some()) && ax::is_settable(element, "AXValue") {
        states |= ElementStates::SETTABLE;
        if is_text {
            states |= ElementStates::EDITABLE;
        }
    }
    if matches!(role_str, "AXDisclosureTriangle" | "AXRow" | "AXComboBox" | "AXPopUpButton")
        && let Some(expanded) = ax::boolean(element, AX_EXPANDED)
    {
        states |= if expanded { ElementStates::EXPANDED } else { ElementStates::COLLAPSED };
    }
    data.states = states;
    data.actions = actions(element);
    data
}

fn actions(element: &AxElement) -> Vec<Action> {
    ax::action_names(element)
        .into_iter()
        .map(|name| {
            let primary = name == "AXPress";
            let mut action = Action::new(name, primary);
            // Custom actions look like "Name:Copy\nTarget:0x0\nSelector:(null)".
            if let Some(rest) = action.name.strip_prefix("Name:") {
                action.label = rest.lines().next().unwrap_or(rest).trim().to_owned();
            }
            action
        })
        .collect()
}

fn humanize_role(role: &str) -> String {
    let trimmed = role.strip_prefix("AX").unwrap_or(role);
    let mut out = String::with_capacity(trimmed.len() + 4);
    for (i, c) in trimmed.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            out.push(' ');
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn format_number(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 { format!("{n:.0}") } else { format!("{n}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humanizes_roles() {
        assert_eq!(humanize_role("AXPopUpButton"), "pop up button");
        assert_eq!(humanize_role("AXWindow"), "window");
    }

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(3.0), "3");
        assert_eq!(format_number(0.5), "0.5");
    }
}
