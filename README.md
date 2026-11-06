# computer-use-rs

Cross-platform computer-use primitives for Rust (imported as `computer_use`). Read any desktop app's accessibility tree,
screenshot its window, and drive it with accessibility actions and synthetic input.

It is a general-purpose library: use it for UI automation and scripting, end-to-end tests of
desktop apps, accessibility tooling, or AI agents that operate a computer. You get a typed Rust
API with no runtime, server or protocol attached.

```rust,no_run
use computer_use::{CaptureOptions, ClickOptions, ClickTarget, Computer, ElementQuery};

fn main() -> computer_use::Result<()> {
    let mut computer = Computer::new()?;

    // Observe: an indexed element tree, a PNG screenshot, and a plain-text outline.
    let state = computer.get_app_state("TextEdit", &CaptureOptions::default())?;
    println!("{state}");

    // Act by element index…
    let text_area = state.find(&ElementQuery::role("text area")).next().map(|e| e.index).unwrap();
    computer.click("TextEdit", ClickTarget::Element(text_area), ClickOptions::default())?;
    computer.type_text("TextEdit", "Hello from Rust!")?;

    // …or by screenshot pixel.
    computer.click("TextEdit", ClickTarget::Pixel { x: 120.0, y: 80.0 }, ClickOptions::default())?;
    computer.press_key("TextEdit", "Return")?;
    Ok(())
}
```

## What's included

`Computer` is the main entry point:

| Method                     | Does                                                                                                                                             |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `list_apps`                | Running apps (frontmost first), then installed apps                                                                                              |
| `list_windows`             | The app's windows, to pick one with `CaptureOptions::window` (by ID or title)                                                                    |
| `get_app_state`            | Captures the app's window: element tree, focus, selection, screenshot, and optionally the menu bar. Launches the app in the background if needed |
| `query`                    | Finds elements by text and/or role without taking a screenshot                                                                                   |
| `click`                    | Clicks an element (by accessibility action when that's equivalent), a screenshot pixel, or a screen point. Left/right/middle, multi-click        |
| `perform_secondary_action` | Invokes a named action such as `Show Menu`, `Raise` or `Increment`                                                                               |
| `scroll` / `scroll_at`     | Scrolls an element, a screenshot pixel or a screen point by pages (fractions allowed)                                                            |
| `drag` / `drag_screen`     | Drags between two screenshot pixels, or two screen points                                                                                        |
| `type_text`                | Types literal text, inserting through accessibility when possible                                                                                |
| `press_key`                | Presses xdotool-style chords such as `super+c`, `ctrl+shift+Tab` or `KP_0`                                                                       |
| `set_value`                | Writes the value of a settable element                                                                                                           |
| `select_text`              | Selects text inside an element, or puts the caret before or after it                                                                             |

`Computer<NativeBackend>` is `Send`, so it can live behind a mutex and be used from
`spawn_blocking` or any worker thread.

The building blocks are public as well. `AppState` and `Element` are plain data.
`keys::KeyChord` is an xdotool key parser. `text::TextSelection` resolves text ranges.
`Screenshot` maps screenshot pixels to screen points. The `Backend` trait is the platform
layer, so you can drive a fake desktop in tests or add a platform.

## Platforms

| Platform    | Tree & actions    | Screenshot                                   | Input                                                                                         |
| ----------- | ----------------- | -------------------------------------------- | --------------------------------------------------------------------------------------------- |
| macOS 14+   | Accessibility API | Window-server capture, then ScreenCaptureKit | `CGEvent` posted to the app's process (optionally with SkyLight synthetic focus), or globally |
| Windows 10+ | UI Automation     | `PrintWindow` / GDI                          | Window messages to the app (background) or `SendInput`                                        |
| Linux (X11) | AT-SPI            | X11 `GetImage`                               | AT-SPI device event controller                                                                |

By default, synthetic input uses `Delivery::Background`. Events go to the target app without
moving your pointer or changing focus, where the OS allows it. On macOS, Command-key shortcuts
still work in the background: they are resolved to the app's menu item. `Delivery::FocusedBackground`
(or `ClickMethod::FocusedBackground`) also gives the target window synthetic focus inside its
own process, which Chromium/Electron apps need for background clicks and typing. It uses
private SkyLight calls (see `src/platform/macos/skylight.rs`). Use
`Delivery::Global` for apps that only accept real system input; on macOS it brings the app to
the front first.

### Permissions

- **macOS:** the process running your binary (often your terminal) needs **Accessibility**
  and **Screen Recording** in System Settings → Privacy & Security. Without them, apps can
  appear to have no windows. `Computer::permissions()` reports the current state, and
  `Computer::request_permissions()` shows the system prompts.
- **Linux:** the session's accessibility bus must be running. It is by default on GNOME and
  KDE. On Wayland you get trees and actions but no screenshots, and input only reaches
  X11/XWayland apps.
- **Windows:** no setup. The process opts into per-monitor DPI awareness, so coordinates are
  physical pixels.

## Examples

```sh
cargo run --example list_apps
cargo run --example inspect -- TextEdit /tmp/textedit.png
cargo run --example textedit   # macOS: create a document, type, select, delete
```

## Development

```sh
cargo clippy --all-targets -- -D warnings
cargo test
# Check the other platforms from any host:
cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings
cargo clippy --target x86_64-unknown-linux-gnu --all-targets -- -D warnings
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or
[MIT license](LICENSE-MIT), at your option.
