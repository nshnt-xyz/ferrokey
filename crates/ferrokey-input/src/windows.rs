//! Windows input injection backend.

use ferrokey_protocol::InputEvent;

use crate::{Error, InputBackend};

pub struct WindowsBackend;

impl WindowsBackend {
    pub fn new() -> Result<Self, Error> {
        Ok(Self)
    }
}

impl InputBackend for WindowsBackend {
    fn inject(&mut self, _event: InputEvent) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
