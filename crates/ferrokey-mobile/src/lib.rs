//! Kotlin-facing API for the Android app, exported through UniFFI.
//!
//! The Kotlin bindings are generated from this crate at build time, so the
//! app always speaks the same [`InputEvent`] types as the server.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ferrokey_protocol::{InputEvent, MouseButton};

uniffi::setup_scaffolding!();

#[uniffi::remote(Enum)]
pub enum InputEvent {
    MouseMove { dx: i32, dy: i32 },
    MouseButton { button: MouseButton, pressed: bool },
    Scroll { dx: i32, dy: i32 },
    Key { code: u32, pressed: bool },
}

#[uniffi::remote(Enum)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Connection to a ferrokey server.
///
/// There is no networking yet: events are counted and dropped.
#[derive(uniffi::Object, Default)]
pub struct Client {
    events_sent: AtomicU64,
}

#[uniffi::export]
impl Client {
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    pub fn send(&self, _event: InputEvent) {
        self.events_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Number of events passed to [`Client::send`] so far.
    pub fn events_sent(&self) -> u64 {
        self.events_sent.load(Ordering::Relaxed)
    }
}
