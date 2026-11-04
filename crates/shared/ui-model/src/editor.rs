//! 查字/设置窗的界面状态：草稿、查询、设置分区与色板浮层。Windows、Mac 与 Linux 视图共用；
//! 只产生请求和平台任务，不读盘、不执行请求，也不碰窗口。复用 Model 的在途请求匹配。
use crate::{
    layout::{COLORS, FONT, FORM, FORMS, MODE, PAGES, SIZE_CHOICE, THEME},
    model::{Effect, Model, details},
};
use tigertype_contracts::{
    CandidateFont, LookupItem, Options, OutputForm, Theme, TypingMode,
    ui::{Reply, Request},
};

/// 设置窗的一个可选值：开关取反，单选写入该值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Auto,
    Common,
    Learning,
    Mode(TypingMode),
    Form(OutputForm),
    Font(CandidateFont),
    Size(u8),
    Theme(Theme),
}
impl Setting {
    /// 控件编号对应的设置项；不是设置控件时为 None。
    pub fn of(id: usize) -> Option<Self> {
        use crate::layout::{AUTO, COMMON, LEARNING};
        Some(match id {
            AUTO => Self::Auto,
            COMMON => Self::Common,
            LEARNING => Self::Learning,
            id if (MODE..MODE + 2).contains(&id) => Self::Mode(TypingMode::ALL[id - MODE]),
            id if (FORM..FORM + 3).contains(&id) => Self::Form(FORMS[id - FORM].0),
            id if (FONT..FONT + 2).contains(&id) => Self::Font(CandidateFont::ALL[id - FONT]),
            id if (SIZE_CHOICE..SIZE_CHOICE + 7).contains(&id) => {
                Self::Size((id - SIZE_CHOICE + 3) as u8)
            }
            id if (THEME..THEME + Theme::ALL.len()).contains(&id) => {
                Self::Theme(Theme::ALL[id - THEME])
            }
            _ => return None,
        })
    }
    pub fn apply(self, o: &mut Options) {
        match self {
            Self::Auto => o.auto_commit_four = !o.auto_commit_four,
            Self::Common => o.common_only = !o.common_only,
            Self::Learning => o.show_learning = !o.show_learning,
            Self::Mode(v) => o.typing_mode = v,
            Self::Form(v) => o.output_form = v,
            Self::Font(v) => o.candidate_font = v,
            Self::Size(v) => o.page_size = v,
            Self::Theme(v) => o.theme = v,
        }
    }
    pub fn selected(self, o: Options) -> bool {
        match self {
            Self::Auto => o.auto_commit_four,
            Self::Common => o.common_only,
            Self::Learning => o.show_learning,
            Self::Mode(v) => o.typing_mode == v,
            Self::Form(v) => o.output_form == v,
            Self::Font(v) => o.candidate_font == v,
            Self::Size(v) => o.page_size == v,
            Self::Theme(v) => o.theme == v,
        }
    }
}

/// 用户在查字/设置窗上的动作。打开设置窗、复制与打开配置文件由平台执行，见 [`Task`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 查询框改动或回车：边输边查首页。
    Query(String),
    /// 方向键在字块间移动：true 为下一个。
    Move(bool),
    Select(usize),
    Previous,
    Next,
    Copy,
    Settings,
    Page(usize),
    Set(Setting),
    /// 自定义色输入框改动（序号、文字）。
    Color(usize, String),
    /// 打开某一行的色板浮层。
    Palette(usize),
    /// 在色板浮层里选色。
    Swatch(u32),
    Reset,
    Reload,
    Open,
}
/// 动作留给平台完成的后续。
#[derive(Debug)]
pub enum Task {
    /// 交给工作线程的请求。
    Send(Request),
    /// 写入剪贴板的当前字资料。
    Copy(String),
    ShowSettings,
    /// 用默认编辑器打开 settings.toml。
    OpenConfig,
}

pub struct Editor {
    pub model: Model,
    pub draft: Options,
    pub colors: [String; 4],
    pub invalid: Option<String>,
    /// 当前字块序号。
    pub selected: usize,
    /// 回复与在途操作错配：此后只显示错误，不再接受编辑。
    pub fatal: bool,
    /// 查询框当前文字；在途期间改过时，回复后按它补查。
    pub query: String,
    /// 设置窗当前分区。
    pub page: usize,
    /// 打开了色板浮层的自定义色行。
    pub swatch: Option<usize>,
}
impl Editor {
    /// 非法草稿只留在输入框中，绝不能把偏蓝/低对比度颜色画到预览或其他窗口。
    pub fn appearance(&self) -> Options {
        if self.invalid.is_none() && self.draft.validate().is_ok() {
            self.draft
        } else {
            self.model.options
        }
    }
    pub fn new(options: Options) -> Self {
        let mut this = Self {
            model: Model::new(options),
            draft: options,
            colors: Default::default(),
            invalid: None,
            selected: 0,
            fatal: false,
            query: String::new(),
            page: 0,
            swatch: None,
        };
        this.reset();
        this
    }
    fn reset(&mut self) {
        self.draft = self.model.options;
        let c = self.draft.custom;
        self.colors = [c.window, c.text, c.highlight, c.accent].map(|c| format!("#{c:06X}"));
        self.invalid = None;
    }
    /// 当前字块的资料。
    pub fn item(&self) -> Option<&LookupItem> {
        self.model
            .result
            .as_ref()
            .and_then(|r| r.items.get(self.selected))
    }
    /// 色板浮层只在外观区、自定义主题且未在重新载入时显示。
    pub fn swatch_open(&self) -> bool {
        self.swatch.is_some()
            && self.page == 1
            && self.draft.theme == Theme::Custom
            && !self.model.reloading()
    }
    /// Esc 先收起色板浮层；返回 true 表示已收起，不再关闭窗口。
    pub fn dismiss_swatch(&mut self) -> bool {
        self.swatch.take().is_some()
    }
    /// 关闭设置窗：收起色板，未填完或不合格的色值不留到下次打开。
    pub fn close_settings(&mut self) {
        self.swatch = None;
        self.drop_invalid();
        if self.model.settings_notice.starts_with('✗') {
            self.model.settings_notice.clear();
        }
    }
    /// 四色退回最后一份合法设置（正在保存的，否则已确认的）；合法草稿都已立即保存或在途。
    fn drop_invalid(&mut self) {
        if self.invalid.take().is_some() {
            self.draft.custom = self.model.saving().unwrap_or(self.model.options).custom;
            let c = self.draft.custom;
            self.colors = [c.window, c.text, c.highlight, c.accent].map(|c| format!("#{c:06X}"));
        }
    }
    pub fn act(&mut self, action: Action) -> Option<Task> {
        match action {
            // 唤起设置窗与打开配置文件不改数据，故障后仍可用。
            Action::Settings => return Some(Task::ShowSettings),
            Action::Open => return Some(Task::OpenConfig),
            _ if self.fatal => return None,
            Action::Copy => return self.item().map(|item| Task::Copy(details(item))),
            _ => {}
        }
        let request = match action {
            Action::Query(query) => {
                self.query = query.clone();
                self.model.edit(query)
            }
            Action::Move(next) => {
                let count = self.model.result.as_ref().map_or(0, |r| r.items.len());
                self.selected = if next {
                    self.selected.saturating_add(1).min(count.saturating_sub(1))
                } else {
                    self.selected.saturating_sub(1)
                };
                None
            }
            Action::Select(index) => {
                if self
                    .model
                    .result
                    .as_ref()
                    .is_some_and(|r| index < r.items.len())
                {
                    self.selected = index;
                }
                None
            }
            Action::Previous => self.turn(self.model.page.saturating_sub(1)),
            Action::Next => self.turn(self.model.page.saturating_add(1)),
            Action::Page(page) => {
                self.page = page.min(PAGES.len() - 1);
                self.swatch = None;
                None
            }
            Action::Set(setting) => {
                // 离开自定义主题时，不合格的四色退回合法值，免得挡住换预设主题的保存。
                if matches!(setting, Setting::Theme(t) if t != Theme::Custom)
                    && !self.model.reloading()
                {
                    self.drop_invalid();
                }
                self.edit(|o| setting.apply(o))
            }
            Action::Color(index, value) => self.color(index, value),
            Action::Palette(index) => {
                self.swatch = (index < COLORS.len()).then_some(index);
                None
            }
            Action::Swatch(value) => self
                .swatch
                .take()
                .and_then(|i| self.color(i, format!("#{value:06X}"))),
            Action::Reset => self.reset_colors(),
            Action::Reload => self
                .model
                .reload()
                .map_err(|e| self.model.settings_notice = e.into())
                .ok(),
            Action::Copy | Action::Settings | Action::Open => None,
        };
        request.map(Task::Send)
    }
    /// 换页只基于已显示的查询；失效时把原因写进查字提示。
    fn turn(&mut self, page: usize) -> Option<Request> {
        let query = self.model.query.clone();
        self.model
            .lookup(query, page)
            .map_err(|e| self.model.notice = e.into())
            .ok()
    }
    /// 工作线程回复或通道故障；返回下一个要发的请求：补存设置优先，其回复后再补查最新查询。
    pub fn reply(&mut self, reply: Result<Reply, String>) -> Option<Request> {
        let next = match reply {
            Ok(reply) => self.receive(reply),
            Err(error) => {
                self.failed(error);
                None
            }
        };
        match next {
            None if !self.fatal && self.model.take_stale() => {
                let query = self.query.clone();
                self.model.edit(query)
            }
            next => next,
        }
    }
    /// 自定义色输入：可带或不带“#”，须恰为 6 位 ASCII 十六进制，不按字符索引截断 UTF-8。
    pub fn color(&mut self, index: usize, value: String) -> Option<Request> {
        if self.fatal || self.model.reloading() || index >= self.colors.len() {
            return None;
        }
        self.colors[index] = value;
        let parsed: Result<Vec<_>, _> = self
            .colors
            .iter()
            .enumerate()
            .map(|(k, text)| {
                let hex = text.trim().trim_start_matches('#');
                u32::from_str_radix(hex, 16)
                    .ok()
                    .filter(|_| hex.len() == 6 && hex.bytes().all(|c| c.is_ascii_hexdigit()))
                    .ok_or_else(|| {
                        format!("{}色“{text}”须为 6 位十六进制，如 #F9F8F5", COLORS[k].0)
                    })
            })
            .collect();
        match parsed {
            Ok(c) => {
                self.draft.custom = tigertype_contracts::CustomColors {
                    window: c[0],
                    text: c[1],
                    highlight: c[2],
                    accent: c[3],
                };
                self.invalid = self.draft.validate().err();
            }
            Err(error) => self.invalid = Some(error),
        }
        self.save()
    }
    pub fn edit(&mut self, change: impl FnOnce(&mut Options)) -> Option<Request> {
        if self.fatal || self.model.reloading() {
            return None;
        }
        change(&mut self.draft);
        self.save()
    }
    /// 只重置四个基色，不覆盖同一草稿里的输入设置。
    pub fn reset_colors(&mut self) -> Option<Request> {
        if self.fatal || self.model.reloading() {
            return None;
        }
        self.draft.custom = Default::default();
        let c = self.draft.custom;
        self.colors = [c.window, c.text, c.highlight, c.accent].map(|c| format!("#{c:06X}"));
        self.invalid = None;
        self.save()
    }
    /// 即时生效：草稿合法且与已确认设置不同才保存；在途时不排队，回复后再比一次。
    /// 草稿不合格时侧栏写明未保存，在其他分区改设置时也看得到；草稿恢复合法即撤下。
    pub fn save(&mut self) -> Option<Request> {
        if self.fatal {
            return None;
        }
        if let Some(error) = self.invalid.clone().or_else(|| self.draft.validate().err()) {
            self.model.settings_notice = format!("✗ 未保存：{error}");
            self.invalid = Some(error);
            return None;
        }
        if self.model.settings_notice.starts_with('✗') {
            self.model.settings_notice.clear();
        }
        if self.model.busy || self.draft == self.model.options {
            return None;
        }
        self.model.save(self.draft).ok()
    }
    pub fn receive(&mut self, reply: Reply) -> Option<Request> {
        match self.model.receive(reply) {
            Ok(Effect::Controls) => self.reset(),
            Ok(Effect::Page) => self.selected = 0,
            Ok(Effect::Nothing) => {}
            Err(error) => {
                self.fatal = true;
                self.model.notice = error.into();
                self.model.settings_notice = error.into();
            }
        }
        // 最多一个在途；其间的编辑合并成最新草稿，不积累保存队列。
        self.save()
    }
    pub fn failed(&mut self, error: String) {
        self.model.fail(error);
        self.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{AUTO, NAV, SWATCHES};
    use tigertype_contracts::{LearningInfo, LookupPage};
    fn page(texts: &[&str], has_more: bool) -> Reply {
        Reply::Lookup(Ok(LookupPage {
            items: texts
                .iter()
                .map(|text| LookupItem {
                    text: (*text).into(),
                    learning: LearningInfo::default(),
                    roots: vec![],
                })
                .collect(),
            has_more,
        }))
    }
    #[test]
    fn draft_coalesces_and_failed_save_rolls_back_without_retrying_forever() {
        let mut e = Editor::new(Options::default());
        let Some(Request::SaveSettings(first)) = e.edit(|o| o.page_size = 3) else {
            panic!("缺少保存");
        };
        assert!(e.edit(|o| o.page_size = 9).is_none());
        assert_eq!(e.model.options.page_size, 5);
        let Some(Request::SaveSettings(last)) = e.receive(Reply::Saved(Ok(first))) else {
            panic!("未合并");
        };
        assert_eq!(last.page_size, 9);
        assert!(e.receive(Reply::Saved(Err("磁盘冲突".into()))).is_none());
        assert_eq!(e.draft.page_size, 3);
        assert_eq!(e.model.options.page_size, 3);
    }
    #[test]
    fn invalid_color_is_never_saved_even_after_an_earlier_save_finishes() {
        let mut e = Editor::new(Options::default());
        let Some(Request::SaveSettings(first)) = e.edit(|o| o.page_size = 3) else {
            panic!();
        };
        for bad in ["#汉汉", "#0078D4", "#FFFFF", "#GGGGGG"] {
            assert!(e.color(3, bad.into()).is_none());
            assert!(e.invalid.is_some());
            assert_eq!(e.appearance(), e.model.options);
        }
        assert!(e.receive(Reply::Saved(Ok(first))).is_none());
        e.failed("通道断开".into());
        assert!(e.invalid.is_none());
        assert_eq!(e.draft, e.model.options);
        assert!(SWATCHES.into_iter().all(tigertype_contracts::without_blue));
    }
    #[test]
    fn mismatched_reply_is_terminal_and_reload_adopts_confirmed_options() {
        let mut e = Editor::new(Options::default());
        e.model.reload().unwrap();
        let updated = Options {
            page_size: 7,
            ..e.draft
        };
        assert!(e.receive(Reply::Saved(Ok(updated))).is_none());
        assert_eq!(e.draft, updated);
        assert!(e.receive(Reply::Saved(Ok(updated))).is_none());
        assert!(e.fatal);
        assert!(e.edit(|o| o.page_size = 9).is_none());
        // 故障后只剩不改数据的唤起与打开文件。
        assert!(e.act(Action::Query("han".into())).is_none());
        assert!(matches!(e.act(Action::Settings), Some(Task::ShowSettings)));
        assert!(matches!(e.act(Action::Open), Some(Task::OpenConfig)));
    }
    #[test]
    fn resetting_colors_recovers_invalid_input_without_resetting_other_drafts() {
        let mut e = Editor::new(Options::default());
        e.color(3, "#invalid".into());
        assert!(e.edit(|o| o.page_size = 7).is_none());
        let Some(Request::SaveSettings(saved)) = e.reset_colors() else {
            panic!("没有保存修复后的草稿");
        };
        assert_eq!(saved.page_size, 7);
        assert_eq!(saved.custom, Default::default());
        assert!(e.invalid.is_none());
    }
    #[test]
    fn reload_locks_the_draft_until_success_or_failure() {
        let mut e = Editor::new(Options::default());
        let original = e.draft;
        let colors = e.colors.clone();
        e.model.reload().unwrap();
        assert!(e.edit(|o| o.page_size = 9).is_none());
        assert_eq!(e.draft, original, "重新载入期间不能接受随后会被覆盖的编辑");
        assert!(e.color(3, "#0078D4".into()).is_none());
        assert_eq!(e.colors, colors);
        let loaded = Options {
            page_size: 7,
            ..original
        };
        assert!(e.receive(Reply::Saved(Ok(loaded))).is_none());
        assert_eq!(e.draft, loaded);
        e.model.reload().unwrap();
        assert!(
            e.receive(Reply::Saved(Err("合成读取失败".into())))
                .is_none()
        );
        assert!(e.edit(|o| o.page_size = 9).is_some(), "读取失败后恢复编辑");
    }
    #[test]
    fn settings_map_from_control_ids_and_round_trip_their_selection() {
        assert_eq!(Setting::of(AUTO), Some(Setting::Auto));
        assert_eq!(Setting::of(SIZE_CHOICE + 6), Some(Setting::Size(9)));
        assert_eq!(Setting::of(THEME + 4), Some(Setting::Theme(Theme::Custom)));
        assert!(Setting::of(NAV).is_none());
        let mut o = Options::default();
        for id in 0..400 {
            if let Some(setting) = Setting::of(id) {
                let before = setting.selected(o);
                setting.apply(&mut o);
                // 开关取反，单选写入后即选中。
                let toggle = matches!(setting, Setting::Auto | Setting::Common | Setting::Learning);
                assert!(setting.selected(o) != before || !toggle, "控件 {id}");
                assert!(setting.selected(o) || toggle, "控件 {id}");
            }
        }
    }
    #[test]
    fn lookup_actions_page_select_copy_and_catch_up_the_latest_query() {
        let mut e = Editor::new(Options::default());
        assert!(matches!(
            e.act(Action::Query("n".into())),
            Some(Task::Send(Request::Lookup { .. }))
        ));
        // 在途时继续输入只记下，回复后按最新文本补查一次。
        assert!(e.act(Action::Query("ni".into())).is_none());
        let Some(Request::Lookup { query, page: 0 }) = e.reply(Ok(page(&["你", "尼"], true)))
        else {
            panic!("没有补查");
        };
        assert_eq!(query, "ni");
        assert!(e.reply(Ok(page(&["你", "尼"], true))).is_none());
        assert!(e.act(Action::Move(true)).is_none());
        assert_eq!(e.item().unwrap().text, "尼");
        assert!(e.act(Action::Move(true)).is_none(), "末尾不越界");
        assert_eq!(e.selected, 1);
        assert!(e.act(Action::Select(9)).is_none());
        assert_eq!(e.selected, 1, "越界的字块序号被忽略");
        let Some(Task::Copy(text)) = e.act(Action::Copy) else {
            panic!("没有复制");
        };
        assert!(text.starts_with("尼\r\n"));
        assert!(matches!(
            e.act(Action::Next),
            Some(Task::Send(Request::Lookup { page: 1, .. }))
        ));
        assert!(e.act(Action::Previous).is_none(), "在途时不能再翻页");
        assert_eq!(e.model.notice, "操作尚未完成");
        // 通道断开同样结束在途状态并给出提示。
        assert!(e.reply(Err("工作线程未返回结果".into())).is_none());
        assert!(!e.model.busy);
        assert_eq!(e.model.notice, "工作线程未返回结果");
    }
    #[test]
    fn leaving_or_closing_custom_drops_invalid_colors_and_unblocks_saving() {
        let mut e = Editor::new(Options::default());
        let Some(Task::Send(Request::SaveSettings(custom))) =
            e.act(Action::Set(Setting::Theme(Theme::Custom)))
        else {
            panic!("切换主题未保存");
        };
        // 在途期间填了偏蓝色：不保存，侧栏写明原因，回复后也不补存。
        assert!(e.act(Action::Color(3, "#0078D4".into())).is_none());
        assert!(e.model.settings_notice.starts_with("✗ 未保存："));
        assert!(e.reply(Ok(Reply::Saved(Ok(custom)))).is_none());
        // 换回预设主题：四色退回合法值，主题照常保存，提示撤下。
        let Some(Task::Send(Request::SaveSettings(amber))) =
            e.act(Action::Set(Setting::Theme(Theme::Amber)))
        else {
            panic!("换预设主题被不合格色值挡住");
        };
        assert_eq!((amber.theme, amber.custom), (Theme::Amber, custom.custom));
        assert!(e.invalid.is_none() && e.model.settings_notice.is_empty());
        assert!(e.reply(Ok(Reply::Saved(Ok(amber)))).is_none());
        // 保存在途时关窗：未填完的色值退回正在保存的那份，而不是更早的已确认设置。
        let Some(Task::Send(Request::SaveSettings(next))) =
            e.act(Action::Set(Setting::Theme(Theme::Custom)))
        else {
            panic!();
        };
        e.act(Action::Color(0, "#12".into()));
        e.act(Action::Palette(1));
        e.close_settings();
        assert!(e.invalid.is_none() && e.swatch.is_none());
        assert_eq!(e.draft, next);
        assert!(e.model.settings_notice.is_empty());
        assert!(e.reply(Ok(Reply::Saved(Ok(next)))).is_none());
    }
    #[test]
    fn settings_pages_and_palette_overlay_follow_the_custom_theme() {
        let mut e = Editor::new(Options::default());
        assert!(e.act(Action::Page(9)).is_none());
        assert_eq!(e.page, PAGES.len() - 1, "分区越界时停在最后一区");
        e.act(Action::Page(1));
        assert!(e.act(Action::Palette(4)).is_none());
        assert!(e.swatch.is_none(), "只有四个自定义色");
        e.act(Action::Palette(2));
        assert!(!e.swatch_open(), "预设主题下不显示色板");
        let Some(Task::Send(Request::SaveSettings(saved))) =
            e.act(Action::Set(Setting::Theme(Theme::Custom)))
        else {
            panic!("切换主题未保存");
        };
        assert!(e.swatch_open());
        assert!(e.reply(Ok(Reply::Saved(Ok(saved)))).is_none());
        let Some(Task::Send(Request::SaveSettings(next))) = e.act(Action::Swatch(0xFEF3C7)) else {
            panic!("选色未保存");
        };
        assert_eq!(next.custom.highlight, 0xFEF3C7);
        assert!(e.swatch.is_none(), "选色后收起浮层");
        e.act(Action::Palette(0));
        assert!(e.dismiss_swatch());
        assert!(!e.dismiss_swatch(), "已收起时 Esc 交给窗口关闭");
        e.act(Action::Palette(0));
        e.act(Action::Page(0));
        assert!(e.swatch.is_none(), "换分区收起浮层");
    }
}
