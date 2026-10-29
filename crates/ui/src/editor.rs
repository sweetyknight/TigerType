//! Mac 编辑交互只管理草稿；复用 Model 的在途请求匹配，不执行磁盘或热重载。
use crate::model::{Effect, Model};
use tigertype_contracts::{
    Options,
    ui::{Reply, Request},
};

#[cfg(test)]
use crate::layout::SWATCHES;
pub(crate) struct Editor {
    pub model: Model,
    pub draft: Options,
    pub colors: [String; 4],
    pub invalid: Option<String>,
    pub selected: usize,
    pub fatal: bool,
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
    /// 界面输入只接受 ASCII #RRGGBB，不按字符索引截断 UTF-8。
    pub fn color(&mut self, index: usize, value: String) -> Option<Request> {
        if self.fatal || self.model.reloading() || index >= self.colors.len() {
            return None;
        }
        self.colors[index] = value;
        let parsed: Result<Vec<_>, _> = self
            .colors
            .iter()
            .map(|v| {
                let v = v.trim().strip_prefix('#').ok_or("颜色格式应为 #RRGGBB")?;
                if v.len() != 6 || !v.bytes().all(|c| c.is_ascii_hexdigit()) {
                    return Err("颜色格式应为 #RRGGBB");
                }
                u32::from_str_radix(v, 16).map_err(|_| "颜色格式应为 #RRGGBB")
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
            Err(error) => self.invalid = Some(error.into()),
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
    pub fn save(&mut self) -> Option<Request> {
        if self.fatal
            || self.model.busy
            || self.invalid.is_some()
            || self.draft == self.model.options
        {
            return None;
        }
        if let Err(error) = self.draft.validate() {
            self.invalid = Some(error);
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
}
