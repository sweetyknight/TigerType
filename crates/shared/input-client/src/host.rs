//! 宿主侧会话：懒连接；开会话/焦点/清空与本键在同一次请求中执行，
//! 均与本键共用同一截止时间。通道失败即作废连接，已发出的请求不重放；各平台前端同一份。
use std::time::Instant;
use tigertype_contracts::{
    InputState, Key, KeyOutcome,
    protocol::{Command, Panel, Prepare, Rejection, Reply},
};

/// 会话失败的原因；调用方一律取消未上屏组合、放行原键，原因只供诊断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// 通道连不上、传输失败或超时；实现已作废连接。
    Transport,
    /// 后台拒绝会话请求。
    Rejected,
    /// 会话编号或焦点代次耗尽。
    Exhausted,
}

/// 宿主到后台的通道：跨进程（Windows 命名管道）或进程内（macOS/Linux 引擎线程）；请求带绝对截止时间。
pub trait Channel {
    /// true 表示刚建立新连接：后台上的旧会话与焦点均已不存在。
    fn ensure(&mut self, deadline: Instant) -> Result<bool, Error>;
    /// 只在本次 ensure 成功后调用；传输错误时实现须作废连接，不重放。
    fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> Result<Reply, Error>;
    /// 只在本次 ensure 成功后调用：请后台显示查字或设置窗，不占会话；传输错误时实现须作废连接。
    fn show(&mut self, panel: Panel, deadline: Instant) -> Result<(), Error>;
    /// 只在本次 ensure 成功后调用：报告本宿主线程是否持有输入焦点，不占会话；传输错误时实现须作废连接。
    fn indicate(&mut self, visible: bool, deadline: Instant) -> Result<(), Error>;
    /// 释放当前连接；下次 ensure 重新连接。
    fn cancel(&mut self);
}

// 平台组装通过工厂注入具体通道；动态持有不改变期限、取消或重放约束。
impl<C: Channel + ?Sized> Channel for Box<C> {
    fn ensure(&mut self, deadline: Instant) -> Result<bool, Error> {
        (**self).ensure(deadline)
    }
    fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> Result<Reply, Error> {
        (**self).request(session, command, deadline)
    }
    fn show(&mut self, panel: Panel, deadline: Instant) -> Result<(), Error> {
        (**self).show(panel, deadline)
    }
    fn indicate(&mut self, visible: bool, deadline: Instant) -> Result<(), Error> {
        (**self).indicate(visible, deadline)
    }
    fn cancel(&mut self) {
        (**self).cancel();
    }
}

pub struct Host<C> {
    channel: C,
    /// 当前连接上已打开的会话；0 表示尚未打开。编号跨连接单调，不复用。
    session: u64,
    last_session: u64,
    /// 已向后台报告的焦点代次，只增不减。
    epoch: u64,
    focused: bool,
    /// 后台会话可能留有文档中已不存在的组合，下次按键前须 Clear。
    dirty: bool,
    state: InputState,
    /// 本宿主线程持有输入焦点（任务栏正显示本输入法图标）；后台随连接断开而遗忘，新连接须补报。
    indicated: bool,
}
impl<C: Channel> Host<C> {
    pub fn new(channel: C) -> Self {
        Self {
            channel,
            session: 0,
            last_session: 0,
            epoch: 0,
            focused: false,
            dirty: false,
            state: InputState::default(),
            indicated: false,
        }
    }
    /// 上次回复的状态，只供本地预判，不发请求。
    pub fn state(&self) -> &InputState {
        &self.state
    }
    /// 确认连接；新连接上旧会话不存在、判定按新会话默认状态，且须先补报持有的输入焦点。
    fn ensure(&mut self, deadline: Instant) -> Result<bool, Error> {
        let fresh = self.channel.ensure(deadline)?;
        if fresh {
            self.session = 0;
            self.state = InputState::default();
            if self.indicated {
                self.channel.indicate(true, deadline)?;
            }
        }
        Ok(fresh)
    }
    /// 吃键前确认后台可用：已有连接直接复用，否则在期限内只做连接握手，不发输入请求。
    /// 宿主未必把“预判吃下、处理时放行”的原键交还应用，所以连不上就不能吃键。
    pub fn ready(&mut self, deadline: Instant) -> bool {
        let ready = self.ensure(deadline).is_ok();
        if !ready {
            self.fail();
        }
        ready
    }
    /// 本宿主线程取得或失去输入焦点：告诉后台任务栏是否正显示本输入法图标，后台据此隐藏或恢复通知区图标。
    /// 新连接已在 ensure 中补报取得；失去时新建的连接本无持有，无需再报。失败作废连接。
    pub fn indicate(&mut self, visible: bool, deadline: Instant) -> bool {
        self.indicated = visible;
        let sent = match self.ensure(deadline) {
            Ok(true) => Ok(()),
            Ok(false) => self.channel.indicate(visible, deadline),
            Err(error) => Err(error),
        };
        if sent.is_err() {
            self.fail();
        }
        sent.is_ok()
    }
    /// 文档焦点变化：下次按键携带 Prepare::Focus，后台据此清除旧组合。
    pub fn blur(&mut self) {
        self.focused = false;
        self.state.composing = false;
    }
    /// 宿主终止组合或写入失败：下次按键携带 Prepare::Clear。
    pub fn discard(&mut self) {
        self.dirty = true;
        self.state.composing = false;
    }
    pub fn key(&mut self, key: Key, deadline: Instant) -> Result<KeyOutcome, Error> {
        let result = self.exchange(key, deadline);
        match &result {
            Ok(outcome) => self.state = outcome.state.clone(),
            Err(_) => self.fail(),
        }
        result
    }
    /// 点击选字只针对当前连接上已显示的快照：未打开、未聚焦、待清空或刚重连都说明快照已过期，
    /// 不发请求；后台明确拒绝（旧快照、焦点被夺）时状态未变，返回 None，不作废连接。
    pub fn select(
        &mut self,
        revision: u64,
        index: u8,
        deadline: Instant,
    ) -> Result<Option<KeyOutcome>, Error> {
        if self.session == 0 || !self.focused || self.dirty {
            return Ok(None);
        }
        match self.ensure(deadline) {
            Ok(false) => {}
            Ok(true) => return Ok(None),
            Err(error) => {
                self.fail();
                return Err(error);
            }
        }
        let command = Command::Select {
            revision,
            index,
            epoch: self.epoch,
        };
        match self.channel.request(self.session, command, deadline) {
            Ok(Reply::Key(outcome)) => {
                self.state = outcome.state.clone();
                Ok(Some(outcome))
            }
            Ok(Reply::Rejected(
                Rejection::StaleSnapshot | Rejection::NotFocused | Rejection::StaleFocus,
            )) => Ok(None),
            Ok(_) => {
                self.fail();
                Err(Error::Rejected)
            }
            Err(error) => {
                self.fail();
                Err(error)
            }
        }
    }
    /// 图标菜单请后台显示窗口：与会话无关，成功时会话与输入状态不变；失败作废连接，下一键重建会话。
    pub fn show(&mut self, panel: Panel, deadline: Instant) -> bool {
        if !self.ready(deadline) {
            return false;
        }
        let shown = self.channel.show(panel, deadline).is_ok();
        if !shown {
            self.fail();
        }
        shown
    }
    /// 停用：立即断开，后台随即移除本宿主的会话。
    pub fn close(&mut self) {
        self.fail();
    }
    /// 结果不确定：作废连接，下次重建宿主编号、会话与焦点；新会话的状态即默认值。
    fn fail(&mut self) {
        self.channel.cancel();
        self.session = 0;
        self.focused = false;
        self.dirty = false;
        self.state = InputState::default();
    }
    fn exchange(&mut self, key: Key, deadline: Instant) -> Result<KeyOutcome, Error> {
        self.ensure(deadline)?;
        let new = self.session == 0;
        if new {
            self.last_session = self.last_session.checked_add(1).ok_or(Error::Exhausted)?;
            self.session = self.last_session;
            self.focused = false;
        }
        for retry in [false, true] {
            // 前置操作互斥：新建包含焦点/清空，重取焦点包含清空。
            let prepare = if new {
                Prepare::Open
            } else if !self.focused {
                Prepare::Focus
            } else if self.dirty {
                Prepare::Clear
            } else {
                Prepare::Keep
            };
            if !self.focused {
                self.epoch = self.epoch.checked_add(1).ok_or(Error::Exhausted)?;
            }
            let command = Command::Key {
                key,
                epoch: self.epoch,
                prepare,
            };
            match self.channel.request(self.session, command, deadline)? {
                Reply::Key(outcome) => {
                    self.focused = true;
                    self.dirty = false;
                    return Ok(outcome);
                }
                // 其他宿主取得了后台焦点：被拒的键未处理，重新报告焦点后补发一次不算重放。
                Reply::Rejected(Rejection::NotFocused | Rejection::StaleFocus)
                    if !retry && !new =>
                {
                    self.focused = false;
                }
                _ => break,
            }
        }
        Err(Error::Rejected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc, time::Duration};
    use tigertype_contracts::Composition;

    #[derive(Default)]
    struct Script {
        fresh: VecDeque<Result<bool, Error>>,
        replies: VecDeque<Result<Reply, Error>>,
        log: Vec<(u64, Command)>,
        cancelled: u32,
        /// 唤起结果，缺省为成功；shown 记录每次唤起的目标窗口。
        show: VecDeque<Result<(), Error>>,
        shown: Vec<Panel>,
        /// 焦点报告结果，缺省为成功；indicated 记录每次报告。
        indicate: VecDeque<Result<(), Error>>,
        indicated: Vec<bool>,
    }
    #[derive(Clone, Default)]
    struct Fake(Rc<RefCell<Script>>);
    impl Channel for Fake {
        fn ensure(&mut self, _: Instant) -> Result<bool, Error> {
            self.0.borrow_mut().fresh.pop_front().unwrap_or(Ok(false))
        }
        fn request(&mut self, session: u64, command: Command, _: Instant) -> Result<Reply, Error> {
            let mut script = self.0.borrow_mut();
            script.log.push((session, command));
            script.replies.pop_front().expect("脚本缺少回复")
        }
        fn show(&mut self, panel: Panel, _: Instant) -> Result<(), Error> {
            let mut script = self.0.borrow_mut();
            script.shown.push(panel);
            script.show.pop_front().unwrap_or(Ok(()))
        }
        fn indicate(&mut self, visible: bool, _: Instant) -> Result<(), Error> {
            let mut script = self.0.borrow_mut();
            script.indicated.push(visible);
            script.indicate.pop_front().unwrap_or(Ok(()))
        }
        fn cancel(&mut self) {
            self.0.borrow_mut().cancelled += 1;
        }
    }
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(1)
    }
    fn outcome(text: &str) -> Reply {
        Reply::Key(KeyOutcome {
            handled: true,
            commit: None,
            state: InputState {
                composing: !text.is_empty(),
                composition: Composition {
                    text: text.into(),
                    cursor: text.len(),
                    selection: 0..text.len(),
                },
                ..Default::default()
            },
        })
    }
    fn take_log(fake: &Fake) -> Vec<(u64, Command)> {
        std::mem::take(&mut fake.0.borrow_mut().log)
    }
    fn prepared(epoch: u64, prepare: Prepare) -> Command {
        Command::Key {
            key: Key::Character('u'),
            epoch,
            prepare,
        }
    }
    fn key(epoch: u64) -> Command {
        prepared(epoch, Prepare::Keep)
    }
    fn open(epoch: u64) -> Command {
        prepared(epoch, Prepare::Open)
    }
    fn focus(epoch: u64) -> Command {
        prepared(epoch, Prepare::Focus)
    }

    #[test]
    fn held_input_focus_is_reported_once_per_connection_and_released_without_a_new_one() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        let indicated = |fake: &Fake| std::mem::take(&mut fake.0.borrow_mut().indicated);
        // 取得时须新建连接：连接上补报一次即可，不重复发送；不打开会话。
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.indicate(true, deadline()));
        assert_eq!(indicated(&fake), [true]);
        assert!(take_log(&fake).is_empty());
        // 连接作废后，下一键的新连接先补报取得，再合并开会话/焦点与本键。
        host.close();
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(indicated(&fake), [true]);
        assert_eq!(take_log(&fake), [(1, open(1))]);
        // 失去经现有连接报告；此后的新连接不再补报。
        assert!(host.indicate(false, deadline()));
        assert_eq!(indicated(&fake), [false]);
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.ready(deadline()));
        assert!(indicated(&fake).is_empty());
        // 报告失败作废连接（后台随断开遗忘），仍记下取得，下个新连接补报。
        let cancelled = fake.0.borrow().cancelled;
        fake.0
            .borrow_mut()
            .indicate
            .push_back(Err(Error::Transport));
        assert!(!host.indicate(true, deadline()));
        assert_eq!(fake.0.borrow().cancelled, cancelled + 1);
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.ready(deadline()));
        assert_eq!(indicated(&fake), [true, true]);
    }

    #[test]
    fn fresh_connection_opens_and_focuses_before_the_first_key_then_sends_keys_alone() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .extend([Ok(outcome("u")), Ok(outcome("uu"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.state().composing);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, open(1)), (1, key(1))]);
    }

    #[test]
    fn blur_refocuses_and_discard_clears_but_focus_alone_suffices_when_both_happen() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        take_log(&fake);
        host.discard();
        assert!(!host.state().composing);
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, prepared(1, Prepare::Clear))]);
        host.blur();
        host.discard();
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, focus(2))]);
    }

    #[test]
    fn lost_backend_focus_is_reported_once_and_a_second_rejection_fails_without_looping() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.push_back(Ok(outcome("u")));
        host.key(Key::Character('u'), deadline()).unwrap();
        take_log(&fake);
        // 已有会话被其他宿主夺走焦点，拒绝的键未处理，第二次合并取焦点与本键。
        fake.0
            .borrow_mut()
            .replies
            .extend([Ok(Reply::Rejected(Rejection::NotFocused)), Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, key(1)), (1, focus(2))]);
        fake.0.borrow_mut().replies.extend([
            Ok(Reply::Rejected(Rejection::StaleFocus)),
            Ok(Reply::Rejected(Rejection::NotFocused)),
        ]);
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert_eq!(take_log(&fake), [(1, key(2)), (1, focus(3))]);
    }

    #[test]
    fn transport_failure_cancels_without_replay_and_reconnect_uses_a_new_session() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .extend([Ok(outcome("u")), Err(Error::Transport)]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert!(!host.state().composing);
        // 失败的键不在本层重发；下一键才在新连接上重建会话。
        assert_eq!(take_log(&fake).len(), 2);
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("a"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(2, open(2))]);
    }

    #[test]
    fn connect_failure_sends_nothing_and_rejected_open_is_not_retried() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Err(Error::Transport));
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert!(take_log(&fake).is_empty());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .push_back(Ok(Reply::Rejected(Rejection::SessionLimit)));
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(take_log(&fake), [(1, open(1))]);
        assert_eq!(fake.0.borrow().cancelled, 2);
    }

    #[test]
    fn select_targets_only_the_shown_snapshot_and_stale_rejections_keep_the_connection() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        // 尚未打开会话：快照不可能属于当前连接，不发请求。
        assert!(host.select(5, 0, deadline()).unwrap().is_none());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        take_log(&fake);
        let select = |revision| Command::Select {
            revision,
            index: 1,
            epoch: 1,
        };
        for rejection in [
            Rejection::StaleSnapshot,
            Rejection::NotFocused,
            Rejection::StaleFocus,
        ] {
            fake.0
                .borrow_mut()
                .replies
                .push_back(Ok(Reply::Rejected(rejection)));
            assert!(host.select(5, 1, deadline()).unwrap().is_none());
        }
        fake.0.borrow_mut().replies.push_back(Ok(outcome("")));
        assert!(host.select(6, 1, deadline()).unwrap().is_some());
        assert!(!host.state().composing);
        assert_eq!(
            take_log(&fake),
            [
                (1, select(5)),
                (1, select(5)),
                (1, select(5)),
                (1, select(6))
            ]
        );
        assert_eq!(fake.0.borrow().cancelled, 0);
        // 待清空或刚重连时快照已过期；传输失败才作废连接。
        host.discard();
        assert!(host.select(7, 1, deadline()).unwrap().is_none());
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.select(8, 1, deadline()).unwrap().is_none());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .extend([Ok(outcome("u")), Err(Error::Transport)]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.select(9, 1, deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert_eq!(take_log(&fake).len(), 3);
    }

    #[test]
    fn readiness_connects_without_requests_and_a_fresh_link_reopens_on_the_next_key() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        // 后台不在：判定不可吃键，也不发任何请求。
        fake.0.borrow_mut().fresh.push_back(Err(Error::Transport));
        assert!(!host.ready(deadline()));
        assert!(take_log(&fake).is_empty());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        assert!(host.ready(deadline()));
        assert!(take_log(&fake).is_empty());
        host.key(Key::Character('u'), deadline()).unwrap();
        // 预判时建立的连接在按键时复用：合并开会话/焦点与本键。
        assert_eq!(take_log(&fake), [(1, open(1))]);
        // 预判时发现连接被重建：旧会话作废，判定状态回到默认，下一键重新打开会话。
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.ready(deadline()));
        assert!(!host.state().composing);
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(2, open(2))]);
    }

    #[test]
    fn showing_a_panel_keeps_the_session_and_a_failure_reopens_on_the_next_key() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        // 后台不在：不发唤起请求。
        fake.0.borrow_mut().fresh.push_back(Err(Error::Transport));
        assert!(!host.show(Panel::Settings, deadline()));
        assert!(fake.0.borrow().shown.is_empty());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .extend([Ok(outcome("u")), Ok(outcome("uu"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        take_log(&fake);
        // 唤起成功不打断组合：下一键仍在原会话上直接发送。
        assert!(host.show(Panel::Settings, deadline()));
        assert!(host.state().composing);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, key(1))]);
        // 唤起失败作废连接，下一键在新连接上重建会话。
        fake.0.borrow_mut().show.push_back(Err(Error::Transport));
        assert!(!host.show(Panel::Lookup, deadline()));
        assert_eq!(fake.0.borrow().cancelled, 2);
        assert_eq!(fake.0.borrow().shown, [Panel::Settings, Panel::Lookup]);
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(2, open(2))]);
    }
}
