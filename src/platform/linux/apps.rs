use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use atspi::proxy::accessible::AccessibleProxyBlocking;
use zbus::blocking::Connection;
use zbus::blocking::fdo::DBusProxy;
use zbus::names::BusName;

use super::LinuxElement;
use super::a11y::{self, REGISTRY, ROOT_PATH, dbus, proxy};
use super::session::SessionEnv;
use crate::app::AppInfo;
use crate::error::{Error, Result};

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone)]
pub(crate) struct RunningApp {
    pub(crate) element: LinuxElement,
    pub(crate) name: String,
    pub(crate) pid: Option<u32>,
}

pub(crate) fn running_apps(conn: &Connection) -> Result<Vec<RunningApp>> {
    let desktop = LinuxElement::new(REGISTRY, ROOT_PATH);
    let dbus_proxy = DBusProxy::new(conn).map_err(dbus)?;
    let mut apps = Vec::new();
    for element in a11y::children(conn, &desktop)? {
        let Ok(name) = proxy::<AccessibleProxyBlocking<'_>>(conn, &element).and_then(|a| a.name().map_err(dbus)) else {
            continue;
        };
        if name.trim().is_empty() {
            continue;
        }
        let pid = BusName::try_from(element.bus_name())
            .ok()
            .and_then(|bus| dbus_proxy.get_connection_unix_process_id(bus).ok());
        apps.push(RunningApp { element, name, pid });
    }
    Ok(apps)
}

pub(crate) fn find_running(conn: &Connection, app: &AppInfo) -> Result<LinuxElement> {
    if app.needs_restart {
        return Err(Error::AppNotFound(format!(
            "{name} is running but does not expose its UI to accessibility, because it was started before \
             accessibility was enabled. Quit {name} completely and start it again",
            name = app.name
        )));
    }
    let running = running_apps(conn)?;
    running
        .iter()
        .find(|r| app.pid.is_some() && r.pid == app.pid)
        .or_else(|| running.iter().find(|r| r.name.eq_ignore_ascii_case(&app.name)))
        .map(|r| r.element.clone())
        .ok_or_else(|| {
            Error::AppNotFound(format!(
                "{} is not registered on the accessibility bus. Some apps only register if \
                 accessibility was enabled when they started: quit {} completely and start it again",
                app.name, app.name
            ))
        })
}

pub(crate) fn list_apps(conn: &Connection) -> Result<Vec<AppInfo>> {
    let entries = installed_apps();
    let processes = Processes::scan();
    let mut apps: Vec<AppInfo> = Vec::new();
    for running in running_apps(conn)? {
        let info = running_info(conn, &running, &entries, &processes);
        // Skip toolkit helpers (portals, input methods, frame drawers) that register on the bus but
        // have no desktop entry and no windows.
        if info.bundle_id.is_none() && a11y::children(conn, &running.element).map_or(true, |w| w.is_empty()) {
            continue;
        }
        apps.push(info);
    }
    apps.extend(unregistered_apps(&entries, &processes, &apps));
    let running_ids: HashSet<String> = apps.iter().filter_map(|a| a.bundle_id.clone()).collect();
    apps.extend(
        entries.into_iter().filter(|e| !e.hidden && !running_ids.contains(&e.id)).map(DesktopEntry::into_app_info),
    );
    Ok(apps)
}

fn running_info(conn: &Connection, running: &RunningApp, entries: &[DesktopEntry], processes: &Processes) -> AppInfo {
    let process = running.pid.and_then(|pid| processes.get(pid));
    let exe = process.map(|p| p.exe.clone()).or_else(|| running.pid.and_then(read_exe));
    let exe_name = exe.as_deref().and_then(Path::file_name).and_then(|n| n.to_str());
    // The process is a more reliable identity than the accessible name, which apps sharing a
    // browser engine often have in common.
    let entry = process
        .and_then(|p| entry_for_process(entries, processes, p))
        .or_else(|| match_entry(entries, &running.name, exe_name));
    let mut info = AppInfo::new(entry.map_or_else(|| running.name.clone(), |e| e.name.clone()));
    if let Some(pid) = running.pid {
        info = info.with_pid(pid);
    }
    info.is_running = true;
    info.is_active = a11y::has_active_window(conn, &running.element);
    if let Some(entry) = entry {
        info = info.with_bundle_id(entry.id.clone());
    }
    // The executable, so queries by binary name still match.
    if let Some(path) = exe.or_else(|| entry.map(|e| e.path.clone())) {
        info = info.with_path(path);
    }
    info
}

/// Running browser-engine apps that are not on the accessibility bus.
///
/// Browser-engine apps only register if accessibility was enabled when they started. They are
/// still reported, so callers can see they run and ask the user to restart them.
fn unregistered_apps(entries: &[DesktopEntry], processes: &Processes, registered: &[AppInfo]) -> Vec<AppInfo> {
    let registered_pids: HashSet<u32> = registered.iter().filter_map(|a| a.pid).collect();
    let mut seen: HashSet<String> = registered.iter().filter_map(|a| a.bundle_id.clone()).collect();
    let own_pid = std::process::id();
    let mut apps = Vec::new();
    for process in processes.top_level() {
        // Only browser engines decide at startup whether to register. Any other app missing from
        // the bus is a windowless background instance.
        if process.pid == own_pid
            || registered_pids.contains(&process.pid)
            || !processes.has_helper_children(process.pid)
        {
            continue;
        }
        let Some(entry) = entry_for_process(entries, processes, process) else { continue };
        if !seen.insert(entry.id.clone()) {
            continue;
        }
        apps.push(
            AppInfo::new(entry.name.clone())
                .with_pid(process.pid)
                .with_bundle_id(entry.id.clone())
                .with_path(process.exe.clone())
                .with_needs_restart(true),
        );
    }
    apps
}

/// Interpreters and launchers whose name says nothing about the app they run.
const GENERIC_PROGRAMS: [&str; 16] = [
    "sh",
    "bash",
    "env",
    "python",
    "python3",
    "java",
    "node",
    "electron",
    "flatpak",
    "gjs",
    "perl",
    "ruby",
    "wine",
    "snap",
    "bwrap",
    "dbus-launch",
];

/// Finds the desktop entry of a process: by executable, then by the `app-<id>` systemd scope the
/// desktop put it in when it launched it.
fn entry_for_process<'a>(
    entries: &'a [DesktopEntry],
    processes: &Processes,
    process: &Process,
) -> Option<&'a DesktopEntry> {
    let exe_name = process.exe.file_name()?.to_str()?;
    if exe_name.contains("crashpad") {
        return None;
    }
    let by_exe = entries.iter().find(|e| {
        e.program_name()
            .is_some_and(|p| p == exe_name && !GENERIC_PROGRAMS.contains(&p.as_str()) && !p.starts_with("python"))
    });
    by_exe
        .or_else(|| {
            // A launcher that resolves into the same install directory as the running binary.
            let dir = process.exe.parent().filter(|d| !d.ends_with("bin"))?;
            entries
                .iter()
                .find(|e| e.program_path().and_then(|p| p.canonicalize().ok()).is_some_and(|p| p.parent() == Some(dir)))
        })
        .or_else(|| {
            // Children inherit the scope, so only its first process counts.
            let parent_scope = processes.get(process.ppid).map(|p| p.scope.as_str());
            if process.scope.is_empty() || parent_scope == Some(process.scope.as_str()) {
                return None;
            }
            scope_app_ids(&process.scope).find_map(|id| entries.iter().find(|e| e.id == id))
        })
        .map(|entry| listed(entries, entry))
        // An entry nobody can see in the launcher means a daemon or a URL handler, not an app.
        .filter(|entry| !entry.hidden)
}

/// Candidate desktop IDs in a systemd unit named per the XDG convention:
/// `app[-<launcher>]-<id>[@<random>].service` or `app[-<launcher>]-<id>-<random>.scope`.
fn scope_app_ids(unit: &str) -> impl Iterator<Item = String> {
    let body = unit
        .strip_suffix(".scope")
        .and_then(|u| u.rsplit_once('-').map(|(head, _)| head))
        .or_else(|| unit.strip_suffix(".service").map(|u| u.split_once('@').map_or(u, |(head, _)| head)))
        .and_then(|u| u.strip_prefix("app-"))
        .unwrap_or_default();
    let unescape = |s: &str| s.replace("\\x2d", "-").replace("\\x2e", ".").replace("\\x5f", "_");
    let without_launcher = body.split_once('-').map(|(_, rest)| unescape(rest));
    [Some(unescape(body)), without_launcher].into_iter().flatten().filter(|id| !id.is_empty())
}

fn read_exe(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[derive(Debug, Clone)]
struct Process {
    pid: u32,
    ppid: u32,
    exe: PathBuf,
    /// The last component of the process's cgroup path, e.g. `app-<id>-<n>.scope`.
    scope: String,
    /// A browser-engine child process (`--type=...` or `-contentproc`).
    is_helper: bool,
}

/// A snapshot of the current user's processes.
#[derive(Debug, Default)]
struct Processes(std::collections::HashMap<u32, Process>);

impl Processes {
    fn scan() -> Self {
        use std::os::unix::fs::MetadataExt as _;

        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
        let mut processes = std::collections::HashMap::new();
        let Ok(dir) = std::fs::read_dir("/proc") else { return Self::default() };
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            if entry.metadata().ok().map(|m| m.uid()) != uid {
                continue;
            }
            if let Some(process) = read_process(pid) {
                processes.insert(pid, process);
            }
        }
        Self(processes)
    }

    fn get(&self, pid: u32) -> Option<&Process> {
        self.0.get(&pid)
    }

    /// Whether `pid` runs a browser engine that spawned helper processes.
    fn has_helper_children(&self, pid: u32) -> bool {
        self.0.values().any(|p| p.ppid == pid && p.is_helper)
    }

    /// App main processes: not a browser-engine helper and not forked from the same executable.
    fn top_level(&self) -> impl Iterator<Item = &Process> {
        let mut processes: Vec<&Process> = self
            .0
            .values()
            .filter(|p| !p.is_helper && self.get(p.ppid).is_none_or(|parent| parent.exe != p.exe))
            .collect();
        processes.sort_by_key(|p| p.pid);
        processes.into_iter()
    }
}

fn read_process(pid: u32) -> Option<Process> {
    let exe = read_exe(pid)?;
    let proc_stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name is parenthesized and may contain spaces; fields after it are fixed.
    let parent_pid = proc_stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()?;
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
    let is_helper =
        cmdline.split(|&b| b == 0 || b == b' ').any(|arg| arg.starts_with(b"--type=") || arg == b"-contentproc");
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).unwrap_or_default();
    let scope = cgroup.lines().find_map(|l| l.strip_prefix("0::")).and_then(|p| p.rsplit('/').next()).unwrap_or("");
    Some(Process { pid, ppid: parent_pid, exe, scope: scope.to_owned(), is_helper })
}

pub(crate) fn launch_app(conn: &Connection, session: &SessionEnv, app: &AppInfo) -> Result<AppInfo> {
    let entries = installed_apps();
    let command = launch_argv(app, &entries)?;
    let (program, args) =
        command.split_first().ok_or_else(|| Error::invalid(format!("{} has no command", app.name)))?;
    let pid = spawn_detached(session, program, args)?;
    let deadline = Instant::now() + LAUNCH_TIMEOUT;
    loop {
        let found = running_apps(conn)?.into_iter().find(|r| {
            r.pid == Some(pid)
                || r.name.eq_ignore_ascii_case(&app.name)
                || (app.bundle_id.is_some()
                    && match_entry(&entries, &r.name, None).map(|e| e.id.as_str()) == app.bundle_id.as_deref())
        });
        if let Some(running) = found {
            let mut info = running_info(conn, &running, &entries, &Processes::scan());
            if info.bundle_id.is_none()
                && let Some(id) = &app.bundle_id
            {
                info = info.with_bundle_id(id.clone());
            }
            return Ok(info);
        }
        if Instant::now() >= deadline {
            return Err(Error::action(format!(
                "{} was started but did not appear on the accessibility bus within {}s",
                app.name,
                LAUNCH_TIMEOUT.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn launch_argv(app: &AppInfo, entries: &[DesktopEntry]) -> Result<Vec<String>> {
    let not_launchable = || Error::Unsupported(format!("{} has no desktop entry or executable to launch", app.name));
    if let Some(path) = &app.path {
        if path.extension().is_some_and(|e| e == "desktop") {
            let contents = std::fs::read_to_string(path)?;
            let parsed = parse_desktop_entry(&contents).ok_or_else(not_launchable)?;
            return parsed.exec.as_deref().and_then(exec_argv).ok_or_else(not_launchable);
        }
        if path.is_file() {
            return Ok(vec![path.to_string_lossy().into_owned()]);
        }
    }
    let entry =
        app.bundle_id.as_deref().and_then(|id| entries.iter().find(|e| e.id == id)).ok_or_else(not_launchable)?;
    exec_argv(&entry.exec).ok_or_else(not_launchable)
}

fn spawn_detached(session: &SessionEnv, program: &str, args: &[String]) -> Result<u32> {
    use std::os::unix::process::CommandExt as _;

    let mut command = Command::new(program);
    session.apply(&mut command);
    let mut child =
        command.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn()?;
    let pid = child.id();
    std::thread::spawn(move || child.wait());
    Ok(pid)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) exec: String,
    pub(crate) startup_wm_class: Option<String>,
    pub(crate) path: PathBuf,
    /// `NoDisplay`/`Hidden`: not listed, but still used to identify running apps.
    pub(crate) hidden: bool,
}

impl DesktopEntry {
    fn into_app_info(self) -> AppInfo {
        AppInfo::new(self.name).with_bundle_id(self.id).with_path(self.path)
    }

    /// The absolute path of the program in `Exec`, looked up on `PATH` when needed.
    fn program_path(&self) -> Option<PathBuf> {
        let program = PathBuf::from(exec_argv(&self.exec)?.first()?);
        if program.is_absolute() {
            return Some(program);
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|dir| dir.join(&program)).find(|p| p.is_file())
    }

    fn program_name(&self) -> Option<String> {
        let argv = exec_argv(&self.exec)?;
        let program = argv.first()?;
        Some(Path::new(program).file_name()?.to_string_lossy().into_owned())
    }
}

fn match_entry<'a>(entries: &'a [DesktopEntry], name: &str, exe: Option<&str>) -> Option<&'a DesktopEntry> {
    let visible = || entries.iter().filter(|e| !e.hidden);
    let last_id_part = |e: &DesktopEntry| e.id.rsplit('.').next().map(str::to_owned).unwrap_or_default();
    visible()
        .find(|e| e.name.eq_ignore_ascii_case(name))
        .or_else(|| visible().find(|e| e.startup_wm_class.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(name))))
        .or_else(|| visible().find(|e| e.id.eq_ignore_ascii_case(name) || last_id_part(e).eq_ignore_ascii_case(name)))
        .or_else(|| {
            let exe = exe?;
            visible().find(|e| e.program_name().is_some_and(|p| p == exe))
        })
}

/// Swaps a `NoDisplay` alias for the listed entry of the same app, when there is one.
fn listed<'a>(entries: &'a [DesktopEntry], entry: &'a DesktopEntry) -> &'a DesktopEntry {
    if !entry.hidden {
        return entry;
    }
    entries.iter().find(|e| !e.hidden && (e.exec == entry.exec || e.name == entry.name)).unwrap_or(entry)
}

fn application_dirs() -> Vec<PathBuf> {
    let non_empty = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());
    let data_home = non_empty("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| non_empty("HOME").map(|home| Path::new(&home).join(".local/share")));
    let data_dirs = non_empty("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    data_home
        .into_iter()
        .chain(data_dirs.split(':').filter(|d| !d.is_empty()).map(PathBuf::from))
        .map(|dir| dir.join("applications"))
        .collect()
}

pub(crate) fn installed_apps() -> Vec<DesktopEntry> {
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for dir in application_dirs() {
        let mut files = Vec::new();
        collect_desktop_files(&dir, &dir, &mut files);
        files.sort();
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(contents) = std::fs::read_to_string(&path) else { continue };
            let Some(parsed) = parse_desktop_entry(&contents) else { continue };
            let Some(exec) = parsed.exec else { continue };
            entries.push(DesktopEntry {
                id,
                name: parsed.name,
                exec,
                startup_wm_class: parsed.startup_wm_class,
                path,
                hidden: parsed.hidden,
            });
        }
    }
    entries
}

fn collect_desktop_files(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(read_dir) = std::fs::read_dir(dir) else { return };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_desktop_files(base, &path, out);
        } else if let Some(id) = desktop_id(base, &path) {
            out.push((id, path));
        }
    }
}

fn desktop_id(base: &Path, path: &Path) -> Option<String> {
    let relative = path.strip_prefix(base).ok()?.to_str()?;
    let stem = relative.strip_suffix(".desktop")?;
    Some(stem.replace('/', "-"))
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct ParsedEntry {
    pub(crate) name: String,
    pub(crate) exec: Option<String>,
    pub(crate) startup_wm_class: Option<String>,
    pub(crate) hidden: bool,
}

pub(crate) fn parse_desktop_entry(contents: &str) -> Option<ParsedEntry> {
    let mut entry = ParsedEntry::default();
    let mut in_main_group = false;
    let mut is_application = true;
    for line in contents.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_main_group = line == "[Desktop Entry]";
            continue;
        }
        let Some((key, value)) = line.split_once('=').filter(|_| in_main_group) else { continue };
        let value = unescape_value(value.trim());
        match key.trim() {
            "Name" => entry.name = value,
            "Exec" => entry.exec = Some(value).filter(|v| !v.is_empty()),
            "StartupWMClass" => entry.startup_wm_class = Some(value).filter(|v| !v.is_empty()),
            "Type" => is_application = value == "Application",
            "NoDisplay" | "Hidden" => entry.hidden |= value == "true",
            _ => {}
        }
    }
    (is_application && !entry.name.is_empty()).then_some(entry)
}

fn unescape_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => {
                // Keep unknown escapes (and `\\`) for the Exec quoting rules to resolve.
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

pub(crate) fn exec_argv(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' => current.push(chars.next()?),
            c if c.is_whitespace() && !quoted => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => current.push(c),
        }
    }
    if quoted {
        return None;
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    let args: Vec<String> = args.iter().filter_map(|a| strip_field_codes(a)).collect();
    (!args.first()?.is_empty()).then_some(args)
}

fn strip_field_codes(arg: &str) -> Option<String> {
    let mut out = String::with_capacity(arg.len());
    let mut removed = false;
    let mut chars = arg.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some(_) | None => removed = true,
        }
    }
    (!removed || !out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_exec_field_codes() {
        assert_eq!(exec_argv("gedit %U"), Some(vec!["gedit".to_owned()]));
        assert_eq!(
            exec_argv(r#"env "FOO=a b" /usr/bin/app --name=%c -x 100%% %f"#),
            Some(vec![
                "env".into(),
                "FOO=a b".into(),
                "/usr/bin/app".into(),
                "--name=".into(),
                "-x".into(),
                "100%".into()
            ])
        );
        assert_eq!(exec_argv(r#""/opt/My App/run" "" %F"#), Some(vec!["/opt/My App/run".into(), String::new()]));
        assert_eq!(exec_argv(r#"sh -c "echo \"hi\"""#), Some(vec!["sh".into(), "-c".into(), r#"echo "hi""#.into()]));
        assert_eq!(exec_argv(r#"broken "quote"#), None);
        assert_eq!(exec_argv("%U"), None);
    }

    #[test]
    fn parses_desktop_entries() {
        let file = "# comment\n[Desktop Entry]\nType=Application\nName=Text Editor\nName[de]=Texteditor\n\
                    Exec=gnome-text-editor %U\nStartupWMClass=gnome-text-editor\n\n[Desktop Action new]\nName=New\nExec=other\n";
        let entry = parse_desktop_entry(file);
        assert_eq!(
            entry,
            Some(ParsedEntry {
                name: "Text Editor".into(),
                exec: Some("gnome-text-editor %U".into()),
                startup_wm_class: Some("gnome-text-editor".into()),
                hidden: false,
            })
        );
        assert!(parse_desktop_entry("[Desktop Entry]\nType=Link\nName=Docs\nURL=x\n").is_none());
        let hidden = parse_desktop_entry("[Desktop Entry]\nName=Daemon\nExec=d\nNoDisplay=true\n");
        assert!(hidden.is_some_and(|e| e.hidden));
        assert_eq!(unescape_value(r"a\sb\\c"), r"a b\\c");
    }

    #[test]
    fn derives_desktop_ids() {
        let base = Path::new("/usr/share/applications");
        assert_eq!(desktop_id(base, &base.join("org.gnome.TextEditor.desktop")), Some("org.gnome.TextEditor".into()));
        assert_eq!(desktop_id(base, &base.join("kde4/kate.desktop")), Some("kde4-kate".into()));
        assert_eq!(desktop_id(base, &base.join("mimeinfo.cache")), None);
    }

    #[test]
    fn parses_scope_app_ids() {
        let ids = |unit| scope_app_ids(unit).collect::<Vec<_>>();
        assert_eq!(ids("app-com.google.Chrome-3365.scope"), ["com.google.Chrome"]);
        assert_eq!(ids("app-gnome-google\\x2dchrome-3365.scope"), ["gnome-google-chrome", "google-chrome"]);
        assert_eq!(ids("app-flatpak-org.gnome.Maps-1234.scope"), ["flatpak-org.gnome.Maps", "org.gnome.Maps"]);
        assert_eq!(ids("app-gnome-org.gnome.Console@a1b2.service"), ["gnome-org.gnome.Console", "org.gnome.Console"]);
        assert_eq!(ids("session-2.scope"), Vec::<String>::new());
        assert_eq!(ids("user@1000.service"), Vec::<String>::new());
    }

    #[test]
    fn matches_running_apps_to_entries() {
        let entry = |id: &str, name: &str, exec: &str| DesktopEntry {
            id: id.into(),
            name: name.into(),
            exec: exec.into(),
            startup_wm_class: None,
            path: PathBuf::from(format!("/usr/share/applications/{id}.desktop")),
            hidden: false,
        };
        let entries = vec![
            entry("org.gnome.TextEditor", "Text Editor", "gnome-text-editor %U"),
            entry("firefox", "Firefox", "/usr/lib/firefox/firefox %u"),
        ];
        let id = |name, exe| match_entry(&entries, name, exe).map(|e| e.id.as_str());
        assert_eq!(id("Firefox", None), Some("firefox"));
        assert_eq!(id("texteditor", None), Some("org.gnome.TextEditor"));
        assert_eq!(id("gedit-like", Some("gnome-text-editor")), Some("org.gnome.TextEditor"));
        assert_eq!(id("unknown", None), None);
    }
}
