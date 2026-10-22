//! 候选显示和点击只传拥有所有权的数据；宿主编号由认证后的应用层分配，不能由协议正文自报。
use crate::InputState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}
impl ScreenRect {
    pub fn valid(self) -> bool {
        // 保留负坐标多显示器；限制数值以免后续尺寸运算溢出。
        self.left <= self.right
            && self.top <= self.bottom
            && [self.left, self.top, self.right, self.bottom]
                .iter()
                .all(|v| (-1_000_000..=1_000_000).contains(v))
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateToken {
    pub host: u64,
    pub generation: u64,
    pub session: u64,
    pub focus: u64,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateFrame {
    pub token: CandidateToken,
    pub caret: ScreenRect,
    pub state: InputState,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CandidateChoice {
    pub token: CandidateToken,
    pub index: u8,
}
