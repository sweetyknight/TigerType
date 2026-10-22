//! 视图状态独立于窗口句柄，单个在途动作避免查询换页或保存回复错配。
use tigerrime_contracts::{
    LookupPage, MAX_QUERY_CHARS, Options,
    ui::{Reply, Request},
};

pub(crate) struct Model {
    pub options: Options,
    pub busy: bool,
    pub query: String,
    pub page: usize,
    pub result: Option<LookupPage>,
    pub notice: String,
    pub settings_notice: String,
    pub settings_open: bool,
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
            notice: "查字始终使用全字集和原字形；设置仅供后续输入使用。".into(),
            settings_notice: "关闭本窗放弃未发送的修改；已发送的保存不会回滚。".into(),
            settings_open: false,
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
        self.notice = "正在查询，请稍候…".into();
        Ok(Request::Lookup { query, page })
    }
    pub fn save(&mut self, options: Options) -> Result<Request, &'static str> {
        if self.busy {
            return Err("操作尚未完成");
        }
        self.pending = Some(Request::SaveSettings(options));
        self.busy = true;
        self.settings_notice = "正在保存，请稍候…".into();
        Ok(Request::SaveSettings(options))
    }
    pub fn fail(&mut self, notice: String) {
        if matches!(self.pending.take(), Some(Request::SaveSettings(_))) {
            self.settings_notice = notice;
        } else {
            self.notice = notice;
        }
        self.busy = false;
    }
    /// 返回是否替换学习资料；设置回复不能重置资料框的选择和滚动位置。
    pub fn receive(&mut self, reply: Reply) -> Result<bool, &'static str> {
        let pending = self.pending.take();
        self.busy = false;
        match (pending, reply) {
            (Some(Request::Lookup { query, page }), Reply::Lookup(Ok(result))) => {
                self.query = query;
                self.page = page;
                self.notice = if result.items.is_empty() {
                    "没有匹配的单字".into()
                } else {
                    format!("第 {} 页；查字不产生上屏提交", page + 1)
                };
                self.result = Some(result);
                return Ok(true);
            }
            (Some(Request::SaveSettings(expected)), Reply::Saved(Ok(options)))
                if expected == options =>
            {
                self.options = options;
                self.settings_notice = "设置已保存；查字仍显示全字集和原字形。".into();
            }
            (Some(Request::Lookup { .. }), Reply::Lookup(Err(error))) => self.notice = error,
            (Some(Request::SaveSettings(_)), Reply::Saved(Err(error))) => {
                self.settings_notice = error
            }
            // 内部消息错配说明工作线程契约已经失效，不能继续接收不确定的后续回复。
            _ => return Err("回复与在途操作不符，请重新启动工具"),
        }
        Ok(false)
    }
    pub fn open_settings(&mut self) -> bool {
        // 仅首次打开/取消后重开载入确认值，重复唤起现有窗口不丢弃用户草稿。
        !std::mem::replace(&mut self.settings_open, true)
    }
}

pub(crate) fn details(page: &LookupPage) -> String {
    fn list(items: &[String]) -> String {
        if items.is_empty() {
            "缺资料".into()
        } else {
            items.join("、")
        }
    }
    page.items
        .iter()
        .map(|item| {
            format!(
                "{}\r\n可用编码：{}\r\n完整编码：{}\r\n读音：{}\r\n拆分：{}\r\n",
                item.character,
                list(&item.learning.codes),
                list(&item.learning.full_codes),
                list(&item.learning.readings),
                item.learning.decomposition.as_deref().unwrap_or("缺资料")
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigerrime_contracts::{LearningInfo, LookupItem};
    #[test]
    fn single_pending_action_and_page_binding() {
        let mut model = Model::new(Options::default());
        assert!(model.lookup("𠀀".repeat(65), 0).is_err());
        assert!(model.lookup("han".into(), 1).is_err());
        model.lookup("han".into(), 0).unwrap();
        assert!(model.save(Options::default()).is_err());
        assert!(
            model
                .receive(Reply::Lookup(Ok(LookupPage {
                    items: vec![],
                    has_more: true,
                })))
                .unwrap()
        );
        assert!(model.lookup("new".into(), 1).is_err());
        model.lookup("han".into(), 1).unwrap();
        assert!(!model.receive(Reply::Lookup(Err("失败".into()))).unwrap());
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
        assert!(!model.receive(Reply::Saved(Err("冲突".into()))).unwrap());
        assert!(!model.options.auto_commit_four);
        model.save(changed).unwrap();
        assert!(model.receive(Reply::Saved(Ok(Options::default()))).is_err());
        assert!(!model.options.auto_commit_four);
        model.save(changed).unwrap();
        assert!(!model.receive(Reply::Saved(Ok(changed))).unwrap());
        assert!(model.options.auto_commit_four);
    }
    #[test]
    fn settings_draft_and_notices_are_isolated_from_queries() {
        let mut model = Model::new(Options::default());
        assert!(model.open_settings());
        assert!(!model.open_settings());
        model.settings_open = false;
        assert!(model.open_settings());
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
        let text = details(&LookupPage {
            items: vec![LookupItem {
                character: '𠀀',
                learning: LearningInfo {
                    codes: vec!["ab".into()],
                    ..Default::default()
                },
            }],
            has_more: false,
        });
        assert!(text.contains("𠀀\r\n可用编码：ab"));
        assert!(text.contains("完整编码：缺资料"));
        assert!(text.contains("拆分：缺资料"));
    }
}
