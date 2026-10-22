//! 候选视图只生成带版本的选择意图；不查询引擎、不改宿主文字。
use tigerrime_contracts::{
    MAX_CANDIDATES,
    candidate::{CandidateChoice, CandidateFrame, ScreenRect},
};
#[cfg(windows)]
mod mailbox;
#[cfg(windows)]
mod popup;
#[cfg(windows)]
pub use mailbox::{FrameInbox, FrameSender, mailbox};
#[cfg(windows)]
pub use popup::run;

#[derive(Default)]
pub struct View {
    pub(crate) frame: Option<CandidateFrame>,
    serial: u32,
    selected: bool,
}
impl View {
    pub fn update(&mut self, frame: Option<CandidateFrame>) -> Result<u32, &'static str> {
        if let Some(frame) = &frame {
            let t = frame.token;
            if [t.host, t.generation, t.session, t.focus, t.revision].contains(&0)
                || t.revision != frame.state.revision
                || !frame.caret.valid()
                || frame.state.candidates.len() > MAX_CANDIDATES
            {
                return Err("候选快照不合法");
            }
        }
        // 窗口通知携带本地展示代次，即便业务快照相同，旧鼠标消息也不能重解释。
        self.serial = self.serial.checked_add(1).ok_or("候选展示编号耗尽")?;
        self.frame = frame;
        self.selected = false;
        Ok(self.serial)
    }
    pub fn choose(&mut self, serial: u32, index: u8) -> Option<CandidateChoice> {
        let frame = self.frame.as_ref()?;
        if serial != self.serial
            || self.selected
            || !frame.state.composing
            || index as usize >= frame.state.candidates.len()
        {
            return None;
        }
        self.selected = true;
        Some(CandidateChoice {
            token: frame.token,
            index,
        })
    }
}

pub(crate) fn position(caret: ScreenRect, work: ScreenRect, width: i32, height: i32) -> ScreenRect {
    let width = width.min(work.right - work.left).max(0);
    let height = height.min(work.bottom - work.top).max(0);
    let left = caret.left.clamp(work.left, work.right - width);
    let top = if caret.bottom <= work.bottom - height {
        caret.bottom
    } else {
        caret.top - height
    }
    .clamp(work.top, work.bottom - height);
    ScreenRect {
        left,
        top,
        right: left + width,
        bottom: top + height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigerrime_contracts::{Candidate, InputState, candidate::CandidateToken};
    pub(super) fn frame() -> CandidateFrame {
        CandidateFrame {
            token: CandidateToken {
                host: 1,
                generation: 2,
                session: 3,
                focus: 4,
                revision: 5,
            },
            caret: ScreenRect {
                left: 0,
                top: 0,
                right: 1,
                bottom: 20,
            },
            state: InputState {
                revision: 5,
                composing: true,
                candidates: vec![Candidate {
                    text: "𠀀".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        }
    }
    #[test]
    fn old_frames_double_clicks_and_hidden_views_do_not_create_choices() {
        let mut view = View::default();
        let old = view.update(Some(frame())).unwrap();
        let current = view.update(Some(frame())).unwrap();
        assert!(view.choose(old, 0).is_none());
        assert!(view.choose(current, 9).is_none());
        assert_eq!(view.choose(current, 0).unwrap().token, frame().token);
        assert!(view.choose(current, 0).is_none());
        view.update(None).unwrap();
        assert!(view.choose(current, 0).is_none());
        let mut bad = frame();
        bad.token.revision = 6;
        assert!(view.update(Some(bad)).is_err());
        view.serial = u32::MAX - 1;
        assert_eq!(view.update(Some(frame())).unwrap(), u32::MAX);
        assert!(view.choose(u32::MAX, 0).is_some());
        assert!(view.update(Some(frame())).is_err());
    }
    #[test]
    fn position_is_inside_negative_coordinate_monitor_and_above_bottom_edge() {
        let work = ScreenRect {
            left: -1920,
            top: -100,
            right: 0,
            bottom: 980,
        };
        let rect = position(
            ScreenRect {
                left: -10,
                top: 960,
                right: 0,
                bottom: 980,
            },
            work,
            300,
            400,
        );
        assert_eq!(
            rect,
            ScreenRect {
                left: -300,
                top: 560,
                right: 0,
                bottom: 960
            }
        );
        assert_eq!(position(work, work, 4000, 4000), work);
    }
}
