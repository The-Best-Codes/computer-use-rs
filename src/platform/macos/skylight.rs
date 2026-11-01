//! Runtime bridge to private `SkyLight` window-server functions.
//!
//! These give a background app a *synthetic* active/key state inside its own process, so it
//! accepts clicks, typing, and menu key equivalents without the real frontmost app changing,
//! the window being raised, or the pointer moving. They also capture a window's backing store
//! directly, which works for covered windows and ones on other Spaces.
//!
//! All undocumented ABI lives in this file; every symbol is resolved with `dlsym` and
//! each feature degrades gracefully when one is missing.

use std::ffi::{CStr, c_void};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::Duration;

use objc2_application_services::AXUIElement;
use objc2_core_foundation::{CFArray, CFRetained, CGPoint};
use objc2_core_graphics::{CGEvent, CGEventSource, CGEventSourceStateID, CGEventType, CGImage, CGMouseButton};

use crate::error::{Error, Result};
use crate::geometry::Point;

type PostToPid = unsafe extern "C" fn(i32, *const c_void);
type SetIntegerField = unsafe extern "C" fn(*const c_void, u32, i64);
type SetWindowLocation = unsafe extern "C" fn(*const c_void, f64, f64);
type PostEventRecord = unsafe extern "C" fn(*const u8, *const u8) -> i32;
type ProcessForPid = unsafe extern "C" fn(i32, *mut u8) -> i32;
type MainConnection = unsafe extern "C" fn() -> u32;
type HardwareCapture = unsafe extern "C" fn(u32, *const u32, i32, u32) -> *const CFArray;
type ElementWindow = unsafe extern "C" fn(*const AXUIElement, *mut u32) -> i32;

const SKYLIGHT: &CStr = c"/System/Library/PrivateFrameworks/SkyLight.framework/SkyLight";
const APP_SERVICES: &CStr = c"/System/Library/Frameworks/ApplicationServices.framework/ApplicationServices";

const HW_CAPTURE_OPTIONS: u32 = 0x800;
const FOCUS_SETTLE: Duration = Duration::from_millis(10);
const CLICK_DELAY_SCALE: f64 = 0.2;

mod field {
    pub(super) const GESTURE_PHASE: u32 = 0;
    pub(super) const CLICK_STATE: u32 = 1;
    pub(super) const BUTTON_NUMBER: u32 = 3;
    pub(super) const SUBTYPE: u32 = 7;
    pub(super) const TARGET_PID: u32 = 40;
    pub(super) const WINDOW_NUMBER: u32 = 51;
    pub(super) const CLICK_GROUP: u32 = 58;
    pub(super) const WINDOW_UNDER_POINTER: u32 = 91;
    pub(super) const HANDLING_WINDOW_UNDER_POINTER: u32 = 92;
}

pub(super) struct SkyLight {
    post_to_pid: Option<PostToPid>,
    set_integer_field: Option<SetIntegerField>,
    set_window_location: Option<SetWindowLocation>,
    post_event_record: Option<PostEventRecord>,
    process_for_pid: Option<ProcessForPid>,
    main_connection: Option<MainConnection>,
    hardware_capture: Option<HardwareCapture>,
    element_window: Option<ElementWindow>,
    lock: Mutex<()>,
}

pub(super) fn get() -> &'static SkyLight {
    static INSTANCE: OnceLock<SkyLight> = OnceLock::new();
    INSTANCE.get_or_init(SkyLight::load)
}

fn open(path: &CStr) -> *mut c_void {
    // SAFETY: `path` is a valid C string; system frameworks are never unloaded.
    unsafe { libc::dlopen(path.as_ptr(), libc::RTLD_LAZY | libc::RTLD_GLOBAL) }
}

/// # Safety
///
/// `F` must be an `extern "C"` function pointer type matching the symbol's real ABI.
unsafe fn resolve<F: Copy>(handle: *mut c_void, symbol: &CStr) -> Option<F> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: `handle` came from dlopen and `symbol` is a valid C string.
    let pointer = unsafe { libc::dlsym(handle, symbol.as_ptr()) };
    if pointer.is_null() {
        return None;
    }
    debug_assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    // SAFETY: the caller guarantees `F` is the symbol's function-pointer type.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&pointer) })
}

impl SkyLight {
    fn load() -> Self {
        let sky = open(SKYLIGHT);
        let services = open(APP_SERVICES);
        Self {
            // SAFETY: `post_to_pid` matches the ABI of `SLEventPostToPid`.
            post_to_pid: unsafe { resolve(sky, c"SLEventPostToPid") },
            // SAFETY: `set_integer_field` matches the ABI of `SLEventSetIntegerValueField`.
            set_integer_field: unsafe { resolve(sky, c"SLEventSetIntegerValueField") },
            // SAFETY: `set_window_location` matches the ABI of `CGEventSetWindowLocation`.
            set_window_location: unsafe { resolve(sky, c"CGEventSetWindowLocation") },
            // SAFETY: `post_event_record` matches the ABI of `SLPSPostEventRecordTo`.
            post_event_record: unsafe { resolve(sky, c"SLPSPostEventRecordTo") },
            // SAFETY: `process_for_pid` matches the ABI of `GetProcessForPID`.
            process_for_pid: unsafe { resolve(services, c"GetProcessForPID") },
            // SAFETY: `main_connection` matches the ABI of `SLSMainConnectionID`.
            main_connection: unsafe { resolve(sky, c"SLSMainConnectionID") },
            // SAFETY: `hardware_capture` matches the ABI of `SLSHWCaptureWindowList`.
            hardware_capture: unsafe { resolve(sky, c"SLSHWCaptureWindowList") },
            // SAFETY: `element_window` matches the ABI of `_AXUIElementGetWindow`.
            element_window: unsafe { resolve(services, c"_AXUIElementGetWindow") },
            lock: Mutex::new(()),
        }
    }

    fn focus_available(&self) -> bool {
        self.post_event_record.is_some() && self.process_for_pid.is_some()
    }

    fn click_available(&self) -> bool {
        self.focus_available()
            && self.post_to_pid.is_some()
            && self.set_integer_field.is_some()
            && self.set_window_location.is_some()
    }

    pub(super) fn window_id(&self, element: &AXUIElement) -> Option<u32> {
        let function = self.element_window?;
        let mut id = 0u32;
        // SAFETY: `element` is a live AXUIElement and `id` a valid out-pointer.
        let status = unsafe { function(element, &raw mut id) };
        (status == 0 && id != 0).then_some(id)
    }

    pub(super) fn capture(&self, window_id: u32) -> Option<CFRetained<CGImage>> {
        let (main, capture) = (self.main_connection?, self.hardware_capture?);
        // SAFETY: SLSMainConnectionID takes no arguments.
        let connection = unsafe { main() };
        // SAFETY: `window_id` is a single valid u32 for the duration of the call.
        let array = unsafe { capture(connection, &raw const window_id, 1, HW_CAPTURE_OPTIONS) };
        let array = std::ptr::NonNull::new(array.cast_mut())?;
        // SAFETY: the function returns a +1 CFArray of CGImages (Create rule).
        let array: CFRetained<CFArray<CGImage>> = unsafe { CFRetained::from_raw(array.cast()) };
        array.get(0).filter(|image| CGImage::width(Some(image)) > 0 && CGImage::height(Some(image)) > 0)
    }

    pub(super) fn focus(&self, pid: i32, window_id: u32) -> Result<Option<FocusGuard<'_>>> {
        let (Some(process_for_pid), true) = (self.process_for_pid, self.focus_available()) else {
            return Ok(None);
        };
        let lock = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let mut psn = [0u8; 8];
        // SAFETY: `psn` is an 8-byte ProcessSerialNumber out-buffer.
        let status = unsafe { process_for_pid(pid, psn.as_mut_ptr()) };
        if status != 0 {
            return Err(Error::platform(format!("could not resolve pid {pid} to a process (OSStatus {status})")));
        }
        let guard = FocusGuard { sky: self, psn, window_id, _lock: lock };
        guard.post(&activation_record(window_id, true))?;
        thread::sleep(FOCUS_SETTLE);
        for record in key_window_records(window_id) {
            guard.post(&record)?;
        }
        thread::sleep(FOCUS_SETTLE);
        Ok(Some(guard))
    }

    pub(super) fn click(&self, pid: i32, window_id: u32, screen: Point, window: Point, count: u32) -> Result<bool> {
        if !self.click_available() || !(1..=2).contains(&count) {
            return Ok(false);
        }
        let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
            .ok_or_else(|| Error::platform("could not create a CGEventSource"))?;
        let group = i64::from(
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos()),
        );
        let primer = Point::new(-1.0, -1.0);
        let mut steps = vec![
            (CGEventType::MouseMoved, screen, window, 0, 2, 15.0),
            (CGEventType::LeftMouseDown, primer, primer, 1, 1, 1.0),
            (CGEventType::LeftMouseUp, primer, primer, 1, 2, 100.0),
        ];
        for n in 1..=i64::from(count) {
            steps.push((CGEventType::LeftMouseDown, screen, window, n, 3, 1.0));
            steps.push((CGEventType::LeftMouseUp, screen, window, n, 3, if n < i64::from(count) { 80.0 } else { 0.0 }));
        }

        let _focus = self.focus(pid, window_id)?;
        for (kind, at, local, click_state, phase, delay_ms) in steps {
            let event = CGEvent::new_mouse_event(Some(&source), kind, CGPoint::new(at.x, at.y), CGMouseButton::Left)
                .ok_or_else(|| Error::platform("could not create a mouse event"))?;
            self.stamp(&event, pid, window_id, local, click_state, phase, group);
            let raw: *const c_void = std::ptr::from_ref::<CGEvent>(&event).cast();
            if let Some(post) = self.post_to_pid {
                // SAFETY: `raw` points to a live CGEvent for the duration of the call.
                unsafe { post(pid, raw) };
            }
            // SkyLight reaches Chromium/Catalyst; the public path is still needed for AppKit.
            CGEvent::post_to_pid(pid, Some(&event));
            thread::sleep(Duration::from_secs_f64(delay_ms * CLICK_DELAY_SCALE / 1000.0));
        }
        // Let asynchronous renderers consume the final mouse-up before focus is released.
        thread::sleep(Duration::from_secs_f64(100.0 * CLICK_DELAY_SCALE / 1000.0));
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn stamp(&self, event: &CGEvent, pid: i32, window_id: u32, local: Point, click: i64, phase: i64, group: i64) {
        let (Some(set), Some(locate)) = (self.set_integer_field, self.set_window_location) else { return };
        let raw: *const c_void = std::ptr::from_ref(event).cast();
        let window = i64::from(window_id);
        for (key, value) in [
            (field::GESTURE_PHASE, phase),
            (field::CLICK_STATE, click),
            (field::BUTTON_NUMBER, 0),
            (field::SUBTYPE, 3),
            (field::TARGET_PID, i64::from(pid)),
            (field::WINDOW_NUMBER, window),
            (field::CLICK_GROUP, group),
            (field::WINDOW_UNDER_POINTER, window),
            (field::HANDLING_WINDOW_UNDER_POINTER, window),
        ] {
            // SAFETY: `raw` points to a live CGEvent.
            unsafe { set(raw, key, value) };
        }
        // SAFETY: `raw` points to a live CGEvent.
        unsafe { locate(raw, local.x, local.y) };
    }
}

pub(super) struct FocusGuard<'a> {
    sky: &'a SkyLight,
    psn: [u8; 8],
    window_id: u32,
    _lock: std::sync::MutexGuard<'a, ()>,
}

impl FocusGuard<'_> {
    fn post(&self, record: &[u8; RECORD_LEN]) -> Result<()> {
        let Some(post) = self.sky.post_event_record else { return Ok(()) };
        // SAFETY: `psn` is an 8-byte PSN and `record` a full 0xF8-byte event record.
        let status = unsafe { post(self.psn.as_ptr(), record.as_ptr()) };
        if status == 0 {
            Ok(())
        } else {
            Err(Error::platform(format!("synthetic focus record failed (OSStatus {status})")))
        }
    }
}

impl Drop for FocusGuard<'_> {
    fn drop(&mut self) {
        let _ = self.post(&activation_record(self.window_id, false));
        thread::sleep(FOCUS_SETTLE);
    }
}

const RECORD_LEN: usize = 0xF8;

fn write_window(record: &mut [u8; RECORD_LEN], window_id: u32) {
    record[0x3C..0x40].copy_from_slice(&window_id.to_le_bytes());
}

fn activation_record(window_id: u32, focused: bool) -> [u8; RECORD_LEN] {
    let mut record = [0u8; RECORD_LEN];
    record[0x04] = 0xF8;
    record[0x08] = 0x0D;
    write_window(&mut record, window_id);
    record[0x8A] = if focused { 0x01 } else { 0x02 };
    record
}

fn key_window_records(window_id: u32) -> [[u8; RECORD_LEN]; 2] {
    let mut record = [0u8; RECORD_LEN];
    record[0x04] = 0xF8;
    record[0x3A] = 0x10;
    write_window(&mut record, window_id);
    record[0x20..0x30].fill(0xFF);
    let mut first = record;
    first[0x08] = 0x01;
    record[0x08] = 0x02;
    [first, record]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_match_reference_layout() {
        let a = activation_record(0x0102_0304, true);
        assert_eq!((a[0x04], a[0x08], a[0x8A]), (0xF8, 0x0D, 0x01));
        assert_eq!(&a[0x3C..0x40], &[0x04, 0x03, 0x02, 0x01]);
        assert_eq!(activation_record(1, false)[0x8A], 0x02);
        let [k1, k2] = key_window_records(7);
        assert_eq!((k1[0x08], k2[0x08], k1[0x3A], k1[0x3C]), (0x01, 0x02, 0x10, 7));
        assert!(k1[0x20..0x30].iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn symbols_resolve_on_this_system() {
        let sky = get();
        assert!(sky.focus_available());
        assert!(sky.click_available());
    }
}
