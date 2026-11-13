//! Regression tests against a private recording bus; never connect to the real desktop.

#![allow(clippy::unwrap_used)]

use std::io::{BufRead as _, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use atspi::proxy::device_event_controller::KeySynthType;
use zbus::blocking::connection::Builder;

use super::session::SessionEnv;
use super::{Backend, Connection, Error, InputTarget, LinuxBackend, MouseButton, Point};
use crate::{AppInfo, Delivery};

#[derive(Clone, Debug, Default)]
struct Recorder(Arc<Mutex<Vec<String>>>);

#[zbus::interface(name = "org.a11y.atspi.DeviceEventController")]
impl Recorder {
    fn generate_keyboard_event(&self, code: i32, text: &str, kind: KeySynthType) {
        self.0.lock().unwrap().push(format!("key {code} {text:?} {kind:?}"));
    }

    fn generate_mouse_event(&self, x: i32, y: i32, event: &str) {
        self.0.lock().unwrap().push(format!("mouse {x},{y} {event}"));
    }
}

struct PrivateBus(Child);

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn recording_backend() -> (PrivateBus, Connection, LinuxBackend, Recorder) {
    let mut bus = PrivateBus(
        Command::new("dbus-daemon")
            .args(["--session", "--nofork", "--nopidfile", "--address=unix:tmpdir=/tmp", "--print-address=1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .expect("these Linux regression tests require dbus-daemon (package dbus)"),
    );
    let mut address = String::new();
    BufReader::new(bus.0.stdout.take().unwrap()).read_line(&mut address).unwrap();
    let address = address.trim();
    assert!(!address.is_empty(), "private dbus-daemon did not start (check local socket permissions)");
    let recorder = Recorder::default();
    let service = Builder::address(address)
        .unwrap()
        .name("org.a11y.atspi.Registry")
        .unwrap()
        .serve_at("/org/a11y/atspi/registry/deviceeventcontroller", recorder.clone())
        .unwrap()
        .build()
        .unwrap();
    let backend = LinuxBackend {
        conn: Builder::address(address).unwrap().build().unwrap(),
        // Do not discover the host session, enable accessibility, or open its X server.
        session: Arc::new(SessionEnv::default()),
    };
    (bus, service, backend, recorder)
}

#[test]
fn background_input_never_emits_desktop_events() {
    let (_bus, _service, backend, recorder) = recording_backend();
    let app = AppInfo::new("unfocused-target-app").with_pid(42);
    let chord = "Return".parse().unwrap();
    let mut unexpected = Vec::new();
    for delivery in [Delivery::Background, Delivery::FocusedBackground] {
        let target = || InputTarget { app: &app, window: None, delivery };
        let results = [
            ("click", backend.click(target(), Point::new(10.0, 20.0), MouseButton::Left, 1)),
            ("scroll", backend.scroll(target(), Point::new(10.0, 20.0), 0.0, 64.0)),
            ("drag", backend.drag(target(), Point::new(10.0, 20.0), Point::new(30.0, 40.0))),
            ("type_text", backend.type_text(target(), "CU_REPRO_MARKER\n")),
            ("press_key", backend.press_key(target(), &chord)),
        ];
        for (name, result) in results {
            if !matches!(result, Err(Error::Unsupported(_))) {
                unexpected.push(format!("{delivery:?} {name}: {result:?}"));
            }
        }
    }
    let events = recorder.0.lock().unwrap().clone();
    assert!(
        unexpected.is_empty() && events.is_empty(),
        "Background input must fail before emitting events. Unexpected results: {unexpected:#?}; recorded events: {events:#?}"
    );
}

#[test]
fn explicit_global_input_still_reaches_the_recording_controller() {
    let (_bus, _service, backend, recorder) = recording_backend();
    let app = AppInfo::new("target-app").with_pid(42);
    let target = InputTarget { app: &app, window: None, delivery: Delivery::Global };
    backend.type_text(target, "CU_GLOBAL_MARKER").unwrap();
    assert_eq!(*recorder.0.lock().unwrap(), ["key 0 \"CU_GLOBAL_MARKER\" String"]);
}
