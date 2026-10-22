//! 固定引擎线程的多宿主路由；宿主标识来自连接所有者，不接受协议正文冒认宿主。
use crate::Result;
use std::collections::BTreeMap;
use tigerrime_contracts::{
    candidate::{CandidateChoice, CandidateFrame, CandidateToken, ScreenRect},
    protocol::{Command, Message, Rejection, Reply, Request, Response},
};
use tigerrime_engine::{Dictionary, Session};
const MAX_HOSTS: usize = 16;
const MAX_SESSIONS: usize = 64;

struct Host<'a> {
    sessions: BTreeMap<u64, Session<'a>>,
    last_request: u64,
    last_session: u64,
    last_focus: u64,
}
#[derive(Clone, Copy)]
struct Focus {
    host: u64,
    session: u64,
    epoch: u64,
    caret: ScreenRect,
}
pub struct Dispatcher<'a> {
    dictionary: &'a Dictionary,
    hosts: BTreeMap<u64, Host<'a>>,
    generation: u64,
    last_host: u64,
    active: Option<Focus>,
    pending: Option<CandidateChoice>,
    failed: bool,
}
impl<'a> Dispatcher<'a> {
    pub fn new(dictionary: &'a Dictionary, generation: u64) -> Result<Self> {
        if generation == 0 {
            return Err("后台代次不能为零".into());
        }
        Ok(Self {
            dictionary,
            hosts: BTreeMap::new(),
            generation,
            last_host: 0,
            active: None,
            pending: None,
            failed: false,
        })
    }
    /// 只在认证成功的连接建立时分配；连接断开后必须 remove_host，不能复用编号。
    pub fn add_host(&mut self) -> Result<u64> {
        if self.failed || self.hosts.len() >= MAX_HOSTS {
            return Err("后台不可用或宿主数超限".into());
        }
        self.last_host = self.last_host.checked_add(1).ok_or("宿主编号耗尽")?;
        self.hosts.insert(
            self.last_host,
            Host {
                sessions: BTreeMap::new(),
                last_request: 0,
                last_session: 0,
                last_focus: 0,
            },
        );
        Ok(self.last_host)
    }
    pub fn remove_host(&mut self, host: u64) {
        self.hosts.remove(&host);
        if self.active.is_some_and(|focus| focus.host == host) {
            self.active = None;
            self.pending = None;
        }
    }
    pub fn dispatch(&mut self, host: u64, frame: &[u8]) -> Result<Vec<u8>> {
        let Message::Request(request) = tigerrime_ipc::decode(frame)? else {
            return Err("需要请求帧".into());
        };
        let reply = if self.failed {
            Reply::Rejected(Rejection::Unavailable)
        } else if let Err(reason) = self.admit(host, &request) {
            Reply::Rejected(reason)
        } else {
            match self.execute(host, &request) {
                Ok(reply) => reply,
                Err(error) => {
                    self.invalidate();
                    return Err(error);
                }
            }
        };
        // 编码失败后不能重放已改变状态的请求；整条调度失效，宿主应取消未上屏组合。
        match tigerrime_ipc::encode(&Message::Response(Response {
            route: request.route,
            reply,
        })) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                self.invalidate();
                Err(error.into())
            }
        }
    }
    fn admit(&mut self, id: u64, request: &Request) -> std::result::Result<(), Rejection> {
        if request.route.generation != self.generation {
            return Err(Rejection::StaleGeneration);
        }
        let total: usize = self.hosts.values().map(|host| host.sessions.len()).sum();
        let host = self.hosts.get_mut(&id).ok_or(Rejection::UnknownHost)?;
        if request.route.request_id <= host.last_request {
            return Err(Rejection::OutOfOrder);
        }
        host.last_request = request.route.request_id;
        if matches!(request.command, Command::Open { .. }) {
            if request.route.session_id <= host.last_session {
                return Err(Rejection::SessionReused);
            }
            host.last_session = request.route.session_id;
            if total >= MAX_SESSIONS {
                return Err(Rejection::SessionLimit);
            }
        } else if !host.sessions.contains_key(&request.route.session_id) {
            return Err(Rejection::UnknownSession);
        }
        Ok(())
    }
    fn execute(&mut self, host: u64, request: &Request) -> Result<Reply> {
        let session = request.route.session_id;
        if let Command::Focus { epoch, caret } = request.command {
            if epoch <= self.hosts[&host].last_focus {
                return Ok(Reply::Rejected(Rejection::StaleFocus));
            }
            self.hosts.get_mut(&host).unwrap().last_focus = epoch;
            // 新焦点取消上一个未上屏组合；迟到的旧宿主失焦不能清掉其他宿主的候选。
            if caret.is_some()
                || self
                    .active
                    .is_some_and(|f| f.host == host && f.session == session)
            {
                if let Some(old) = self.active.take()
                    && (old.host != host || old.session != session)
                {
                    self.hosts
                        .get_mut(&old.host)
                        .unwrap()
                        .sessions
                        .get_mut(&old.session)
                        .unwrap()
                        .clear()?;
                }
                self.pending = None;
                self.active = caret.map(|caret| Focus {
                    host,
                    session,
                    epoch,
                    caret,
                });
            }
            let state = self
                .hosts
                .get_mut(&host)
                .unwrap()
                .sessions
                .get_mut(&session)
                .unwrap()
                .clear()?;
            return Ok(Reply::Focused(state));
        }
        if matches!(
            request.command,
            Command::Key { .. } | Command::Select { .. } | Command::TakeSelection { .. }
        ) && !self
            .active
            .is_some_and(|f| f.host == host && f.session == session)
        {
            return Ok(Reply::Rejected(Rejection::NotFocused));
        }
        let command = if let Command::TakeSelection { epoch } = request.command {
            if self.active.unwrap().epoch != epoch {
                return Ok(Reply::Rejected(Rejection::StaleFocus));
            }
            let Some(choice) = self.pending.take() else {
                return Ok(Reply::Rejected(Rejection::StaleSnapshot));
            };
            if self.frame().as_ref().map(|frame| frame.token) != Some(choice.token) {
                return Ok(Reply::Rejected(Rejection::StaleSnapshot));
            }
            Command::Select {
                revision: choice.token.revision,
                index: choice.index,
                epoch,
            }
        } else {
            request.command.clone()
        };
        if let Command::Key { epoch, .. } | Command::Select { epoch, .. } = command
            && self.active.is_none_or(|focus| focus.epoch != epoch)
        {
            return Ok(Reply::Rejected(Rejection::StaleFocus));
        }
        // 同一会话的按键/清屏/配置都使待领取点击失效；不能在新状态上选旧索引。
        if self
            .active
            .is_some_and(|f| f.host == host && f.session == session)
        {
            self.pending = None;
        }
        let owner = self.hosts.get_mut(&host).ok_or("宿主准入状态不一致")?;
        Ok(match command {
            Command::Open { options } => {
                let engine = Session::new(self.dictionary, options);
                let state = engine.snapshot();
                owner.sessions.insert(session, engine);
                Reply::Opened(state)
            }
            Command::Close => {
                owner.sessions.remove(&session);
                if self
                    .active
                    .is_some_and(|f| f.host == host && f.session == session)
                {
                    self.active = None;
                }
                Reply::Closed
            }
            _ => {
                let engine = owner
                    .sessions
                    .get_mut(&session)
                    .ok_or("会话准入状态不一致")?;
                match command {
                    Command::Key { key, .. } => Reply::Key(engine.process(key)?),
                    Command::Clear => Reply::Cleared(engine.clear()?),
                    Command::Configure { options } => Reply::Configured(engine.configure(options)?),
                    Command::Select {
                        revision, index, ..
                    } => match engine.select(revision, index) {
                        Ok(value) => Reply::Key(value),
                        Err(_) => Reply::Rejected(Rejection::StaleSnapshot),
                    },
                    _ => return Err("会话动作不一致".into()),
                }
            }
        })
    }
    pub fn frame(&self) -> Option<CandidateFrame> {
        let focus = self.active?;
        let state = self
            .hosts
            .get(&focus.host)?
            .sessions
            .get(&focus.session)?
            .snapshot();
        Some(CandidateFrame {
            token: CandidateToken {
                host: focus.host,
                generation: self.generation,
                session: focus.session,
                focus: focus.epoch,
                revision: state.revision,
            },
            caret: focus.caret,
            state,
        })
    }
    /// 返回应通知的宿主；这里只存一次选字意图，UI 线程不能直接产生上屏提交。
    pub fn offer_selection(
        &mut self,
        choice: CandidateChoice,
    ) -> std::result::Result<u64, Rejection> {
        let frame = self.frame().ok_or(Rejection::NotFocused)?;
        if frame.token != choice.token || choice.index as usize >= frame.state.candidates.len() {
            return Err(Rejection::StaleSnapshot);
        }
        if self.pending.is_some() {
            return Err(Rejection::Busy);
        }
        self.pending = Some(choice);
        Ok(choice.token.host)
    }
    pub fn invalidate(&mut self) {
        self.failed = true;
        self.hosts.clear();
        self.active = None;
        self.pending = None;
    }
}
