//! Platform input injection and screen capture backends.

use ferrokey_protocol::InputEvent;

#[cfg(all(target_os = "linux", feature = "screen"))]
mod pipewire;
#[cfg(target_os = "linux")]
mod portal;
pub mod screen;
#[cfg(target_os = "windows")]
mod windows;

/// Injects input events into the local desktop session.
pub trait InputBackend: Send {
    fn inject(&mut self, event: InputEvent) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    /// The backend has not been implemented for this platform yet.
    Unsupported,
    /// The backend's session ended, so no more events can be injected.
    SessionClosed,
    /// Absolute pointer motion needs a shared screen, and there is none.
    NoScreen,
    /// The platform backend reported an error.
    Backend(Box<dyn std::error::Error + Send + Sync>),
}

impl Error {
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    pub(crate) fn backend(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Error::Backend(Box::new(err))
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => f.write_str("input injection is not supported on this platform"),
            Error::SessionClosed => f.write_str("input session was closed"),
            Error::NoScreen => f.write_str("absolute pointer motion needs screen sharing"),
            Error::Backend(err) => write!(f, "input backend error: {err}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Backend(err) => Some(err.as_ref()),
            _ => None,
        }
    }
}

/// What [`backend`] should set up.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Share the screen as well, where the platform supports it.
    pub screen: bool,
}

/// The platform's input injection and, if requested and available, screen
/// capture.
pub struct Backend {
    pub input: Box<dyn InputBackend>,
    pub screen: Option<screen::Screen>,
}

/// Returns the backend for the current platform.
pub fn backend(options: Options) -> Result<Backend, Error> {
    #[cfg(target_os = "linux")]
    {
        let (input, screen) = portal::PortalBackend::new(options.screen)?;
        Ok(Backend {
            input: Box::new(input),
            screen,
        })
    }
    #[cfg(target_os = "windows")]
    {
        let _ = options;
        Ok(Backend {
            input: Box::new(windows::WindowsBackend::new()?),
            screen: None,
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = options;
        Err(Error::Unsupported)
    }
}
