//! 展示身份和坐标转换不依赖 AppKit，可在 Windows 上验证迟到点击与多屏边界。
use tigertype_contracts::{InputState, Key, MAX_CANDIDATES, MAX_TEXT_BYTES, protocol::Panel};
use tigertype_input_client::{Rect, bounded, position};

/// AppKit 屏幕坐标，左下原点，单位为 point，不预乘 Retina 缩放。
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NativeRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl NativeRect {
    fn rect(self) -> Option<Rect> {
        if ![self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            || self.width < 0.0
            || self.height < 0.0
            || self == Self::default()
            || [self.x, self.y, self.width, self.height]
                .iter()
                .any(|v| v.abs() > 500_000.0)
        {
            return None;
        }
        let rect = Rect {
            left: self.x.floor() as i32,
            top: (-self.y - self.height).floor() as i32,
            right: (self.x + self.width).ceil() as i32,
            bottom: (-self.y).ceil() as i32,
        };
        bounded(&rect).then_some(rect)
    }
    fn from_rect(rect: Rect) -> Self {
        Self {
            x: rect.left as f64,
            y: -rect.bottom as f64,
            width: (rect.right - rect.left) as f64,
            height: (rect.bottom - rect.top) as f64,
        }
    }
}

/// 屏幕归属按完整边界选择，放置按可见工作区夹取；Dock/菜单栏不应导致归属丢失。
#[derive(Debug, Clone, Copy)]
pub struct Screen {
    pub bounds: NativeRect,
    pub visible: NativeRect,
}

/// 宿主没有布局通知，显示时拿到的坐标可能还是旧的；组合写入后按这些延迟（秒）补查，
/// 覆盖 Chromium 系宿主布局完成的实测范围（6–38ms，繁忙时更长）。
pub const RECHECK: [f64; 2] = [0.03, 0.12];

/// 每个控制器独占，禁止把其他应用的历史位置作为本控制器的退路。
#[derive(Debug, Default)]
pub struct Location {
    last: Option<NativeRect>,
}
impl Location {
    /// 只使用包含锚点的屏幕；宿主不给坐标且本控制器无历史时退到鼠标位置，只是近似，
    /// 但候选窗不能不出现。之后保存为历史，鼠标再动候选窗也不跟着跑。
    pub fn place(
        &mut self,
        composition: Option<NativeRect>,
        caret: Option<NativeRect>,
        pointer: Option<NativeRect>,
        screens: &[Screen],
        size: (i32, i32),
    ) -> Option<NativeRect> {
        if size.0 <= 0 || size.1 <= 0 {
            return None;
        }
        // 位置有界但已离开所有屏幕时仍应继续退路，不能挡住有效的光标或历史锚点。
        let (anchor, work) = [composition, caret, self.last, pointer]
            .into_iter()
            .flatten()
            .filter_map(NativeRect::rect)
            .find_map(|anchor| {
                screens.iter().find_map(|screen| {
                    let r = screen.bounds.rect()?;
                    let work = screen.visible.rect()?;
                    (r.left <= anchor.left
                        && anchor.left < r.right
                        && r.top <= anchor.top
                        && anchor.top < r.bottom
                        && r.left <= work.left
                        && work.right <= r.right
                        && r.top <= work.top
                        && work.bottom <= r.bottom
                        && work.left < work.right
                        && work.top < work.bottom)
                        .then_some((anchor, work))
                })
            })?;
        let placed = NativeRect::from_rect(position(anchor, work, size.0, size.1));
        // 保存锚点而非面板矩形，避免连续无位置更新时面板逐次漂移。
        self.last = Some(NativeRect::from_rect(anchor));
        Some(placed)
    }
}

/// 展示编号区分同一 revision 在不同控制器/连接中的重复出现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection {
    pub owner: u64,
    pub display: u64,
    pub revision: u64,
    pub index: u8,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Key(Key),
    Show(Panel),
}
impl MenuAction {
    /// IMK 转发菜单命令时不依赖原 target 对象；整数 tag 同时绑定激活身份和固定语义动作。
    pub fn tag(self, owner: u64) -> Option<i64> {
        let kind = match self {
            Self::Key(Key::ToggleAscii) => 1,
            Self::Key(Key::ToggleFullShape) => 2,
            Self::Key(Key::TogglePunctuation) => 3,
            Self::Show(Panel::Lookup) => 4,
            Self::Show(Panel::Settings) => 5,
            _ => return None,
        };
        if owner == 0 {
            return None;
        }
        owner.checked_mul(8)?.checked_add(kind)?.try_into().ok()
    }
    /// 转发者还须是 tag 对应的控制器，不能让失焦控制器借用另一个当前宿主的菜单身份。
    pub fn from_tag(tag: i64, expected_owner: u64) -> Option<Action> {
        let tag = u64::try_from(tag).ok()?;
        let owner = tag / 8;
        if owner == 0 || owner != expected_owner {
            return None;
        }
        let action = match tag % 8 {
            1 => Self::Key(Key::ToggleAscii),
            2 => Self::Key(Key::ToggleFullShape),
            3 => Self::Key(Key::TogglePunctuation),
            4 => Self::Show(Panel::Lookup),
            5 => Self::Show(Panel::Settings),
            _ => return None,
        };
        Some(Action::Menu { owner, action })
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Select(Selection),
    /// 调用者在执行前核对控制器身份/焦点；不直接保存宿主对象或裸指针。
    Menu {
        owner: u64,
        action: MenuAction,
    },
    /// 一次性补查组合坐标；控制器已换时直接作废。
    Reanchor {
        owner: u64,
    },
}

#[derive(Debug, Default)]
pub struct Presentation {
    owner: u64,
    display: u64,
    state: Option<InputState>,
    pressed: Option<Selection>,
}
impl Presentation {
    pub fn state(&self) -> Option<&InputState> {
        self.state.as_ref()
    }
    pub fn owner(&self) -> u64 {
        self.owner
    }
    /// 仅有合法组合才展示；计数耗尽不能复用身份。
    pub fn update(&mut self, owner: u64, state: InputState) -> Result<bool, &'static str> {
        if owner == 0
            || !state.composing
            || state.candidates.len() > MAX_CANDIDATES
            || state
                .highlighted
                .is_some_and(|i| i >= state.candidates.len())
            || state.options.validate().is_err()
            || state.page_index.checked_add(1).is_none()
            || state
                .candidates
                .iter()
                .flat_map(|c| [c.label.len(), c.text.len(), c.comment.len()])
                .try_fold(state.composition.text.len(), usize::checked_add)
                .is_none_or(|n| n > MAX_TEXT_BYTES)
        {
            self.hide();
            return Err("候选快照或控制器身份无效");
        }
        if self.owner == owner && self.state.as_ref() == Some(&state) {
            return Ok(false);
        }
        self.hide();
        self.display = self.display.checked_add(1).ok_or("展示编号已耗尽")?;
        self.owner = owner;
        self.state = Some(state);
        Ok(true)
    }
    pub fn hide(&mut self) {
        self.state = None;
        self.pressed = None;
        self.owner = 0;
    }
    pub fn selection(&self, index: usize) -> Option<Selection> {
        let state = self.state.as_ref()?;
        (index < state.candidates.len()).then_some(Selection {
            owner: self.owner,
            display: self.display,
            revision: state.revision,
            index: index as u8,
        })
    }
    pub fn press(&mut self, index: Option<usize>) {
        self.pressed = index.and_then(|i| self.selection(i));
    }
    pub fn release(&mut self, index: Option<usize>) -> Option<Selection> {
        let pressed = self.pressed.take()?;
        index
            .and_then(|i| self.selection(i))
            .filter(|current| *current == pressed)
    }
    pub fn accepts(&self, selection: Selection) -> bool {
        self.selection(selection.index as usize) == Some(selection)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn menu_tags_bind_identity_and_reject_unknown_actions_and_overflow() {
        for owner in [1, 17, i64::MAX as u64 / 8] {
            for action in [
                MenuAction::Key(Key::ToggleAscii),
                MenuAction::Key(Key::ToggleFullShape),
                MenuAction::Key(Key::TogglePunctuation),
                MenuAction::Show(Panel::Lookup),
                MenuAction::Show(Panel::Settings),
            ] {
                assert_eq!(
                    MenuAction::from_tag(action.tag(owner).unwrap(), owner),
                    Some(Action::Menu { owner, action })
                );
                assert!(MenuAction::from_tag(action.tag(owner).unwrap(), owner + 1).is_none());
            }
        }
        for tag in [-1, 0, 1, 7, 8, 14, 15] {
            assert!(MenuAction::from_tag(tag, 1).is_none());
        }
        assert!(MenuAction::Show(Panel::Lookup).tag(0).is_none());
        assert!(MenuAction::Show(Panel::Lookup).tag(u64::MAX).is_none());
        assert!(MenuAction::Key(Key::Character('a')).tag(1).is_none());
    }
    use tigertype_contracts::Candidate;
    fn state() -> InputState {
        InputState {
            composing: true,
            revision: 5,
            candidates: vec![Candidate::default(), Candidate::default()],
            ..Default::default()
        }
    }
    #[test]
    fn clicks_bind_owner_display_snapshot_and_press_release_row() {
        let mut p = Presentation::default();
        p.update(1, state()).unwrap();
        p.press(Some(0));
        assert!(p.release(Some(1)).is_none());
        p.press(Some(0));
        let first = p.release(Some(0)).unwrap();
        assert!(p.accepts(first));
        p.press(Some(0));
        assert!(!p.update(1, state()).unwrap());
        assert_eq!(p.release(Some(0)), Some(first));
        p.press(Some(0));
        p.update(2, state()).unwrap();
        assert!(p.release(Some(0)).is_none());
        assert!(!p.accepts(first));
        p.press(Some(0));
        p.hide();
        assert!(p.release(Some(0)).is_none());
        p.update(1, state()).unwrap();
        assert!(!p.accepts(first));
        assert!(p.update(0, state()).is_err());
        assert!(p.state().is_none());
        p.display = u64::MAX;
        assert!(p.update(1, state()).is_err());
        assert!(p.state().is_none());
    }
    #[test]
    fn coordinates_reject_unknown_nan_and_inverted_keep_negative_monitors_and_no_drift() {
        let work = NativeRect {
            x: -1600.0,
            y: -200.0,
            width: 1600.0,
            height: 1000.0,
        };
        let anchor = NativeRect {
            x: -20.5,
            y: -190.0,
            width: 1.0,
            height: 20.0,
        };
        let screen = Screen {
            bounds: work,
            visible: work,
        };
        let mut loc = Location::default();
        assert!(
            loc.place(
                Some(NativeRect::default()),
                None,
                None,
                &[screen],
                (300, 200)
            )
            .is_none()
        );
        assert!(
            NativeRect {
                x: f64::NAN,
                ..anchor
            }
            .rect()
            .is_none()
        );
        assert!(
            NativeRect {
                width: -1.0,
                ..anchor
            }
            .rect()
            .is_none()
        );
        let pos = loc
            .place(Some(anchor), None, None, &[screen], (300, 200))
            .unwrap();
        assert_eq!(pos.x, -300.0);
        assert_eq!(pos.y, -170.0);
        assert_eq!(
            loc.place(None, None, None, &[screen], (300, 200)),
            Some(pos)
        );
        assert!(loc.place(None, None, None, &[], (300, 200)).is_none());
        let dock = Screen {
            bounds: work,
            visible: NativeRect {
                y: -100.0,
                height: 900.0,
                ..work
            },
        };
        assert!(
            loc.place(Some(anchor), None, None, &[dock], (300, 200))
                .is_some(),
            "Dock 不改变屏幕归属"
        );
    }
    #[test]
    fn pointer_is_only_the_last_resort_and_does_not_drag_the_panel_afterwards() {
        let bounds = NativeRect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        let screen = Screen {
            bounds,
            visible: bounds,
        };
        let at = |x, y| NativeRect {
            x,
            y,
            width: 0.0,
            height: 0.0,
        };
        let size = (200, 100);
        let caret = NativeRect {
            x: 500.0,
            y: 300.0,
            width: 1.0,
            height: 20.0,
        };
        // 宿主什么坐标都不给、本控制器也无历史：候选窗仍要出现。
        let mut location = Location::default();
        let first = location
            .place(None, None, Some(at(100.0, 400.0)), &[screen], size)
            .expect("没有坐标时候选窗不能不出现");
        assert_eq!((first.x, first.y), (100.0, 300.0));
        // 鼠标移开后下一键留在原处，不跟着鼠标跑。
        let moved = at(900.0, 700.0);
        assert_eq!(
            location.place(None, None, Some(moved), &[screen], size),
            Some(first)
        );
        // 宿主补上真实坐标后立刻以它为准，鼠标不抢真实位置。
        let real = location
            .place(None, Some(caret), Some(moved), &[screen], size)
            .unwrap();
        assert_eq!(real.x, 500.0);
        // 屏幕外的鼠标位置同样无效，不能把窗口摆到屏幕外。
        let mut fresh = Location::default();
        assert!(
            fresh
                .place(None, None, Some(at(5000.0, 0.0)), &[screen], size)
                .is_none()
        );
        // 坐标原点是无效的默认值，宿主返回全零时不当作鼠标位置。
        assert!(
            fresh
                .place(None, None, Some(at(0.0, 0.0)), &[screen], size)
                .is_none()
        );
    }
    #[test]
    fn rechecks_are_bounded_and_ascending() {
        // 空闲不得有周期唤醒：补查次数固定，且按时间递增。
        assert!(RECHECK.len() <= 3);
        assert!(RECHECK.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(RECHECK.iter().all(|delay| *delay > 0.0 && *delay < 0.5));
    }
    #[test]
    fn invalid_snapshots_clear_pending_clicks_before_rendering() {
        let mut p = Presentation::default();
        p.update(1, state()).unwrap();
        p.press(Some(0));
        let mut bad = state();
        bad.page_index = usize::MAX;
        assert!(p.update(1, bad).is_err());
        assert!(p.release(Some(0)).is_none());
        let mut bad = state();
        bad.candidates[0].text = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(p.update(1, bad).is_err());
        let mut bad = state();
        bad.highlighted = Some(99);
        assert!(p.update(1, bad).is_err());
    }
    #[test]
    fn offscreen_composition_uses_caret_and_rejects_work_areas_outside_the_screen() {
        let bounds = NativeRect {
            x: 0.0,
            y: 0.0,
            width: 1000.0,
            height: 800.0,
        };
        let screen = Screen {
            bounds,
            visible: bounds,
        };
        let caret = NativeRect {
            x: 20.0,
            y: 100.0,
            width: 1.0,
            height: 20.0,
        };
        let offscreen = NativeRect { x: 2000.0, ..caret };
        let mut location = Location::default();
        assert!(
            location
                .place(Some(offscreen), Some(caret), None, &[screen], (200, 100))
                .is_some()
        );
        let invalid = Screen {
            bounds,
            visible: NativeRect {
                x: 1500.0,
                ..bounds
            },
        };
        assert!(
            location
                .place(Some(caret), None, None, &[invalid], (200, 100))
                .is_none()
        );
    }
}
