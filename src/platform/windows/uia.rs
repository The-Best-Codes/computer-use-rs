use std::ops::Range;

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, ExpandCollapseState, ExpandCollapseState_Collapsed, ExpandCollapseState_Expanded,
    ExpandCollapseState_PartiallyExpanded, IUIAutomation, IUIAutomationCacheRequest, IUIAutomationElement,
    IUIAutomationElement3, IUIAutomationElement6, IUIAutomationExpandCollapsePattern, IUIAutomationInvokePattern,
    IUIAutomationLegacyIAccessiblePattern, IUIAutomationRangeValuePattern, IUIAutomationScrollItemPattern,
    IUIAutomationScrollPattern, IUIAutomationSelectionItemPattern, IUIAutomationTextPattern,
    IUIAutomationTogglePattern, IUIAutomationTreeWalker, IUIAutomationValuePattern, ScrollAmount,
    ScrollAmount_LargeDecrement, ScrollAmount_LargeIncrement, ScrollAmount_NoAmount, TextPatternRangeEndpoint_End,
    TextPatternRangeEndpoint_Start, TextUnit_Character, ToggleState, ToggleState_Indeterminate, ToggleState_On,
    TreeScope_Element, UIA_AppBarControlTypeId, UIA_AutomationIdPropertyId, UIA_BoundingRectanglePropertyId,
    UIA_ButtonControlTypeId, UIA_CONTROLTYPE_ID, UIA_CalendarControlTypeId, UIA_CheckBoxControlTypeId,
    UIA_ComboBoxControlTypeId, UIA_ControlTypePropertyId, UIA_CustomControlTypeId, UIA_DataGridControlTypeId,
    UIA_DataItemControlTypeId, UIA_DocumentControlTypeId, UIA_EditControlTypeId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId, UIA_ExpandCollapsePatternId, UIA_FullDescriptionPropertyId,
    UIA_GroupControlTypeId, UIA_HasKeyboardFocusPropertyId, UIA_HeaderControlTypeId, UIA_HeaderItemControlTypeId,
    UIA_HelpTextPropertyId, UIA_HyperlinkControlTypeId, UIA_ImageControlTypeId, UIA_InvokePatternId,
    UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId,
    UIA_LegacyIAccessibleDefaultActionPropertyId, UIA_LegacyIAccessiblePatternId, UIA_ListControlTypeId,
    UIA_ListItemControlTypeId, UIA_MenuBarControlTypeId, UIA_MenuControlTypeId, UIA_MenuItemControlTypeId,
    UIA_NamePropertyId, UIA_PATTERN_ID, UIA_PROPERTY_ID, UIA_PaneControlTypeId, UIA_ProgressBarControlTypeId,
    UIA_RadioButtonControlTypeId, UIA_RangeValueIsReadOnlyPropertyId, UIA_RangeValuePatternId,
    UIA_RangeValueValuePropertyId, UIA_ScrollBarControlTypeId, UIA_ScrollHorizontallyScrollablePropertyId,
    UIA_ScrollItemPatternId, UIA_ScrollPatternId, UIA_ScrollVerticallyScrollablePropertyId,
    UIA_SelectionItemIsSelectedPropertyId, UIA_SelectionItemPatternId, UIA_SemanticZoomControlTypeId,
    UIA_SeparatorControlTypeId, UIA_SliderControlTypeId, UIA_SpinnerControlTypeId, UIA_SplitButtonControlTypeId,
    UIA_StatusBarControlTypeId, UIA_TabControlTypeId, UIA_TabItemControlTypeId, UIA_TableControlTypeId,
    UIA_TextControlTypeId, UIA_TextPatternId, UIA_ThumbControlTypeId, UIA_TitleBarControlTypeId, UIA_TogglePatternId,
    UIA_ToggleToggleStatePropertyId, UIA_ToolBarControlTypeId, UIA_ToolTipControlTypeId, UIA_TreeControlTypeId,
    UIA_TreeItemControlTypeId, UIA_ValueIsReadOnlyPropertyId, UIA_ValuePatternId, UIA_ValueValuePropertyId,
    UIA_WindowControlTypeId,
};
use windows::core::{BOOL, BSTR, Interface};

use super::{WinResultExt as _, rect_from};
use crate::element::{Action, ElementData, ElementStates, RawNode};
use crate::error::{Error, Result};
use crate::geometry::Point;
use crate::text::char_offset;

const PROPERTIES: [UIA_PROPERTY_ID; 19] = [
    UIA_ControlTypePropertyId,
    UIA_NamePropertyId,
    UIA_AutomationIdPropertyId,
    UIA_BoundingRectanglePropertyId,
    UIA_IsEnabledPropertyId,
    UIA_HasKeyboardFocusPropertyId,
    UIA_IsOffscreenPropertyId,
    UIA_IsPasswordPropertyId,
    UIA_HelpTextPropertyId,
    UIA_FullDescriptionPropertyId,
    UIA_ValueValuePropertyId,
    UIA_ValueIsReadOnlyPropertyId,
    UIA_ToggleToggleStatePropertyId,
    UIA_ExpandCollapseExpandCollapseStatePropertyId,
    UIA_SelectionItemIsSelectedPropertyId,
    UIA_RangeValueValuePropertyId,
    UIA_RangeValueIsReadOnlyPropertyId,
    UIA_ScrollHorizontallyScrollablePropertyId,
    UIA_ScrollVerticallyScrollablePropertyId,
];

const PATTERNS: [UIA_PATTERN_ID; 9] = [
    UIA_InvokePatternId,
    UIA_TogglePatternId,
    UIA_ExpandCollapsePatternId,
    UIA_SelectionItemPatternId,
    UIA_ScrollItemPatternId,
    UIA_ScrollPatternId,
    UIA_ValuePatternId,
    UIA_RangeValuePatternId,
    UIA_LegacyIAccessiblePatternId,
];

pub(super) struct Uia {
    automation: IUIAutomation,
    cache: IUIAutomationCacheRequest,
    walker: IUIAutomationTreeWalker,
}

pub(super) struct Walk {
    pub(super) nodes: Vec<RawNode<IUIAutomationElement>>,
    pub(super) truncated: bool,
}

impl Uia {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: COM is initialized on this thread; `CUIAutomation` is an in-process server.
        let automation: IUIAutomation = unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
            .or_platform("creating the UI Automation client")?;
        let cache = com!(automation.CreateCacheRequest()).or_platform("creating a UI Automation cache request")?;
        for property in PROPERTIES {
            com!(cache.AddProperty(property)).or_platform("configuring the UI Automation cache")?;
        }
        com!(cache.AddProperty(UIA_LegacyIAccessibleDefaultActionPropertyId))
            .or_platform("configuring the UI Automation cache")?;
        for pattern in PATTERNS {
            com!(cache.AddPattern(pattern)).or_platform("configuring the UI Automation cache")?;
        }
        com!(cache.SetTreeScope(TreeScope_Element)).or_platform("configuring the UI Automation cache")?;
        let walker = com!(automation.ControlViewWalker()).or_platform("creating a UI Automation tree walker")?;
        Ok(Self { automation, cache, walker })
    }

    pub(super) fn walk(&self, hwnd: HWND, max_nodes: usize, max_depth: usize) -> Result<Walk> {
        let root = com!(self.automation.ElementFromHandleBuildCache(hwnd, &self.cache))
            .or_platform("reading the window's UI Automation element")?;
        let mut walk = Walk { nodes: Vec::new(), truncated: false };
        if max_nodes == 0 {
            walk.truncated = true;
        } else {
            self.visit(root, None, 0, (max_nodes, max_depth), &mut walk);
        }
        Ok(walk)
    }

    fn visit(
        &self,
        element: IUIAutomationElement,
        parent: Option<usize>,
        depth: usize,
        limits: (usize, usize),
        walk: &mut Walk,
    ) {
        let (max_nodes, max_depth) = limits;
        let index = walk.nodes.len();
        let data = element_data(&element);
        let first = com!(self.walker.GetFirstChildElementBuildCache(&element, &self.cache)).ok();
        walk.nodes.push(RawNode { handle: element, parent, data });
        let Some(mut child) = first else {
            return;
        };
        if depth >= max_depth {
            walk.truncated = true;
            return;
        }
        loop {
            if walk.nodes.len() >= max_nodes {
                walk.truncated = true;
                return;
            }
            let next = com!(self.walker.GetNextSiblingElementBuildCache(&child, &self.cache)).ok();
            self.visit(child, Some(index), depth + 1, limits, walk);
            match next {
                Some(sibling) => child = sibling,
                None => return,
            }
        }
    }

    pub(super) fn focused_native_window(&self, pid: u32) -> Option<HWND> {
        let focused = com!(self.automation.GetFocusedElement()).ok()?;
        if live_pid(&focused).ok()? != pid {
            return None;
        }
        let walker = com!(self.automation.RawViewWalker()).ok()?;
        let mut element = focused;
        for _ in 0..64 {
            if let Ok(hwnd) = com!(element.CurrentNativeWindowHandle())
                && !hwnd.is_invalid()
            {
                return Some(hwnd);
            }
            element = com!(walker.GetParentElement(&element)).ok()?;
        }
        None
    }

    pub(super) fn focus(&self, pid: u32, nodes: &[RawNode<IUIAutomationElement>]) -> (Option<usize>, Option<String>) {
        let focused = com!(self.automation.GetFocusedElement())
            .ok()
            .filter(|f| com!(f.CurrentProcessId()).is_ok_and(|p| u32::try_from(p).is_ok_and(|p| p == pid)));
        let flagged = nodes.iter().rposition(|n| n.data.states.contains(ElementStates::FOCUSED));
        let index = flagged.or_else(|| {
            let focused = focused.as_ref()?;
            nodes
                .iter()
                .position(|n| com!(self.automation.CompareElements(focused, &n.handle)).is_ok_and(BOOL::as_bool))
        });
        (index, focused.as_ref().and_then(selected_text))
    }
}

fn element_data(element: &IUIAutomationElement) -> ElementData {
    let control_type = com!(element.CachedControlType()).unwrap_or(UIA_CustomControlTypeId);
    let (role, platform_role) = control_type_names(control_type);
    let mut data = ElementData::new(role, platform_role);
    data.title = text(com!(element.CachedName()));
    data.identifier = text(com!(element.CachedAutomationId()));
    data.help = text(com!(element.CachedHelpText()));
    data.description = element
        .cast::<IUIAutomationElement6>()
        .ok()
        .and_then(|e| text(com!(e.CachedFullDescription())))
        .filter(|d| data.title.as_ref() != Some(d));
    data.frame =
        com!(element.CachedBoundingRectangle()).ok().filter(|r| r.right > r.left && r.bottom > r.top).map(rect_from);

    let mut states = ElementStates::empty();
    states.set(ElementStates::FOCUSED, flag(com!(element.CachedHasKeyboardFocus())));
    states.set(ElementStates::DISABLED, !com!(element.CachedIsEnabled()).map_or(true, BOOL::as_bool));
    states.set(ElementStates::OFFSCREEN, flag(com!(element.CachedIsOffscreen())));
    let password = flag(com!(element.CachedIsPassword()));

    let mut patterns = Patterns::default();
    if let Some(value) = cached::<IUIAutomationValuePattern>(element, UIA_ValuePatternId) {
        if !password {
            data.value = text(com!(value.CachedValue()));
        }
        if !flag(com!(value.CachedIsReadOnly())) {
            states |= ElementStates::SETTABLE;
            if [UIA_EditControlTypeId, UIA_DocumentControlTypeId, UIA_ComboBoxControlTypeId].contains(&control_type) {
                states |= ElementStates::EDITABLE;
            }
        }
    }
    if let Some(toggle) = cached::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId) {
        patterns.supports.set(Supports::TOGGLE, true);
        let state = com!(toggle.CachedToggleState()).ok();
        states.set(ElementStates::CHECKED, state == Some(ToggleState_On));
        if data.value.is_none() {
            data.value = state.map(|s| toggle_label(s).to_owned());
        }
    }
    if let Some(range) = cached::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId) {
        if data.value.is_none() {
            data.value = com!(range.CachedValue()).ok().map(format_number);
        }
        if !flag(com!(range.CachedIsReadOnly())) {
            states |= ElementStates::SETTABLE;
        }
    }
    if let Some(selection) = cached::<IUIAutomationSelectionItemPattern>(element, UIA_SelectionItemPatternId) {
        patterns.supports.set(Supports::SELECT, true);
        states.set(ElementStates::SELECTED, flag(com!(selection.CachedIsSelected())));
    }
    if let Some(expand) = cached::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId) {
        let state = com!(expand.CachedExpandCollapseState()).ok();
        patterns.expand = state.and_then(expand_state);
        match patterns.expand {
            Some(Expansion::Collapsed) => states |= ElementStates::COLLAPSED,
            Some(Expansion::Expanded) => states |= ElementStates::EXPANDED,
            None => {}
        }
    }
    if let Some(scroll) = cached::<IUIAutomationScrollPattern>(element, UIA_ScrollPatternId) {
        patterns.supports.set(Supports::SCROLL_HORIZONTAL, flag(com!(scroll.CachedHorizontallyScrollable())));
        patterns.supports.set(Supports::SCROLL_VERTICAL, flag(com!(scroll.CachedVerticallyScrollable())));
    }
    if let Some(legacy) = cached::<IUIAutomationLegacyIAccessiblePattern>(element, UIA_LegacyIAccessiblePatternId) {
        patterns.supports.set(Supports::DEFAULT_ACTION, text(com!(legacy.CachedDefaultAction())).is_some());
    }
    patterns
        .supports
        .set(Supports::INVOKE, cached::<IUIAutomationInvokePattern>(element, UIA_InvokePatternId).is_some());
    patterns.supports.set(
        Supports::SCROLL_INTO_VIEW,
        cached::<IUIAutomationScrollItemPattern>(element, UIA_ScrollItemPatternId).is_some(),
    );
    patterns.supports.set(Supports::CONTEXT_MENU, has_context_menu(control_type));

    data.states = states;
    data.actions = patterns.actions();
    data
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expansion {
    Collapsed,
    Expanded,
}

fn expand_state(state: ExpandCollapseState) -> Option<Expansion> {
    if state == ExpandCollapseState_Collapsed {
        Some(Expansion::Collapsed)
    } else if state == ExpandCollapseState_Expanded || state == ExpandCollapseState_PartiallyExpanded {
        Some(Expansion::Expanded)
    } else {
        None
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct Supports: u16 {
        const INVOKE = 1 << 0;
        const TOGGLE = 1 << 1;
        const SELECT = 1 << 2;
        const SCROLL_INTO_VIEW = 1 << 3;
        const SCROLL_HORIZONTAL = 1 << 4;
        const SCROLL_VERTICAL = 1 << 5;
        const DEFAULT_ACTION = 1 << 6;
        const CONTEXT_MENU = 1 << 7;
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct Patterns {
    supports: Supports,
    expand: Option<Expansion>,
}

impl Patterns {
    fn actions(self) -> Vec<Action> {
        let mut actions: Vec<Action> = Vec::new();
        let push = |actions: &mut Vec<Action>, name: &str, clicks: bool| {
            let primary = clicks && !actions.iter().any(|a| a.primary);
            actions.push(Action::new(name, primary));
        };
        if self.supports.contains(Supports::INVOKE) {
            push(&mut actions, "Invoke", true);
        }
        if self.supports.contains(Supports::TOGGLE) {
            push(&mut actions, "Toggle", true);
        }
        if self.supports.contains(Supports::SELECT) {
            push(&mut actions, "Select", true);
        }
        match self.expand {
            Some(Expansion::Collapsed) => push(&mut actions, "Expand", true),
            Some(Expansion::Expanded) => push(&mut actions, "Collapse", true),
            None => {}
        }
        if self.supports.contains(Supports::DEFAULT_ACTION) {
            push(&mut actions, "DoDefaultAction", true);
        }
        if self.supports.contains(Supports::CONTEXT_MENU) || !actions.is_empty() {
            push(&mut actions, "ShowMenu", false);
        }
        if self.supports.contains(Supports::SCROLL_INTO_VIEW) {
            push(&mut actions, "ScrollIntoView", false);
        }
        if self.supports.contains(Supports::SCROLL_VERTICAL) {
            push(&mut actions, "ScrollUpByPage", false);
            push(&mut actions, "ScrollDownByPage", false);
        }
        if self.supports.contains(Supports::SCROLL_HORIZONTAL) {
            push(&mut actions, "ScrollLeftByPage", false);
            push(&mut actions, "ScrollRightByPage", false);
        }
        actions
    }
}

fn has_context_menu(control_type: UIA_CONTROLTYPE_ID) -> bool {
    [
        UIA_EditControlTypeId,
        UIA_DocumentControlTypeId,
        UIA_ListItemControlTypeId,
        UIA_TreeItemControlTypeId,
        UIA_DataItemControlTypeId,
        UIA_TabItemControlTypeId,
        UIA_HyperlinkControlTypeId,
    ]
    .contains(&control_type)
}

const CONTROL_TYPES: [(UIA_CONTROLTYPE_ID, &str, &str); 41] = [
    (UIA_ButtonControlTypeId, "button", "Button"),
    (UIA_CalendarControlTypeId, "calendar", "Calendar"),
    (UIA_CheckBoxControlTypeId, "check box", "CheckBox"),
    (UIA_ComboBoxControlTypeId, "combo box", "ComboBox"),
    (UIA_EditControlTypeId, "text field", "Edit"),
    (UIA_HyperlinkControlTypeId, "link", "Hyperlink"),
    (UIA_ImageControlTypeId, "image", "Image"),
    (UIA_ListItemControlTypeId, "list item", "ListItem"),
    (UIA_ListControlTypeId, "list", "List"),
    (UIA_MenuControlTypeId, "menu", "Menu"),
    (UIA_MenuBarControlTypeId, "menu bar", "MenuBar"),
    (UIA_MenuItemControlTypeId, "menu item", "MenuItem"),
    (UIA_ProgressBarControlTypeId, "progress bar", "ProgressBar"),
    (UIA_RadioButtonControlTypeId, "radio button", "RadioButton"),
    (UIA_ScrollBarControlTypeId, "scroll bar", "ScrollBar"),
    (UIA_SliderControlTypeId, "slider", "Slider"),
    (UIA_SpinnerControlTypeId, "spinner", "Spinner"),
    (UIA_StatusBarControlTypeId, "status bar", "StatusBar"),
    (UIA_TabControlTypeId, "tab list", "Tab"),
    (UIA_TabItemControlTypeId, "tab", "TabItem"),
    (UIA_TextControlTypeId, "text", "Text"),
    (UIA_ToolBarControlTypeId, "toolbar", "ToolBar"),
    (UIA_ToolTipControlTypeId, "tooltip", "ToolTip"),
    (UIA_TreeControlTypeId, "tree", "Tree"),
    (UIA_TreeItemControlTypeId, "tree item", "TreeItem"),
    (UIA_GroupControlTypeId, "group", "Group"),
    (UIA_ThumbControlTypeId, "thumb", "Thumb"),
    (UIA_DataGridControlTypeId, "data grid", "DataGrid"),
    (UIA_DataItemControlTypeId, "data item", "DataItem"),
    (UIA_DocumentControlTypeId, "document", "Document"),
    (UIA_SplitButtonControlTypeId, "split button", "SplitButton"),
    (UIA_WindowControlTypeId, "window", "Window"),
    (UIA_PaneControlTypeId, "pane", "Pane"),
    (UIA_HeaderControlTypeId, "header", "Header"),
    (UIA_HeaderItemControlTypeId, "header item", "HeaderItem"),
    (UIA_TableControlTypeId, "table", "Table"),
    (UIA_TitleBarControlTypeId, "title bar", "TitleBar"),
    (UIA_SeparatorControlTypeId, "separator", "Separator"),
    (UIA_SemanticZoomControlTypeId, "semantic zoom", "SemanticZoom"),
    (UIA_AppBarControlTypeId, "app bar", "AppBar"),
    (UIA_CustomControlTypeId, "custom", "Custom"),
];

fn control_type_names(control_type: UIA_CONTROLTYPE_ID) -> (&'static str, &'static str) {
    CONTROL_TYPES
        .iter()
        .find(|(id, _, _)| *id == control_type)
        .map_or(("unknown", "Unknown"), |&(_, role, raw)| (role, raw))
}

fn toggle_label(state: ToggleState) -> &'static str {
    if state == ToggleState_On {
        "on"
    } else if state == ToggleState_Indeterminate {
        "mixed"
    } else {
        "off"
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{value:.0}") } else { value.to_string() }
}

pub(super) enum ActionOutcome {
    Done,
    RightClick { pid: u32, point: Point },
}

pub(super) fn perform_action(element: &IUIAutomationElement, action: &str) -> Result<ActionOutcome> {
    let normalized: String =
        action.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    let failed = |what: &str| format!("{what} failed");
    match normalized.as_str() {
        "invoke" | "press" => {
            let invoke = required::<IUIAutomationInvokePattern>(element, UIA_InvokePatternId, action)?;
            com!(invoke.Invoke()).or_action(&failed("Invoke"))?;
        }
        "toggle" => {
            let toggle = required::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId, action)?;
            com!(toggle.Toggle()).or_action(&failed("Toggle"))?;
        }
        "select" => {
            let selection = required::<IUIAutomationSelectionItemPattern>(element, UIA_SelectionItemPatternId, action)?;
            com!(selection.Select()).or_action(&failed("Select"))?;
        }
        "expand" | "collapse" => {
            let expand = required::<IUIAutomationExpandCollapsePattern>(element, UIA_ExpandCollapsePatternId, action)?;
            if normalized == "expand" {
                com!(expand.Expand()).or_action(&failed("Expand"))?;
            } else {
                com!(expand.Collapse()).or_action(&failed("Collapse"))?;
            }
        }
        "scrollintoview" => {
            let item = required::<IUIAutomationScrollItemPattern>(element, UIA_ScrollItemPatternId, action)?;
            com!(item.ScrollIntoView()).or_action(&failed("ScrollIntoView"))?;
        }
        "scrollupbypage" => scroll(element, ScrollAmount_NoAmount, ScrollAmount_LargeDecrement, action)?,
        "scrolldownbypage" => scroll(element, ScrollAmount_NoAmount, ScrollAmount_LargeIncrement, action)?,
        "scrollleftbypage" => scroll(element, ScrollAmount_LargeDecrement, ScrollAmount_NoAmount, action)?,
        "scrollrightbypage" => scroll(element, ScrollAmount_LargeIncrement, ScrollAmount_NoAmount, action)?,
        "dodefaultaction" => {
            let legacy =
                required::<IUIAutomationLegacyIAccessiblePattern>(element, UIA_LegacyIAccessiblePatternId, action)?;
            com!(legacy.DoDefaultAction()).or_action(&failed("DoDefaultAction"))?;
        }
        "focus" => com!(element.SetFocus()).or_action(&failed("SetFocus"))?,
        "showmenu" | "showcontextmenu" => return show_menu(element),
        _ => return Err(Error::invalid(format!("unknown UI Automation action {action:?}"))),
    }
    Ok(ActionOutcome::Done)
}

fn scroll(
    element: &IUIAutomationElement,
    horizontal: ScrollAmount,
    vertical: ScrollAmount,
    action: &str,
) -> Result<()> {
    let pattern = required::<IUIAutomationScrollPattern>(element, UIA_ScrollPatternId, action)?;
    com!(pattern.Scroll(horizontal, vertical)).or_action("Scroll failed")
}

fn show_menu(element: &IUIAutomationElement) -> Result<ActionOutcome> {
    if let Ok(element3) = element.cast::<IUIAutomationElement3>()
        && com!(element3.ShowContextMenu()).is_ok()
    {
        return Ok(ActionOutcome::Done);
    }
    let pid = live_pid(element)?;
    let rect: RECT = com!(element.CurrentBoundingRectangle()).or_action("reading the element's frame")?;
    let frame = rect_from(rect);
    if frame.is_empty() {
        return Err(Error::action("the element has no context menu and no on-screen frame"));
    }
    Ok(ActionOutcome::RightClick { pid, point: frame.center() })
}

pub(super) fn set_value(element: &IUIAutomationElement, value: &str) -> Result<()> {
    live_pid(element)?;
    if let Some(pattern) = current::<IUIAutomationValuePattern>(element, UIA_ValuePatternId)
        && !flag(com!(pattern.CurrentIsReadOnly()))
    {
        return com!(pattern.SetValue(&BSTR::from(value))).or_action("setting the value failed");
    }
    if let Some(pattern) = current::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId)
        && !flag(com!(pattern.CurrentIsReadOnly()))
    {
        let number: f64 =
            value.trim().parse().map_err(|_| Error::invalid(format!("{value:?} is not a number for this element")))?;
        return com!(pattern.SetValue(number)).or_action("setting the value failed");
    }
    Err(Error::action("the element's value is not settable"))
}

pub(super) fn value(element: &IUIAutomationElement) -> Result<Option<String>> {
    live_pid(element)?;
    if let Some(pattern) = current::<IUIAutomationTextPattern>(element, UIA_TextPatternId)
        && let Ok(range) = com!(pattern.DocumentRange())
        && let Ok(text) = com!(range.GetText(-1))
    {
        return Ok(Some(text.to_string()));
    }
    if let Some(pattern) = current::<IUIAutomationValuePattern>(element, UIA_ValuePatternId)
        && let Ok(value) = com!(pattern.CurrentValue())
    {
        return Ok(Some(value.to_string()));
    }
    if let Some(pattern) = current::<IUIAutomationRangeValuePattern>(element, UIA_RangeValuePatternId)
        && let Ok(value) = com!(pattern.CurrentValue())
    {
        return Ok(Some(format_number(value)));
    }
    if let Some(pattern) = current::<IUIAutomationTogglePattern>(element, UIA_TogglePatternId)
        && let Ok(state) = com!(pattern.CurrentToggleState())
    {
        return Ok(Some(toggle_label(state).to_owned()));
    }
    Ok(None)
}

pub(super) fn select_text(element: &IUIAutomationElement, value: &str, range: Range<usize>) -> Result<()> {
    if range.start > range.end || !value.is_char_boundary(range.start) || !value.is_char_boundary(range.end) {
        return Err(Error::invalid(format!("{range:?} is not a valid range in the element's text")));
    }
    let to_units = |bytes: usize| {
        i32::try_from(char_offset(value, bytes)).map_err(|_| Error::invalid("selection offset is too large"))
    };
    let (start, end) = (to_units(range.start)?, to_units(range.end)?);
    let pattern = current::<IUIAutomationTextPattern>(element, UIA_TextPatternId)
        .ok_or_else(|| Error::Unsupported("the element does not expose the UI Automation Text pattern".into()))?;
    let document = com!(pattern.DocumentRange()).or_action("reading the element's text")?;
    let selection = com!(document.Clone()).or_action("reading the element's text")?;
    com!(selection.MoveEndpointByRange(TextPatternRangeEndpoint_End, &document, TextPatternRangeEndpoint_Start))
        .or_action("collapsing the text range")?;
    if end > 0 {
        com!(selection.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, end))
            .or_action("moving the selection end")?;
    }
    if start > 0 {
        com!(selection.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, start))
            .or_action("moving the selection start")?;
    }
    com!(selection.Select()).or_action("selecting the text")
}

fn selected_text(element: &IUIAutomationElement) -> Option<String> {
    let pattern = current::<IUIAutomationTextPattern>(element, UIA_TextPatternId)?;
    let ranges = com!(pattern.GetSelection()).ok()?;
    let count = com!(ranges.Length()).ok()?;
    let mut selected = String::new();
    for i in 0..count {
        if let Ok(range) = com!(ranges.GetElement(i))
            && let Ok(text) = com!(range.GetText(-1))
        {
            selected.push_str(&text.to_string());
        }
    }
    (!selected.is_empty()).then_some(selected)
}

fn live_pid(element: &IUIAutomationElement) -> Result<u32> {
    let pid = com!(element.CurrentProcessId()).or_action("the element is no longer available")?;
    u32::try_from(pid).map_err(|_| Error::action("the element reported an invalid process ID"))
}

fn cached<T: Interface>(element: &IUIAutomationElement, pattern: UIA_PATTERN_ID) -> Option<T> {
    com!(element.GetCachedPatternAs::<T>(pattern)).ok()
}

fn current<T: Interface>(element: &IUIAutomationElement, pattern: UIA_PATTERN_ID) -> Option<T> {
    com!(element.GetCurrentPatternAs::<T>(pattern)).ok()
}

fn required<T: Interface>(element: &IUIAutomationElement, pattern: UIA_PATTERN_ID, action: &str) -> Result<T> {
    live_pid(element)?;
    current(element, pattern).ok_or_else(|| Error::action(format!("the element does not support {action:?}")))
}

fn text(value: windows::core::Result<BSTR>) -> Option<String> {
    value.ok().map(|s| s.to_string()).filter(|s| !s.trim().is_empty())
}

fn flag(value: windows::core::Result<BOOL>) -> bool {
    value.is_ok_and(BOOL::as_bool)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(patterns: Patterns) -> Vec<(String, bool)> {
        patterns.actions().into_iter().map(|a| (a.name, a.primary)).collect()
    }

    #[test]
    fn maps_control_types() {
        assert_eq!(control_type_names(UIA_EditControlTypeId), ("text field", "Edit"));
        assert_eq!(control_type_names(UIA_CheckBoxControlTypeId), ("check box", "CheckBox"));
        assert_eq!(control_type_names(UIA_PaneControlTypeId), ("pane", "Pane"));
        assert_eq!(control_type_names(UIA_CONTROLTYPE_ID(1)), ("unknown", "Unknown"));
    }

    #[test]
    fn synthesizes_actions_with_one_primary() {
        let button = Patterns { supports: Supports::INVOKE | Supports::TOGGLE, expand: None };
        assert_eq!(names(button), vec![("Invoke".into(), true), ("Toggle".into(), false), ("ShowMenu".into(), false)]);
        let item =
            Patterns { supports: Supports::SELECT | Supports::SCROLL_INTO_VIEW | Supports::CONTEXT_MENU, expand: None };
        assert_eq!(
            names(item),
            vec![("Select".into(), true), ("ShowMenu".into(), false), ("ScrollIntoView".into(), false)]
        );
        let combo = Patterns { supports: Supports::empty(), expand: Some(Expansion::Collapsed) };
        assert_eq!(names(combo)[0], ("Expand".into(), true));
        assert_eq!(names(Patterns::default()), Vec::new());
        let list = Patterns { supports: Supports::SCROLL_VERTICAL, expand: None };
        let actions = list.actions();
        assert!(actions.iter().any(|a| a.matches("Scroll down by page")));
        let menu = Patterns { supports: Supports::CONTEXT_MENU, expand: None }.actions();
        assert!(menu.iter().any(|a| a.matches("Show Menu")));
    }

    #[test]
    fn formats_values() {
        assert_eq!(format_number(42.0), "42");
        assert_eq!(format_number(0.5), "0.5");
        assert_eq!(toggle_label(ToggleState_On), "on");
        assert_eq!(expand_state(ExpandCollapseState_PartiallyExpanded), Some(Expansion::Expanded));
    }
}
