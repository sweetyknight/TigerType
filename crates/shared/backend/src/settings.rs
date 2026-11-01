//! 受管设置文件 settings.toml：按带中文注释的固定模板写出，按固定结构读回。
//! 颜色写作 "#RRGGBB"；缺键、多键或类型不符都拒绝并指出行号，取值范围另由 `Options::validate` 把关。
use serde::{Deserialize, Deserializer};
use tigertype_contracts::{CandidateFont, CustomColors, Options, OutputForm, Theme, TypingMode};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    input: Input,
    candidate: Candidate,
    theme: ThemeSection,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    mode: TypingMode,
    common_only: bool,
    auto_commit_four: bool,
    output_form: OutputForm,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Candidate {
    font: CandidateFont,
    page_size: u8,
    show_learning: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeSection {
    name: Theme,
    custom: Custom,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Custom {
    #[serde(deserialize_with = "color")]
    background: u32,
    #[serde(deserialize_with = "color")]
    text: u32,
    #[serde(deserialize_with = "color")]
    highlight: u32,
    #[serde(deserialize_with = "color")]
    accent: u32,
}
fn color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let text = String::deserialize(deserializer)?;
    text.strip_prefix('#')
        .filter(|hex| hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
        .ok_or_else(|| serde::de::Error::custom(format!("颜色“{text}”须写成 \"#RRGGBB\"")))
}

/// 读回设置并校验；错误带行号，供设置页“重新载入”直接显示。
pub fn parse(bytes: &[u8]) -> Result<Options, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| "settings.toml 须为 UTF-8 编码".to_owned())?;
    // 记事本可能存成带 BOM 的 UTF-8；TOML 本身不认 BOM，读前去掉。
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let file: File = toml::from_str(text).map_err(|error| match error.span() {
        Some(span) => format!(
            "settings.toml 第 {} 行：{}",
            text[..span.start].matches('\n').count() + 1,
            error.message()
        ),
        None => format!("settings.toml 格式有误：{}", error.message()),
    })?;
    let options = Options {
        typing_mode: file.input.mode,
        common_only: file.input.common_only,
        auto_commit_four: file.input.auto_commit_four,
        output_form: file.input.output_form,
        candidate_font: file.candidate.font,
        page_size: file.candidate.page_size,
        show_learning: file.candidate.show_learning,
        theme: file.theme.name,
        custom: CustomColors {
            window: file.theme.custom.background,
            text: file.theme.custom.text,
            highlight: file.theme.custom.highlight,
            accent: file.theme.custom.accent,
        },
    };
    options.validate()?;
    Ok(options)
}

/// 按模板写出全部设置；枚举名取自序列化结果（ASCII 名的 JSON 字符串即 TOML 基本字符串），与读回同源。
pub fn render(options: &Options) -> Result<String, serde_json::Error> {
    let custom = options.custom;
    Ok(format!(
        r##"# 虎码 TigerType 设置
# 位置：%APPDATA%\TigerType\config\settings.toml
#
# 设置窗口里的修改会立即写回本文件（按本模板重写，自加的注释不保留）。
# 手工修改后，在“设置 → 高级 → 重新载入”生效；
# 格式有误时保留当前设置，并提示出错的行。

[input]
# 普通输入的码表："single" 单字 | "word" 词（同官方词版，字词都能打）
mode = {mode}

# 只出常用字；false 时普通输入也用全字集（查字始终用全字集）
common_only = {common_only}

# 编码满四码且只有一个候选时直接上屏
auto_commit_four = {auto_commit_four}

# 上屏字形："original" 原字 | "simplified" 简体 | "traditional" 繁体
output_form = {output_form}

[candidate]
# 候选字体："wenkai" 霞鹜文楷 | "kaiti" 楷体；未安装时回退到系统字体
font = {font}

# 每页候选数：3–9；数字键 1–N 选择，候选多于一页时才显示翻页
page_size = {page_size}

# 候选旁显示〔拆分 · 编码〕；输入编码时 Ctrl+L 可临时切换
show_learning = {show_learning}

[theme]
# 配色："amber" 琥珀 | "ink" 墨 | "bamboo" 竹 | "rose" 玫红 | "custom" 自定义
name = {theme}

# 自定义配色：仅 name = "custom" 时生效，其余颜色由这四色推导。
# 格式 "#RRGGBB"；不得偏蓝（蓝分量不超过红、绿），文字须清晰可读。
[theme.custom]
background = "#{background:06X}"  # 窗口与候选窗底色
text       = "#{text:06X}"  # 正文与候选字
highlight  = "#{highlight:06X}"  # 当前候选的底色
accent     = "#{accent:06X}"  # 开关、按钮与选中标记
"##,
        mode = serde_json::to_string(&options.typing_mode)?,
        common_only = options.common_only,
        auto_commit_four = options.auto_commit_four,
        output_form = serde_json::to_string(&options.output_form)?,
        font = serde_json::to_string(&options.candidate_font)?,
        page_size = options.page_size,
        show_learning = options.show_learning,
        theme = serde_json::to_string(&options.theme)?,
        background = custom.window,
        text = custom.text,
        highlight = custom.highlight,
        accent = custom.accent,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_option_roundtrips_through_the_commented_template() {
        let options = Options {
            auto_commit_four: true,
            common_only: false,
            show_learning: true,
            output_form: OutputForm::Traditional,
            theme: Theme::Custom,
            custom: CustomColors {
                window: 0xFFF8F6,
                text: 0x2A1A17,
                highlight: 0xFBE3DE,
                accent: 0xC2410C,
            },
            candidate_font: CandidateFont::KaiTi,
            typing_mode: TypingMode::Word,
            page_size: 7,
        };
        let text = render(&options).unwrap();
        assert!(text.contains("font = \"kaiti\"") && text.contains("accent     = \"#C2410C\""));
        assert_eq!(parse(text.as_bytes()).unwrap(), options);
        let default = render(&Options::default()).unwrap();
        assert_eq!(parse(default.as_bytes()).unwrap(), Options::default());
        // 带 BOM 的 UTF-8 同样接受。
        let bom = format!("\u{FEFF}{default}");
        assert_eq!(parse(bom.as_bytes()).unwrap(), Options::default());
    }

    #[test]
    fn mistakes_are_rejected_with_the_line_number() {
        let default = render(&Options::default()).unwrap();
        let line = |needle: &str| {
            default
                .lines()
                .position(|line| line.starts_with(needle))
                .unwrap()
                + 1
        };
        for (from, to, expected) in [
            ("name = \"amber\"", "name = \"blue\"", line("name =")),
            ("page_size = 5", "page_size = \"5\"", line("page_size")),
            (
                "accent     = \"#B45309\"",
                "accent     = \"B45309\"",
                line("accent"),
            ),
            (
                "common_only = true",
                "common_only = true\nextra = 1",
                line("common_only") + 1,
            ),
        ] {
            let broken = default.replace(from, to);
            assert_ne!(broken, default, "{from}");
            let error = parse(broken.as_bytes()).unwrap_err();
            assert!(
                error.starts_with(&format!("settings.toml 第 {expected} 行")),
                "{error}"
            );
        }
        // 缺一节、范围外、偏蓝与非 UTF-8 也都拒绝。
        let missing = default.replace("[candidate]", "[candidates]");
        assert!(parse(missing.as_bytes()).is_err());
        let range = default.replace("page_size = 5", "page_size = 12");
        assert!(parse(range.as_bytes()).unwrap_err().contains("每页候选数"));
        let blue = default
            .replace("name = \"amber\"", "name = \"custom\"")
            .replace("#B45309", "#0078D4");
        assert!(parse(blue.as_bytes()).unwrap_err().contains("偏蓝"));
        assert!(parse(&[0xFF, 0xFE]).unwrap_err().contains("UTF-8"));
    }
}
