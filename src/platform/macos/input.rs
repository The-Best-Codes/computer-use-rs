use std::thread;
use std::time::Duration;

use objc2_core_foundation::{CFRetained, CGPoint};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGEventType,
    CGMouseButton, CGScrollEventUnit,
};

use super::{ax, keys};
use crate::backend::{Delivery, InputTarget, MouseButton};
use crate::error::{Error, Result};
use crate::geometry::Point;
use crate::keys::KeyChord;

const EVENT_GAP: Duration = Duration::from_millis(12);
const UNICODE_CHUNK: usize = 16;
const SCROLL_STEP: f64 = 120.0;
const ACTIVATION_DELAY: Duration = Duration::from_millis(200);

struct Sink {
    pid: Option<i32>,
    window: Option<i64>,
    source: CFRetained<CGEventSource>,
}

impl Sink {
    fn new(target: InputTarget<'_>) -> Result<Self> {
        if target.delivery == Delivery::Global {
            bring_to_front(target)?;
        }
        let pid = match target.delivery {
            Delivery::Background | Delivery::FocusedBackground => Some(
                target
                    .app
                    .pid
                    .and_then(|p| i32::try_from(p).ok())
                    .ok_or_else(|| Error::invalid(format!("{} is not running", target.app.name)))?,
            ),
            _ => None,
        };
        let state = if pid.is_some() {
            CGEventSourceStateID::CombinedSessionState
        } else {
            CGEventSourceStateID::HIDSystemState
        };
        let source = CGEventSource::new(state).ok_or_else(|| Error::platform("could not create a CGEventSource"))?;
        Ok(Self { pid, window: target.window.and_then(|w| w.id).and_then(|id| i64::try_from(id).ok()), source })
    }

    fn post(&self, event: &CGEvent) {
        match self.pid {
            Some(pid) => CGEvent::post_to_pid(pid, Some(event)),
            None => CGEvent::post(CGEventTapLocation::HIDEventTap, Some(event)),
        }
    }

    fn mouse(&self, kind: CGEventType, point: Point, button: CGMouseButton, clicks: i64) -> Result<()> {
        let event = CGEvent::new_mouse_event(Some(&self.source), kind, CGPoint::new(point.x, point.y), button)
            .ok_or_else(|| Error::platform("could not create a mouse event"))?;
        CGEvent::set_integer_value_field(Some(&event), CGEventField::MouseEventClickState, clicks);
        if let Some(window) = self.window {
            for field in [
                CGEventField::MouseEventWindowUnderMousePointer,
                CGEventField::MouseEventWindowUnderMousePointerThatCanHandleThisEvent,
            ] {
                CGEvent::set_integer_value_field(Some(&event), field, window);
            }
        }
        self.post(&event);
        Ok(())
    }

    fn send_key(&self, code: u16, down: bool, flags: CGEventFlags) -> Result<()> {
        let event = self.key(code, down, flags)?;
        self.post(&event);
        Ok(())
    }

    fn key(&self, code: u16, down: bool, flags: CGEventFlags) -> Result<CFRetained<CGEvent>> {
        let event = CGEvent::new_keyboard_event(Some(&self.source), code, down)
            .ok_or_else(|| Error::platform("could not create a keyboard event"))?;
        CGEvent::set_flags(Some(&event), flags);
        Ok(event)
    }
}

fn bring_to_front(target: InputTarget<'_>) -> Result<()> {
    let pid = target
        .app
        .pid
        .and_then(|p| i32::try_from(p).ok())
        .ok_or_else(|| Error::invalid(format!("{} is not running", target.app.name)))?;
    let app = ax::application(pid);
    if ax::boolean(&app, "AXFrontmost") == Some(true) {
        return Ok(());
    }
    ax::set_bool(&app, "AXFrontmost", true)?;
    if let Some(window) = ax::element(&app, "AXFocusedWindow").or_else(|| ax::element(&app, "AXMainWindow")) {
        let _ = ax::perform(&window, "AXRaise");
    }
    thread::sleep(ACTIVATION_DELAY);
    Ok(())
}

const fn button_events(button: MouseButton) -> (CGEventType, CGEventType, CGMouseButton) {
    match button {
        MouseButton::Right => (CGEventType::RightMouseDown, CGEventType::RightMouseUp, CGMouseButton::Right),
        MouseButton::Middle => (CGEventType::OtherMouseDown, CGEventType::OtherMouseUp, CGMouseButton::Center),
        _ => (CGEventType::LeftMouseDown, CGEventType::LeftMouseUp, CGMouseButton::Left),
    }
}

pub(super) fn click(target: InputTarget<'_>, point: Point, button: MouseButton, count: u32) -> Result<()> {
    let sink = Sink::new(target)?;
    let (down, up, cg_button) = button_events(button);
    sink.mouse(CGEventType::MouseMoved, point, cg_button, 0)?;
    for n in 1..=i64::from(count) {
        sink.mouse(down, point, cg_button, n)?;
        thread::sleep(EVENT_GAP);
        sink.mouse(up, point, cg_button, n)?;
        thread::sleep(EVENT_GAP);
    }
    Ok(())
}

pub(super) fn drag(target: InputTarget<'_>, from: Point, to: Point) -> Result<()> {
    const STEPS: u32 = 24;
    let sink = Sink::new(target)?;
    let left = CGMouseButton::Left;
    sink.mouse(CGEventType::MouseMoved, from, left, 0)?;
    sink.mouse(CGEventType::LeftMouseDown, from, left, 1)?;
    thread::sleep(EVENT_GAP * 4);
    for step in 1..=STEPS {
        let point = from.lerp(to, f64::from(step) / f64::from(STEPS));
        sink.mouse(CGEventType::LeftMouseDragged, point, left, 1)?;
        thread::sleep(EVENT_GAP);
    }
    thread::sleep(EVENT_GAP * 4);
    sink.mouse(CGEventType::LeftMouseUp, to, left, 1)
}

pub(super) fn scroll(target: InputTarget<'_>, point: Point, dx: f64, dy: f64) -> Result<()> {
    let sink = Sink::new(target)?;
    sink.mouse(CGEventType::MouseMoved, point, CGMouseButton::Left, 0)?;
    let steps = (dx.abs().max(dy.abs()) / SCROLL_STEP).ceil().max(1.0);
    let (step_x, step_y) = (wheel(-dx / steps), wheel(-dy / steps));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // bounded by the scroll distance
    for _ in 0..steps as u32 {
        let event =
            CGEvent::new_scroll_wheel_event2(Some(&sink.source), CGScrollEventUnit::Pixel, 2, step_y, step_x, 0)
                .ok_or_else(|| Error::platform("could not create a scroll event"))?;
        CGEvent::set_location(Some(&event), CGPoint::new(point.x, point.y));
        sink.post(&event);
        thread::sleep(EVENT_GAP);
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation)]
fn wheel(value: f64) -> i32 {
    value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

pub(super) fn press_key(target: InputTarget<'_>, chord: &KeyChord) -> Result<()> {
    let mapped = keys::map(chord.key).ok_or_else(|| Error::invalid(format!("no macOS key code for {chord}")))?;
    let modifiers = chord.modifiers | mapped.implied;
    let sink = Sink::new(target)?;
    let mut held = CGEventFlags::empty();
    let pressed: Vec<_> = keys::MODIFIER_KEYS.iter().filter(|(m, _, _)| modifiers.contains(*m)).collect();
    for (_, code, flag) in &pressed {
        held |= *flag;
        sink.send_key(*code, true, held)?;
    }
    let all = keys::flags(modifiers);
    sink.send_key(mapped.code, true, all)?;
    thread::sleep(EVENT_GAP);
    sink.send_key(mapped.code, false, all)?;
    for (_, code, flag) in pressed.iter().rev() {
        held -= *flag;
        sink.send_key(*code, false, held)?;
    }
    thread::sleep(EVENT_GAP);
    Ok(())
}

pub(super) fn type_text(target: InputTarget<'_>, text: &str) -> Result<()> {
    let sink = Sink::new(target)?;
    let mut pending: Vec<u16> = Vec::new();
    for c in text.chars() {
        let control = match c {
            '\n' | '\r' => Some(keys::RETURN),
            '\t' => Some(keys::TAB),
            _ => None,
        };
        if let Some(code) = control {
            flush_unicode(&sink, &mut pending)?;
            sink.send_key(code, true, CGEventFlags::empty())?;
            sink.send_key(code, false, CGEventFlags::empty())?;
            thread::sleep(EVENT_GAP);
            continue;
        }
        let mut buf = [0u16; 2];
        if pending.len() + c.len_utf16() > UNICODE_CHUNK {
            flush_unicode(&sink, &mut pending)?;
        }
        pending.extend_from_slice(c.encode_utf16(&mut buf));
    }
    flush_unicode(&sink, &mut pending)
}

fn flush_unicode(sink: &Sink, pending: &mut Vec<u16>) -> Result<()> {
    if pending.is_empty() {
        return Ok(());
    }
    for down in [true, false] {
        let event = sink.key(0, down, CGEventFlags::empty())?;
        // SAFETY: `pending` holds `pending.len()` valid UTF-16 units for the duration of the call.
        unsafe { CGEvent::keyboard_set_unicode_string(Some(&event), pending.len() as _, pending.as_ptr()) };
        sink.post(&event);
    }
    pending.clear();
    thread::sleep(EVENT_GAP);
    Ok(())
}
