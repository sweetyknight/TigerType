//! 宿主线程与后台的会话编号：懒连接；新连接先 Open，文档焦点变化后先 Focus，宿主终止组合后先 Clear，
//! 均与本键共用同一截止时间。传输失败作废连接，已写出的请求不重放。
use std::time::Instant;
use tigertype_contracts::{
    InputState, Key, KeyOutcome, Options,
    protocol::{Command, Rejection, Reply},
};
use tigertype_ipc::Error;

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
    /// 释放当前连接；下次 ensure 重新连接。
    fn cancel(&mut self);
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
        }
    }
    /// 上次回复的状态，只供 OnTestKeyDown 本地判定，不发 IPC。
    pub fn state(&self) -> &InputState {
        &self.state
    }
    /// 吃键前确认后台可用：已有连接直接复用，否则在期限内只做连接握手，不发输入请求。
    /// 宿主未必把“预判吃下、处理时放行”的原键交还应用，所以连不上就不能吃键。
    pub fn ready(&mut self, deadline: Instant) -> bool {
        match self.channel.ensure(deadline) {
            Ok(fresh) => {
                if fresh {
                    // 新连接上的会话尚未打开，判定按新会话的默认状态。
                    self.session = 0;
                    self.state = InputState::default();
                }
                true
            }
            Err(_) => {
                self.fail();
                false
            }
        }
    }
    /// 文档焦点变化：下次按键先报告焦点，后台据此清除旧组合。
    pub fn blur(&mut self) {
        self.focused = false;
        self.state.composing = false;
    }
    /// 宿主终止组合或写入失败：下次按键先 Clear。
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
        match self.channel.ensure(deadline) {
            Ok(false) => {}
            Ok(true) => {
                self.session = 0;
                self.state = InputState::default();
                return Ok(None);
            }
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
                Err(Error::Invalid("后台拒绝选字"))
            }
            Err(error) => {
                self.fail();
                Err(error)
            }
        }
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
    fn call(&mut self, command: Command, deadline: Instant) -> Result<(), Error> {
        // 回复类型已由协议层与请求配对校验，这里只需区分拒绝。
        match self.channel.request(self.session, command, deadline)? {
            Reply::Rejected(_) => Err(Error::Invalid("后台拒绝会话请求")),
            _ => Ok(()),
        }
    }
    fn exchange(&mut self, key: Key, deadline: Instant) -> Result<KeyOutcome, Error> {
        if self.channel.ensure(deadline)? {
            self.session = 0;
        }
        if self.session == 0 {
            self.last_session = self.last_session.checked_add(1).ok_or(Error::Closed)?;
            self.session = self.last_session;
            self.focused = false;
            // 选项以后台启动时读入的设置为准，宿主不读盘，Open 只携带占位默认值。
            self.call(
                Command::Open {
                    options: Options::default(),
                },
                deadline,
            )?;
            self.dirty = false;
        }
        for retry in [false, true] {
            if !self.focused {
                self.epoch = self.epoch.checked_add(1).ok_or(Error::Closed)?;
                self.call(
                    Command::Focus {
                        epoch: self.epoch,
                        focused: true,
                    },
                    deadline,
                )?;
                // 后台处理焦点时已清空本会话组合。
                self.focused = true;
                self.dirty = false;
            }
            if self.dirty {
                self.call(Command::Clear, deadline)?;
                self.dirty = false;
            }
            let command = Command::Key {
                key,
                epoch: self.epoch,
            };
            match self.channel.request(self.session, command, deadline)? {
                Reply::Key(outcome) => return Ok(outcome),
                // 其他宿主取得了后台焦点：被拒的键未处理，重新报告焦点后补发一次不算重放。
                Reply::Rejected(Rejection::NotFocused | Rejection::StaleFocus) if !retry => {
                    self.focused = false;
                }
                _ => break,
            }
        }
        Err(Error::Invalid("后台拒绝按键"))
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
    fn ok(state: fn(InputState) -> Reply) -> Result<Reply, Error> {
        Ok(state(InputState::default()))
    }
    fn take_log(fake: &Fake) -> Vec<(u64, Command)> {
        std::mem::take(&mut fake.0.borrow_mut().log)
    }
    fn key(epoch: u64) -> Command {
        Command::Key {
            key: Key::Character('u'),
            epoch,
        }
    }
    fn focus(epoch: u64) -> Command {
        Command::Focus {
            epoch,
            focused: true,
        }
    }
    fn open() -> Command {
        Command::Open {
            options: Options::default(),
        }
    }

    #[test]
    fn fresh_connection_opens_and_focuses_before_the_first_key_then_sends_keys_alone() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
            Ok(outcome("uu")),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.state().composing);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(
            take_log(&fake),
            [(1, open()), (1, focus(1)), (1, key(1)), (1, key(1))]
        );
    }

    #[test]
    fn blur_refocuses_and_discard_clears_but_focus_alone_suffices_when_both_happen() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        take_log(&fake);
        host.discard();
        assert!(!host.state().composing);
        fake.0
            .borrow_mut()
            .replies
            .extend([ok(Reply::Cleared), Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, Command::Clear), (1, key(1))]);
        host.blur();
        host.discard();
        fake.0
            .borrow_mut()
            .replies
            .extend([ok(Reply::Focused), Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(1, focus(2)), (1, key(2))]);
    }

    #[test]
    fn lost_backend_focus_is_reported_once_and_a_second_rejection_fails_without_looping() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(Reply::Rejected(Rejection::NotFocused)),
            ok(Reply::Focused),
            Ok(outcome("u")),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(
            take_log(&fake),
            [
                (1, open()),
                (1, focus(1)),
                (1, key(1)),
                (1, focus(2)),
                (1, key(2))
            ]
        );
        fake.0.borrow_mut().replies.extend([
            Ok(Reply::Rejected(Rejection::StaleFocus)),
            ok(Reply::Focused),
            Ok(Reply::Rejected(Rejection::NotFocused)),
        ]);
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert_eq!(take_log(&fake).len(), 3);
    }

    #[test]
    fn transport_failure_cancels_without_replay_and_reconnect_uses_a_new_session() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
            Err(Error::TimedOut),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert!(!host.state().composing);
        // 失败的键不在本层重发；下一键才在新连接上重建会话。
        assert_eq!(take_log(&fake).len(), 4);
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("a")),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(2, open()), (2, focus(2)), (2, key(2))]);
    }

    #[test]
    fn connect_failure_and_rejected_open_never_send_the_key() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        fake.0.borrow_mut().fresh.push_back(Err(Error::Busy));
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert!(take_log(&fake).is_empty());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0
            .borrow_mut()
            .replies
            .push_back(Ok(Reply::Rejected(Rejection::SessionLimit)));
        assert!(host.key(Key::Character('u'), deadline()).is_err());
        assert_eq!(take_log(&fake), [(1, open())]);
        assert_eq!(fake.0.borrow().cancelled, 2);
    }

    #[test]
    fn select_targets_only_the_shown_snapshot_and_stale_rejections_keep_the_connection() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        // 尚未打开会话：快照不可能属于当前连接，不发请求。
        assert!(host.select(5, 0, deadline()).unwrap().is_none());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
        ]);
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
        fake.0
            .borrow_mut()
            .replies
            .extend([ok(Reply::Cleared), Ok(outcome("u"))]);
        host.key(Key::Character('u'), deadline()).unwrap();
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.select(8, 1, deadline()).unwrap().is_none());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
            Err(Error::TimedOut),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert!(host.select(9, 1, deadline()).is_err());
        assert_eq!(fake.0.borrow().cancelled, 1);
        assert_eq!(take_log(&fake).len(), 6);
    }

    #[test]
    fn readiness_connects_without_requests_and_a_fresh_link_reopens_on_the_next_key() {
        let fake = Fake::default();
        let mut host = Host::new(fake.clone());
        // 后台不在：判定不可吃键，也不发任何请求。
        fake.0.borrow_mut().fresh.push_back(Err(Error::Busy));
        assert!(!host.ready(deadline()));
        assert!(take_log(&fake).is_empty());
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
        ]);
        assert!(host.ready(deadline()));
        assert!(take_log(&fake).is_empty());
        host.key(Key::Character('u'), deadline()).unwrap();
        // 预判时建立的连接在按键时复用：先 Open/Focus 再发键。
        assert_eq!(take_log(&fake), [(1, open()), (1, focus(1)), (1, key(1))]);
        // 预判时发现连接被重建：旧会话作废，判定状态回到默认，下一键重新打开会话。
        fake.0.borrow_mut().fresh.push_back(Ok(true));
        assert!(host.ready(deadline()));
        assert!(!host.state().composing);
        fake.0.borrow_mut().replies.extend([
            ok(Reply::Opened),
            ok(Reply::Focused),
            Ok(outcome("u")),
        ]);
        host.key(Key::Character('u'), deadline()).unwrap();
        assert_eq!(take_log(&fake), [(2, open()), (2, focus(2)), (2, key(2))]);
    }
}
