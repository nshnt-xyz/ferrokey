//! Wayland input injection backend.

use ferrokey_protocol::InputEvent;

use crate::{Error, InputBackend};

pub struct WaylandBackend;

impl WaylandBackend {
    pub fn new() -> Result<Self, Error> {
        Ok(Self)
    }
}

impl InputBackend for WaylandBackend {
    fn inject(&mut self, _event: InputEvent) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
