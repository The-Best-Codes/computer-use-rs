use std::ffi::c_void;
use std::ptr::{self, NonNull};

use objc2_application_services::{AXCopyMultipleAttributeOptions, AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{
    CFArray, CFBoolean, CFNumber, CFRange, CFRetained, CFString, CFType, CFURL, CGPoint, CGSize,
};

use crate::error::{Error, Result};
use crate::geometry::Rect;

pub(super) type AxElement = CFRetained<AXUIElement>;

const MESSAGING_TIMEOUT: f32 = 2.0;

pub(super) fn application(pid: i32) -> AxElement {
    // SAFETY: AXUIElementCreateApplication accepts any pid and returns a new element.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: setting a timeout on a valid element has no other preconditions.
    let _ = unsafe { app.set_messaging_timeout(MESSAGING_TIMEOUT) };
    app
}

pub(super) fn system_wide() -> AxElement {
    // SAFETY: AXUIElementCreateSystemWide has no preconditions.
    unsafe { AXUIElement::new_system_wide() }
}

pub(super) fn ax_error(context: &str, err: AXError) -> Error {
    match err {
        AXError::APIDisabled => Error::PermissionDenied(crate::error::Permission::Accessibility),
        AXError::InvalidUIElement => Error::action(format!("{context}: the element no longer exists")),
        AXError::AttributeUnsupported | AXError::ActionUnsupported => {
            Error::action(format!("{context}: not supported by this element"))
        }
        AXError::CannotComplete => Error::action(format!("{context}: the app did not respond")),
        other => Error::action(format!("{context}: AXError {}", other.0)),
    }
}

pub(super) fn attribute(element: &AXUIElement, name: &str) -> Option<CFRetained<CFType>> {
    let name = CFString::from_str(name);
    let mut value: *const CFType = ptr::null();
    // SAFETY: `value` is a valid out-pointer for the duration of the call.
    let err = unsafe { element.copy_attribute_value(&name, NonNull::from(&mut value)) };
    if err != AXError::Success {
        return None;
    }
    let value = NonNull::new(value.cast_mut())?;
    // SAFETY: AXUIElementCopyAttributeValue follows the Create rule; we own this +1 reference.
    Some(unsafe { CFRetained::from_raw(value) })
}

pub(super) fn attributes(element: &AXUIElement, names: &CFArray<CFString>) -> Vec<Option<CFRetained<CFType>>> {
    let mut values: *const CFArray = ptr::null();
    // SAFETY: `names` is a CFArray of CFStrings and `values` is a valid out-pointer.
    let err = unsafe {
        element.copy_multiple_attribute_values(
            names.as_opaque(),
            AXCopyMultipleAttributeOptions::empty(),
            NonNull::from(&mut values),
        )
    };
    let Some(values) = NonNull::new(values.cast_mut()).filter(|_| err == AXError::Success) else {
        return vec![None; names.len()];
    };
    // SAFETY: the array was returned under the Create rule and holds CFTypes.
    let values: CFRetained<CFArray<CFType>> = unsafe { CFRetained::from_raw(values.cast()) };
    (0..names.len()).map(|i| values.get(i).filter(|v| !is_ax_error_value(v))).collect()
}

fn is_ax_error_value(value: &CFType) -> bool {
    value
        .downcast_ref::<AXValue>()
        // SAFETY: AXValueGetType only reads the value's type tag.
        .is_some_and(|v| unsafe { v.r#type() } == AXValueType::AXError)
}

pub(super) fn attribute_names(names: &[&str]) -> CFRetained<CFArray<CFString>> {
    let strings: Vec<CFRetained<CFString>> = names.iter().map(|n| CFString::from_str(n)).collect();
    CFArray::from_retained_objects(&strings)
}

pub(super) fn string(element: &AXUIElement, name: &str) -> Option<String> {
    attribute(element, name).as_deref().and_then(as_string)
}

pub(super) fn boolean(element: &AXUIElement, name: &str) -> Option<bool> {
    attribute(element, name).as_deref().and_then(as_bool)
}

pub(super) fn element(element: &AXUIElement, name: &str) -> Option<AxElement> {
    attribute(element, name).and_then(|v| v.downcast::<AXUIElement>().ok())
}

pub(super) fn elements(element: &AXUIElement, name: &str) -> Vec<AxElement> {
    attribute(element, name).map_or_else(Vec::new, |v| as_elements(&v))
}

pub(super) fn as_elements(value: &CFType) -> Vec<AxElement> {
    let Some(array) = value.downcast_ref::<CFArray>() else {
        return Vec::new();
    };
    (0..array.len())
        .filter_map(|i| {
            // SAFETY: every CFArray element is a CFType.
            let typed: &CFArray<CFType> = unsafe { array.cast_unchecked() };
            // SAFETY: `i` is below the array's length; the type is checked by `downcast_ref`.
            let item: &CFType = unsafe { typed.get_unchecked(cf_index(i)) };
            item.downcast_ref::<AXUIElement>().map(CFRetained::from)
        })
        .collect()
}

pub(super) fn as_string(value: &CFType) -> Option<String> {
    if let Some(s) = value.downcast_ref::<CFString>() {
        return Some(s.to_string());
    }
    if let Some(url) = value.downcast_ref::<CFURL>() {
        return Some(url.string().to_string());
    }
    None
}

pub(super) fn as_bool(value: &CFType) -> Option<bool> {
    if let Some(b) = value.downcast_ref::<CFBoolean>() {
        return Some(b.as_bool());
    }
    value.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64).map(|n| n != 0)
}

pub(super) fn as_number(value: &CFType) -> Option<f64> {
    value.downcast_ref::<CFNumber>().and_then(CFNumber::as_f64)
}

pub(super) fn as_point(value: &CFType) -> Option<CGPoint> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut point = CGPoint::default();
    // SAFETY: `point` is a CGPoint, matching the requested AXValueType.
    let ok = unsafe { value.value(AXValueType::CGPoint, NonNull::from(&mut point).cast::<c_void>()) };
    ok.then_some(point)
}

pub(super) fn as_size(value: &CFType) -> Option<CGSize> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut size = CGSize::default();
    // SAFETY: `size` is a CGSize, matching the requested AXValueType.
    let ok = unsafe { value.value(AXValueType::CGSize, NonNull::from(&mut size).cast::<c_void>()) };
    ok.then_some(size)
}

pub(super) fn as_range(value: &CFType) -> Option<CFRange> {
    let value = value.downcast_ref::<AXValue>()?;
    let mut range = CFRange::new(0, 0);
    // SAFETY: `range` is a CFRange, matching the requested AXValueType.
    let ok = unsafe { value.value(AXValueType::CFRange, NonNull::from(&mut range).cast::<c_void>()) };
    ok.then_some(range)
}

pub(super) fn rect(position: Option<CGPoint>, size: Option<CGSize>) -> Option<Rect> {
    let (p, s) = (position?, size?);
    Some(Rect::new(p.x, p.y, s.width, s.height))
}

pub(super) fn frame(element: &AXUIElement) -> Option<Rect> {
    rect(
        attribute(element, "AXPosition").as_deref().and_then(as_point),
        attribute(element, "AXSize").as_deref().and_then(as_size),
    )
}

pub(super) fn action_names(element: &AXUIElement) -> Vec<String> {
    let mut names: *const CFArray = ptr::null();
    // SAFETY: `names` is a valid out-pointer.
    let err = unsafe { element.copy_action_names(NonNull::from(&mut names)) };
    let Some(names) = NonNull::new(names.cast_mut()).filter(|_| err == AXError::Success) else {
        return Vec::new();
    };
    // SAFETY: returned under the Create rule; an array of CFStrings.
    let names: CFRetained<CFArray<CFType>> = unsafe { CFRetained::from_raw(names.cast()) };
    names.iter().filter_map(|n| as_string(&n)).collect()
}

pub(super) fn is_settable(element: &AXUIElement, name: &str) -> bool {
    let name = CFString::from_str(name);
    let mut settable: u8 = 0;
    // SAFETY: `settable` is a valid Boolean out-pointer.
    let err = unsafe { element.is_attribute_settable(&name, NonNull::from(&mut settable)) };
    err == AXError::Success && settable != 0
}

pub(super) fn set(element: &AXUIElement, name: &str, value: &CFType) -> Result<()> {
    let attr = CFString::from_str(name);
    // SAFETY: `value` is a valid CFType of the kind the attribute expects (caller's contract).
    let err = unsafe { element.set_attribute_value(&attr, value) };
    if err == AXError::Success { Ok(()) } else { Err(ax_error(&format!("setting {name}"), err)) }
}

pub(super) fn set_bool(element: &AXUIElement, name: &str, value: bool) -> Result<()> {
    set(element, name, CFBoolean::new(value))
}

pub(super) fn set_range(element: &AXUIElement, name: &str, range: CFRange) -> Result<()> {
    // SAFETY: the pointer refers to a live CFRange, matching AXValueType::CFRange.
    let value = unsafe { AXValue::new(AXValueType::CFRange, NonNull::from(&range).cast::<c_void>()) }
        .ok_or_else(|| Error::platform("AXValueCreate failed"))?;
    set(element, name, &value)
}

pub(super) fn perform(element: &AXUIElement, action: &str) -> Result<()> {
    let name = CFString::from_str(action);
    // SAFETY: performing an action on a valid element has no other preconditions.
    let err = unsafe { element.perform_action(&name) };
    if err == AXError::Success { Ok(()) } else { Err(ax_error(action, err)) }
}

pub(super) fn pid(element: &AXUIElement) -> Option<i32> {
    let mut pid = 0;
    // SAFETY: `pid` is a valid out-pointer.
    let err = unsafe { element.pid(NonNull::from(&mut pid)) };
    (err == AXError::Success).then_some(pid)
}

#[allow(clippy::cast_possible_wrap)]
const fn cf_index(i: usize) -> isize {
    i as isize
}
