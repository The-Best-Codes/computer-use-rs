use std::fmt;

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// An OS permission that computer use depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Permission {
    /// Reading and driving other apps through the accessibility API.
    Accessibility,
    /// Capturing pixels of other apps' windows.
    ScreenRecording,
}

impl fmt::Display for Permission {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Accessibility => "accessibility",
            Self::ScreenRecording => "screen recording",
        })
    }
}

/// Everything that can go wrong while observing or driving an app.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// No running or installed app matched the query.
    #[error("app not found: {0}")]
    AppNotFound(String),

    /// The app is running but has no window that can be observed.
    #[error("no usable window for app {0}")]
    WindowNotFound(String),

    /// An action referenced app state that was never captured (or was discarded).
    #[error("no app state for {0}; call get_app_state first")]
    NoAppState(String),

    /// An element index did not exist in the latest captured state.
    #[error("element {index} not found in the latest state of {app}")]
    ElementNotFound {
        /// App the lookup was made against.
        app: String,
        /// The requested element index.
        index: usize,
    },

    /// A caller-supplied argument was invalid.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// A required OS permission has not been granted to this process.
    #[error("{0} permission has not been granted to this process")]
    PermissionDenied(Permission),

    /// The operation is not available on this platform or backend.
    #[error("unsupported: {0}")]
    Unsupported(String),

    /// The platform accepted the request but reported that it failed.
    #[error("action failed: {0}")]
    ActionFailed(String),

    /// The state the action needs is missing (for example, no screenshot was captured).
    #[error("state unavailable: {0}")]
    StateUnavailable(String),

    /// A platform API returned an unexpected error.
    #[error("platform error: {0}")]
    Platform(String),

    /// Encoding a screenshot failed.
    #[error("image encoding failed: {0}")]
    Image(#[from] image::ImageError),

    /// An I/O error, for example while launching an app.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::InvalidArgument(message.into())
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn platform(message: impl Into<String>) -> Self {
        Self::Platform(message.into())
    }

    pub(crate) fn action(message: impl Into<String>) -> Self {
        Self::ActionFailed(message.into())
    }
}
