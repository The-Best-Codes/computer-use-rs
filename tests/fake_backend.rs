//! Drives `Computer` through an in-memory backend to test the platform-neutral logic.

use std::cell::RefCell;
use std::ops::Range;

use computer_use::keys::{Key, KeyChord, Modifiers};
use computer_use::{
    Action, AppInfo, Backend, Capture, CaptureOptions, CaptureRequest, ClickMethod, ClickOptions, ClickOutcome,
    ClickTarget, Computer, Delivery, ElementData, ElementQuery, ElementStates, Error, InputTarget, MouseButton, Point,
    RawImage, RawNode, Rect, Result, ScrollDirection, SelectionMode, TextSelection, WindowInfo, WindowSelector,
};

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Action(usize, String),
    SetValue(usize, String),
    Select(usize, Range<usize>),
    Click(Point, MouseButton, u32, Delivery),
    Scroll(Point, f64, f64),
    Drag(Point, Point),
    Type(String),
    Key(KeyChord),
}

#[derive(Default)]
struct Fake {
    calls: RefCell<Vec<Call>>,
    requests: RefCell<Vec<CaptureRequest>>,
}

const WINDOW: Rect = Rect::new(100.0, 50.0, 400.0, 300.0);
const INSPECTOR: Rect = Rect::new(600.0, 50.0, 200.0, 300.0);
const DOC: &str = "one two one three";

fn node(parent: Option<usize>, role: &str, title: Option<&str>, frame: Rect) -> RawNode<usize> {
    let mut data = ElementData::new(role, format!("AX{role}"));
    data.title = title.map(str::to_owned);
    data.frame = Some(frame);
    RawNode { handle: 0, parent, data }
}

impl Backend for Fake {
    type Handle = usize;

    fn list_apps(&self) -> Result<Vec<AppInfo>> {
        Ok(vec![
            AppInfo::new("Editor").with_bundle_id("com.example.editor").with_pid(42),
            AppInfo::new("Viewer").with_bundle_id("com.example.viewer"),
        ])
    }

    fn list_windows(&self, _app: &AppInfo) -> Result<Vec<WindowInfo>> {
        Ok(vec![
            WindowInfo::new(Some(7), Some("Doc".into()), WINDOW),
            WindowInfo::new(Some(8), Some("Inspector".into()), INSPECTOR),
        ])
    }

    fn capture(&self, app: &AppInfo, request: &CaptureRequest) -> Result<Capture<usize>> {
        self.requests.borrow_mut().push(*request);
        let window = match request.window {
            None | Some(7) => WindowInfo::new(Some(7), Some("Doc".into()), WINDOW),
            Some(8) => WindowInfo::new(Some(8), Some("Inspector".into()), INSPECTOR),
            Some(_) => return Err(Error::WindowNotFound(app.name.clone())),
        };
        let mut capture = Capture::new(window);
        if request.include_tree {
            let mut nodes = vec![
                node(None, "window", Some("Doc"), WINDOW),
                node(Some(0), "group", None, WINDOW),
                node(Some(1), "button", Some("Save"), Rect::new(110.0, 60.0, 40.0, 20.0)),
                node(Some(1), "text area", None, Rect::new(110.0, 100.0, 380.0, 200.0)),
                node(Some(0), "button", Some("Draw"), Rect::new(160.0, 60.0, 40.0, 20.0)),
            ];
            nodes[2].data.actions = vec![Action::new("AXPress", true), Action::new("AXShowMenu", false)];
            nodes[3].data.value = Some(DOC.into());
            nodes[3].data.states = ElementStates::FOCUSED | ElementStates::SETTABLE;
            nodes[3].data.actions = vec![Action::new("AXScrollDownByPage", false)];
            for (i, n) in nodes.iter_mut().enumerate() {
                n.handle = i;
            }
            capture.nodes = nodes;
            capture.focused = Some(3);
        }
        if request.include_screenshot {
            capture.screenshot = Some(RawImage { width: 800, height: 600, rgba: vec![0; 800 * 600 * 4] });
        }
        Ok(capture)
    }

    fn perform_action(&self, element: &usize, action: &str) -> Result<()> {
        self.calls.borrow_mut().push(Call::Action(*element, action.into()));
        Ok(())
    }

    fn set_value(&self, element: &usize, value: &str) -> Result<()> {
        self.calls.borrow_mut().push(Call::SetValue(*element, value.into()));
        Ok(())
    }

    fn value(&self, element: &usize) -> Result<Option<String>> {
        Ok((*element == 3).then(|| DOC.to_owned()))
    }

    fn select_text(&self, element: &usize, _value: &str, range: Range<usize>) -> Result<()> {
        self.calls.borrow_mut().push(Call::Select(*element, range));
        Ok(())
    }

    fn click(&self, target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()> {
        self.calls.borrow_mut().push(Call::Click(point, button, count, target.delivery));
        Ok(())
    }

    fn scroll(&self, _target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()> {
        self.calls.borrow_mut().push(Call::Scroll(point, dx, dy));
        Ok(())
    }

    fn drag(&self, _target: InputTarget<'_>, from: Point, to: Point) -> Result<()> {
        self.calls.borrow_mut().push(Call::Drag(from, to));
        Ok(())
    }

    fn type_text(&self, _target: InputTarget<'_>, text: &str) -> Result<()> {
        self.calls.borrow_mut().push(Call::Type(text.into()));
        Ok(())
    }

    fn press_key(&self, _target: InputTarget<'_>, chord: &KeyChord) -> Result<()> {
        self.calls.borrow_mut().push(Call::Key(*chord));
        Ok(())
    }
}

fn computer() -> Result<Computer<Fake>> {
    let mut computer = Computer::with_backend(Fake::default());
    computer.get_app_state("Editor", &CaptureOptions::default())?;
    Ok(computer)
}

fn calls(computer: &Computer<Fake>) -> Vec<Call> {
    computer.backend().calls.borrow().clone()
}

#[test]
fn captures_and_prunes_tree() -> Result<()> {
    let mut computer = Computer::with_backend(Fake::default());
    let state = computer.get_app_state("com.example.editor", &CaptureOptions::default())?;
    let roles: Vec<_> = state.elements.iter().map(|e| (e.index, e.depth, e.role.as_str())).collect();
    assert_eq!(roles, vec![(0, 0, "window"), (1, 1, "button"), (2, 1, "text area"), (3, 1, "button")]);
    assert_eq!(state.focused, Some(2));
    let shot = state.screenshot.as_ref().ok_or(Error::StateUnavailable("screenshot".into()))?;
    assert_eq!((shot.width(), shot.height()), (800, 600));
    assert!(state.render().contains("1 button Save Secondary Actions: Show Menu"));
    assert!(computer.state("Editor").is_some());
    assert!(computer.state("pid:42").is_some());
    Ok(())
}

#[test]
fn reports_missing_apps_and_state() {
    let mut computer = Computer::with_backend(Fake::default());
    assert!(matches!(computer.get_app_state("Nope", &CaptureOptions::default()), Err(Error::AppNotFound(_))));
    assert!(matches!(computer.get_app_state("Viewer", &CaptureOptions::default()), Err(Error::Unsupported(_))));
    assert!(matches!(computer.type_text("Viewer", "x"), Err(Error::AppNotFound(_))));
    assert!(matches!(
        computer.click("Editor", ClickTarget::Element(0), ClickOptions::default()),
        Err(Error::NoAppState(_))
    ));
}

#[test]
fn click_prefers_accessibility_actions() -> Result<()> {
    let mut computer = computer()?;
    let outcome = computer.click("Editor", ClickTarget::Element(1), ClickOptions::default())?;
    assert_eq!(outcome, ClickOutcome::Action("AXPress".into()));
    let right = ClickOptions::default().button(MouseButton::Right);
    computer.click("Editor", ClickTarget::Element(1), right)?;
    assert_eq!(calls(&computer), vec![Call::Action(2, "AXPress".into()), Call::Action(2, "AXShowMenu".into())]);
    Ok(())
}

#[test]
fn click_falls_back_to_pointer_at_element_center() -> Result<()> {
    let mut computer = computer()?;
    let outcome = computer.click("Editor", ClickTarget::Element(3), ClickOptions::default().count(2))?;
    assert_eq!(outcome, ClickOutcome::Pointer(Delivery::Background));
    assert_eq!(
        calls(&computer),
        vec![Call::Click(Point::new(180.0, 70.0), MouseButton::Left, 2, Delivery::Background)]
    );

    let ax_only = ClickOptions::default().method(ClickMethod::Accessibility);
    assert!(computer.click("Editor", ClickTarget::Element(3), ax_only).is_err());
    assert!(matches!(
        computer.click("Editor", ClickTarget::Element(99), ClickOptions::default()),
        Err(Error::ElementNotFound { index: 99, .. })
    ));
    Ok(())
}

#[test]
fn pixel_clicks_map_screenshot_pixels_to_screen_points() -> Result<()> {
    let mut computer = computer()?;
    let global = ClickOptions::default().method(ClickMethod::Global);
    computer.click("Editor", ClickTarget::Pixel { x: 400.0, y: 300.0 }, global)?;
    assert_eq!(calls(&computer), vec![Call::Click(Point::new(300.0, 200.0), MouseButton::Left, 1, Delivery::Global)]);
    assert!(computer.click("Editor", ClickTarget::Pixel { x: 900.0, y: 10.0 }, global).is_err());

    computer.drag("Editor", (0.0, 0.0), (800.0, 600.0))?;
    assert_eq!(calls(&computer)[1], Call::Drag(Point::new(100.0, 50.0), Point::new(500.0, 350.0)));
    Ok(())
}

#[test]
fn coordinate_actions_need_a_screenshot() -> Result<()> {
    let mut computer = Computer::with_backend(Fake::default());
    computer.get_app_state("Editor", &CaptureOptions::default().include_screenshot(false))?;
    let result = computer.click("Editor", ClickTarget::Pixel { x: 1.0, y: 1.0 }, ClickOptions::default());
    assert!(matches!(result, Err(Error::StateUnavailable(_))));
    Ok(())
}

#[test]
fn scroll_uses_page_actions_or_wheel() -> Result<()> {
    let mut computer = computer()?;
    computer.scroll("Editor", 2, ScrollDirection::Down, 2.0)?;
    computer.scroll("Editor", 2, ScrollDirection::Up, 0.5)?;
    assert_eq!(
        calls(&computer),
        vec![
            Call::Action(3, "AXScrollDownByPage".into()),
            Call::Action(3, "AXScrollDownByPage".into()),
            Call::Scroll(Point::new(300.0, 200.0), 0.0, -80.0),
        ]
    );
    assert!(computer.scroll("Editor", 2, ScrollDirection::Down, 0.0).is_err());
    Ok(())
}

#[test]
fn text_value_and_keys() -> Result<()> {
    let mut computer = computer()?;
    computer.set_value("Editor", 2, "hello")?;
    computer.select_text("Editor", 2, &TextSelection::new("one").prefix("two "))?;
    computer.select_text("Editor", 2, &TextSelection::new("three").mode(SelectionMode::CursorAfter))?;
    assert!(computer.select_text("Editor", 2, &TextSelection::new("one")).is_err());
    computer.type_text("Editor", "abc")?;
    computer.press_key("Editor", "ctrl+shift+Tab")?;
    assert!(computer.press_key("Editor", "hyper+q").is_err());
    computer.perform_secondary_action("Editor", 1, "show menu")?;
    assert!(computer.perform_secondary_action("Editor", 1, "explode").is_err());
    assert_eq!(
        calls(&computer),
        vec![
            Call::SetValue(3, "hello".into()),
            Call::Select(3, 8..11),
            Call::Select(3, 17..17),
            Call::Type("abc".into()),
            Call::Key(KeyChord::new(Key::Tab).with(Modifiers::CONTROL | Modifiers::SHIFT)),
            Call::Action(2, "AXShowMenu".into()),
        ]
    );
    Ok(())
}

#[test]
fn query_refreshes_indexes_and_keeps_screenshot() -> Result<()> {
    let mut computer = computer()?;
    let matches = computer.query("Editor", &ElementQuery::role("button"), 100)?;
    assert_eq!(matches.iter().map(|e| e.title.as_deref()).collect::<Vec<_>>(), vec![Some("Save"), Some("Draw")]);
    let state = computer.state("Editor").ok_or(Error::NoAppState("Editor".into()))?;
    assert!(state.screenshot.is_some(), "screenshot from the earlier capture is kept");
    assert!(computer.query("Editor", &ElementQuery::default(), 100).is_err());
    Ok(())
}

#[test]
fn list_apps_puts_running_apps_first() -> Result<()> {
    let computer = Computer::with_backend(Fake::default());
    let names: Vec<_> = computer.list_apps()?.into_iter().map(|a| a.name).collect();
    assert_eq!(names, vec!["Editor", "Viewer"]);
    Ok(())
}

fn last_request(computer: &Computer<Fake>) -> Result<CaptureRequest> {
    computer.backend().requests.borrow().last().copied().ok_or(Error::StateUnavailable("no capture".into()))
}

#[test]
fn selects_windows_by_id_or_title() -> Result<()> {
    let mut computer = Computer::with_backend(Fake::default());
    let titles: Vec<_> = computer.list_windows("Editor")?.into_iter().filter_map(|w| w.title).collect();
    assert_eq!(titles, vec!["Doc", "Inspector"]);

    let state =
        computer.get_app_state("Editor", &CaptureOptions::default().window(WindowSelector::Title("insp".into())))?;
    assert_eq!(state.window.id, Some(8));
    assert_eq!(last_request(&computer)?.window, Some(8));

    // A later query stays on the chosen window.
    computer.query("Editor", &ElementQuery::role("button"), 100)?;
    assert_eq!(last_request(&computer)?.window, Some(8));

    let doc =
        computer.get_app_state("Editor", &CaptureOptions::default().window(WindowSelector::Title("DOC".into())))?;
    assert_eq!(doc.window.id, Some(7));
    computer.get_app_state("Editor", &CaptureOptions::default())?;
    computer.query("Editor", &ElementQuery::role("button"), 100)?;
    assert_eq!(last_request(&computer)?.window, None, "a default capture resets the choice");

    let missing = CaptureOptions::default().window(WindowSelector::Title("nope".into()));
    assert!(matches!(computer.get_app_state("Editor", &missing), Err(Error::WindowNotFound(_))));
    let unknown_id = CaptureOptions::default().window(WindowSelector::Id(99));
    assert!(matches!(computer.get_app_state("Editor", &unknown_id), Err(Error::WindowNotFound(_))));
    Ok(())
}

#[test]
fn passes_menu_bar_option_to_backend() -> Result<()> {
    let mut computer = computer()?;
    assert!(!last_request(&computer)?.include_menu_bar);
    computer.get_app_state("Editor", &CaptureOptions::default().include_menu_bar(true))?;
    assert!(last_request(&computer)?.include_menu_bar);
    Ok(())
}

#[test]
fn screen_point_actions_need_no_capture() -> Result<()> {
    let mut computer = Computer::with_backend(Fake::default());
    let at = ClickTarget::Screen { x: 10.0, y: 20.0 };
    computer.click("Editor", at, ClickOptions::default())?;
    computer.drag_screen("Editor", Point::new(1.0, 2.0), Point::new(3.0, 4.0))?;
    computer.scroll_at("Editor", at, ScrollDirection::Down, 1.0)?;
    assert_eq!(
        calls(&computer),
        vec![
            Call::Click(Point::new(10.0, 20.0), MouseButton::Left, 1, Delivery::Background),
            Call::Drag(Point::new(1.0, 2.0), Point::new(3.0, 4.0)),
            Call::Scroll(Point::new(10.0, 20.0), 0.0, 400.0),
        ]
    );
    let ax_only = ClickOptions::default().method(ClickMethod::Accessibility);
    assert!(computer.click("Editor", at, ax_only).is_err());
    assert!(computer.click("Editor", ClickTarget::Screen { x: f64::NAN, y: 0.0 }, ClickOptions::default()).is_err());
    assert!(computer.click("Viewer", at, ClickOptions::default()).is_err(), "the app must be running");
    Ok(())
}

#[test]
fn scroll_at_points_and_elements() -> Result<()> {
    let mut computer = computer()?;
    // Pixel (400, 300) is screen (300, 200); a page is 80% of the 300-point-high window.
    computer.scroll_at("Editor", ClickTarget::Pixel { x: 400.0, y: 300.0 }, ScrollDirection::Up, 0.5)?;
    computer.scroll_at("Editor", ClickTarget::Element(2), ScrollDirection::Down, 1.0)?;
    assert_eq!(
        calls(&computer),
        vec![Call::Scroll(Point::new(300.0, 200.0), 0.0, -120.0), Call::Action(3, "AXScrollDownByPage".into())]
    );
    assert!(computer.scroll_at("Editor", ClickTarget::Pixel { x: 900.0, y: 0.0 }, ScrollDirection::Up, 1.0).is_err());
    assert!(computer.scroll_at("Editor", ClickTarget::Pixel { x: 1.0, y: 1.0 }, ScrollDirection::Up, -1.0).is_err());
    Ok(())
}
