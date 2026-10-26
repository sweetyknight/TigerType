//! 候选窗视图模型与摆放规则：各平台只负责绘制与取系统矩形，行、页脚与位置退路在此统一。
use tigertype_contracts::{InputMode, InputState};

/// 屏幕矩形：左上原点、y 向下；macOS 的左下原点坐标由前端换算后再交给这里。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 矩形须有序且数值有界，防止尺寸运算溢出；保留多显示器负坐标。
pub fn bounded(rect: &Rect) -> bool {
    rect.left <= rect.right
        && rect.top <= rect.bottom
        && [rect.left, rect.top, rect.right, rect.bottom]
            .iter()
            .all(|v| (-1_000_000..=1_000_000).contains(v))
}

/// 位置退路：按平台给出的来源顺序取第一个有界矩形；来源惰性求值，取到即停。
pub fn locate(sources: &[&dyn Fn() -> Option<Rect>]) -> Option<Rect> {
    sources.iter().find_map(|source| source().filter(bounded))
}

/// 在工作区内放置：优先放在锚点下方，下方不足则翻到上方，左右夹取。
pub fn position(anchor: Rect, work: Rect, width: i32, height: i32) -> Rect {
    let width = width.min(work.right - work.left).max(0);
    let height = height.min(work.bottom - work.top).max(0);
    let left = anchor.left.clamp(work.left, work.right - width);
    let top = if anchor.bottom <= work.bottom - height {
        anchor.bottom
    } else {
        anchor.top - height
    }
    .clamp(work.top, work.bottom - height);
    Rect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

/// 一行候选：序号、候选字与注释（学习信息等，由引擎排好），以及是否为高亮项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row<'a> {
    pub label: &'a str,
    pub text: &'a str,
    pub comment: &'a str,
    pub chosen: bool,
}
pub fn rows(state: &InputState) -> impl Iterator<Item = Row<'_>> {
    state
        .candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| Row {
            label: &candidate.label,
            text: &candidate.text,
            comment: &candidate.comment,
            chosen: state.highlighted == Some(index),
        })
}

/// 翻页指示：页码从 1 起，前后箭头是否可用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pager {
    pub page: usize,
    pub previous: bool,
    pub next: bool,
}
/// 页脚只为拼音反查标签或翻页指示存在；翻页指示只在候选多于一页时出现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    pub pinyin: bool,
    pub pager: Option<Pager>,
}
pub fn footer(state: &InputState) -> Option<Footer> {
    let paged = !state.candidates.is_empty() && (state.page_index > 0 || !state.is_last_page);
    let pinyin = state.mode == InputMode::Pinyin;
    (pinyin || paged).then_some(Footer {
        pinyin,
        pager: paged.then_some(Pager {
            page: state.page_index + 1,
            previous: state.page_index > 0,
            next: !state.is_last_page,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::Candidate;

    #[test]
    fn position_stays_inside_a_negative_coordinate_monitor_and_flips_above_the_bottom_edge() {
        let work = Rect {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        };
        let anchor = Rect {
            left: -10,
            top: 960,
            right: 0,
            bottom: 980,
        };
        let expected = Rect {
            left: -300,
            top: 560,
            right: 0,
            bottom: 960,
        };
        assert_eq!(position(anchor, work, 300, 400), expected);
        assert_eq!(position(work, work, 4000, 4000), work);
        assert!(bounded(&anchor));
        assert!(!bounded(&Rect {
            left: 1,
            ..Rect::default()
        }));
        assert!(!bounded(&Rect {
            right: 2_000_000,
            ..Rect::default()
        }));
    }

    #[test]
    fn fallbacks_take_the_first_bounded_source_and_stop_there() {
        let near = Rect {
            left: 10,
            top: 10,
            right: 20,
            bottom: 30,
        };
        let inverted = Rect {
            left: 5,
            ..Rect::default()
        };
        let asked = std::cell::Cell::new(0);
        let later = || {
            asked.set(asked.get() + 1);
            Some(Rect::default())
        };
        assert_eq!(
            locate(&[&|| None, &|| Some(inverted), &|| Some(near), &later]),
            Some(near)
        );
        assert_eq!(asked.get(), 0, "取到位置后不再求值后续来源");
        assert_eq!(locate(&[&|| None, &later]), Some(Rect::default()));
        assert_eq!(locate(&[&|| None, &|| Some(inverted)]), None);
    }

    #[test]
    fn rows_mark_the_highlight_and_the_footer_appears_only_for_paging_or_pinyin() {
        let mut state = InputState {
            candidates: (0..3)
                .map(|index| Candidate {
                    text: format!("字{index}"),
                    comment: format!("u{index}"),
                    label: (index + 1).to_string(),
                    ..Default::default()
                })
                .collect(),
            highlighted: Some(1),
            is_last_page: true,
            ..Default::default()
        };
        let rows: Vec<_> = rows(&state).collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[1],
            Row {
                label: "2",
                text: "字1",
                comment: "u1",
                chosen: true
            }
        );
        assert!(!rows[0].chosen && !rows[2].chosen);
        // 不足一页：没有页脚。
        assert_eq!(footer(&state), None);
        // 还有下一页：首页只能后翻。
        state.is_last_page = false;
        let pager = Pager {
            page: 1,
            previous: false,
            next: true,
        };
        assert_eq!(
            footer(&state),
            Some(Footer {
                pinyin: false,
                pager: Some(pager)
            })
        );
        // 末页只能前翻。
        state.is_last_page = true;
        state.page_index = 2;
        assert_eq!(
            footer(&state).and_then(|footer| footer.pager),
            Some(Pager {
                page: 3,
                previous: true,
                next: false
            })
        );
        // 拼音反查只有一页时只留标签，不画翻页指示。
        state.page_index = 0;
        state.mode = InputMode::Pinyin;
        assert_eq!(
            footer(&state),
            Some(Footer {
                pinyin: true,
                pager: None
            })
        );
        // 没有候选时不显示翻页。
        state.mode = InputMode::Tiger;
        state.candidates.clear();
        state.page_index = 1;
        assert_eq!(footer(&state), None);
    }
}
