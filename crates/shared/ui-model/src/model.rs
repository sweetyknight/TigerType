//! 视图状态独立于窗口句柄，单个在途动作避免查询换页或保存回复错配。
//! 设置即时生效：Editor 持有草稿；在途时的新改动不排队，回复后按最新草稿与已确认设置比较再保存。
use tigertype_contracts::{
    LookupItem, LookupPage, MAX_QUERY_CHARS, Options,
    ui::{Reply, Request},
};

/// 回复对界面的影响。
#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// 换上新的查字结果。
    Page,
    /// 设置控件须改回已确认设置（重新载入成功，或保存失败要撤回界面上的改动）。
    Controls,
    Nothing,
}

pub struct Model {
    pub options: Options,
    pub busy: bool,
    pub query: String,
    pub page: usize,
    pub result: Option<LookupPage>,
    pub notice: String,
    pub settings_notice: String,
    pending: Option<Request>,
    /// 在途期间查询框又改过，回复后要按最新文本补查一次。
    stale: bool,
}

const IDLE: &str = "查字使用全字集与原字形，不受输入设置影响";

impl Model {
    pub fn new(options: Options) -> Self {
        Self {
            options,
            busy: false,
            query: String::new(),
            page: 0,
            result: None,
            notice: IDLE.into(),
            // 设置侧栏只在出错时说话：空闲、保存成功与重新载入成功都不显示确认文字。
            settings_notice: String::new(),
            pending: None,
            stale: false,
        }
    }
    /// 查询框每次改动即查首页。在途时只记下，回复后由 `take_stale` 补查最新文本：
    /// 至多一个在途、一个待补，不排队、不用计时器，连续输入只多查最后一次。
    /// 返回 None 且不忙时结果已清空（空查询或查询无效），界面要随之重画。
    pub fn edit(&mut self, query: String) -> Option<Request> {
        if self.busy {
            self.stale = true;
            return None;
        }
        self.stale = false;
        if query.trim().is_empty() {
            self.clear(IDLE.into());
            return None;
        }
        self.lookup(query, 0)
            .map_err(|error| self.clear(error.into()))
            .ok()
    }
    /// 回复后调用：在途期间查询框改过时返回 true，调用方取当前文本再 `edit`。
    pub fn take_stale(&mut self) -> bool {
        !self.busy && std::mem::take(&mut self.stale)
    }
    /// 撤下旧结果，避免它和新查询框文字并列；提示改为空闲说明或错误。
    pub fn clear(&mut self, notice: String) {
        self.result = None;
        self.query.clear();
        self.page = 0;
        self.notice = notice;
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
        Ok(Request::ReloadSettings)
    }
    /// 草稿在重新载入期间只读；保存期间仍允许合并后续编辑。
    pub fn reloading(&self) -> bool {
        matches!(self.pending, Some(Request::ReloadSettings))
    }
    /// 正在保存的设置：丢弃未完成的草稿时退回它，而不是更早的已确认设置。
    pub fn saving(&self) -> Option<Options> {
        match self.pending {
            Some(Request::SaveSettings(options)) => Some(options),
            _ => None,
        }
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
        // 类型正确不等于设置有效；界面不能展示或确认违反颜色/页数约束的工作线程回复。
        if let Reply::Saved(Ok(options)) = &reply
            && options.validate().is_err()
        {
            return Err("回复包含无效设置，请重新启动工具");
        }
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
                self.settings_notice.clear();
                Effect::Nothing
            }
            (Some(Request::ReloadSettings), Reply::Saved(Ok(options))) => {
                self.options = options;
                self.settings_notice.clear();
                Effect::Controls
            }
            // 新查询失败时撤下上一查询的结果，否则错误提示被旧字块挡住；换页失败保留当前页。
            (Some(Request::Lookup { page: 0, .. }), Reply::Lookup(Err(error))) => {
                self.clear(error);
                Effect::Page
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

/// 拆分格的控件文字：字根后紧跟其编码，字根间一个空格，字与字之间两个空格；任一字缺拆分即“缺资料”。
/// Windows 自绘静态控件与 Mac 绘制视图都只拿到这段文字，用 `tiles` 拆回。
pub fn parts(item: &LookupItem) -> String {
    if item.roots.is_empty() || item.roots.iter().any(Vec::is_empty) {
        return "缺资料".into();
    }
    let group = |roots: &Vec<(char, String)>| {
        let tiles: Vec<_> = roots
            .iter()
            .map(|(root, code)| format!("{root}{code}"))
            .collect();
        tiles.join(" ")
    };
    item.roots.iter().map(group).collect::<Vec<_>>().join("  ")
}
/// `parts` 的逆过程：Some((字根, 编码))；None 是字与字之间的间隔。字根总是首个 Unicode 标量。
pub fn tiles(text: &str) -> impl Iterator<Item = Option<(char, &str)>> {
    text.split(' ').map(|tile| {
        let mut chars = tile.chars();
        chars.next().map(|root| (root, chars.as_str()))
    })
}

/// 复制用的单字资料；缺项照写“缺资料”，不猜测。
pub fn details(item: &LookupItem) -> String {
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
        parts(item)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::LearningInfo;
    #[test]
    fn invalid_settings_reply_never_changes_confirmed_options() {
        let mut model = Model::new(Options::default());
        let original = model.options;
        model.reload().unwrap();
        assert!(
            model
                .receive(Reply::Saved(Ok(Options {
                    page_size: 0,
                    ..original
                })))
                .is_err()
        );
        assert_eq!(model.options, original);
        assert!(!model.busy);
    }
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
        assert!(model.settings_notice.is_empty(), "成功不显示确认文字");
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
        // 保存成功清掉此前的错误，且不换成确认文字。
        assert!(model.settings_notice.is_empty());
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
            roots: vec![vec![]],
        });
        assert!(text.contains("𠀀\r\n可用编码：ab"));
        assert!(text.contains("完整编码：缺资料"));
        assert!(text.contains("拆分：缺资料"));
    }
    #[test]
    fn parts_text_round_trips_roots_codes_and_word_gaps() {
        let roots = vec![
            vec![
                ('亻', "jr".to_owned()),
                ('\u{e4e4}', "xd".into()),
                // 扩展区字根占两个 UTF-16 单元，无码字根照样成格。
                ('𠂉', "".into()),
            ],
            vec![('女', "b".into())],
        ];
        let item = LookupItem {
            text: "你好".into(),
            learning: LearningInfo::default(),
            roots,
        };
        let text = parts(&item);
        assert_eq!(text, "亻jr \u{e4e4}xd 𠂉  女b");
        assert_eq!(
            tiles(&text).collect::<Vec<_>>(),
            [
                Some(('亻', "jr")),
                Some(('\u{e4e4}', "xd")),
                Some(('𠂉', "")),
                None,
                Some(('女', "b")),
            ]
        );
        assert!(details(&item).contains("拆分：亻jr"));
        // 反例：词里任一字缺拆分就整项标缺，不只画出一半。
        let partial = LookupItem {
            roots: vec![vec![('女', "bn".into())], vec![]],
            ..item
        };
        assert_eq!(parts(&partial), "缺资料");
    }
    #[test]
    fn typing_queries_live_with_one_lookup_in_flight_and_one_catch_up() {
        let mut model = Model::new(Options::default());
        assert!(matches!(
            model.edit("n".into()),
            Some(Request::Lookup { ref query, page: 0 }) if query == "n"
        ));
        // 在途时继续输入不发请求，只记下要补查。
        assert!(model.edit("ni".into()).is_none());
        assert!(model.edit("nih".into()).is_none());
        assert!(!model.take_stale(), "在途时不补查");
        model
            .receive(Reply::Lookup(Ok(LookupPage {
                items: vec![],
                has_more: true,
            })))
            .unwrap();
        assert!(model.take_stale());
        assert!(!model.take_stale(), "只补查一次");
        assert!(model.edit("nihao".into()).is_some());
        // 首页查询失败撤下旧结果，错误提示不被旧字块挡住。
        model.result = Some(LookupPage {
            items: vec![],
            has_more: false,
        });
        assert_eq!(
            model
                .receive(Reply::Lookup(Err("请输入无调全拼".into())))
                .unwrap(),
            Effect::Page
        );
        assert!(model.result.is_none());
        assert_eq!(model.notice, "请输入无调全拼");
        // 清空查询框回到空闲说明；超长文本在界面侧就拒绝。
        assert!(model.edit("  ".into()).is_none());
        assert_eq!(model.notice, IDLE);
        assert!(model.edit("字".repeat(65)).is_none());
        assert!(model.result.is_none() && !model.busy);
        assert!(model.notice.contains("64"));
        // 保存在途时改查询框，保存回复后同样补查。
        model.save(Options::default()).unwrap();
        assert!(model.edit("hao".into()).is_none());
        model.receive(Reply::Saved(Ok(Options::default()))).unwrap();
        assert!(model.take_stale());
    }
}
