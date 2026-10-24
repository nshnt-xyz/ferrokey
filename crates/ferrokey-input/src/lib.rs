//! Platform input injection backends.

use ferrokey_protocol::InputEvent;

#[cfg(target_os = "linux")]
mod wayland;
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => f.write_str("input injection is not supported on this platform"),
        }
    }
}

impl std::error::Error for Error {}

/// Returns the input backend for the current platform.
pub fn backend() -> Result<Box<dyn InputBackend>, Error> {
    #[cfg(target_os = "linux")]
    return Ok(Box::new(wayland::WaylandBackend::new()?));
    #[cfg(target_os = "windows")]
    return Ok(Box::new(windows::WindowsBackend::new()?));
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    Err(Error::Unsupported)
}
