//! The native session must be movable to a worker thread (e.g. `spawn_blocking`).

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
#[test]
fn native_computer_is_send() {
    const fn assert_send<T: Send>() {}
    assert_send::<computer_use::NativeBackend>();
    assert_send::<computer_use::Computer<computer_use::NativeBackend>>();
}
