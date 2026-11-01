use std::thread::sleep;
use std::time::Duration;

use atspi::proxy::device_event_controller::{DeviceEventControllerProxyBlocking, KeySynthType};
use zbus::blocking::Connection;

use super::a11y::{REGISTRY, dbus, proxy_at};
use super::keys::{self, KeyStroke};
use crate::backend::MouseButton;
use crate::error::{Error, Result};
use crate::geometry::Point;
use crate::keys::KeyChord;

const DEC_PATH: &str = "/org/a11y/atspi/registry/deviceeventcontroller";

const POINTS_PER_WHEEL_NOTCH: f64 = 64.0;

const MAX_WHEEL_NOTCHES: u32 = 200;

fn controller(conn: &Connection) -> Result<DeviceEventControllerProxyBlocking<'static>> {
    proxy_at(conn, REGISTRY, DEC_PATH)
}

#[allow(clippy::cast_possible_truncation)] // Clamped to the i32 range first.
fn pixel(value: f64) -> i32 {
    value.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
}

fn mouse(dec: &DeviceEventControllerProxyBlocking<'_>, point: Point, event: &str) -> Result<()> {
    dec.generate_mouse_event(pixel(point.x), pixel(point.y), event).map_err(dbus)
}

const fn button_number(button: MouseButton) -> u8 {
    match button {
        MouseButton::Left => 1,
        MouseButton::Middle => 2,
        MouseButton::Right => 3,
    }
}

pub(crate) fn click(conn: &Connection, point: Point, button: MouseButton, count: u32) -> Result<()> {
    let dec = controller(conn)?;
    let n = button_number(button);
    mouse(&dec, point, "abs")?;
    for _ in 0..count.max(1) {
        mouse(&dec, point, &format!("b{n}p"))?;
        sleep(Duration::from_millis(35));
        mouse(&dec, point, &format!("b{n}r"))?;
        sleep(Duration::from_millis(50));
    }
    Ok(())
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Non-negative and clamped.
fn notches(distance: f64) -> u32 {
    if !distance.is_finite() || distance == 0.0 {
        return 0;
    }
    (distance.abs() / POINTS_PER_WHEEL_NOTCH).ceil().clamp(1.0, f64::from(MAX_WHEEL_NOTCHES)) as u32
}

pub(crate) fn scroll(conn: &Connection, point: Point, dx: f64, dy: f64) -> Result<()> {
    let dec = controller(conn)?;
    mouse(&dec, point, "abs")?;
    let vertical = if dy < 0.0 { "b4c" } else { "b5c" };
    let horizontal = if dx < 0.0 { "b6c" } else { "b7c" };
    for (event, count) in [(vertical, notches(dy)), (horizontal, notches(dx))] {
        for _ in 0..count {
            mouse(&dec, point, event)?;
            sleep(Duration::from_millis(10));
        }
    }
    Ok(())
}

pub(crate) fn drag(conn: &Connection, from: Point, to: Point) -> Result<()> {
    const STEPS: u32 = 12;
    let dec = controller(conn)?;
    mouse(&dec, from, "abs")?;
    mouse(&dec, from, "b1p")?;
    for step in 1..=STEPS {
        sleep(Duration::from_millis(20));
        mouse(&dec, from.lerp(to, f64::from(step) / f64::from(STEPS)), "abs")?;
    }
    mouse(&dec, to, "b1r")
}

fn keyboard(dec: &DeviceEventControllerProxyBlocking<'_>, code: i32, text: &str, kind: KeySynthType) -> Result<()> {
    dec.generate_keyboard_event(code, text, kind).map_err(dbus)
}

fn keysym_code(sym: u32) -> Result<i32> {
    i32::try_from(sym).map_err(|_| Error::Unsupported(format!("keysym {sym:#x} is out of range")))
}

pub(crate) fn type_text(conn: &Connection, text: &str) -> Result<()> {
    let dec = controller(conn)?;
    let mut run = String::new();
    let flush = |run: &mut String| -> Result<()> {
        if !run.is_empty() {
            keyboard(&dec, 0, run, KeySynthType::String)?;
            run.clear();
        }
        Ok(())
    };
    for c in text.chars() {
        let sym = match c {
            '\n' => keys::XK_RETURN,
            '\t' => keys::XK_TAB,
            '\r' => continue,
            c => {
                run.push(c);
                continue;
            }
        };
        flush(&mut run)?;
        keyboard(&dec, keysym_code(sym)?, "", KeySynthType::Sym)?;
    }
    flush(&mut run)
}

pub(crate) fn press_key(conn: &Connection, chord: &KeyChord) -> Result<()> {
    let (mask, stroke) = keys::resolve(chord)?;
    let dec = controller(conn)?;
    if mask != 0 {
        keyboard(&dec, mask, "", KeySynthType::Lockmodifiers)?;
    }
    let sent = match &stroke {
        KeyStroke::Text(text) => keyboard(&dec, 0, text, KeySynthType::String),
        KeyStroke::Sym(sym) => keysym_code(*sym).and_then(|code| keyboard(&dec, code, "", KeySynthType::Sym)),
    };
    if mask != 0 {
        keyboard(&dec, mask, "", KeySynthType::Unlockmodifiers)?;
    }
    sent
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_wheel_notches() {
        assert_eq!(notches(0.0), 0);
        assert_eq!(notches(1.0), 1);
        assert_eq!(notches(-130.0), 3);
        assert_eq!(notches(f64::NAN), 0);
        assert_eq!(notches(1e12), MAX_WHEEL_NOTCHES);
    }

    #[test]
    fn rounds_pixels() {
        assert_eq!(pixel(10.5), 11);
        assert_eq!(pixel(-3.2), -3);
        assert_eq!(pixel(1e20), i32::MAX);
    }
}
