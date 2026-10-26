//! 物理键到逻辑键的规则与单击 Shift 判定；各平台只把系统键码换成 `PhysicalKey`、读出修饰键与布局字符。
//! 是否吃键另由 InputState::consumes 按上次状态判定，这里只决定“是哪个逻辑键”。
use tigertype_contracts::Key;

/// 规则需要区分的物理键；其余键只看当前布局给出的字符。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalKey {
    Space,
    Enter,
    Backspace,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    /// 句点键：带 Ctrl 时切换中英标点。
    Period,
    /// L 键：组合中带 Ctrl 时切换学习信息。
    L,
    Other,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    /// Windows 的 Alt/AltGr；macOS 的 ⌥ 或 ⌘。带上即一律放行。
    pub alt: bool,
}

/// Alt 类组合一律交还系统；Ctrl 只认产品快捷键，其余透传给应用。
/// `text` 是当前布局在不改变键盘状态时得到的单个字符，死键或多字符为 None。
pub fn map(
    key: PhysicalKey,
    modifiers: Modifiers,
    text: Option<char>,
    composing: bool,
) -> Option<Key> {
    if modifiers.alt {
        return None;
    }
    if modifiers.control {
        return match key {
            PhysicalKey::Period if !modifiers.shift => Some(Key::TogglePunctuation),
            // 学习信息只在组合中切换，不占用应用自身的 Ctrl+L。
            PhysicalKey::L if composing && !modifiers.shift => Some(Key::ToggleLearning),
            _ => None,
        };
    }
    Some(match key {
        PhysicalKey::Space => Key::Space,
        PhysicalKey::Enter => Key::Enter,
        PhysicalKey::Backspace => Key::Backspace,
        PhysicalKey::Escape => Key::Escape,
        PhysicalKey::Tab => Key::Tab,
        PhysicalKey::PageUp => Key::PageUp,
        PhysicalKey::PageDown => Key::PageDown,
        // 竖排候选以上下移动高亮；左右同样移动，避免组合中把光标移出自有范围。
        PhysicalKey::Up | PhysicalKey::Left => Key::Previous,
        PhysicalKey::Down | PhysicalKey::Right => Key::Next,
        PhysicalKey::Period | PhysicalKey::L | PhysicalKey::Other => match text {
            Some(c) if !c.is_control() => Key::Character(c),
            _ => return None,
        },
    })
}

/// 单击 Shift 切换中英：Shift 按下时无 Ctrl/Alt 即开始判定，期间按下其他键即作废，抬起时仍无 Ctrl/Alt 才算单击。
#[derive(Debug, Clone, Copy, Default)]
pub struct ShiftTap {
    armed: bool,
}
impl ShiftTap {
    /// 任一键按下（含 Shift 自身）后的判定状态。
    #[must_use]
    pub fn press(self, shift: bool, modifiers: Modifiers) -> Self {
        Self {
            armed: shift && !modifiers.control && !modifiers.alt,
        }
    }
    /// 抬起的键是否构成单击 Shift；预判与处理共用，不改变判定状态。
    pub fn released(self, shift: bool, modifiers: Modifiers) -> bool {
        shift && self.armed && !modifiers.control && !modifiers.alt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PLAIN: Modifiers = Modifiers {
        shift: false,
        control: false,
        alt: false,
    };
    const CONTROL: Modifiers = Modifiers {
        shift: false,
        control: true,
        alt: false,
    };

    #[test]
    fn named_keys_and_characters_map_without_reading_system_state() {
        for (physical, key) in [
            (PhysicalKey::Space, Key::Space),
            (PhysicalKey::Enter, Key::Enter),
            (PhysicalKey::Backspace, Key::Backspace),
            (PhysicalKey::Escape, Key::Escape),
            (PhysicalKey::Tab, Key::Tab),
            (PhysicalKey::PageUp, Key::PageUp),
            (PhysicalKey::PageDown, Key::PageDown),
            (PhysicalKey::Up, Key::Previous),
            (PhysicalKey::Left, Key::Previous),
            (PhysicalKey::Down, Key::Next),
            (PhysicalKey::Right, Key::Next),
        ] {
            // 命名键不受布局字符影响，Shift 不改变其含义。
            assert_eq!(map(physical, PLAIN, Some('x'), false), Some(key));
            let shifted = Modifiers {
                shift: true,
                ..PLAIN
            };
            assert_eq!(map(physical, shifted, None, true), Some(key));
        }
        assert_eq!(
            map(PhysicalKey::Other, PLAIN, Some('a'), false),
            Some(Key::Character('a'))
        );
        assert_eq!(
            map(PhysicalKey::Period, PLAIN, Some('.'), false),
            Some(Key::Character('.'))
        );
        assert_eq!(
            map(PhysicalKey::L, PLAIN, Some('l'), true),
            Some(Key::Character('l'))
        );
        // 死键、多字符和控制字符都不交给引擎。
        assert_eq!(map(PhysicalKey::Other, PLAIN, None, false), None);
        assert_eq!(map(PhysicalKey::Other, PLAIN, Some('\u{1}'), false), None);
    }

    #[test]
    fn modifier_combinations_pass_through_except_product_shortcuts() {
        let alt = Modifiers { alt: true, ..PLAIN };
        let altgr = Modifiers {
            alt: true,
            control: true,
            shift: false,
        };
        for modifiers in [alt, altgr] {
            assert_eq!(map(PhysicalKey::Other, modifiers, Some('a'), true), None);
            assert_eq!(map(PhysicalKey::Space, modifiers, None, true), None);
            assert_eq!(map(PhysicalKey::Period, modifiers, Some('.'), true), None);
        }
        assert_eq!(
            map(PhysicalKey::Period, CONTROL, None, false),
            Some(Key::TogglePunctuation)
        );
        let control_shift = Modifiers {
            shift: true,
            ..CONTROL
        };
        assert_eq!(map(PhysicalKey::Period, control_shift, None, false), None);
        assert_eq!(map(PhysicalKey::L, CONTROL, None, false), None);
        assert_eq!(
            map(PhysicalKey::L, CONTROL, None, true),
            Some(Key::ToggleLearning)
        );
        // Ctrl+字母/空格/退格等应用快捷键原样放行。
        for physical in [
            PhysicalKey::Other,
            PhysicalKey::Space,
            PhysicalKey::Backspace,
            PhysicalKey::Enter,
        ] {
            assert_eq!(map(physical, CONTROL, Some('\u{3}'), true), None);
        }
    }

    #[test]
    fn a_shift_tap_needs_no_other_key_and_no_control_or_alt() {
        let shift = Modifiers {
            shift: true,
            ..PLAIN
        };
        let tap = ShiftTap::default().press(true, shift);
        assert!(tap.released(true, PLAIN));
        // 预判与处理各问一次，结果相同。
        assert!(tap.released(true, PLAIN));
        // 抬起的不是 Shift、期间按了别的键、或按下/抬起时带 Ctrl/Alt，都不算单击。
        assert!(!tap.released(false, PLAIN));
        assert!(!tap.press(false, shift).released(true, PLAIN));
        assert!(!tap.released(true, CONTROL));
        assert!(
            !ShiftTap::default()
                .press(true, CONTROL)
                .released(true, PLAIN)
        );
        let alt = Modifiers { alt: true, ..PLAIN };
        assert!(!ShiftTap::default().press(true, alt).released(true, PLAIN));
        assert!(!ShiftTap::default().released(true, PLAIN));
    }
}
