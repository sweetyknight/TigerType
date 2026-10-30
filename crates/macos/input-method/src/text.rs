//! IMK 标记文本的位置边界；先验证整个组合，再让 AppKit 构造字符串，禁止在非法字节边界切片。
use std::ops::Range;
use tigertype_contracts::{Composition, MAX_TEXT_BYTES};

/// setMarkedText 的 selectedRange 是 UTF-16 折叠光标，不把引擎的 selection 直接作为系统选区。
pub fn marked_selection(composition: &Composition) -> Result<Range<usize>, &'static str> {
    if composition.text.len() > MAX_TEXT_BYTES {
        return Err("组合文本长度超限");
    }
    if composition.selection.start > composition.selection.end
        || ![
            composition.cursor,
            composition.selection.start,
            composition.selection.end,
        ]
        .into_iter()
        .all(|index| composition.text.is_char_boundary(index))
    {
        return Err("组合位置越界或不在 UTF-8 边界");
    }
    let cursor = composition.text[..composition.cursor]
        .encode_utf16()
        .count();
    Ok(cursor..cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marked_caret_counts_utf16_units_and_does_not_select_preedit() {
        let text = "a中𠀀e\u{301}";
        for (cursor, units) in [(0, 0), (1, 1), (4, 2), (8, 4), (9, 5), (11, 6)] {
            let composition = Composition {
                text: text.into(),
                cursor,
                selection: 0..text.len(),
            };
            assert_eq!(marked_selection(&composition), Ok(units..units));
        }
        assert_eq!(marked_selection(&Composition::default()), Ok(0..0));
    }

    #[test]
    fn malformed_positions_and_oversized_text_are_rejected_without_panicking() {
        let mut composition = Composition {
            text: "中𠀀".into(),
            cursor: 7,
            selection: 0..7,
        };
        for cursor in [1, 2, 4, 5, 6, 8, usize::MAX] {
            composition.cursor = cursor;
            assert!(marked_selection(&composition).is_err());
        }
        composition.cursor = 7;
        for (start, end) in [(1, 7), (0, 4), (7, 3), (0, 8), (usize::MAX, usize::MAX)] {
            composition.selection = start..end;
            assert!(marked_selection(&composition).is_err());
        }
        composition = Composition {
            text: "a".repeat(MAX_TEXT_BYTES),
            cursor: MAX_TEXT_BYTES,
            selection: 0..MAX_TEXT_BYTES,
        };
        assert_eq!(
            marked_selection(&composition),
            Ok(MAX_TEXT_BYTES..MAX_TEXT_BYTES)
        );
        composition.text.push('a');
        assert!(marked_selection(&composition).is_err());
    }
}
