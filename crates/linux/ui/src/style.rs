//! GTK 样式表与按钮文字：由主题色现生成 CSS，盖掉主题强调色（Adwaita 默认蓝）、选区、拖放与焦点环；
//! 只作用于带 [`ROOT`] 类的窗口，不改同进程其他窗口。纯文本生成，不链接 GTK，各宿主都测。
use tigertype_contracts::Palette;
use tigertype_ui_model::layout::{COPY, RELOAD, shades};

/// 查字/设置窗口根节点的样式类。
pub const ROOT: &str = "tigertype";

fn hex(rgb: u32) -> String {
    format!("#{:06X}", rgb & 0xFF_FFFF)
}

/// 整套窗口样式：标题栏用侧栏色（同预览），自绘控件只清掉原生底框；文本框、滚动条、
/// 右键菜单与拖放提示这些仍由 GTK 绘制的部分全部改用主题色。
pub fn css(p: Palette) -> String {
    let s = shades(p);
    let [
        window,
        surface,
        text,
        label,
        muted,
        line,
        field,
        accent,
        selection,
        sidebar,
        track,
    ] = [
        p.window,
        p.surface,
        p.text,
        p.label,
        p.muted,
        p.line,
        p.field,
        p.accent,
        p.selection,
        s.sidebar,
        s.track,
    ]
    .map(hex);
    let r = format!("window.{ROOT}");
    format!(
        "{r} {{ background-color: {window}; color: {text}; }}
{r} headerbar {{ background: {sidebar}; color: {text}; min-height: 45px; padding: 0 10px; border: none; border-bottom: 1px solid {line}; box-shadow: none; }}
{r} headerbar:backdrop {{ background: {sidebar}; color: {muted}; }}
{r} headerbar .title {{ font-size: 14px; font-weight: 600; }}
{r} windowcontrols {{ border-spacing: 10px; }}
{r} windowcontrols button {{ min-width: 24px; min-height: 24px; padding: 0; margin: 0; border: none; border-radius: 12px; background: {track}; color: {text}; box-shadow: none; outline-color: {text}; }}
{r} windowcontrols button:hover {{ background: {line}; }}
{r} windowcontrols button:active {{ background: {field}; }}
{r} windowcontrols button image {{ -gtk-icon-size: 14px; }}
{r} button.tt {{ background: none; border: none; border-radius: 0; box-shadow: none; outline: none; padding: 0; margin: 0; min-width: 0; min-height: 0; }}
{r} text.tt {{ background: none; color: {text}; caret-color: {accent}; border: none; box-shadow: none; outline: none; padding: 0; margin: 0; min-height: 0; font-size: 14px; }}
{r} text.tt.mono {{ font-family: \"Cascadia Mono\", monospace; font-size: 13px; }}
{r} text.tt selection {{ background-color: {selection}; color: {text}; }}
{r} text.tt cursor-handle {{ color: {accent}; }}
{r} text.tt placeholder {{ color: {muted}; }}
{r} label.tt {{ color: {text}; }}
{r} label.tt.label {{ color: {label}; }}
{r} label.tt.muted {{ color: {muted}; }}
{r} label.tt.strong {{ font-weight: 600; }}
{r} label.tt.mono {{ font-family: \"Cascadia Mono\", monospace; }}
{r} .size-12 {{ font-size: 12px; }}
{r} .size-13 {{ font-size: 13px; }}
{r} .size-14 {{ font-size: 14px; }}
{r} .size-20 {{ font-size: 20px; }}
{r} scrollbar {{ background: none; border: none; }}
{r} scrollbar slider {{ background-color: {field}; border: none; }}
{r} scrollbar slider:hover {{ background-color: {muted}; }}
{r} scrollbar slider:active {{ background-color: {text}; }}
{r} overshoot, {r} undershoot {{ background: none; box-shadow: none; }}
{r} *:drop(active) {{ box-shadow: none; outline: none; }}
{r} popover > contents {{ background-color: {surface}; color: {text}; }}
{r} popover modelbutton:hover, {r} popover modelbutton:focus {{ background-color: {track}; color: {text}; }}
{r} popover separator {{ background-color: {line}; }}
"
    )
}

/// 自绘按钮显示的文字与 Alt 助记字母：同 Windows，(&X) 画作 (X)；带图标的按钮只画括号前的名称。
pub fn caption(id: usize, text: &str) -> (String, Option<char>) {
    let key = text
        .find("(&")
        .and_then(|at| text[at + 2..].chars().next())
        .filter(char::is_ascii_alphabetic)
        .map(|c| c.to_ascii_lowercase());
    let shown = if matches!(id, COPY | RELOAD) {
        text.split('(').next().unwrap_or(text).to_owned()
    } else {
        text.replace("(&", "(")
    };
    (shown, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::{Options, Theme, without_blue};
    use tigertype_ui_model::layout::{AUTO, OPEN_CONFIG};

    #[test]
    fn every_color_in_the_stylesheet_comes_from_a_blue_free_theme() {
        for theme in Theme::ALL {
            let css = css(Options {
                theme,
                ..Default::default()
            }
            .palette());
            let colors: Vec<u32> = css
                .match_indices('#')
                .map(|(at, _)| u32::from_str_radix(&css[at + 1..at + 7], 16).unwrap())
                .collect();
            assert!(colors.len() > 20, "{theme:?} 样式表缺少主题色");
            assert!(colors.iter().all(|&c| without_blue(c)), "{theme:?}");
            // 系统强调色、选区与拖放这几处蓝色来源都须被主题色覆盖，并且只作用于本程序窗口。
            for selector in [
                "text.tt selection",
                "caret-color",
                "*:drop(active)",
                "scrollbar slider:active",
                "headerbar",
            ] {
                assert!(css.contains(selector), "{selector}");
            }
            assert!(
                css.lines()
                    .filter(|l| !l.trim().is_empty())
                    .all(|l| l.starts_with("window.tigertype"))
            );
        }
    }
    #[test]
    fn captions_follow_windows_mnemonics() {
        assert_eq!(
            caption(OPEN_CONFIG, "打开(&O)"),
            ("打开(O)".into(), Some('o'))
        );
        assert_eq!(caption(COPY, "复制(&C)"), ("复制".into(), Some('c')));
        assert_eq!(
            caption(RELOAD, "重新载入(&R)"),
            ("重新载入".into(), Some('r'))
        );
        assert_eq!(
            caption(AUTO, "四码唯一时自动上屏(&A)"),
            ("四码唯一时自动上屏(A)".into(), Some('a'))
        );
        assert_eq!(caption(AUTO, "外观"), ("外观".into(), None));
    }
}
