//! Platform input injection backends.

use ferrokey_protocol::InputEvent;

#[cfg(target_os = "linux")]
mod portal;
#[cfg(target_os = "windows")]
mod windows;

/// Injects input events into the local desktop session.
pub trait InputBackend {
    fn inject(&mut self, event: InputEvent) -> Result<(), Error>;
}

#[derive(Debug)]
pub enum Error {
    /// The backend has not been implemented for this platform yet.
    Unsupported,
    /// The backend's session ended, so no more events can be injected.
    SessionClosed,
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

/// Returns the input backend for the current platform.
pub fn backend() -> Result<Box<dyn InputBackend>, Error> {
    #[cfg(target_os = "linux")]
    return Ok(Box::new(portal::PortalBackend::new()?));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(windows::WindowsBackend::new()?));
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Err(Error::Unsupported)
}
