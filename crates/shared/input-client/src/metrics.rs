//! Windows/macOS 候选窗共用的逻辑尺寸；字体 point 在 Win32 换算 DPI，在 AppKit 换算为逻辑像素。
pub const TEXT_POINT: i32 = 15;
pub const SMALL_POINT: i32 = 11;
pub const PAD: i32 = 4;
pub const ROW: i32 = 32;
pub const ROW_LEFT: i32 = 12;
pub const ROW_RIGHT: i32 = 14;
pub const GAP: i32 = 10;
pub const FOOTER: i32 = 26;
pub const MIN_WIDTH: i32 = 112;
pub const MAX_WIDTH: i32 = 480;
