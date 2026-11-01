//! Lists running and installed apps.

#![allow(clippy::print_stdout)]

#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
fn main() -> computer_use::Result<()> {
    let computer = computer_use::Computer::new()?;
    for app in computer.list_apps()? {
        let status = match (app.is_running, app.is_active) {
            (true, true) => "active",
            (true, false) => "running",
            _ => "installed",
        };
        let pid = app.pid.map(|p| format!(" pid {p}")).unwrap_or_default();
        println!("{:<9} {} [{}]{pid}", status, app.name, app.reference());
    }
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn main() {}
