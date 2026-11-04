//! IBus 与 XKB 共用键符号；不读键盘配置，不把 Ctrl/Alt/Super 的应用快捷键交给引擎。
use tigertype_contracts::Key;
use tigertype_input_client::{Modifiers, PhysicalKey, ShiftTap, map};

#[derive(Default)]
pub struct Keyboard(ShiftTap);
impl Keyboard {
    pub fn reset(&mut self) {
        self.0 = ShiftTap::default();
    }
    pub fn key(&mut self, symbol: u32, state: u32, composing: bool) -> Option<Key> {
        // 已由框架标为转发的事件不能再次切换状态，包括带转发标记的 Shift 抬键。
        if state & (1 << 25) != 0 {
            self.reset();
            return None;
        }
        let modifiers = Modifiers {
            shift: state & 1 != 0,
            control: state & (1 << 2) != 0,
            // Mod5 包含 AltGr；Super/Hyper/Meta 不能误当普通汉字输入。
            alt: state & ((1 << 3) | (1 << 6) | (1 << 7) | (7 << 26)) != 0,
        };
        let shift = matches!(symbol, 0xffe1 | 0xffe2);
        if state & (1 << 30) != 0 {
            let toggle = self.0.released(shift, modifiers);
            if shift {
                self.reset();
            }
            return toggle.then_some(Key::ToggleAscii);
        }
        self.0 = self.0.press(shift, modifiers);
        if shift {
            return None;
        }
        let physical = match symbol {
            0x20 | 0xff80 => PhysicalKey::Space,
            0xff0d | 0xff8d => PhysicalKey::Enter,
            0xff08 => PhysicalKey::Backspace,
            0xff1b => PhysicalKey::Escape,
            0xff09 | 0xfe20 => PhysicalKey::Tab,
            0xff55 | 0xff9a => PhysicalKey::PageUp,
            0xff56 | 0xff9b => PhysicalKey::PageDown,
            0xff52 | 0xff97 => PhysicalKey::Up,
            0xff54 | 0xff99 => PhysicalKey::Down,
            0xff51 | 0xff96 => PhysicalKey::Left,
            0xff53 | 0xff98 => PhysicalKey::Right,
            0x2e => PhysicalKey::Period,
            0x6c | 0x4c => PhysicalKey::L,
            _ => PhysicalKey::Other,
        };
        let text = match symbol {
            0x20..=0xff => char::from_u32(symbol),
            0xffb0..=0xffb9 => char::from_u32(symbol - 0xffb0 + u32::from(b'0')),
            0x01000000..=0x0110ffff => char::from_u32(symbol & 0x00ffffff),
            _ => None,
        };
        map(physical, modifiers, text, composing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shortcuts_releases_shift_taps_and_unicode_do_not_leak() {
        let mut keyboard = Keyboard::default();
        assert_eq!(
            keyboard.key(u32::from(b'a'), 0, false),
            Some(Key::Character('a'))
        );
        assert_eq!(
            keyboard.key(0x01020000, 0, true),
            Some(Key::Character('𠀀'))
        );
        for mask in [1 << 2, 1 << 3, 1 << 6, 1 << 7, 1 << 26, 1 << 30] {
            assert_eq!(keyboard.key(u32::from(b'a'), mask, true), None);
        }
        assert_eq!(keyboard.key(0x20, 1, false), Some(Key::ToggleFullShape));
        assert_eq!(
            keyboard.key(0x2e, 1 << 2, false),
            Some(Key::TogglePunctuation)
        );
        assert_eq!(keyboard.key(0x6c, 1 << 2, false), None);
        assert_eq!(keyboard.key(0x6c, 1 << 2, true), Some(Key::ToggleLearning));
        assert_eq!(keyboard.key(0xffe1, 0, false), None);
        assert_eq!(keyboard.key(0xffe1, 1 << 30, false), Some(Key::ToggleAscii));
        assert_eq!(keyboard.key(0xffe1, 1 << 30, false), None);
        keyboard.key(0xffe1, 0, false);
        keyboard.key(0x41, 1, false);
        assert_eq!(keyboard.key(0xffe1, 1 << 30, false), None);
        keyboard.key(0xffe1, 0, false);
        keyboard.reset();
        assert_eq!(keyboard.key(0xffe1, 1 << 30, false), None);
        assert_eq!(keyboard.key(0x0100d800, 0, false), None);
        keyboard.key(0xffe1, 0, false);
        assert_eq!(keyboard.key(0xffe1, (1 << 25) | (1 << 30), false), None);
    }
}
