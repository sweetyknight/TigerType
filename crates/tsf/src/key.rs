//! Windows 虚拟键码到 frontend 物理键；逻辑键规则与单击 Shift 判定在 frontend，本模块不读系统状态。
use tigertype_frontend::PhysicalKey;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

pub fn physical(vk: u16) -> PhysicalKey {
    match VIRTUAL_KEY(vk) {
        VK_SPACE => PhysicalKey::Space,
        VK_RETURN => PhysicalKey::Enter,
        VK_BACK => PhysicalKey::Backspace,
        VK_ESCAPE => PhysicalKey::Escape,
        VK_TAB => PhysicalKey::Tab,
        VK_PRIOR => PhysicalKey::PageUp,
        VK_NEXT => PhysicalKey::PageDown,
        VK_UP => PhysicalKey::Up,
        VK_DOWN => PhysicalKey::Down,
        VK_LEFT => PhysicalKey::Left,
        VK_RIGHT => PhysicalKey::Right,
        VK_OEM_PERIOD => PhysicalKey::Period,
        VK_L => PhysicalKey::L,
        _ => PhysicalKey::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::Key;
    use tigertype_frontend::{Modifiers, map};

    #[test]
    fn virtual_keys_reach_the_shared_rules() {
        let plain = Modifiers::default();
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
            assert_eq!(map(physical(vk.0), plain, Some('x'), false), Some(key));
        }
        assert_eq!(
            map(physical(VK_A.0), plain, Some('a'), false),
            Some(Key::Character('a'))
        );
        assert_eq!(map(physical(VK_SHIFT.0), plain, None, false), None);
        let control = Modifiers {
            control: true,
            ..plain
        };
        assert_eq!(
            map(physical(VK_OEM_PERIOD.0), control, None, false),
            Some(Key::TogglePunctuation)
        );
        assert_eq!(
            map(physical(VK_L.0), control, None, true),
            Some(Key::ToggleLearning)
        );
        assert_eq!(map(physical(VK_C.0), control, Some('\u{3}'), true), None);
    }
}
