#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
pub(crate) mod macos;
#[cfg(target_os = "windows")]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub use linux::LinuxBackend as NativeBackend;
#[cfg(target_os = "macos")]
pub use macos::MacBackend as NativeBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsBackend as NativeBackend;
