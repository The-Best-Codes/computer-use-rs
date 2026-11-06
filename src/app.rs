use std::path::PathBuf;

use crate::error::{Error, Result};

/// An application that is running or installed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct AppInfo {
    /// Human-readable name, e.g. `TextEdit`.
    pub name: String,
    /// Platform identifier: a bundle ID on macOS, an AUMID or executable name on Windows,
    /// a desktop-file ID on Linux.
    pub bundle_id: Option<String>,
    /// Process ID, when running.
    pub pid: Option<u32>,
    /// Path to the app bundle or executable.
    pub path: Option<PathBuf>,
    /// Whether the app currently has a process.
    pub is_running: bool,
    /// Whether the app is the frontmost (focused) app.
    pub is_active: bool,
    /// Title of the app's main window, when the platform reports one. Queries also match it.
    pub window_title: Option<String>,
    /// The app is running but does not expose its UI to accessibility, so it cannot be
    /// inspected or controlled until it is quit and started again.
    ///
    /// Only set on Linux, for browser-engine apps started before desktop accessibility was
    /// turned on.
    pub needs_restart: bool,
}

impl AppInfo {
    /// Creates a record for an app with only a name; fill the rest with the builder methods.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..Self::default() }
    }

    /// Sets the platform identifier.
    #[must_use]
    pub fn with_bundle_id(mut self, bundle_id: impl Into<String>) -> Self {
        self.bundle_id = Some(bundle_id.into());
        self
    }

    /// Marks the app as running with `pid`.
    #[must_use]
    pub const fn with_pid(mut self, pid: u32) -> Self {
        self.pid = Some(pid);
        self.is_running = true;
        self
    }

    /// Sets the bundle or executable path.
    #[must_use]
    pub fn with_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// Marks the app as frontmost.
    #[must_use]
    pub const fn with_active(mut self, active: bool) -> Self {
        self.is_active = active;
        self
    }

    /// Marks the app as running without an accessibility tree (see [`AppInfo::needs_restart`]).
    #[must_use]
    pub const fn with_needs_restart(mut self, needs_restart: bool) -> Self {
        self.needs_restart = needs_restart;
        self
    }

    /// Sets the main window title.
    #[must_use]
    pub fn with_window_title(mut self, title: impl Into<String>) -> Self {
        self.window_title = Some(title.into()).filter(|t: &String| !t.is_empty());
        self
    }

    /// The identifier shown to callers: bundle ID when known, otherwise the name.
    #[must_use]
    pub fn reference(&self) -> &str {
        self.bundle_id.as_deref().unwrap_or(&self.name)
    }

    /// How well this app matches a user query; `None` means no match.
    ///
    /// Queries may be a name, bundle ID, path, or `pid:<n>`. Lower scores are better.
    #[must_use]
    pub fn match_score(&self, query: &str) -> Option<u8> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        if let Some(pid) = query.strip_prefix("pid:") {
            return (pid.trim().parse::<u32>().ok() == self.pid && self.pid.is_some()).then_some(0);
        }
        let q = query.to_lowercase();
        let q_stem = q.strip_suffix(".app").or_else(|| q.strip_suffix(".exe")).unwrap_or(&q);
        let name = self.name.to_lowercase();
        let bundle = self.bundle_id.as_deref().map(str::to_lowercase);

        if name == q || name == q_stem || bundle.as_deref() == Some(q.as_str()) {
            return Some(0);
        }
        if let Some(path) = &self.path {
            let path_str = path.to_string_lossy().to_lowercase();
            if path_str == q {
                return Some(1);
            }
            let stem = path.file_stem().map(|s| s.to_string_lossy().to_lowercase());
            if stem.as_deref() == Some(q_stem) {
                return Some(2);
            }
        }
        if bundle.as_deref().and_then(|b| b.rsplit('.').next()).is_some_and(|last| last == q_stem) {
            return Some(3);
        }
        if name.starts_with(q_stem) || name.split_whitespace().any(|word| word == q_stem) {
            return Some(4);
        }
        let title = self.window_title.as_deref().map(str::to_lowercase);
        if title.as_deref() == Some(q.as_str()) {
            return Some(5);
        }
        if title.is_some_and(|t| t.contains(q.as_str())) {
            return Some(6);
        }
        None
    }
}

/// The best match for `query`, or [`Error::AmbiguousApp`] when several running apps tie.
pub(crate) fn resolve(apps: &[AppInfo], query: &str) -> Result<AppInfo> {
    let mut matches: Vec<_> = apps.iter().filter_map(|app| Some((app.match_score(query)?, app))).collect();
    matches.sort_by_key(|(score, app)| (*score, !app.is_running, !app.is_active, app.pid));
    let Some(&(best, first)) = matches.first() else {
        return Err(Error::AppNotFound(query.to_owned()));
    };
    let tied: Vec<AppInfo> =
        matches.iter().filter(|(score, app)| *score == best && app.is_running).map(|(_, app)| (*app).clone()).collect();
    if tied.len() > 1 {
        return Err(Error::AmbiguousApp { query: query.to_owned(), candidates: tied });
    }
    Ok(first.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps() -> Vec<AppInfo> {
        vec![
            AppInfo::new("TextEdit")
                .with_bundle_id("com.apple.TextEdit")
                .with_path("/System/Applications/TextEdit.app"),
            AppInfo::new("TextEdit").with_bundle_id("com.apple.TextEdit").with_pid(42),
            AppInfo::new("Safari").with_bundle_id("com.apple.Safari").with_pid(7),
        ]
    }

    fn best_match<'a>(apps: &'a [AppInfo], query: &str) -> Option<&'a AppInfo> {
        apps.iter().find(|app| resolve(apps, query).is_ok_and(|best| best == **app))
    }

    #[test]
    fn same_named_running_apps_are_ambiguous() {
        let apps = vec![AppInfo::new("Code").with_pid(2), AppInfo::new("Code").with_pid(1).with_window_title("a.rs")];
        let Err(Error::AmbiguousApp { candidates, .. }) = resolve(&apps, "code") else { panic!("expected ambiguity") };
        assert_eq!(candidates.iter().filter_map(|a| a.pid).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(resolve(&apps, "pid:2").ok().and_then(|a| a.pid), Some(2));
        assert_eq!(resolve(&apps, "a.rs").ok().and_then(|a| a.pid), Some(1));
    }

    #[test]
    fn matches_name_bundle_and_pid() {
        let apps = apps();
        assert_eq!(best_match(&apps, "textedit").and_then(|a| a.pid), Some(42));
        assert_eq!(best_match(&apps, "com.apple.Safari").map(|a| a.name.as_str()), Some("Safari"));
        assert_eq!(best_match(&apps, "pid:7").map(|a| a.name.as_str()), Some("Safari"));
        assert_eq!(best_match(&apps, "Safari.app").map(|a| a.name.as_str()), Some("Safari"));
        assert!(best_match(&apps, "Xcode").is_none());
    }

    #[test]
    fn matches_window_titles_last() {
        let apps = vec![
            AppInfo::new("notepad").with_pid(1).with_window_title("todo.txt - Notepad"),
            AppInfo::new("todo").with_pid(2),
        ];
        assert_eq!(best_match(&apps, "todo.txt - notepad").and_then(|a| a.pid), Some(1));
        assert_eq!(best_match(&apps, "todo").and_then(|a| a.pid), Some(2));
        assert_eq!(best_match(&apps, "todo.txt").and_then(|a| a.pid), Some(1));
    }

    #[test]
    fn prefix_match_is_weaker_than_exact() {
        let apps = vec![AppInfo::new("Notes Helper").with_pid(1), AppInfo::new("Notes").with_pid(2)];
        assert_eq!(best_match(&apps, "notes").and_then(|a| a.pid), Some(2));
    }
}
