//! 物理按键到逻辑按键的纯映射；字符由调用方按当前键盘布局取得，本模块不读系统状态。
//! 是否吃键另由 InputState::consumes 按上次状态判定，这里只决定“是哪个逻辑键”。
use tigertype_contracts::Key;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
}

/// Alt/AltGr 组合一律交还系统；Ctrl 只认产品快捷键，其余透传给应用。
/// `text` 是当前布局在不改变键盘状态时得到的单个字符，死键或多字符为 None。
pub fn map(vk: u16, modifiers: Modifiers, text: Option<char>, composing: bool) -> Option<Key> {
    if modifiers.alt {
        return None;
    }
    let vk = VIRTUAL_KEY(vk);
    if modifiers.control {
        return match vk {
            VK_OEM_PERIOD if !modifiers.shift => Some(Key::TogglePunctuation),
            // 学习信息只在组合中切换，不占用应用自身的 Ctrl+L。
            VK_L if composing && !modifiers.shift => Some(Key::ToggleLearning),
            _ => None,
        };
    }
    Some(match vk {
        VK_SPACE => Key::Space,
        VK_RETURN => Key::Enter,
        VK_BACK => Key::Backspace,
        VK_ESCAPE => Key::Escape,
        VK_TAB => Key::Tab,
        VK_PRIOR => Key::PageUp,
        VK_NEXT => Key::PageDown,
        // 竖排候选以上下移动高亮；左右同样移动，避免组合中把光标移出自有范围。
        VK_UP | VK_LEFT => Key::Previous,
        VK_DOWN | VK_RIGHT => Key::Next,
        _ => match text {
            Some(c) if !c.is_control() => Key::Character(c),
            _ => return None,
        },
    })
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
    fn named_keys_characters_and_shortcuts_map_without_reading_system_state() {
        for (vk, key) in [
            (VK_SPACE, Key::Space),
            (VK_RETURN, Key::Enter),
            (VK_BACK, Key::Backspace),
            (VK_ESCAPE, Key::Escape),
            (VK_TAB, Key::Tab),
            (VK_PRIOR, Key::PageUp),
            (VK_NEXT, Key::PageDown),
            (VK_UP, Key::Previous),
            (VK_LEFT, Key::Previous),
            (VK_DOWN, Key::Next),
            (VK_RIGHT, Key::Next),
        ] {
            // 命名键不受布局字符影响，Shift 不改变其含义。
            assert_eq!(map(vk.0, PLAIN, Some('x'), false), Some(key));
            let shifted = Modifiers {
                shift: true,
                ..PLAIN
            };
            assert_eq!(map(vk.0, shifted, None, true), Some(key));
        }
        assert_eq!(
            map(VK_A.0, PLAIN, Some('a'), false),
            Some(Key::Character('a'))
        );
        assert_eq!(
            map(VK_OEM_7.0, PLAIN, Some('"'), false),
            Some(Key::Character('"'))
        );
        // 死键、多字符和控制字符都不交给引擎。
        assert_eq!(map(VK_OEM_7.0, PLAIN, None, false), None);
        assert_eq!(map(VK_A.0, PLAIN, Some('\u{1}'), false), None);
        assert_eq!(map(VK_SHIFT.0, PLAIN, None, false), None);
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
            assert_eq!(map(VK_A.0, modifiers, Some('a'), true), None);
            assert_eq!(map(VK_SPACE.0, modifiers, None, true), None);
            assert_eq!(map(VK_OEM_PERIOD.0, modifiers, Some('.'), true), None);
        }
        assert_eq!(
            map(VK_OEM_PERIOD.0, CONTROL, None, false),
            Some(Key::TogglePunctuation)
        );
        let control_shift = Modifiers {
            shift: true,
            ..CONTROL
        };
        assert_eq!(map(VK_OEM_PERIOD.0, control_shift, None, false), None);
        assert_eq!(map(VK_L.0, CONTROL, None, false), None);
        assert_eq!(map(VK_L.0, CONTROL, None, true), Some(Key::ToggleLearning));
        // Ctrl+字母/空格/退格等应用快捷键原样放行。
        for vk in [VK_A, VK_C, VK_V, VK_SPACE, VK_BACK, VK_RETURN] {
            assert_eq!(map(vk.0, CONTROL, Some('\u{3}'), true), None);
        }
    }
}
