use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication};
use objc2_foundation::NSBundle;

use super::ax;
use crate::app::AppInfo;
use crate::error::{Error, Result};

const INSTALLED_TTL: Duration = Duration::from_secs(60);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Default)]
pub(super) struct InstalledCache(Mutex<Option<(Instant, Vec<AppInfo>)>>);

impl InstalledCache {
    pub(super) fn get(&self) -> Vec<AppInfo> {
        let mut guard = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((at, apps)) = guard.as_ref()
            && at.elapsed() < INSTALLED_TTL
        {
            return apps.clone();
        }
        let apps = installed_apps();
        *guard = Some((Instant::now(), apps.clone()));
        apps
    }
}

pub(super) fn running_apps() -> Vec<AppInfo> {
    let frontmost = ax::element(&ax::system_wide(), "AXFocusedApplication").and_then(|app| ax::pid(&app));
    all_pids()
        .into_iter()
        .filter_map(|pid| {
            let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular || app.isTerminated() {
                return None;
            }
            let path = app.bundleURL().and_then(|url| url.path()).map(|p| PathBuf::from(p.to_string()));
            let name = app.localizedName().map(|n| n.to_string()).or_else(|| path.as_deref().and_then(stem))?;
            let mut info = AppInfo::new(name).with_pid(u32::try_from(pid).ok()?).with_active(frontmost == Some(pid));
            info.bundle_id = app.bundleIdentifier().map(|b| b.to_string());
            info.path = path;
            Some(info)
        })
        .collect()
}

fn all_pids() -> Vec<i32> {
    // SAFETY: a null buffer asks for the number of processes.
    let count = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
    let Ok(capacity) = usize::try_from(count) else { return Vec::new() };
    // Leave headroom for processes started between the two calls.
    let mut pids = vec![0i32; capacity + 64];
    let bytes = i32::try_from(pids.len() * size_of::<i32>()).unwrap_or(i32::MAX);
    // SAFETY: `pids` is a writable buffer of exactly `bytes` bytes.
    let filled = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), bytes) };
    pids.truncate(usize::try_from(filled).unwrap_or(0));
    pids.retain(|&p| p > 0);
    pids
}

fn installed_apps() -> Vec<AppInfo> {
    let mut dirs = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(Path::new(&home).join("Applications"));
    }
    dirs.iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "app"))
        .filter_map(|path| {
            let name = stem(&path)?;
            let bundle_id = path
                .to_str()
                .and_then(|p| NSBundle::bundleWithPath(&objc2_foundation::NSString::from_str(p)))
                .and_then(|b| b.bundleIdentifier())
                .map(|id| id.to_string());
            let mut info = AppInfo::new(name).with_path(path);
            info.bundle_id = bundle_id;
            Some(info)
        })
        .collect()
}

fn stem(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().into_owned())
}

pub(super) fn launch(app: &AppInfo) -> Result<AppInfo> {
    let mut command = Command::new("/usr/bin/open");
    command.arg("-g");
    match (&app.bundle_id, &app.path) {
        (Some(id), _) => command.args(["-b", id]),
        (None, Some(path)) => command.arg("-a").arg(path),
        (None, None) => command.args(["-a", &app.name]),
    };
    let status = command.status()?;
    if !status.success() {
        return Err(Error::action(format!("could not launch {}", app.name)));
    }
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    let mut launched = None;
    while Instant::now() < deadline {
        launched = running_apps().into_iter().find(|running| {
            (app.bundle_id.is_some() && running.bundle_id == app.bundle_id)
                || (app.path.is_some() && running.path == app.path)
        });
        let has_window = launched
            .as_ref()
            .and_then(|found| found.pid)
            .and_then(|pid| i32::try_from(pid).ok())
            .is_some_and(|pid| !ax::elements(&ax::application(pid), "AXWindows").is_empty());
        if has_window {
            break;
        }
        thread::sleep(Duration::from_millis(200));
    }
    launched.ok_or_else(|| Error::action(format!("{} did not start within {LAUNCH_TIMEOUT:?}", app.name)))
}
