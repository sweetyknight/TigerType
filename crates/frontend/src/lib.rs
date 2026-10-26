//! 平台无关的宿主侧前端逻辑：会话与通道、物理键规则、单击 Shift、候选窗视图模型与摆放。
//! 各平台前端（TSF、IMK）只做系统绑定，不依赖引擎、后台或界面；本 crate 不接触任何平台包。
#![forbid(unsafe_code)]
mod host;
mod key;
mod view;
pub use host::{Channel, Error, Host};
pub use key::{Modifiers, PhysicalKey, ShiftTap, map};
pub use view::{Footer, Pager, Rect, Row, bounded, footer, locate, position, rows};
