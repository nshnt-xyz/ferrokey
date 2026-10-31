//! Wire types shared by the ferrokey client and server.

pub mod keys;

/// TCP port the server listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 47800;

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
    /// A field holds a value outside its valid range.
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

impl InputEvent {
    /// Upper bound on the size of one encoded event.
    pub const MAX_ENCODED_LEN: usize = 9;

    /// Appends the binary encoding of this event to `out`.
    ///
    /// Each event is a one-byte tag followed by fixed-size big-endian fields,
    /// so several events can be concatenated into one message.
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

    fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(i32::from_be_bytes(self.take()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVENTS: [InputEvent; 6] = [
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
    }
}
