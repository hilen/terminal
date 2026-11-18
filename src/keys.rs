//! What a terminal sends a shell for a key, the codes of xterm that
//! every program reads.

use hilen::ui::{Modifiers, NamedKey};

/// The number xterm puts into a code for the held modifiers, none for
/// a plain key.
fn modifier_code(modifiers: Modifiers) -> Option<u8> {
    let code =
        1 + u8::from(modifiers.shift()) + 2 * u8::from(modifiers.alt()) + 4 * u8::from(modifiers.ctrl());
    (code > 1).then_some(code)
}

/// A key whose code ends in a letter: the arrows, Home and End. A
/// program can ask for the other form of the plain key, `application`.
fn lettered(letter: char, modifiers: Modifiers, application: bool) -> String {
    match modifier_code(modifiers) {
        Some(code) => format!("\x1b[1;{code}{letter}"),
        None if application => format!("\x1bO{letter}"),
        None => format!("\x1b[{letter}"),
    }
}

/// A key whose code is a number and a tilde, like Delete.
fn numbered(number: u8, modifiers: Modifiers) -> String {
    match modifier_code(modifiers) {
        Some(code) => format!("\x1b[{number};{code}~"),
        None => format!("\x1b[{number}~"),
    }
}

/// The bytes for a key with a name, none for a key a shell takes
/// nothing for. `application` says a program asked for the other codes
/// of the arrows.
pub fn named(key: NamedKey, modifiers: Modifiers, application: bool) -> Option<Vec<u8>> {
    let alt = modifiers.alt();
    let code = match key {
        NamedKey::Enter if alt => "\x1b\r".to_string(),
        NamedKey::Enter => "\r".to_string(),
        NamedKey::Tab if modifiers.shift() => "\x1b[Z".to_string(),
        NamedKey::Tab => "\t".to_string(),
        NamedKey::Backspace if modifiers.ctrl() => "\x08".to_string(),
        NamedKey::Backspace if alt => "\x1b\x7f".to_string(),
        NamedKey::Backspace => "\x7f".to_string(),
        NamedKey::Escape => "\x1b".to_string(),
        NamedKey::ArrowUp => lettered('A', modifiers, application),
        NamedKey::ArrowDown => lettered('B', modifiers, application),
        NamedKey::ArrowRight => lettered('C', modifiers, application),
        NamedKey::ArrowLeft => lettered('D', modifiers, application),
        NamedKey::Home => lettered('H', modifiers, application),
        NamedKey::End => lettered('F', modifiers, application),
        NamedKey::Insert => numbered(2, modifiers),
        NamedKey::Delete => numbered(3, modifiers),
        NamedKey::PageUp => numbered(5, modifiers),
        NamedKey::PageDown => numbered(6, modifiers),
        NamedKey::F1 => lettered('P', modifiers, true),
        NamedKey::F2 => lettered('Q', modifiers, true),
        NamedKey::F3 => lettered('R', modifiers, true),
        NamedKey::F4 => lettered('S', modifiers, true),
        NamedKey::F5 => numbered(15, modifiers),
        NamedKey::F6 => numbered(17, modifiers),
        NamedKey::F7 => numbered(18, modifiers),
        NamedKey::F8 => numbered(19, modifiers),
        NamedKey::F9 => numbered(20, modifiers),
        NamedKey::F10 => numbered(21, modifiers),
        NamedKey::F11 => numbered(23, modifiers),
        NamedKey::F12 => numbered(24, modifiers),
        _ => return None,
    };
    Some(code.into_bytes())
}

/// The bytes for a typed char. None for a char that came with Cmd
/// held, that is a command of the app, and for a control char, the key
/// that makes one has a name and comes through `named`.
pub fn typed(char: char, modifiers: Modifiers) -> Option<Vec<u8>> {
    if modifiers.meta() || char.is_control() {
        return None;
    }
    if modifiers.ctrl()
        && let Some(code) = control(char)
    {
        return Some(vec![code]);
    }
    let mut bytes = [0; 4];
    Some(char.encode_utf8(&mut bytes).as_bytes().to_vec())
}

/// The control code Ctrl makes of a char, like 3 of `c`.
fn control(char: char) -> Option<u8> {
    let byte = u8::try_from(char).ok()?;
    match byte {
        b'a'..=b'z' | b'A'..=b'Z' | b'[' | b'\\' | b']' | b'^' | b'_' => Some(byte & 0x1f),
        b'@' | b' ' | b'2' => Some(0),
        b'?' => Some(0x7f),
        _ => None,
    }
}

/// A pasted text as the shell takes it: with its marks around it when
/// the shell asked for them, so a new line in it runs nothing. A new
/// line is sent as the Enter key, the way a terminal types it.
pub fn pasted(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // The end mark inside the text would end the paste early.
        let text = text.replace("\x1b[201~", "");
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use hilen::ui::{Modifiers, NamedKey};

    use super::{named, pasted, typed};

    const NONE: Modifiers = Modifiers::empty();

    #[test]
    fn the_arrows_have_3_forms() {
        assert_eq!(named(NamedKey::ArrowUp, NONE, false).unwrap(), b"\x1b[A");
        assert_eq!(named(NamedKey::ArrowUp, NONE, true).unwrap(), b"\x1bOA");
        let held = Modifiers::CONTROL | Modifiers::SHIFT;
        assert_eq!(named(NamedKey::ArrowLeft, held, true).unwrap(), b"\x1b[1;6D");
    }

    #[test]
    fn the_keys_of_a_shell_line() {
        assert_eq!(named(NamedKey::Enter, NONE, false).unwrap(), b"\r");
        assert_eq!(named(NamedKey::Backspace, NONE, false).unwrap(), b"\x7f");
        assert_eq!(named(NamedKey::Tab, Modifiers::SHIFT, false).unwrap(), b"\x1b[Z");
        assert_eq!(named(NamedKey::Delete, NONE, false).unwrap(), b"\x1b[3~");
        assert_eq!(
            named(NamedKey::PageUp, Modifiers::SHIFT, false).unwrap(),
            b"\x1b[5;2~"
        );
        assert_eq!(named(NamedKey::F5, NONE, false).unwrap(), b"\x1b[15~");
        assert_eq!(named(NamedKey::Shift, NONE, false), None);
    }

    #[test]
    fn ctrl_makes_a_control_code_and_cmd_types_nothing() {
        assert_eq!(typed('c', Modifiers::CONTROL).unwrap(), [3]);
        assert_eq!(typed('W', Modifiers::CONTROL).unwrap(), [0x17]);
        assert_eq!(typed(' ', Modifiers::CONTROL).unwrap(), [0]);
        assert_eq!(typed('1', Modifiers::CONTROL).unwrap(), b"1");
        assert_eq!(typed('ž', NONE).unwrap(), "ž".as_bytes());
        assert_eq!(typed('v', Modifiers::META), None);
        // Enter comes as a named key, its char is not typed again.
        assert_eq!(typed('\r', NONE), None);
    }

    #[test]
    fn a_paste_is_marked_only_when_the_shell_asked() {
        assert_eq!(pasted("a\nb", false), b"a\rb");
        assert_eq!(pasted("a\r\nb", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(pasted("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }
}
