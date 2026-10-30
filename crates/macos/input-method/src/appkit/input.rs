//! 只读取事件、构造标记文本；IMK 回调负责期限、后台往返和实际写入，不在这里假装上屏成功。
use crate::{KeyAction, Keyboard, marked_selection};
use objc2::{AnyThread, rc::Retained};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSEventType, NSUnderlineStyle, NSUnderlineStyleAttributeName,
};
use objc2_foundation::{NSAttributedString, NSDictionary, NSNumber, NSRange, NSString};
use tigertype_contracts::{Composition, InputState};
use tigertype_input_client::Modifiers;

/// 只接 keyDown/flagsChanged；其他事件不读键盘专用属性。ShiftTap 的原事件始终须返回 NO。
pub fn key_event(keyboard: &mut Keyboard, event: &NSEvent, state: &InputState) -> KeyAction {
    let kind = event.r#type();
    if !matches!(kind, NSEventType::KeyDown | NSEventType::FlagsChanged) {
        return KeyAction::Pass;
    }
    let flags = event.modifierFlags();
    let modifiers = Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        alt: flags.intersects(NSEventModifierFlags::Option | NSEventModifierFlags::Command),
    };
    if kind == NSEventType::FlagsChanged {
        return keyboard.flags_changed(event.keyCode(), modifiers);
    }
    // 最多一个 Unicode 标量（两个 UTF-16 单元）；先检查长度，避免复制无界事件文本。
    let characters = event
        .characters()
        .filter(|s| s.length() <= 2)
        .map(|s| s.to_string());
    keyboard.key_down(event.keyCode(), modifiers, characters.as_deref(), state)
}

/// 构造整段单下划线与 UTF-16 折叠光标；调用者使用 NSNotFound replacementRange，不覆盖任意宿主范围。
pub fn marked_text(
    composition: &Composition,
) -> Result<(Retained<NSAttributedString>, NSRange), &'static str> {
    let selection = marked_selection(composition)?;
    let style = NSNumber::new_isize(NSUnderlineStyle::Single.0);
    // SAFETY: 系统下划线属性只接收 NSNumber；字典和字符串由 Objective-C 引用计数持有。
    let text = unsafe {
        let attributes =
            NSDictionary::from_slices(&[NSUnderlineStyleAttributeName], &[style.as_ref()]);
        NSAttributedString::initWithString_attributes(
            NSAttributedString::alloc(),
            &NSString::from_str(&composition.text),
            Some(&attributes),
        )
    };
    Ok((text, NSRange::new(selection.start, selection.len())))
}

#[cfg(test)]
pub(super) mod tests {
    // 经 AppKit 公开出口测试，避免测试旁路掩盖导出遗漏。
    use super::super::{key_event, marked_text};
    use super::*;
    use objc2_foundation::NSPoint;
    use tigertype_contracts::Key;

    /// 合成 NSEvent 只传给本函数，不注入系统；这里只能在 Mac 上实际执行。
    fn event(
        kind: NSEventType,
        code: u16,
        flags: NSEventModifierFlags,
        text: &str,
    ) -> Retained<NSEvent> {
        let text = NSString::from_str(text);
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            kind, NSPoint::new(0.0, 0.0), flags, 0.0, 0, None, &text, &text, false, code,
        ).unwrap()
    }

    pub fn run() {
        let mut keyboard = Keyboard::default();
        let state = InputState {
            composing: true,
            ..Default::default()
        };
        let plain = NSEventModifierFlags::empty();
        for text in ["a", "𠀀"] {
            assert_eq!(
                key_event(
                    &mut keyboard,
                    &event(NSEventType::KeyDown, 0, plain, text),
                    &state
                ),
                KeyAction::Input(Key::Character(text.chars().next().unwrap()))
            );
        }
        for text in ["", "ab", "abc", "e\u{301}", "\u{f728}"] {
            assert_eq!(
                key_event(
                    &mut keyboard,
                    &event(NSEventType::KeyDown, 0, plain, text),
                    &state
                ),
                KeyAction::Pass
            );
        }
        for flags in [NSEventModifierFlags::Option, NSEventModifierFlags::Command] {
            assert_eq!(
                key_event(
                    &mut keyboard,
                    &event(NSEventType::KeyDown, 0, flags, "a"),
                    &state
                ),
                KeyAction::Pass
            );
        }
        assert_eq!(
            key_event(
                &mut keyboard,
                &event(NSEventType::KeyDown, 47, NSEventModifierFlags::Control, "."),
                &state
            ),
            KeyAction::Input(Key::TogglePunctuation)
        );
        assert_eq!(
            key_event(
                &mut keyboard,
                &event(NSEventType::KeyUp, 0, plain, "a"),
                &state
            ),
            KeyAction::Pass
        );
        assert_eq!(
            key_event(
                &mut keyboard,
                &event(
                    NSEventType::FlagsChanged,
                    56,
                    NSEventModifierFlags::Shift,
                    ""
                ),
                &state
            ),
            KeyAction::Pass
        );
        assert_eq!(
            key_event(
                &mut keyboard,
                &event(NSEventType::FlagsChanged, 56, plain, ""),
                &state
            ),
            KeyAction::ShiftTap
        );

        let composition = Composition {
            text: "中𠀀".into(),
            cursor: 7,
            selection: 0..7,
        };
        let (text, range) = marked_text(&composition).unwrap();
        assert_eq!(text.string().to_string(), composition.text);
        assert_eq!(text.length(), 3);
        assert_eq!(range, NSRange::new(3, 0));
        // SAFETY: 索引在已知非空字符串内，不请求写回范围；属性由本函数设置为 NSNumber。
        let underline = unsafe {
            text.attribute_atIndex_effectiveRange(
                NSUnderlineStyleAttributeName,
                0,
                std::ptr::null_mut(),
            )
        }
        .unwrap();
        assert_eq!(
            underline.downcast_ref::<NSNumber>().unwrap().as_isize(),
            NSUnderlineStyle::Single.0
        );
        assert!(
            marked_text(&Composition {
                cursor: 1,
                ..composition
            })
            .is_err()
        );
        assert_eq!(
            marked_text(&Composition::default()).unwrap().1,
            NSRange::new(0, 0)
        );
    }
}
