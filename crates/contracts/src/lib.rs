//! 模块间只传递拥有所有权的数据，逻辑按键与平台键码无关。
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use std::ops::Range;
pub mod protocol;
pub mod ui;

pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_CANDIDATES: usize = 9;
pub const PAGE_SIZE: usize = 9;
pub const MAX_QUERY_CHARS: usize = 64;
pub const MAX_PINYIN_BYTES: usize = 6;
pub const MAX_LEARNING_ITEMS: usize = 64;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Composition {
    pub text: String,
    /// 内部位置使用 UTF-8 字节，TSF 边界再转换 UTF-16。
    pub cursor: usize,
    pub selection: Range<usize>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputForm {
    #[default]
    Original,
    Simplified,
    Traditional,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    Tiger,
    Pinyin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub auto_commit_four: bool,
    pub common_only: bool,
    pub show_learning: bool,
    pub output_form: OutputForm,
    /// 外观项引擎不使用，只随 InputState 带给宿主内候选窗，DLL 因此无需读盘。
    pub theme: Theme,
    /// 仅在主题为自定义时生效；切到预设也保留，便于切回。
    pub custom: CustomColors,
    pub candidate_font: CandidateFont,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            auto_commit_four: false,
            common_only: true,
            show_learning: false,
            output_form: OutputForm::Original,
            theme: Theme::Amber,
            custom: CustomColors::default(),
            candidate_font: CandidateFont::WenKai,
        }
    }
}
impl Options {
    pub fn palette(&self) -> Palette {
        self.theme.preset().unwrap_or_else(|| self.custom.palette())
    }
    /// 保存与载入设置时校验：自定义基色不得偏蓝，推导出的文字必须可读。
    pub fn validate(&self) -> Result<(), String> {
        let custom = self.custom;
        for color in [custom.window, custom.text, custom.highlight, custom.accent] {
            if color > 0xFF_FFFF {
                return Err(format!("自定义颜色 {color:X} 超出 RGB 范围"));
            }
            if !without_blue(color) {
                return Err(format!(
                    "自定义颜色 #{color:06X} 偏蓝：蓝色分量不能高于红、绿分量"
                ));
            }
        }
        match custom.palette().unreadable() {
            Some((fore, back)) => Err(format!(
                "自定义配色对比度不足：#{fore:06X} 与 #{back:06X} 难以辨认"
            )),
            None => Ok(()),
        }
    }
}

/// 候选字与查字字形的字体；未安装时由界面回退到微软雅黑。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateFont {
    #[default]
    WenKai,
    KaiTi,
}
impl CandidateFont {
    pub const ALL: [Self; 2] = [Self::WenKai, Self::KaiTi];
    pub const fn face(self) -> &'static str {
        match self {
            Self::WenKai => "LXGW WenKai GB Screen",
            Self::KaiTi => "KaiTi",
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Self::WenKai => "霞鹜文楷",
            Self::KaiTi => "楷体",
        }
    }
}

/// 配色主题。产品要求界面不得出现蓝色：每个颜色的蓝分量都不超过红、绿分量，由测试逐个核对。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    Amber,
    Ink,
    Bamboo,
    Night,
    Custom,
}
/// 自定义主题只取四个基色，其余按比例混色推导；线性混合不会让蓝分量超过红、绿，
/// 因此基色无蓝则推导色也无蓝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomColors {
    pub window: u32,
    pub text: u32,
    pub highlight: u32,
    pub accent: u32,
}
impl Default for CustomColors {
    fn default() -> Self {
        let amber = Theme::Amber.preset().unwrap();
        Self {
            window: amber.window,
            text: amber.text,
            highlight: amber.highlight,
            accent: amber.accent,
        }
    }
}
impl CustomColors {
    pub fn palette(self) -> Palette {
        let toward = |percent| mix(self.window, self.text, percent);
        let dark = luma(self.window) < luma(self.text);
        let on_accent = if contrast(0xFF_FFFF, self.accent) >= contrast(0x1A_1A1A, self.accent) {
            0xFF_FFFF
        } else {
            0x1A_1A1A
        };
        Palette {
            window: self.window,
            surface: if dark {
                toward(5)
            } else {
                mix(self.window, 0xFF_FFFF, 70)
            },
            border: toward(20),
            line: toward(12),
            field: toward(26),
            text: self.text,
            label: toward(64),
            muted: toward(68),
            faint: toward(36),
            highlight: self.highlight,
            highlight_text: self.text,
            accent: self.accent,
            on_accent,
            selection: self.highlight,
        }
    }
}
/// 逐通道按百分比混合；向下取整对两个参数都单调，保持“蓝不超过红、绿”。
const fn mix(from: u32, to: u32, percent: u32) -> u32 {
    let mut rgb = 0;
    let mut shift = 0;
    while shift < 24 {
        let (a, b) = (from >> shift & 0xFF, to >> shift & 0xFF);
        rgb |= ((a * (100 - percent) + b * percent) / 100) << shift;
        shift += 8;
    }
    rgb
}
/// 粗略明度，只用于判断深浅主题。
const fn luma(rgb: u32) -> u32 {
    (rgb >> 16 & 0xFF) * 2126 + (rgb >> 8 & 0xFF) * 7152 + (rgb & 0xFF) * 722
}
/// WCAG 相对亮度对比度。
pub fn contrast(a: u32, b: u32) -> f64 {
    let luminance = |rgb: u32| {
        let channel = |shift: u32| {
            let value = f64::from(rgb >> shift & 0xFF) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}
/// 主题颜色，0xRRGGBB；候选窗与查字/设置窗共用，避免两处配色漂移。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// 窗口与候选窗底色。
    pub window: u32,
    /// 卡片、输入框、列表底色。
    pub surface: u32,
    pub border: u32,
    /// 分隔线与卡片边。
    pub line: u32,
    /// 输入框、次按钮与关闭态开关的描边。
    pub field: u32,
    pub text: u32,
    /// 候选序号等次要文字。
    pub label: u32,
    /// 注释、说明与提示。
    pub muted: u32,
    /// 停用文字。
    pub faint: u32,
    pub highlight: u32,
    /// 高亮行内的序号与注释。
    pub highlight_text: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub selection: u32,
}
impl Theme {
    pub const ALL: [Self; 5] = [
        Self::Amber,
        Self::Ink,
        Self::Bamboo,
        Self::Night,
        Self::Custom,
    ];
    pub const fn name(self) -> &'static str {
        match self {
            Self::Amber => "琥珀",
            Self::Ink => "墨",
            Self::Bamboo => "竹",
            Self::Night => "夜",
            Self::Custom => "自定义",
        }
    }
    /// 预设主题的固定配色；自定义主题返回 None，由 [`CustomColors::palette`] 推导。
    pub const fn preset(self) -> Option<Palette> {
        Some(match self {
            Self::Custom => return None,
            Self::Amber => Palette {
                window: 0xF9F8F5,
                surface: 0xFFFFFF,
                border: 0xDCD7CF,
                line: 0xE6E2DB,
                field: 0xD0CAC0,
                text: 0x1C1A17,
                label: 0x766F64,
                muted: 0x6E685F,
                faint: 0xB9B2A7,
                highlight: 0xF6E6D0,
                highlight_text: 0x9A4708,
                accent: 0xB45309,
                on_accent: 0xFFFFFF,
                selection: 0xF1D9B8,
            },
            Self::Ink => Palette {
                window: 0xF8F8F8,
                surface: 0xFFFFFF,
                border: 0xD9D9D9,
                line: 0xE3E3E3,
                field: 0xCCCCCC,
                text: 0x1A1A1A,
                label: 0x6E6E6E,
                muted: 0x666666,
                faint: 0xB3B3B3,
                highlight: 0xEBEBEB,
                highlight_text: 0x1A1A1A,
                accent: 0x262626,
                on_accent: 0xFFFFFF,
                selection: 0xDCDCDC,
            },
            Self::Bamboo => Palette {
                window: 0xF7F8F3,
                surface: 0xFFFFFF,
                border: 0xD5D9CC,
                line: 0xE2E5DA,
                field: 0xC9CEBE,
                text: 0x1A1C17,
                label: 0x6A7060,
                muted: 0x62685A,
                faint: 0xAFB5A5,
                highlight: 0xE4EDD3,
                highlight_text: 0x3F6212,
                accent: 0x4D7C0F,
                on_accent: 0xFFFFFF,
                selection: 0xD6E5BC,
            },
            Self::Night => Palette {
                window: 0x24221F,
                surface: 0x2E2B28,
                border: 0x45413B,
                line: 0x3A3733,
                field: 0x57524A,
                text: 0xEDEAE4,
                label: 0xA8A096,
                muted: 0xA39B90,
                faint: 0x6B655D,
                highlight: 0x4A3A28,
                highlight_text: 0xF2B66B,
                accent: 0xE0913A,
                on_accent: 0x1C1A17,
                selection: 0x5C4630,
            },
        })
    }
}
impl Palette {
    /// 深色主题需让系统标题栏按钮改用浅色。
    pub const fn dark(&self) -> bool {
        luma(self.window) < luma(self.text)
    }
    /// 第一对对比度低于 4.5:1 的前景/背景；None 表示全部可读。
    pub fn unreadable(&self) -> Option<(u32, u32)> {
        [
            (self.text, self.window),
            (self.text, self.surface),
            (self.text, self.highlight),
            (self.label, self.window),
            (self.muted, self.surface),
            (self.muted, self.window),
            (self.highlight_text, self.highlight),
            (self.on_accent, self.accent),
            (self.text, self.selection),
        ]
        .into_iter()
        .find(|&(fore, back)| contrast(fore, back) < 4.5)
    }
    pub const fn colors(&self) -> [u32; 14] {
        [
            self.window,
            self.surface,
            self.border,
            self.line,
            self.field,
            self.text,
            self.label,
            self.muted,
            self.faint,
            self.highlight,
            self.highlight_text,
            self.accent,
            self.on_accent,
            self.selection,
        ]
    }
}
/// 蓝分量不超过红、绿分量即不呈蓝、青、紫；中性灰三者相等也满足。
pub const fn without_blue(rgb: u32) -> bool {
    let (red, green, blue) = (rgb >> 16 & 0xFF, rgb >> 8 & 0xFF, rgb & 0xFF);
    blue <= red && blue <= green
}

/// 修饰键组合由 TSF 转成动作；无对应动作的系统快捷键保持透传。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Key {
    Character(char),
    Space,
    Enter,
    Backspace,
    Escape,
    Tab,
    PageUp,
    PageDown,
    Previous,
    Next,
    ToggleAscii,
    ToggleLearning,
    TogglePunctuation,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LearningInfo {
    pub codes: Vec<String>,
    /// 资料中的完整码未必是可输入码，UI 必须与 codes 分开展示。
    pub full_codes: Vec<String>,
    pub decomposition: Option<String>,
    pub readings: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub text: String,
    pub comment: String,
    pub label: String,
    pub source: Option<char>,
    pub learning: Option<LearningInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InputState {
    pub composition: Composition,
    pub candidates: Vec<Candidate>,
    pub page_size: usize,
    pub page_index: usize,
    pub is_last_page: bool,
    pub highlighted: Option<usize>,
    pub commit_preview: String,
    pub mode: InputMode,
    pub options: Options,
    pub ascii_mode: bool,
    /// 英文标点开关；TSF 本地按键判定须据此预判未组合时的标点是否由引擎处理。
    pub ascii_punctuation: bool,
    pub composing: bool,
    pub revision: u64,
}
impl InputState {
    /// TSF 的 OnTestKeyDown 不发 IPC，只凭上次状态判断是否吃键；
    /// 必须与引擎在该状态下的 handled 完全一致，由引擎测试穷举核对，多吃会吞掉应用按键。
    pub fn consumes(&self, key: Key) -> bool {
        match key {
            Key::ToggleAscii | Key::ToggleLearning | Key::TogglePunctuation => true,
            _ if self.ascii_mode => false,
            Key::Character(c) => {
                c == '`'
                    || c.is_ascii_lowercase()
                    || (!c.is_control()
                        && (self.composing
                            || (!self.ascii_punctuation
                                && matches!(
                                    c,
                                    ',' | '.'
                                        | '?'
                                        | '!'
                                        | ':'
                                        | ';'
                                        | '('
                                        | ')'
                                        | '<'
                                        | '>'
                                        | '['
                                        | ']'
                                        | '\\'
                                        | '_'
                                        | '^'
                                        | '\''
                                        | '"'
                                ))))
            }
            _ => self.composing,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyOutcome {
    pub handled: bool,
    pub commit: Option<String>,
    pub state: InputState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupItem {
    pub character: char,
    pub learning: LearningInfo,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookupPage {
    pub items: Vec<LookupItem>,
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_theme_is_free_of_blue_and_keeps_text_readable() {
        // 反例：系统强调蓝、青色与偏蓝灰都必须被拒绝。
        for blue in [0x0078D4, 0x00B7C3, 0x707080] {
            assert!(!without_blue(blue), "{blue:06X}");
        }
        for theme in Theme::ALL {
            let p = Options {
                theme,
                ..Options::default()
            }
            .palette();
            for color in p.colors() {
                assert!(without_blue(color), "{theme:?} {color:06X}");
            }
            assert_eq!(p.unreadable(), None, "{theme:?}");
        }
        assert!(Theme::Night.preset().unwrap().dark() && !Theme::Amber.preset().unwrap().dark());
    }
    #[test]
    fn custom_colors_derive_a_blue_free_palette_and_reject_blue_or_unreadable_input() {
        let mut options = Options {
            theme: Theme::Custom,
            ..Options::default()
        };
        assert!(options.validate().is_ok());
        // 深色、绿色基色同样推导出无蓝、可读的整套配色。
        options.custom = CustomColors {
            window: 0x202418,
            text: 0xE8EDDC,
            highlight: 0x3A4A22,
            accent: 0x9BC53D,
        };
        assert!(options.validate().is_ok());
        let palette = options.palette();
        assert!(palette.dark() && palette.colors().into_iter().all(without_blue));
        for (bad, reason) in [(0x0078D4, "偏蓝"), (0x1_000000, "超出")] {
            options.custom.accent = bad;
            assert!(options.validate().unwrap_err().contains(reason));
        }
        options.custom = CustomColors {
            text: 0xC8C8C8,
            ..CustomColors::default()
        };
        assert!(options.validate().unwrap_err().contains("对比度"));
    }
}
