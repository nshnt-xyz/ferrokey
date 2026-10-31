//! Turning text and named keys into [`InputEvent::Key`] sequences.
//!
//! Key events carry Linux evdev keycodes, which name physical key positions,
//! not characters. The server's keyboard layout decides what a position
//! types, so [`char_key`] assumes the computer uses a US QWERTY layout.

use crate::InputEvent;

// Linux evdev keycodes, from linux/input-event-codes.h.
pub const KEY_ESC: u32 = 1;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_TAB: u32 = 15;
pub const KEY_ENTER: u32 = 28;
pub const KEY_LEFTCTRL: u32 = 29;
pub const KEY_LEFTSHIFT: u32 = 42;
pub const KEY_LEFTALT: u32 = 56;
pub const KEY_SPACE: u32 = 57;
pub const KEY_F1: u32 = 59;
pub const KEY_F11: u32 = 87;
pub const KEY_F12: u32 = 88;
pub const KEY_HOME: u32 = 102;
pub const KEY_UP: u32 = 103;
pub const KEY_PAGEUP: u32 = 104;
pub const KEY_LEFT: u32 = 105;
pub const KEY_RIGHT: u32 = 106;
pub const KEY_END: u32 = 107;
pub const KEY_DOWN: u32 = 108;
pub const KEY_PAGEDOWN: u32 = 109;
pub const KEY_INSERT: u32 = 110;
pub const KEY_DELETE: u32 = 111;
pub const KEY_LEFTMETA: u32 = 125;

/// Keys that don't type a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecialKey {
    Escape,
    Tab,
    Backspace,
    Enter,
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl SpecialKey {
    pub fn code(self) -> u32 {
        match self {
            SpecialKey::Escape => KEY_ESC,
            SpecialKey::Tab => KEY_TAB,
            SpecialKey::Backspace => KEY_BACKSPACE,
            SpecialKey::Enter => KEY_ENTER,
            SpecialKey::Insert => KEY_INSERT,
            SpecialKey::Delete => KEY_DELETE,
            SpecialKey::Home => KEY_HOME,
            SpecialKey::End => KEY_END,
            SpecialKey::PageUp => KEY_PAGEUP,
            SpecialKey::PageDown => KEY_PAGEDOWN,
            SpecialKey::Up => KEY_UP,
            SpecialKey::Down => KEY_DOWN,
            SpecialKey::Left => KEY_LEFT,
            SpecialKey::Right => KEY_RIGHT,
            // F1-F10 are consecutive; F11 and F12 come much later.
            SpecialKey::F1 => KEY_F1,
            SpecialKey::F2 => KEY_F1 + 1,
            SpecialKey::F3 => KEY_F1 + 2,
            SpecialKey::F4 => KEY_F1 + 3,
            SpecialKey::F5 => KEY_F1 + 4,
            SpecialKey::F6 => KEY_F1 + 5,
            SpecialKey::F7 => KEY_F1 + 6,
            SpecialKey::F8 => KEY_F1 + 7,
            SpecialKey::F9 => KEY_F1 + 8,
            SpecialKey::F10 => KEY_F1 + 9,
            SpecialKey::F11 => KEY_F11,
            SpecialKey::F12 => KEY_F12,
        }
    }
}

/// Modifier keys to hold down around a key press.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Super / Windows key.
    pub meta: bool,
}

impl Modifiers {
    fn codes(self) -> impl DoubleEndedIterator<Item = u32> {
        [
            (self.ctrl, KEY_LEFTCTRL),
            (self.alt, KEY_LEFTALT),
            (self.shift, KEY_LEFTSHIFT),
            (self.meta, KEY_LEFTMETA),
        ]
        .into_iter()
        .filter_map(|(held, code)| held.then_some(code))
    }
}

/// The key that types `c` on a US QWERTY layout, and whether it needs Shift.
pub fn char_key(c: char) -> Option<(u32, bool)> {
    const LETTERS: [u32; 26] = [
        30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, // a-m
        49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45, 21, 44, // n-z
    ];
    let key = match c {
        'a'..='z' => (LETTERS[c as usize - 'a' as usize], false),
        'A'..='Z' => (LETTERS[c as usize - 'A' as usize], true),
        // KEY_1 is 2 through KEY_9 at 10, then KEY_0 at 11.
        '1'..='9' => (c as u32 - '1' as u32 + 2, false),
        '0' => (11, false),
        '!' => (2, true),
        '@' => (3, true),
        '#' => (4, true),
        '$' => (5, true),
        '%' => (6, true),
        '^' => (7, true),
        '&' => (8, true),
        '*' => (9, true),
        '(' => (10, true),
        ')' => (11, true),
        '-' => (12, false),
        '_' => (12, true),
        '=' => (13, false),
        '+' => (13, true),
        '[' => (26, false),
        '{' => (26, true),
        ']' => (27, false),
        '}' => (27, true),
        ';' => (39, false),
        ':' => (39, true),
        '\'' => (40, false),
        '"' => (40, true),
        '`' => (41, false),
        '~' => (41, true),
        '\\' => (43, false),
        '|' => (43, true),
        ',' => (51, false),
        '<' => (51, true),
        '.' => (52, false),
        '>' => (52, true),
        '/' => (53, false),
        '?' => (53, true),
        ' ' => (KEY_SPACE, false),
        '\t' => (KEY_TAB, false),
        '\n' => (KEY_ENTER, false),
        _ => return None,
    };
    Some(key)
}

/// Appends the events that tap `code` while holding `modifiers`.
pub fn tap(code: u32, modifiers: Modifiers, out: &mut Vec<InputEvent>) {
    out.extend(modifiers.codes().map(|code| InputEvent::Key {
        code,
        pressed: true,
    }));
    out.push(InputEvent::Key {
        code,
        pressed: true,
    });
    out.push(InputEvent::Key {
        code,
        pressed: false,
    });
    out.extend(modifiers.codes().rev().map(|code| InputEvent::Key {
        code,
        pressed: false,
    }));
}

/// Appends the events that type `text` while holding `modifiers`, and
/// returns how many characters were skipped because no key types them.
pub fn type_text(text: &str, modifiers: Modifiers, out: &mut Vec<InputEvent>) -> usize {
    let mut skipped = 0;
    for c in text.chars() {
        match char_key(c) {
            Some((code, shift)) => {
                let modifiers = Modifiers {
                    shift: modifiers.shift || shift,
                    ..modifiers
                };
                tap(code, modifiers, out);
            }
            None => skipped += 1,
        }
    }
    skipped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: u32, pressed: bool) -> InputEvent {
        InputEvent::Key { code, pressed }
    }

    #[test]
    fn maps_letters_digits_and_symbols() {
        assert_eq!(char_key('a'), Some((30, false)));
        assert_eq!(char_key('Z'), Some((44, true)));
        assert_eq!(char_key('1'), Some((2, false)));
        assert_eq!(char_key('9'), Some((10, false)));
        assert_eq!(char_key('0'), Some((11, false)));
        assert_eq!(char_key('?'), Some((53, true)));
        assert_eq!(char_key('\n'), Some((KEY_ENTER, false)));
        assert_eq!(char_key('é'), None);
        assert_eq!(SpecialKey::F10.code(), 68);
    }

    #[test]
    fn types_with_shift_and_modifiers() {
        let mut out = Vec::new();
        assert_eq!(type_text("hI", Modifiers::default(), &mut out), 0);
        assert_eq!(
            out,
            [
                key(35, true),
                key(35, false),
                key(KEY_LEFTSHIFT, true),
                key(23, true),
                key(23, false),
                key(KEY_LEFTSHIFT, false),
            ]
        );

        out.clear();
        let ctrl_shift = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::default()
        };
        assert_eq!(type_text("t€", ctrl_shift, &mut out), 1);
        assert_eq!(
            out,
            [
                key(KEY_LEFTCTRL, true),
                key(KEY_LEFTSHIFT, true),
                key(20, true),
                key(20, false),
                key(KEY_LEFTSHIFT, false),
                key(KEY_LEFTCTRL, false),
            ]
        );
    }
}
