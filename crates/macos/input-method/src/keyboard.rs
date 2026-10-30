//! Mac 键码与 flagsChanged 的纯逻辑；共享逻辑键规则仍由 input-client 决定。
use tigertype_contracts::{InputState, Key};
use tigertype_input_client::{Modifiers, PhysicalKey, ShiftTap, map};

/// 逻辑动作不是 IMK 回调返回值：ShiftTap 即使切换成功，也必须放行原修饰键事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Pass,
    /// 只有后台处理并成功写入宿主后才可吃下原键；故障时撤回自有组合并放行。
    Input(Key),
    ShiftTap,
}

/// 每个 IMK 控制器独占；激活、失焦及故障时 reset，不能跨宿主继承半次 Shift。
#[derive(Debug, Default)]
pub struct Keyboard {
    tap: ShiftTap,
    shift_down: bool,
}
impl Keyboard {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// characters 已由系统应用布局及 Shift；死键、多字符和应用快捷键不连接后台。
    pub fn key_down(
        &mut self,
        code: u16,
        modifiers: Modifiers,
        characters: Option<&str>,
        state: &InputState,
    ) -> KeyAction {
        // 即使本键透传，仍要取消 Shift 单击，避免 Shift+快捷键松手后误切换。
        self.tap = ShiftTap::default();
        self.shift_down = modifiers.shift;
        let text = characters.and_then(|text| {
            let mut chars = text.chars();
            let first = chars.next()?;
            // AppKit 用这段私用区表示 Home/Delete/Fn 等功能键，不能作为候选文字送给引擎。
            (chars.next().is_none() && !('\u{f700}'..='\u{f8ff}').contains(&first)).then_some(first)
        });
        match map(physical(code), modifiers, text, state.composing) {
            Some(key) if state.consumes(key) => KeyAction::Input(key),
            _ => KeyAction::Pass,
        }
    }

    /// Shift 的按下/抬起从聚合标志的边沿识别；双 Shift 重叠或其他修饰键变化一律取消单击。
    pub fn flags_changed(&mut self, code: u16, modifiers: Modifiers) -> KeyAction {
        let shift = matches!(code, 56 | 60);
        let action =
            if shift && !modifiers.shift && self.shift_down && self.tap.released(true, modifiers) {
                KeyAction::ShiftTap
            } else {
                KeyAction::Pass
            };
        self.tap = if shift && modifiers.shift && !self.shift_down {
            ShiftTap::default().press(true, modifiers)
        } else {
            ShiftTap::default()
        };
        self.shift_down = modifiers.shift;
        action
    }
}

/// NSEvent.keyCode 是硬件位置；仅命名键与产品快捷键依赖它，其余字符不假定美式布局。
fn physical(code: u16) -> PhysicalKey {
    match code {
        49 => PhysicalKey::Space,
        36 | 76 => PhysicalKey::Enter,
        51 => PhysicalKey::Backspace,
        53 => PhysicalKey::Escape,
        48 => PhysicalKey::Tab,
        116 => PhysicalKey::PageUp,
        121 => PhysicalKey::PageDown,
        126 => PhysicalKey::Up,
        125 => PhysicalKey::Down,
        123 => PhysicalKey::Left,
        124 => PhysicalKey::Right,
        47 => PhysicalKey::Period,
        37 => PhysicalKey::L,
        _ => PhysicalKey::Other,
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
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..PLAIN
    };
    const CONTROL: Modifiers = Modifiers {
        control: true,
        ..PLAIN
    };

    fn composing() -> InputState {
        InputState {
            composing: true,
            ..Default::default()
        }
    }

    #[test]
    fn hardware_keys_use_shared_semantics_and_local_consumption() {
        let mut keyboard = Keyboard::default();
        for (code, key) in [
            (49, Key::Space),
            (36, Key::Enter),
            (76, Key::Enter),
            (51, Key::Backspace),
            (53, Key::Escape),
            (48, Key::Tab),
            (116, Key::PageUp),
            (121, Key::PageDown),
            (126, Key::Previous),
            (123, Key::Previous),
            (125, Key::Next),
            (124, Key::Next),
        ] {
            assert_eq!(
                keyboard.key_down(code, PLAIN, None, &composing()),
                KeyAction::Input(key)
            );
            assert_eq!(
                keyboard.key_down(code, PLAIN, None, &InputState::default()),
                KeyAction::Pass
            );
        }
        // 同一硬件键按布局产生不同字符；非 BMP 的单个标量不能被当成两个字符。
        for text in ["a", "z", "𠀀"] {
            assert_eq!(
                keyboard.key_down(0, PLAIN, Some(text), &composing()),
                KeyAction::Input(Key::Character(text.chars().next().unwrap()))
            );
        }
        assert_eq!(
            keyboard.key_down(0, SHIFT, Some("A"), &InputState::default()),
            KeyAction::Pass
        );
        let ascii = InputState {
            ascii_mode: true,
            ..Default::default()
        };
        assert_eq!(
            keyboard.key_down(0, PLAIN, Some("a"), &ascii),
            KeyAction::Pass
        );
    }

    #[test]
    fn shortcuts_dead_keys_multiple_characters_and_function_keys_pass_through() {
        let mut keyboard = Keyboard::default();
        for text in [
            None,
            Some(""),
            Some("ab"),
            Some("e\u{301}"),
            Some("\0"),
            Some("\u{f728}"),
        ] {
            assert_eq!(
                keyboard.key_down(0, PLAIN, text, &composing()),
                KeyAction::Pass
            );
        }
        for code in [115, 119, 117, 122] {
            // Home/End/ForwardDelete/F1 在组合中也不能成为可上屏的私用字符。
            assert_eq!(
                keyboard.key_down(code, PLAIN, Some("\u{f700}"), &composing()),
                KeyAction::Pass
            );
        }
        for modifiers in [
            Modifiers { alt: true, ..PLAIN },
            Modifiers {
                alt: true,
                ..CONTROL
            },
        ] {
            for code in [0, 47, 37, 49] {
                assert_eq!(
                    keyboard.key_down(code, modifiers, Some("a"), &composing()),
                    KeyAction::Pass
                );
            }
        }
        assert_eq!(
            keyboard.key_down(47, CONTROL, None, &InputState::default()),
            KeyAction::Input(Key::TogglePunctuation)
        );
        assert_eq!(
            keyboard.key_down(37, CONTROL, None, &composing()),
            KeyAction::Input(Key::ToggleLearning)
        );
        assert_eq!(
            keyboard.key_down(37, CONTROL, None, &InputState::default()),
            KeyAction::Pass
        );
        for code in [0, 49, 51] {
            assert_eq!(
                keyboard.key_down(code, CONTROL, None, &composing()),
                KeyAction::Pass
            );
        }
        for code in [47, 37] {
            assert_eq!(
                keyboard.key_down(
                    code,
                    Modifiers {
                        shift: true,
                        ..CONTROL
                    },
                    None,
                    &composing()
                ),
                KeyAction::Pass
            );
        }
    }

    #[test]
    fn either_shift_toggles_once_but_orphan_release_and_reset_do_not() {
        for code in [56, 60] {
            let mut keyboard = Keyboard::default();
            assert_eq!(keyboard.flags_changed(code, PLAIN), KeyAction::Pass);
            assert_eq!(keyboard.flags_changed(code, SHIFT), KeyAction::Pass);
            assert_eq!(keyboard.flags_changed(code, PLAIN), KeyAction::ShiftTap);
            assert_eq!(keyboard.flags_changed(code, PLAIN), KeyAction::Pass);
            keyboard.flags_changed(code, SHIFT);
            keyboard.reset();
            assert_eq!(keyboard.flags_changed(code, PLAIN), KeyAction::Pass);
        }
    }

    #[test]
    fn overlapping_shifts_other_modifiers_and_even_passthrough_keys_cancel_tap() {
        let mut keyboard = Keyboard::default();
        for (first, second) in [(56, 60), (60, 56)] {
            keyboard.flags_changed(first, SHIFT);
            keyboard.flags_changed(second, SHIFT);
            assert_eq!(keyboard.flags_changed(first, SHIFT), KeyAction::Pass);
            assert_eq!(keyboard.flags_changed(second, PLAIN), KeyAction::Pass);
        }
        for (code, modifiers) in [
            (59, CONTROL),
            (58, Modifiers { alt: true, ..PLAIN }),
            (57, PLAIN),
        ] {
            keyboard.flags_changed(56, SHIFT);
            keyboard.flags_changed(
                code,
                Modifiers {
                    shift: true,
                    ..modifiers
                },
            );
            keyboard.flags_changed(code, SHIFT);
            assert_eq!(keyboard.flags_changed(56, PLAIN), KeyAction::Pass);
        }
        for text in [Some("a"), None, Some("ab")] {
            keyboard.flags_changed(56, SHIFT);
            keyboard.key_down(0, SHIFT, text, &composing());
            assert_eq!(keyboard.flags_changed(56, PLAIN), KeyAction::Pass);
        }
        keyboard.flags_changed(
            56,
            Modifiers {
                shift: true,
                ..CONTROL
            },
        );
        assert_eq!(keyboard.flags_changed(56, PLAIN), KeyAction::Pass);
        keyboard.flags_changed(56, SHIFT);
        assert_eq!(keyboard.flags_changed(56, CONTROL), KeyAction::Pass);
    }
}
