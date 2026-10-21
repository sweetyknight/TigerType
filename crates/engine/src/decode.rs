//! 检查引擎输出的结构约束，再复制为拥有所有权的数据；这里不做宿主文字操作。

use crate::{Error, Result, raw};
use std::ffi::{CStr, c_char};
use tigerrime_contracts::{Candidate, Composition, InputState, MAX_CANDIDATES, MAX_TEXT_BYTES};

/// 指针必须为空或指向有效 NUL 字符串；长度限制防止大规模复制，不是损坏 DLL 的内存沙箱。
pub unsafe fn text(pointer: *const c_char) -> Result<String> {
    if pointer.is_null() {
        return Ok(String::new());
    }
    // SAFETY: 调用方保证指针来自存活的 Rime 分配，或者测试中的有效 C 字符串。
    let bytes = unsafe { CStr::from_ptr(pointer) }.to_bytes();
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(Error::Invalid("引擎文本长度超限"));
    }
    Ok(std::str::from_utf8(bytes)?.to_owned())
}

fn position(value: i32, text: &str) -> Result<usize> {
    let value = usize::try_from(value).map_err(|_| Error::Invalid("组合位置为负数"))?;
    if !text.is_char_boundary(value) {
        return Err(Error::Invalid("组合位置不在 UTF-8 字符边界"));
    }
    Ok(value)
}

/// 所有非空指针必须在当前 RimeContext 的生命周期内有效。
pub unsafe fn context(context: &raw::Context) -> Result<InputState> {
    let composition = &context.composition;
    // SAFETY: context 在 Allocation guard 释放前保持有效。
    let preedit = unsafe { text(composition.preedit)? };
    if usize::try_from(composition.length).ok() != Some(preedit.len()) {
        return Err(Error::Invalid("组合文本长度不符"));
    }
    let start = position(composition.sel_start, &preedit)?;
    let end = position(composition.sel_end, &preedit)?;
    if start > end {
        return Err(Error::Invalid("组合选择范围倒置"));
    }
    let menu = &context.menu;
    let count =
        usize::try_from(menu.num_candidates).map_err(|_| Error::Invalid("候选数量为负数"))?;
    let page_size =
        usize::try_from(menu.page_size).map_err(|_| Error::Invalid("候选页大小为负数"))?;
    let page_index = usize::try_from(menu.page_no).map_err(|_| Error::Invalid("页码为负数"))?;
    if count > MAX_CANDIDATES || page_size > MAX_CANDIDATES || count > page_size {
        return Err(Error::Invalid("候选页超过容量边界"));
    }
    if count != 0 && menu.candidates.is_null() {
        return Err(Error::Invalid("候选数组为空指针"));
    }
    let highlighted = if count == 0 {
        None
    } else {
        let index = usize::try_from(menu.highlighted_candidate_index)
            .map_err(|_| Error::Invalid("高亮位置为负数"))?;
        if index >= count {
            return Err(Error::Invalid("高亮位置越界"));
        }
        Some(index)
    };
    let mut candidates = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: 已检查数量和非空数组；Rime 合约保证数组及标签覆盖当前页。
        unsafe {
            let candidate = &*menu.candidates.add(index);
            let label = if context.select_labels.is_null() {
                String::new()
            } else {
                text(*context.select_labels.add(index))?
            };
            candidates.push(Candidate {
                text: text(candidate.text)?,
                comment: text(candidate.comment)?,
                label,
            });
        }
    }
    Ok(InputState {
        composition: Composition {
            cursor: position(composition.cursor_pos, &preedit)?,
            text: preedit,
            selection: start..end,
        },
        candidates,
        page_size,
        page_index,
        is_last_page: menu.is_last_page != 0,
        highlighted,
        // SAFETY: 预览指针与 context 共用受保护的生命周期。
        commit_preview: unsafe { text(context.commit_text_preview)? },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn unicode_snapshot_is_owned_after_source_is_freed() {
        let text = CString::new("𠀀😀中").unwrap();
        let context = raw::Context {
            composition: raw::Composition {
                length: 11,
                cursor_pos: 8,
                sel_start: 4,
                sel_end: 11,
                preedit: text.as_ptr(),
            },
            ..Default::default()
        };
        // SAFETY: 所有使用的指针均指向本测试仍存活的 CString。
        let snapshot = unsafe { super::context(&context) }.unwrap();
        drop(text);
        assert_eq!(snapshot.composition.text, "𠀀😀中");
        assert_eq!(snapshot.composition.selection, 4..11);
        assert_eq!(snapshot.composition.cursor, 8);
    }

    #[test]
    fn rejects_invalid_utf8_and_mid_character_positions() {
        let invalid = [0xff_u8, 0];
        // SAFETY: 数组以 NUL 结尾，测试无效 UTF-8 的错误传播。
        assert!(unsafe { text(invalid.as_ptr().cast()) }.is_err());
        assert!(position(1, "𠀀").is_err());
        assert!(position(-1, "a").is_err());
        assert!(position(8, "a").is_err());
    }

    #[test]
    fn rejects_bad_candidate_counts_before_dereferencing() {
        for count in [-1, 1, 101] {
            let mut value = raw::Context::default();
            value.menu.page_size = 9;
            value.menu.num_candidates = count;
            // SAFETY: 故意保留空指针，验证数量/空指针检查发生在解引用之前。
            assert!(unsafe { context(&value) }.is_err());
        }
    }

    #[test]
    fn rejects_oversized_strings() {
        let value = CString::new(vec![b'a'; MAX_TEXT_BYTES + 1]).unwrap();
        // SAFETY: 输入是完整有效的 CString，仅长度超过应用边界。
        assert!(unsafe { text(value.as_ptr()) }.is_err());
    }

    #[test]
    fn candidate_text_comment_and_label_are_copied() {
        let snapshot = {
            let word = CString::new("𠀀").unwrap();
            let comment = CString::new("[U+20000]").unwrap();
            let label = CString::new("㊀").unwrap();
            let candidate = raw::Candidate {
                text: word.as_ptr(),
                comment: comment.as_ptr(),
                ..Default::default()
            };
            let labels = [label.as_ptr()];
            let value = raw::Context {
                menu: raw::Menu {
                    page_size: 9,
                    num_candidates: 1,
                    candidates: &candidate,
                    ..Default::default()
                },
                select_labels: labels.as_ptr(),
                ..Default::default()
            };
            // SAFETY: 字符串、候选和标签数组均在此作用域内存活。
            unsafe { context(&value) }.unwrap()
        };
        assert_eq!(snapshot.candidates[0].text, "𠀀");
        assert_eq!(snapshot.candidates[0].comment, "[U+20000]");
        assert_eq!(snapshot.candidates[0].label, "㊀");
        assert_eq!(snapshot.highlighted, Some(0));
    }

    #[test]
    fn rejects_mismatched_length_and_reversed_selection() {
        let mut value = raw::Context {
            composition: raw::Composition {
                length: 1,
                preedit: c"ab".as_ptr(),
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: 有效的静态字符串，故意提供错误的逻辑长度。
        assert!(unsafe { context(&value) }.is_err());
        value.composition.length = 2;
        value.composition.sel_start = 2;
        value.composition.sel_end = 1;
        // SAFETY: 指针仍有效，选择区间应在访问候选前被拒绝。
        assert!(unsafe { context(&value) }.is_err());
    }
}
