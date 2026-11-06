//! Cross-platform computer-use primitives: observe desktop apps through their accessibility
//! trees and screenshots, and drive them with accessibility actions and synthetic input.
//!
//! Use it for UI automation, end-to-end testing of desktop apps, accessibility tooling, or
//! AI agents. It provides:
//!
//! - [`Computer`]: a session for observing and acting on apps:
//!   [`list_apps`](Computer::list_apps), [`list_windows`](Computer::list_windows),
//!   [`get_app_state`](Computer::get_app_state), [`click`](Computer::click),
//!   [`perform_secondary_action`](Computer::perform_secondary_action), [`scroll`](Computer::scroll),
//!   [`scroll_at`](Computer::scroll_at), [`drag`](Computer::drag),
//!   [`drag_screen`](Computer::drag_screen), [`type_text`](Computer::type_text),
//!   [`press_key`](Computer::press_key), [`set_value`](Computer::set_value),
//!   [`select_text`](Computer::select_text) and [`query`](Computer::query).
//!   With the native backend it is `Send`, so it can be shared across threads behind a mutex.
//! - [`AppState`]: a captured window with an indexed element tree, a PNG [`Screenshot`], and a
//!   compact [text rendering](AppState::render) for logging, diffing or prompts.
//! - [`Backend`]: the OS abstraction. [`NativeBackend`] is the implementation for the current
//!   platform; implement the trait yourself for tests or other platforms.
//!
//! # Platform support
//!
//! | Platform | Tree | Screenshot | Input |
//! |----------|------|------------|-------|
//! | macOS 14+ | Accessibility (`AXUIElement`) | window-server capture, then `ScreenCaptureKit` | `CGEvent` posted to the app's process (optionally with `SkyLight` synthetic focus), or globally |
//! | Windows 10+ | UI Automation | `PrintWindow` / GDI | window messages to the app, or `SendInput` |
//! | Linux (X11) | AT-SPI over D-Bus | X11 `GetImage` | AT-SPI device event controller (global only) |
//!
//! On macOS the host process needs the **Accessibility** and **Screen Recording**
//! permissions; see [`Computer::permissions`] and [`Computer::request_permissions`].
//! Without them, apps may appear to have no windows. On Linux, accessibility must be enabled
//! for the session (it is by default on GNOME and KDE); Wayland sessions get trees and
//! actions but no screenshots, and synthetic input only reaches X11/`XWayland` apps.
//!
//! # Example
//!
//! ```no_run
//! # #[cfg(target_os = "macos")]
//! # fn main() -> computer_use::Result<()> {
//! use computer_use::{CaptureOptions, ClickOptions, ClickTarget, Computer, ElementQuery};
//!
//! let mut computer = Computer::new()?;
//! let state = computer.get_app_state("TextEdit", &CaptureOptions::default())?;
//! println!("{state}");
//!
//! let text_area = state
//!     .find(&ElementQuery::role("text area"))
//!     .next()
//!     .map(|e| e.index)
//!     .expect("TextEdit has a text area");
//! computer.click("TextEdit", ClickTarget::Element(text_area), ClickOptions::default())?;
//! computer.type_text("TextEdit", "Hello from Rust!")?;
//! # Ok(())
//! # }
//! # #[cfg(not(target_os = "macos"))]
//! # fn main() {}
//! ```

#![cfg_attr(docsrs, feature(doc_cfg))]

mod app;
mod backend;
mod computer;
mod element;
mod error;
mod geometry;
pub mod keys;
mod platform;
mod screenshot;
mod state;
pub mod text;

pub use app::AppInfo;
pub use backend::{Backend, Capture, CaptureRequest, Delivery, InputTarget, MouseButton, Permissions, ScrollDirection};
pub use computer::{ClickMethod, ClickOptions, ClickOutcome, ClickTarget, Computer};
pub use element::{Action, Element, ElementData, ElementStates, RawNode};
pub use error::{Error, Permission, Result};
pub use geometry::{Point, Rect, Size};
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
pub use platform::NativeBackend;
#[cfg(target_os = "linux")]
pub use platform::linux::{LinuxBackend, LinuxElement};
#[cfg(target_os = "macos")]
pub use platform::macos::{MacBackend, MacElement};
#[cfg(target_os = "windows")]
pub use platform::windows::{WindowsBackend, WindowsElement};
pub use screenshot::{RawImage, Screenshot};
pub use state::{AppState, CaptureOptions, ElementQuery, WindowInfo, WindowSelector};
pub use text::{SelectionMode, TextSelection};
