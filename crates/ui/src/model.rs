//! 视图状态独立于窗口句柄，单个在途动作避免查询换页或保存回复错配。
//! 设置即时生效：设置窗控件即草稿；在途时的新改动不排队，回复后按最新草稿与已确认设置比较再保存。
use tigertype_contracts::{
    LookupItem, LookupPage, MAX_QUERY_CHARS, Options,
    ui::{Reply, Request},
};

/// 设置窗空闲时的提示。
pub(crate) const SETTINGS_IDLE: &str = "✓ 修改后立即生效";

/// 回复对界面的影响。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    /// 换上新的查字结果。
    Page,
    /// 设置控件须改回已确认设置（重新载入成功，或保存失败要撤回界面上的改动）。
    Controls,
    Nothing,
}

pub(crate) struct Model {
    pub options: Options,
    pub busy: bool,
    pub query: String,
    pub page: usize,
    pub result: Option<LookupPage>,
    pub notice: String,
    pub settings_notice: String,
    pending: Option<Request>,
}

impl Model {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            busy: false,
            query: String::new(),
            page: 0,
            result: None,
            notice: "查字使用全字集与原字形，不受输入设置影响".into(),
            settings_notice: SETTINGS_IDLE.into(),
            pending: None,
        }
    }
    pub fn lookup(&mut self, query: String, page: usize) -> Result<Request, &'static str> {
        if self.busy {
            return Err("操作尚未完成");
        }
        if query.chars().count() > MAX_QUERY_CHARS {
            return Err("查字文本超过 64 个字符");
        }
        // 换页只能基于已经显示的查询，编辑后必须从首页重新查询。
        if page > 0
            && (query != self.query
                || self.result.is_none()
                || (page > self.page
                    && (page != self.page + 1 || !self.result.as_ref().unwrap().has_more)))
        {
            return Err("查询页已失效，请重新查询");
        }
        self.pending = Some(Request::Lookup {
            query: query.clone(),
            page,
        });
        self.busy = true;
        self.notice = "正在查询…".into();
        Ok(Request::Lookup { query, page })
    }
    pub fn save(&mut self, options: Options) -> Result<Request, &'static str> {
        if self.busy {
            return Err("操作尚未完成");
        }
        self.pending = Some(Request::SaveSettings(options));
        self.busy = true;
        Ok(Request::SaveSettings(options))
    }
    pub fn reload(&mut self) -> Result<Request, &'static str> {
        if self.busy {
            return Err("操作尚未完成，请稍后再点");
        }
        self.pending = Some(Request::ReloadSettings);
        self.busy = true;
        self.settings_notice = "正在重新载入…".into();
        Ok(Request::ReloadSettings)
    }
    pub fn fail(&mut self, notice: String) {
        match self.pending.take() {
            Some(Request::Lookup { .. }) | None => self.notice = notice,
            Some(_) => self.settings_notice = notice,
        }
        self.busy = false;
    }
    pub fn receive(&mut self, reply: Reply) -> Result<Effect, &'static str> {
        let pending = self.pending.take();
        self.busy = false;
        Ok(match (pending, reply) {
            (Some(Request::Lookup { query, page }), Reply::Lookup(Ok(result))) => {
                self.query = query;
                self.page = page;
                self.notice = if result.items.is_empty() {
                    "没有匹配的字词".into()
                } else {
                    String::new()
                };
                self.result = Some(result);
                Effect::Page
            }
            (Some(Request::SaveSettings(expected)), Reply::Saved(Ok(options)))
                if expected == options =>
            {
                self.options = options;
                self.settings_notice = "✓ 已保存，新输入立即生效".into();
                Effect::Nothing
            }
            (Some(Request::ReloadSettings), Reply::Saved(Ok(options))) => {
                self.options = options;
                self.settings_notice = "✓ 已重新载入 settings.toml".into();
                Effect::Controls
            }
            (Some(Request::Lookup { .. }), Reply::Lookup(Err(error))) => {
                self.notice = error;
                Effect::Nothing
            }
            (Some(Request::SaveSettings(_)), Reply::Saved(Err(error))) => {
                self.settings_notice = error;
                Effect::Controls
            }
            (Some(Request::ReloadSettings), Reply::Saved(Err(error))) => {
                self.settings_notice = error;
                Effect::Nothing
            }
            // 内部消息错配说明工作线程契约已经失效，不能继续接收不确定的后续回复。
            _ => return Err("回复与在途操作不符，请重新启动工具"),
        })
    }
}

/// 复制用的单字资料；缺项照写“缺资料”，不猜测。
pub(crate) fn details(item: &LookupItem) -> String {
    fn list(items: &[String]) -> String {
        if items.is_empty() {
            "缺资料".into()
        } else {
            items.join("、")
        }
    }
    format!(
        "{}\r\n可用编码：{}\r\n完整编码：{}\r\n读音：{}\r\n拆分：{}\r\n",
        item.text,
        list(&item.learning.codes),
        list(&item.learning.full_codes),
        list(&item.learning.readings),
        item.learning.decomposition.as_deref().unwrap_or("缺资料")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::LearningInfo;
    #[test]
    fn single_pending_action_and_page_binding() {
        let mut model = Model::new(Options::default());
        assert!(model.lookup("𠀀".repeat(65), 0).is_err());
        assert!(model.lookup("han".into(), 1).is_err());
        model.lookup("han".into(), 0).unwrap();
        assert!(model.save(Options::default()).is_err());
        assert!(model.reload().is_err());
        assert_eq!(
            model
                .receive(Reply::Lookup(Ok(LookupPage {
                    items: vec![],
                    has_more: true,
                })))
                .unwrap(),
            Effect::Page
        );
        assert!(model.lookup("new".into(), 1).is_err());
        model.lookup("han".into(), 1).unwrap();
        assert_eq!(
            model.receive(Reply::Lookup(Err("失败".into()))).unwrap(),
            Effect::Nothing
        );
        assert_eq!(model.page, 0);
        assert_eq!(model.query, "han");
        assert!(!model.busy);
        model.lookup("han".into(), 1).unwrap();
        model
            .receive(Reply::Lookup(Ok(LookupPage {
                items: vec![],
                has_more: false,
            })))
            .unwrap();
        assert_eq!(model.page, 1);
        assert!(model.lookup("han".into(), 2).is_err());
        assert!(model.lookup("han".into(), 0).is_ok());
    }
    #[test]
    fn failed_or_mismatched_save_never_changes_confirmed_options() {
        let mut model = Model::new(Options::default());
        let changed = Options {
            auto_commit_four: true,
            ..model.options
        };
        model.save(changed).unwrap();
        // 保存失败要把设置控件撤回到已确认值。
        assert_eq!(
            model.receive(Reply::Saved(Err("冲突".into()))).unwrap(),
            Effect::Controls
        );
        assert!(!model.options.auto_commit_four);
        assert_eq!(model.settings_notice, "冲突");
        model.save(changed).unwrap();
        assert!(model.receive(Reply::Saved(Ok(Options::default()))).is_err());
        assert!(!model.options.auto_commit_four);
        model.save(changed).unwrap();
        assert_eq!(
            model.receive(Reply::Saved(Ok(changed))).unwrap(),
            Effect::Nothing
        );
        assert!(model.options.auto_commit_four);
    }
    #[test]
    fn reload_adopts_whatever_the_file_holds_or_keeps_the_current_options() {
        let mut model = Model::new(Options::default());
        let edited = Options {
            page_size: 7,
            ..Options::default()
        };
        model.reload().unwrap();
        assert_eq!(
            model.receive(Reply::Saved(Ok(edited))).unwrap(),
            Effect::Controls
        );
        assert_eq!(model.options, edited);
        assert!(model.settings_notice.contains("重新载入"));
        model.reload().unwrap();
        model
            .receive(Reply::Saved(Err("settings.toml 第 3 行：bad".into())))
            .unwrap();
        assert_eq!(model.options, edited);
        assert!(model.settings_notice.contains("第 3 行"));
    }
    #[test]
    fn settings_and_lookup_notices_stay_apart() {
        let mut model = Model::new(Options::default());
        model.save(model.options).unwrap();
        model.fail("保存通道满".into());
        assert_eq!(model.settings_notice, "保存通道满");
        model.lookup("han".into(), 0).unwrap();
        model
            .receive(Reply::Lookup(Err("查询失败".into())))
            .unwrap();
        assert_eq!(model.settings_notice, "保存通道满");
        assert_eq!(model.notice, "查询失败");
        model.save(model.options).unwrap();
        model.receive(Reply::Saved(Ok(model.options))).unwrap();
        assert_eq!(model.notice, "查询失败");
    }
    #[test]
    fn unexpected_or_wrong_kind_reply_is_a_fatal_contract_error() {
        let mut model = Model::new(Options::default());
        assert!(model.receive(Reply::Saved(Ok(model.options))).is_err());
        model.lookup("han".into(), 0).unwrap();
        assert!(model.receive(Reply::Saved(Ok(model.options))).is_err());
        assert!(model.result.is_none());
    }
    #[test]
    fn non_bmp_and_missing_data_are_displayed_without_guessing() {
        let text = details(&LookupItem {
            text: "𠀀".into(),
            learning: LearningInfo {
                codes: vec!["ab".into()],
                ..Default::default()
            },
        });
        assert!(text.contains("𠀀\r\n可用编码：ab"));
        assert!(text.contains("完整编码：缺资料"));
        assert!(text.contains("拆分：缺资料"));
    }
}
