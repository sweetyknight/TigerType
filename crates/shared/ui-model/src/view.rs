//! 查字/设置控件的内容与状态：固定文字、文字样式、选中、可用与显隐。
//! 三端视图取用共用草稿的内容与状态；平台只创建原生控件、转发动作并绘制。
use crate::{
    editor::{Editor, Setting},
    layout::*,
    model::parts,
};
use tigertype_contracts::{CandidateFont, LookupItem, Theme, TypingMode};

/// 静态文字的颜色角色，对应 Windows WM_CTLCOLORSTATIC 的正文、分组名与次要色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Text,
    Label,
    Muted,
}
/// 静态文字样式：字号（逻辑像素）、粗细、等宽、颜色、垂直居中（SS_CENTERIMAGE）、水平居中与折行。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    pub size: f64,
    pub strong: bool,
    pub mono: bool,
    pub tone: Tone,
    pub middle: bool,
    pub center: bool,
    pub wrap: bool,
}
pub fn text_style(id: usize) -> TextStyle {
    let (size, strong, mono, tone, middle) = match id {
        HEADING => (20.0, true, false, Tone::Text, false),
        _ if (GROUP..GROUP + GROUPS.len()).contains(&id) => (13.0, true, false, Tone::Label, false),
        FILE_NAME | ABOUT_TITLE => (13.0, true, false, Tone::Text, false),
        FILE_PATH | CODEPOINT => (13.0, false, true, Tone::Muted, false),
        PAGE => (13.0, false, true, Tone::Muted, true),
        CUSTOM_STATUS => (12.0, false, false, Tone::Muted, true),
        _ if (ROW_TITLE..=ROW_TITLE + CHOICES.len()).contains(&id) => {
            (14.0, false, false, Tone::Text, true)
        }
        _ if (FIELD_LABEL..FIELD_LABEL + 4).contains(&id) => {
            (12.0, false, false, Tone::Muted, true)
        }
        _ if (ROW_DESC..=ROW_DESC + CHOICES.len()).contains(&id)
            || matches!(id, NOTICE | KEYS_NOTE | ABOUT_DESC | STATUS) =>
        {
            (12.0, false, false, Tone::Muted, false)
        }
        _ => (14.0, false, false, Tone::Text, false),
    };
    TextStyle {
        size,
        strong,
        mono,
        tone,
        middle,
        center: matches!(id, CODEPOINT | PAGE | STATUS),
        wrap: matches!(id, NOTICE | STATUS | CUSTOM_STATUS) || id == ROW_DESC + CHOICES.len(),
    }
}

/// 控件固定文字，保留 Windows 助记键写法 (&X)：Mac 去掉，Linux 换成 GTK 下划线助记键。
/// page 只决定分区标题；设置路径、查字资料等运行时文字由视图另取，其余控件为空串。
pub fn text(id: usize, page: usize) -> String {
    if let Some((_, name, _)) = TOGGLES.iter().find(|(key, ..)| *key == id) {
        return (*name).into();
    }
    let in_range = |start: usize, count: usize| (start..start + count).contains(&id);
    match id {
        _ if in_range(MODE, 2) => TypingMode::ALL[id - MODE].name().into(),
        _ if in_range(FORM, FORMS.len()) => FORMS[id - FORM].1.into(),
        _ if in_range(FONT, 2) => CandidateFont::ALL[id - FONT].name().into(),
        _ if in_range(SIZE_CHOICE, 7) => (id - SIZE_CHOICE + 3).to_string(),
        _ if in_range(THEME, Theme::ALL.len()) => Theme::ALL[id - THEME].name().into(),
        _ if in_range(SWATCH, COLORS.len()) => format!("{}色", COLORS[id - SWATCH].0),
        _ if in_range(NAV, PAGES.len()) => PAGES[id - NAV].0.into(),
        _ if in_range(GROUP, GROUPS.len()) => GROUPS[id - GROUP].into(),
        _ if in_range(ROW_TITLE, CHOICES.len()) => CHOICES[id - ROW_TITLE].0.into(),
        _ if in_range(ROW_DESC, CHOICES.len()) => CHOICES[id - ROW_DESC].1.into(),
        _ if id == ROW_TITLE + CHOICES.len() => RELOAD_TITLE.into(),
        _ if id == ROW_DESC + CHOICES.len() => RELOAD_DESC.into(),
        _ if in_range(FIELD_LABEL, 4) => {
            ["可用编码", "完整编码", "读音", "拆分"][id - FIELD_LABEL].into()
        }
        HEADING => PAGES[page.min(PAGES.len() - 1)].0.into(),
        OPEN_CONFIG => OPEN_TEXT.into(),
        RELOAD => RELOAD_TEXT.into(),
        RESET => RESET_TEXT.into(),
        FILE_NAME => "settings.toml".into(),
        ABOUT_TITLE => ABOUT_NAME.into(),
        ABOUT_DESC => about_description(),
        KEYS_NOTE => KEYS_NOTE_TEXT.into(),
        QUERY_LABEL => "汉字或拼音(&Q)".into(),
        SETTINGS => "设置(&S)".into(),
        PREVIOUS => "上一页(&P)".into(),
        NEXT => "下一页(&N)".into(),
        COPY => "复制(&C)".into(),
        _ => String::new(),
    }
}
/// 去掉 Windows 助记键 (&X)；只用于不显示助记键的平台。
pub fn strip_mnemonic(text: &str) -> String {
    let mut value = text.to_owned();
    for c in 'A'..='Z' {
        value = value.replace(&format!("(&{c})"), "");
    }
    value
}

/// 快捷键说明：“N”按当前每页候选数显示。
pub fn keys(spec: &str, page_size: u8) -> String {
    spec.replace("[N]", &format!("[{page_size}]"))
}
/// 按方括号切成键帽（true）与说明文字（键名可含空格，如 fn ↑），三端同一切法。
pub fn key_parts(spec: &str) -> Vec<(bool, &str)> {
    spec.split_inclusive(']')
        .flat_map(|part| match part.find('[') {
            Some(at) => vec![&part[..at], &part[at..]],
            None => vec![part],
        })
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(
            |part| match part.strip_prefix('[').and_then(|k| k.strip_suffix(']')) {
                Some(key) => (true, key),
                None => (false, part),
            },
        )
        .collect()
}
/// 读屏朗读的快捷键行：去掉键帽括号，画面仍按 KEYMAP 自绘。
pub fn key_speech(name: &str, spec: &str) -> String {
    let spoken: Vec<&str> = key_parts(spec).into_iter().map(|(_, part)| part).collect();
    format!("{name}：{}", spoken.join(" "))
}

/// 自定义配色行下方的提示：可读时报最低对比度，否则说明原因。
pub fn contrast_status(check: Result<f64, String>) -> String {
    match check {
        Ok(contrast) => format!("✓ 全部文字可读，最低对比度 {contrast:.1} : 1"),
        Err(error) => format!("✗ {error}；修正后自动保存"),
    }
}
pub fn custom_status(e: &Editor) -> String {
    contrast_status(match &e.invalid {
        Some(error) => Err(error.clone()),
        None => Ok(e.appearance().palette().min_contrast()),
    })
}

/// 查字详情四行：可用编码、完整编码（空格分隔）、读音（顿号分隔）与拆分；缺项写“缺资料”。
pub fn detail_texts(item: &LookupItem) -> [String; 4] {
    let join = |items: &[String], separator: &str| {
        if items.is_empty() {
            "缺资料".into()
        } else {
            items.join(separator)
        }
    };
    [
        join(&item.learning.codes, " "),
        join(&item.learning.full_codes, " "),
        join(&item.learning.readings, "、"),
        parts(item),
    ]
}
/// 单字的码位；词不显示。
pub fn codepoint(item: &LookupItem) -> String {
    let mut chars = item.text.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => format!("U+{:04X}", c as u32),
        _ => String::new(),
    }
}
/// 查字窗随结果变化的文字；texts 为 [`detail_texts`] 的结果（无资料时为空）。None 表示文字固定。
pub fn lookup_value(e: &Editor, id: usize, texts: &[String; 4]) -> Option<String> {
    let item = e.item();
    Some(match id {
        STATUS => e.model.notice.clone(),
        PAGE => e
            .model
            .result
            .as_ref()
            .map(|_| (e.model.page + 1).to_string())
            .unwrap_or_default(),
        GLYPH => item.map(|i| i.text.clone()).unwrap_or_default(),
        CODEPOINT => item.map(codepoint).unwrap_or_default(),
        CODES => texts[0].clone(),
        FULL => texts[1].clone(),
        READING => texts[2].clone(),
        PARTS => texts[3].clone(),
        _ => return None,
    })
}
/// 查字窗控件是否显示：没有当前字时只显示提示，有资料时提示让位。
pub fn lookup_visible(e: &Editor, id: usize) -> bool {
    let detail = [GLYPH, CODEPOINT, COPY, CODES, FULL, READING, PARTS].contains(&id)
        || (FIELD_LABEL..FIELD_LABEL + 4).contains(&id);
    let item = e.item().is_some();
    !(detail && !item) && !(id == STATUS && item)
}
/// 控件可用状态：故障后全部停用；重新载入期间不接受设置编辑；在途时不能翻页或再载入。
pub fn enabled(e: &Editor, id: usize) -> bool {
    if e.fatal {
        return false;
    }
    let edits = Setting::of(id).is_some()
        || (SWATCH..SWATCH + COLORS.len()).contains(&id)
        || (CUSTOM..CUSTOM + COLORS.len()).contains(&id)
        || id == RESET;
    match id {
        PREVIOUS => !e.model.busy && e.model.page > 0,
        NEXT => !e.model.busy && e.model.result.as_ref().is_some_and(|r| r.has_more),
        COPY => e.item().is_some(),
        RELOAD => !e.model.busy,
        _ => !edits || !e.model.reloading(),
    }
}
/// 按草稿显示的选中状态：开关、单选与侧栏分区。
pub fn selected(e: &Editor, id: usize) -> bool {
    match Setting::of(id) {
        Some(setting) => setting.selected(e.draft),
        None => (NAV..NAV + PAGES.len()).contains(&id) && e.page == id - NAV,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::Action;
    use tigertype_contracts::{
        LearningInfo, LookupPage, Options,
        ui::{Reply, Request},
    };
    #[test]
    fn key_specs_split_into_keycaps_and_text_with_the_page_size() {
        assert_eq!(
            key_parts(&keys("[1] – [N]", 7)),
            [(true, "1"), (false, "–"), (true, "7")]
        );
        assert_eq!(
            key_parts("[Shift] 单击"),
            [(true, "Shift"), (false, "单击")]
        );
        assert_eq!(
            key_parts("[Ctrl] + [.]"),
            [(true, "Ctrl"), (false, "+"), (true, ".")]
        );
        assert_eq!(
            key_speech("翻页", "[-] [=] [PgUp] [PgDn]"),
            "翻页：- = PgUp PgDn"
        );
        assert_eq!(strip_mnemonic("仅常用字(&C)"), "仅常用字");
    }
    #[test]
    fn every_laid_out_text_control_has_words_and_a_style() {
        for page in 0..PAGES.len() {
            let spec = settings_spec(page, true);
            for (id, _) in spec.fixed.iter().chain(&spec.controls) {
                let blank = [NOTICE, CUSTOM_STATUS, PREVIEW, FILE_PATH].contains(id)
                    || (KEYS..KEYS + KEYMAP.len()).contains(id)
                    || (CUSTOM..CUSTOM + COLORS.len()).contains(id);
                assert_eq!(text(*id, page).is_empty(), blank, "控件 {id}");
            }
        }
        assert_eq!(text(HEADING, 9), "高级", "分区越界时用最后一区标题");
        assert_eq!(text(ROW_TITLE + 4, 3), RELOAD_TITLE);
        assert!(text_style(ROW_DESC + 4).wrap);
        assert_eq!(text_style(GROUP + 2).tone, Tone::Label);
        assert!(text_style(PAGE).middle && text_style(PAGE).center && text_style(PAGE).mono);
    }
    #[test]
    fn lookup_state_shows_details_only_for_a_current_item_and_guards_paging() {
        let mut e = Editor::new(Options::default());
        assert!(lookup_visible(&e, STATUS) && !lookup_visible(&e, GLYPH));
        assert!(!enabled(&e, COPY) && !enabled(&e, NEXT) && !enabled(&e, PREVIOUS));
        e.act(Action::Query("han".into()));
        let item = LookupItem {
            text: "𠀀".into(),
            learning: LearningInfo {
                codes: vec!["aa".into(), "aab".into()],
                ..Default::default()
            },
            roots: vec![],
        };
        e.reply(Ok(Reply::Lookup(Ok(LookupPage {
            items: vec![item.clone()],
            has_more: true,
        }))));
        let texts = detail_texts(&item);
        assert_eq!(
            texts,
            ["aa aab", "缺资料", "缺资料", "缺资料"].map(String::from)
        );
        assert!(!lookup_visible(&e, STATUS) && lookup_visible(&e, PARTS));
        assert!(enabled(&e, COPY) && enabled(&e, NEXT) && !enabled(&e, PREVIOUS));
        assert_eq!(lookup_value(&e, CODEPOINT, &texts).unwrap(), "U+20000");
        assert_eq!(lookup_value(&e, PAGE, &texts).unwrap(), "1");
        assert_eq!(lookup_value(&e, CODES, &texts).unwrap(), "aa aab");
        assert_eq!(lookup_value(&e, QUERY, &texts), None);
        assert_eq!(
            codepoint(&LookupItem {
                text: "虎码".into(),
                ..item
            }),
            ""
        );
    }
    #[test]
    fn settings_state_follows_the_draft_reload_and_fatal_errors() {
        let mut e = Editor::new(Options::default());
        assert!(selected(&e, COMMON) && !selected(&e, AUTO));
        assert!(selected(&e, NAV) && !selected(&e, NAV + 1));
        assert!(selected(&e, SIZE_CHOICE + 2), "默认每页 5 个");
        let Some(crate::editor::Task::Send(Request::ReloadSettings)) = e.act(Action::Reload) else {
            panic!("没有重新载入");
        };
        assert!(!enabled(&e, AUTO) && !enabled(&e, CUSTOM) && !enabled(&e, RELOAD));
        assert!(enabled(&e, OPEN_CONFIG));
        assert!(custom_status(&e).starts_with('✓'));
        e.reply(Ok(Reply::Saved(Ok(Options::default()))));
        assert!(enabled(&e, AUTO));
        e.color(0, "#0078D4".into());
        assert!(custom_status(&e).starts_with("✗ "));
        e.fatal = true;
        assert!(!enabled(&e, OPEN_CONFIG));
    }
}
