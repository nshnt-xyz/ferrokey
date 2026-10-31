//! Wire types shared by the ferrokey client and server.

pub mod keys;

/// TCP port the server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47800;

/// A single input event sent from the client to the server.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    /// Relative pointer motion.
    MouseMove { dx: i32, dy: i32 },
    /// Mouse button press or release.
    MouseButton { button: MouseButton, pressed: bool },
    /// Scroll wheel motion, in detents.
    Scroll { dx: i32, dy: i32 },
    /// Key press or release, identified by a Linux evdev keycode.
    Key { code: u32, pressed: bool },
    /// Moves the pointer to a point on the shared screen, given as fractions
    /// of its width and height: 0 is the left or top edge and
    /// [`InputEvent::ABSOLUTE_MAX`] the right or bottom one. Only works
    /// while the server shares its screen.
    MouseMoveAbsolute { x: u16, y: u16 },
    /// Key press or release, identified by an XKB keysym. The computer finds
    /// the key that types it on its own layout; see [`keys::char_keysym`].
    Keysym { keysym: u32, pressed: bool },
    /// Touchpad-style scrolling, in pixels. `finish` marks the end of a
    /// scroll gesture (the fingers lifted), which lets the computer start
    /// kinetic scrolling; it can come with zero motion.
    SmoothScroll { dx: f32, dy: f32, finish: bool },
    /// A finger touched the shared screen, at a point given like
    /// [`InputEvent::MouseMoveAbsolute`]'s. `id` tells fingers apart while
    /// they're down and can be reused once lifted.
    TouchDown { id: u32, x: u16, y: u16 },
    /// A finger that is down moved.
    TouchMove { id: u32, x: u16, y: u16 },
    /// A finger lifted.
    TouchUp { id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ends partway through an event.
    Truncated,
    /// The event tag is not one this version understands.
    UnknownTag(u8),
    /// A field holds a value outside its valid range, such as a scroll
    /// distance that is NaN or infinite.
    InvalidValue,
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => f.write_str("truncated input event"),
            DecodeError::UnknownTag(tag) => write!(f, "unknown input event tag {tag}"),
            DecodeError::InvalidValue => f.write_str("invalid field in input event"),
        }
    }
}

impl std::error::Error for DecodeError {}

const TAG_MOUSE_MOVE: u8 = 1;
const TAG_MOUSE_BUTTON: u8 = 2;
const TAG_SCROLL: u8 = 3;
const TAG_KEY: u8 = 4;
const TAG_MOUSE_MOVE_ABSOLUTE: u8 = 5;
const TAG_KEYSYM: u8 = 6;
const TAG_SMOOTH_SCROLL: u8 = 7;
const TAG_TOUCH_DOWN: u8 = 8;
const TAG_TOUCH_MOVE: u8 = 9;
const TAG_TOUCH_UP: u8 = 10;

impl InputEvent {
    /// Upper bound on the size of one encoded event.
    pub const MAX_ENCODED_LEN: usize = 10;

    /// The largest coordinate of [`InputEvent::MouseMoveAbsolute`] and the
    /// touch events.
    pub const ABSOLUTE_MAX: u16 = u16::MAX;

    /// Appends the binary encoding of this event to `out`.
    ///
    /// Each event is a one-byte tag followed by fixed-size big-endian fields
    /// (`f32`s as their IEEE 754 bits), so several events can be
    /// concatenated into one message.
    pub fn encode(&self, out: &mut Vec<u8>) {
        match *self {
            InputEvent::MouseMove { dx, dy } => {
                out.push(TAG_MOUSE_MOVE);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            InputEvent::MouseButton { button, pressed } => {
                out.push(TAG_MOUSE_BUTTON);
                out.push(match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                });
                out.push(pressed.into());
            }
            InputEvent::Scroll { dx, dy } => {
                out.push(TAG_SCROLL);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
            }
            InputEvent::Key { code, pressed } => {
                out.push(TAG_KEY);
                out.extend_from_slice(&code.to_be_bytes());
                out.push(pressed.into());
            }
            InputEvent::MouseMoveAbsolute { x, y } => {
                out.push(TAG_MOUSE_MOVE_ABSOLUTE);
                out.extend_from_slice(&x.to_be_bytes());
                out.extend_from_slice(&y.to_be_bytes());
            }
            InputEvent::Keysym { keysym, pressed } => {
                out.push(TAG_KEYSYM);
                out.extend_from_slice(&keysym.to_be_bytes());
                out.push(pressed.into());
            }
            InputEvent::SmoothScroll { dx, dy, finish } => {
                out.push(TAG_SMOOTH_SCROLL);
                out.extend_from_slice(&dx.to_be_bytes());
                out.extend_from_slice(&dy.to_be_bytes());
                out.push(finish.into());
            }
            InputEvent::TouchDown { id, x, y } | InputEvent::TouchMove { id, x, y } => {
                out.push(if matches!(self, InputEvent::TouchDown { .. }) {
                    TAG_TOUCH_DOWN
                } else {
                    TAG_TOUCH_MOVE
                });
                out.extend_from_slice(&id.to_be_bytes());
                out.extend_from_slice(&x.to_be_bytes());
                out.extend_from_slice(&y.to_be_bytes());
            }
            InputEvent::TouchUp { id } => {
                out.push(TAG_TOUCH_UP);
                out.extend_from_slice(&id.to_be_bytes());
            }
        }
    }

    /// Decodes one event from the front of `buf`, returning it and the
    /// number of bytes it took up.
    pub fn decode(buf: &[u8]) -> Result<(Self, usize), DecodeError> {
        let (&tag, rest) = buf.split_first().ok_or(DecodeError::Truncated)?;
        let mut reader = Reader(rest);
        let event = match tag {
            TAG_MOUSE_MOVE => InputEvent::MouseMove {
                dx: reader.i32()?,
                dy: reader.i32()?,
            },
            TAG_MOUSE_BUTTON => {
                let button = match reader.u8()? {
                    0 => MouseButton::Left,
                    1 => MouseButton::Right,
                    2 => MouseButton::Middle,
                    _ => return Err(DecodeError::InvalidValue),
                };
                InputEvent::MouseButton {
                    button,
                    pressed: reader.bool()?,
                }
            }
            TAG_SCROLL => InputEvent::Scroll {
                dx: reader.i32()?,
                dy: reader.i32()?,
            },
            TAG_KEY => InputEvent::Key {
                code: reader.u32()?,
                pressed: reader.bool()?,
            },
            TAG_MOUSE_MOVE_ABSOLUTE => InputEvent::MouseMoveAbsolute {
                x: reader.u16()?,
                y: reader.u16()?,
            },
            TAG_KEYSYM => InputEvent::Keysym {
                keysym: reader.u32()?,
                pressed: reader.bool()?,
            },
            TAG_SMOOTH_SCROLL => InputEvent::SmoothScroll {
                dx: reader.f32()?,
                dy: reader.f32()?,
                finish: reader.bool()?,
            },
            TAG_TOUCH_DOWN => InputEvent::TouchDown {
                id: reader.u32()?,
                x: reader.u16()?,
                y: reader.u16()?,
            },
            TAG_TOUCH_MOVE => InputEvent::TouchMove {
                id: reader.u32()?,
                x: reader.u16()?,
                y: reader.u16()?,
            },
            TAG_TOUCH_UP => InputEvent::TouchUp { id: reader.u32()? },
            tag => return Err(DecodeError::UnknownTag(tag)),
        };
        Ok((event, buf.len() - reader.0.len()))
    }

    /// Decodes every event in `buf`, which must contain only whole events.
    pub fn decode_all(mut buf: &[u8]) -> impl Iterator<Item = Result<Self, DecodeError>> + '_ {
        std::iter::from_fn(move || {
            if buf.is_empty() {
                return None;
            }
            match Self::decode(buf) {
                Ok((event, len)) => {
                    buf = &buf[len..];
                    Some(Ok(event))
                }
                Err(err) => {
                    buf = &[];
                    Some(Err(err))
                }
            }
        })
    }
}

struct Reader<'a>(&'a [u8]);

impl Reader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let (bytes, rest) = self.0.split_first_chunk().ok_or(DecodeError::Truncated)?;
        self.0 = rest;
        Ok(*bytes)
    }

    fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take::<1>()?[0])
    }

    fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(DecodeError::InvalidValue),
        }
    }

    fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.take()?))
    }

    /// Reads a finite `f32`; NaN and infinities are rejected.
    fn f32(&mut self) -> Result<f32, DecodeError> {
        let value = f32::from_be_bytes(self.take()?);
        if value.is_finite() {
            Ok(value)
        } else {
            Err(DecodeError::InvalidValue)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: [InputEvent; 13] = [
        InputEvent::MouseMove {
            dx: -5,
            dy: i32::MAX,
        },
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
        InputEvent::MouseButton {
            button: MouseButton::Middle,
            pressed: false,
        },
        InputEvent::Scroll { dx: 0, dy: -3 },
        InputEvent::Key {
            code: 28,
            pressed: true,
        },
        InputEvent::Key {
            code: u32::MAX,
            pressed: false,
        },
        InputEvent::MouseMoveAbsolute {
            x: 0,
            y: InputEvent::ABSOLUTE_MAX,
        },
        InputEvent::Keysym {
            keysym: 0x0100_20ac,
            pressed: true,
        },
        InputEvent::SmoothScroll {
            dx: -0.5,
            dy: 12.25,
            finish: false,
        },
        InputEvent::SmoothScroll {
            dx: 0.0,
            dy: 0.0,
            finish: true,
        },
        InputEvent::TouchDown {
            id: 3,
            x: 0,
            y: 32768,
        },
        InputEvent::TouchMove {
            id: 3,
            x: InputEvent::ABSOLUTE_MAX,
            y: 1,
        },
        InputEvent::TouchUp { id: u32::MAX },
    ];

    #[test]
    fn round_trips_concatenated_events() {
        let mut buf = Vec::new();
        for event in EVENTS {
            let before = buf.len();
            event.encode(&mut buf);
            assert!(buf.len() - before <= InputEvent::MAX_ENCODED_LEN);
        }
        let decoded: Vec<_> = InputEvent::decode_all(&buf)
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(decoded, EVENTS);
    }

    #[test]
    fn rejects_malformed_input() {
        assert_eq!(InputEvent::decode(&[]), Err(DecodeError::Truncated));
        assert_eq!(
            InputEvent::decode(&[TAG_MOUSE_MOVE, 0, 0]),
            Err(DecodeError::Truncated)
        );
        assert_eq!(InputEvent::decode(&[99]), Err(DecodeError::UnknownTag(99)));
        assert_eq!(
            InputEvent::decode(&[TAG_MOUSE_BUTTON, 7, 1]),
            Err(DecodeError::InvalidValue)
        );
        assert_eq!(
            InputEvent::decode(&[TAG_KEY, 0, 0, 0, 1, 2]),
            Err(DecodeError::InvalidValue)
        );
        let mut nan = vec![TAG_SMOOTH_SCROLL];
        nan.extend_from_slice(&f32::NAN.to_be_bytes());
        nan.extend_from_slice(&0.5f32.to_be_bytes());
        nan.push(0);
        assert_eq!(InputEvent::decode(&nan), Err(DecodeError::InvalidValue));
        assert_eq!(
            InputEvent::decode(&[TAG_TOUCH_UP, 0, 0]),
            Err(DecodeError::Truncated)
        );
    }
}
