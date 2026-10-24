//! Wire types shared by the ferrokey client and server.

/// A single input event sent from the client to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Relative pointer motion.
    MouseMove { dx: i32, dy: i32 },
    /// Mouse button press or release.
    MouseButton { button: MouseButton, pressed: bool },
    /// Scroll wheel motion, in detents.
    Scroll { dx: i32, dy: i32 },
    /// Key press or release, identified by a Linux evdev keycode.
    Key { code: u32, pressed: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}
