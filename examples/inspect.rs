//! Prints an app's accessibility tree and saves its screenshot.
//!
//! ```sh
//! cargo run --example inspect -- Finder
//! cargo run --example inspect -- com.apple.TextEdit /tmp/textedit.png
//! ```

#![allow(clippy::print_stdout)]

use std::process::ExitCode;

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn main() -> ExitCode {
    use computer_use::{CaptureOptions, Computer};

    let mut args = std::env::args().skip(1);
    let Some(app) = args.next() else {
        eprintln!("usage: inspect <app name | bundle id | pid:N> [screenshot.png]");
        return ExitCode::FAILURE;
    };
    let output = args.next();

    let run = || -> computer_use::Result<()> {
        let mut computer = Computer::new()?;
        let permissions = computer.permissions();
        if !permissions.all_granted() {
            eprintln!("warning: missing permissions: {permissions:?}");
        }
        let started = std::time::Instant::now();
        let state = computer.get_app_state(&app, &CaptureOptions::default())?;
        println!("{state}");
        eprintln!("\ncaptured {} elements in {:?}", state.elements.len(), started.elapsed());
        if let (Some(path), Some(shot)) = (output, &state.screenshot) {
            std::fs::write(&path, shot.png())?;
            eprintln!("saved {}x{} screenshot to {path}", shot.width(), shot.height());
        }
        Ok(())
    };
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn main() -> ExitCode {
    eprintln!("this platform has no native backend");
    ExitCode::FAILURE
}
