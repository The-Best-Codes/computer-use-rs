use std::collections::{BTreeMap, HashMap, HashSet};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

const XDG_RUNTIME_DIR: &str = "XDG_RUNTIME_DIR";
const DBUS_SESSION_BUS_ADDRESS: &str = "DBUS_SESSION_BUS_ADDRESS";
const WAYLAND_DISPLAY: &str = "WAYLAND_DISPLAY";
const DISPLAY: &str = "DISPLAY";
const XAUTHORITY: &str = "XAUTHORITY";
const AT_SPI_BUS_ADDRESS: &str = "AT_SPI_BUS_ADDRESS";

const INHERITED_KEYS: [&str; 9] = [
    DISPLAY,
    XAUTHORITY,
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_TYPE",
    "DESKTOP_SESSION",
    "GDK_BACKEND",
    "QT_QPA_PLATFORMTHEME",
    AT_SPI_BUS_ADDRESS,
];

const SIGNAL_KEYS: [&str; 6] =
    [XDG_RUNTIME_DIR, DBUS_SESSION_BUS_ADDRESS, DISPLAY, WAYLAND_DISPLAY, XAUTHORITY, AT_SPI_BUS_ADDRESS];

const DESKTOP_PROCESSES: [(&str, u32); 26] = [
    ("gnome-session", 100),
    ("gnome-shell", 95),
    ("plasmashell", 95),
    ("kwin_wayland", 95),
    ("kwin_x11", 95),
    ("startplasma", 95),
    ("cinnamon-session", 95),
    ("mate-session", 95),
    ("xfce4-session", 95),
    ("lxqt-session", 95),
    ("sway", 95),
    ("wayfire", 95),
    ("xorg", 80),
    ("xwayland", 75),
    ("gnome-terminal-server", 65),
    ("ptyxis", 65),
    ("kgx", 65),
    ("konsole", 65),
    ("xfce4-terminal", 65),
    ("alacritty", 65),
    ("wezterm", 65),
    ("kitty", 65),
    ("tilix", 65),
    ("codex", 50),
    ("dbus-daemon", 45),
    ("systemd --user", 40),
];

type EnvMap = HashMap<String, String>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionEnv {
    vars: BTreeMap<String, String>,
}

impl SessionEnv {
    pub(crate) fn discover() -> Self {
        let base: EnvMap = std::env::vars().collect();
        let uid = current_uid();
        let process_envs = uid.map(desktop_process_environments).unwrap_or_default();
        resolve(&base, uid, &process_envs, &FileSystem { uid })
    }

    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str).filter(|v| !v.trim().is_empty())
    }

    pub(crate) fn display(&self) -> Option<&str> {
        self.get(DISPLAY)
    }

    pub(crate) fn xauthority(&self) -> Option<PathBuf> {
        self.get(XAUTHORITY)
            .map(PathBuf::from)
            .or_else(|| self.get("HOME").map(|home| Path::new(home).join(".Xauthority")))
    }

    pub(crate) fn session_bus(&self) -> Option<&str> {
        self.get(DBUS_SESSION_BUS_ADDRESS)
    }

    pub(crate) fn atspi_bus(&self) -> Option<&str> {
        self.get(AT_SPI_BUS_ADDRESS)
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        command.envs(self.vars.iter().filter(|(_, v)| !v.trim().is_empty()));
    }
}

pub(crate) trait Probe {
    fn is_socket(&self, path: &Path) -> bool;
    fn is_user_dir(&self, path: &Path) -> bool;
    fn list(&self, dir: &Path) -> Vec<String>;
}

struct FileSystem {
    uid: Option<u32>,
}

impl Probe for FileSystem {
    fn is_socket(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.file_type().is_socket())
    }

    fn is_user_dir(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|m| m.is_dir() && Some(m.uid()) == self.uid)
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| entries.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
            .unwrap_or_default()
    }
}

fn current_uid() -> Option<u32> {
    std::fs::metadata("/proc/self").ok().map(|m| m.uid())
}

fn non_empty<'a>(env: &'a EnvMap, key: &str) -> Option<&'a str> {
    env.get(key).map(|v| v.trim()).filter(|v| !v.is_empty())
}

pub(crate) fn resolve(base: &EnvMap, uid: Option<u32>, process_envs: &[EnvMap], probe: &impl Probe) -> SessionEnv {
    let mut vars: BTreeMap<String, String> = base.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    vars.retain(|k, _| {
        INHERITED_KEYS.contains(&k.as_str())
            || [XDG_RUNTIME_DIR, DBUS_SESSION_BUS_ADDRESS, WAYLAND_DISPLAY, "HOME"].contains(&k.as_str())
    });
    let runtime_dir = choose_runtime_dir(base, process_envs, uid, probe);
    if let Some(dir) = &runtime_dir {
        vars.insert(XDG_RUNTIME_DIR.into(), dir.to_string_lossy().into_owned());
    }
    let current_bus = non_empty(base, DBUS_SESSION_BUS_ADDRESS);
    if let Some(bus) = session_bus_address(current_bus, runtime_dir.as_deref(), process_envs, probe) {
        vars.insert(DBUS_SESSION_BUS_ADDRESS.into(), bus);
    }
    let current_wayland = non_empty(base, WAYLAND_DISPLAY);
    if let Some(wayland) = wayland_display(current_wayland, runtime_dir.as_deref(), process_envs, probe) {
        vars.insert(WAYLAND_DISPLAY.into(), wayland);
    }
    for key in INHERITED_KEYS {
        if non_empty(base, key).is_none()
            && let Some(value) = first_session_value(process_envs, key)
        {
            vars.insert(key.into(), value.to_owned());
        }
    }
    SessionEnv { vars }
}

fn choose_runtime_dir(base: &EnvMap, process_envs: &[EnvMap], uid: Option<u32>, probe: &impl Probe) -> Option<PathBuf> {
    let mut seen = HashSet::new();
    std::iter::once(non_empty(base, XDG_RUNTIME_DIR).map(PathBuf::from))
        .chain(process_envs.iter().map(|env| non_empty(env, XDG_RUNTIME_DIR).map(PathBuf::from)))
        .chain(std::iter::once(uid.map(|uid| PathBuf::from(format!("/run/user/{uid}")))))
        .flatten()
        .map(|dir| dir.components().collect::<PathBuf>())
        .filter(|dir| seen.insert(dir.clone()))
        .find(|dir| probe.is_user_dir(dir))
}

fn session_bus_address(
    current: Option<&str>,
    runtime_dir: Option<&Path>,
    process_envs: &[EnvMap],
    probe: &impl Probe,
) -> Option<String> {
    let runtime_bus = runtime_dir.map(|dir| dir.join("bus")).filter(|bus| probe.is_socket(bus));
    let runtime_address = |bus: &Path| format!("unix:path={}", bus.display());
    if let (Some(bus), Some(dir)) = (&runtime_bus, runtime_dir)
        && should_use_runtime_bus(current, dir)
    {
        return Some(runtime_address(bus));
    }
    if let Some(current) = current {
        return Some(current.to_owned());
    }
    if let Some(value) = process_envs.iter().find_map(|env| non_empty(env, DBUS_SESSION_BUS_ADDRESS)) {
        if let Some(bus) = &runtime_bus
            && value.contains(&*bus.to_string_lossy())
        {
            return Some(runtime_address(bus));
        }
        return Some(value.to_owned());
    }
    runtime_bus.as_deref().map(runtime_address)
}

fn should_use_runtime_bus(current: Option<&str>, runtime_dir: &Path) -> bool {
    let Some(current) = current.map(str::trim).filter(|c| !c.is_empty()) else { return true };
    let dir = runtime_dir.to_string_lossy();
    let bus = runtime_dir.join("bus");
    current.contains(&*bus.to_string_lossy()) || (current.contains("/run/user/") && !current.contains(&*dir))
}

fn wayland_display(
    current: Option<&str>,
    runtime_dir: Option<&Path>,
    process_envs: &[EnvMap],
    probe: &impl Probe,
) -> Option<String> {
    let normalize = |value: Option<&str>| -> Option<String> {
        let value = value?.trim();
        if value.is_empty() {
            return None;
        }
        let Some(dir) = runtime_dir else { return Some(value.to_owned()) };
        let path = Path::new(value);
        let socket = if path.is_absolute() { path.to_path_buf() } else { dir.join(path) };
        probe.is_socket(&socket).then(|| value.to_owned())
    };
    normalize(current)
        .or_else(|| process_envs.iter().find_map(|env| normalize(env.get(WAYLAND_DISPLAY).map(String::as_str))))
        .or_else(|| first_wayland_socket(runtime_dir?, probe))
}

fn first_wayland_socket(runtime_dir: &Path, probe: &impl Probe) -> Option<String> {
    let is_socket = |name: &str| probe.is_socket(&runtime_dir.join(name));
    if let Some(name) = ["wayland-0", "wayland-1"].into_iter().find(|n| is_socket(n)) {
        return Some(name.to_owned());
    }
    let mut names: Vec<String> = probe
        .list(runtime_dir)
        .into_iter()
        .filter(|n| {
            n.starts_with("wayland-") && !Path::new(n).extension().is_some_and(|e| e.eq_ignore_ascii_case("lock"))
        })
        .collect();
    names.sort();
    names.into_iter().find(|n| is_socket(n))
}

fn first_session_value<'a>(process_envs: &'a [EnvMap], key: &str) -> Option<&'a str> {
    process_envs.iter().find_map(|env| non_empty(env, key))
}

pub(crate) fn desktop_process_rank(text: &str) -> u32 {
    DESKTOP_PROCESSES.iter().filter(|(needle, _)| text.contains(needle)).map(|(_, rank)| *rank).max().unwrap_or(0)
}

pub(crate) fn session_env_rank(env: &EnvMap) -> u32 {
    let weight = |key: &str, points: u32| if non_empty(env, key).is_some() { points } else { 0 };
    weight(XDG_RUNTIME_DIR, 20)
        + weight(DBUS_SESSION_BUS_ADDRESS, 20)
        + weight(DISPLAY, 10)
        + weight(WAYLAND_DISPLAY, 10)
        + weight(XAUTHORITY, 5)
}

fn has_session_signal(env: &EnvMap) -> bool {
    SIGNAL_KEYS.iter().any(|key| non_empty(env, key).is_some())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub(crate) pid: u32,
    pub(crate) search_text: String,
    pub(crate) env: EnvMap,
}

pub(crate) fn rank_candidates(candidates: Vec<Candidate>) -> Vec<EnvMap> {
    let mut ranked: Vec<(u32, u32, EnvMap)> = candidates
        .into_iter()
        .filter_map(|c| {
            let rank = desktop_process_rank(&c.search_text);
            (rank > 0 && has_session_signal(&c.env)).then(|| (rank + session_env_rank(&c.env), c.pid, c.env))
        })
        .collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    ranked.into_iter().map(|(_, _, env)| env).collect()
}

fn desktop_process_environments(uid: u32) -> Vec<EnvMap> {
    let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
    let candidates = entries
        .flatten()
        .filter_map(|entry| {
            let pid: u32 = entry.file_name().to_str()?.parse().ok()?;
            let dir = entry.path();
            if std::fs::metadata(&dir).ok()?.uid() != uid {
                return None;
            }
            let search_text = process_search_text(&dir);
            if desktop_process_rank(&search_text) == 0 {
                return None;
            }
            let env = parse_null_env(&std::fs::read(dir.join("environ")).ok()?);
            Some(Candidate { pid, search_text, env })
        })
        .collect();
    rank_candidates(candidates)
}

fn process_search_text(dir: &Path) -> String {
    let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default();
    let cmdline = std::fs::read(dir.join("cmdline")).unwrap_or_default();
    let cmdline = String::from_utf8_lossy(&cmdline).trim_matches('\0').replace('\0', " ");
    format!("{} {cmdline}", comm.trim()).to_lowercase()
}

pub(crate) fn parse_null_env(data: &[u8]) -> EnvMap {
    data.split(|&b| b == 0)
        .filter_map(|entry| {
            let entry = String::from_utf8_lossy(entry);
            let (key, value) = entry.split_once('=')?;
            (!key.is_empty()).then(|| (key.to_owned(), value.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFs {
        sockets: Vec<&'static str>,
        dirs: Vec<&'static str>,
    }

    impl Probe for FakeFs {
        fn is_socket(&self, path: &Path) -> bool {
            self.sockets.iter().any(|s| Path::new(s) == path)
        }

        fn is_user_dir(&self, path: &Path) -> bool {
            self.dirs.iter().any(|d| Path::new(d) == path)
        }

        fn list(&self, dir: &Path) -> Vec<String> {
            self.sockets
                .iter()
                .filter_map(|s| Path::new(s).strip_prefix(dir).ok())
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        }
    }

    fn env(pairs: &[(&str, &str)]) -> EnvMap {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn ranks_desktop_processes() {
        assert_eq!(desktop_process_rank("gnome-shell /usr/bin/gnome-shell"), 95);
        assert_eq!(desktop_process_rank("gnome-session-b gnome-session-binary --session=ubuntu"), 100);
        assert_eq!(desktop_process_rank("xwayland xwayland :0 -rootless"), 75);
        assert_eq!(desktop_process_rank("bash -bash"), 0);
        assert_eq!(session_env_rank(&env(&[("DISPLAY", ":0"), ("XDG_RUNTIME_DIR", "/run/user/1000")])), 30);
        assert_eq!(session_env_rank(&env(&[("DISPLAY", " ")])), 0);
    }

    #[test]
    fn orders_candidates_by_rank_then_pid() {
        let candidate =
            |pid, text: &str, pairs: &[(&str, &str)]| Candidate { pid, search_text: text.to_owned(), env: env(pairs) };
        let ranked = rank_candidates(vec![
            candidate(50, "kitty kitty", &[("DISPLAY", ":1")]),
            candidate(10, "gnome-shell", &[("DISPLAY", ":0")]),
            candidate(5, "gnome-shell", &[("DISPLAY", ":2")]),
            candidate(1, "gnome-session", &[]),
            candidate(2, "bash", &[("DISPLAY", ":9")]),
        ]);
        let displays: Vec<_> = ranked.iter().map(|e| e["DISPLAY"].as_str()).collect();
        assert_eq!(displays, vec![":2", ":0", ":1"]);
    }

    #[test]
    fn parses_proc_environ() {
        let parsed = parse_null_env(b"DISPLAY=:0\0EMPTY=\0A=b=c\0junk\0\0");
        assert_eq!(parsed, env(&[("DISPLAY", ":0"), ("EMPTY", ""), ("A", "b=c")]));
    }

    #[test]
    fn fills_missing_values_from_desktop_processes() {
        let fs =
            FakeFs { sockets: vec!["/run/user/1000/bus", "/run/user/1000/wayland-0"], dirs: vec!["/run/user/1000"] };
        let desktop = env(&[
            ("DISPLAY", ":0"),
            ("XAUTHORITY", "/run/user/1000/.mutter-Xwaylandauth.ABC"),
            ("XDG_SESSION_TYPE", "wayland"),
            ("WAYLAND_DISPLAY", "wayland-0"),
        ]);
        let session = resolve(&env(&[("HOME", "/home/me")]), Some(1000), &[desktop], &fs);
        assert_eq!(session.display(), Some(":0"));
        assert_eq!(session.session_bus(), Some("unix:path=/run/user/1000/bus"));
        assert_eq!(session.get("XDG_RUNTIME_DIR"), Some("/run/user/1000"));
        assert_eq!(session.get("WAYLAND_DISPLAY"), Some("wayland-0"));
        assert_eq!(session.xauthority(), Some(PathBuf::from("/run/user/1000/.mutter-Xwaylandauth.ABC")));
    }

    #[test]
    fn keeps_own_values_and_prefers_own_runtime_bus() {
        let fs = FakeFs { sockets: vec!["/run/user/1000/bus"], dirs: vec!["/run/user/1000"] };
        let own = env(&[
            ("DISPLAY", ":5"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/0/bus"),
        ]);
        let session = resolve(&own, Some(1000), &[env(&[("DISPLAY", ":0")])], &fs);
        assert_eq!(session.display(), Some(":5"));
        assert_eq!(session.session_bus(), Some("unix:path=/run/user/1000/bus"));
        assert!(should_use_runtime_bus(None, Path::new("/run/user/1000")));
        assert!(!should_use_runtime_bus(Some("unix:abstract=/tmp/dbus-x"), Path::new("/run/user/1000")));
    }

    #[test]
    fn finds_wayland_socket_by_listing() {
        let fs = FakeFs { sockets: vec!["/run/user/7/wayland-3", "/run/user/7/wayland-3.lock"], dirs: vec![] };
        assert_eq!(first_wayland_socket(Path::new("/run/user/7"), &fs), Some("wayland-3".into()));
    }
}
