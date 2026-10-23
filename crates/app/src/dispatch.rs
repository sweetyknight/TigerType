//! 固定引擎线程的多宿主路由；宿主标识来自连接所有者，不接受协议正文冒认宿主。
use crate::Result;
use std::collections::BTreeMap;
use tigertype_contracts::{
    Options,
    protocol::{Command, Message, Rejection, Reply, Request, Response},
};
use tigertype_engine::{Dictionary, Session};
const MAX_HOSTS: usize = 16;
const MAX_SESSIONS: usize = 64;

struct Host<'a> {
    // 会话及其已采用的设置版本。
    sessions: BTreeMap<u64, (Session<'a>, u64)>,
    last_request: u64,
    last_session: u64,
    last_focus: u64,
}
#[derive(Clone, Copy)]
struct Focus {
    host: u64,
    session: u64,
    epoch: u64,
}
pub struct Dispatcher<'a> {
    dictionary: &'a Dictionary,
    hosts: BTreeMap<u64, Host<'a>>,
    generation: u64,
    last_host: u64,
    active: Option<Focus>,
    failed: bool,
    // 最近一次热重载的设置及其版本；版本 0 表示仍是启动时设置。
    options: Options,
    settings: u64,
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
            failed: false,
            options: Options::default(),
            settings: 0,
        })
    }
    /// 设置热重载：已打开会话在下一次无组合的请求时换上；新会话的设置由调用方随 Open 给出。
    pub fn reconfigure(&mut self, options: Options) {
        self.options = options;
        self.settings += 1;
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
        }
    }
    pub fn dispatch(&mut self, host: u64, frame: &[u8]) -> Result<Vec<u8>> {
        let Message::Request(request) = tigertype_ipc::decode(frame)? else {
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
        match tigertype_ipc::encode(&Message::Response(Response {
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
        let full = self.full();
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
            if full {
                return Err(Rejection::SessionLimit);
            }
        } else if !host.sessions.contains_key(&request.route.session_id) {
            return Err(Rejection::UnknownSession);
        }
        Ok(())
    }
    fn execute(&mut self, host: u64, request: &Request) -> Result<Reply> {
        let session = request.route.session_id;
        if let Command::Focus { epoch, focused } = request.command {
            if epoch <= self.hosts[&host].last_focus {
                return Ok(Reply::Rejected(Rejection::StaleFocus));
            }
            self.hosts.get_mut(&host).unwrap().last_focus = epoch;
            // 新焦点取消上一个未上屏组合；迟到的旧宿主失焦不能清掉其他宿主的候选。
            if focused
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
                        .0
                        .clear()?;
                }
                self.active = focused.then_some(Focus {
                    host,
                    session,
                    epoch,
                });
            }
            let entry = self
                .hosts
                .get_mut(&host)
                .unwrap()
                .sessions
                .get_mut(&session)
                .unwrap();
            entry.0.clear()?;
            adopt(entry, self.options, self.settings)?;
            return Ok(Reply::Focused(entry.0.snapshot()));
        }
        if matches!(
            request.command,
            Command::Key { .. } | Command::Select { .. }
        ) && !self
            .active
            .is_some_and(|f| f.host == host && f.session == session)
        {
            return Ok(Reply::Rejected(Rejection::NotFocused));
        }
        let command = request.command.clone();
        if let Command::Key { epoch, .. } | Command::Select { epoch, .. } = command
            && self.active.is_none_or(|focus| focus.epoch != epoch)
        {
            return Ok(Reply::Rejected(Rejection::StaleFocus));
        }
        let owner = self.hosts.get_mut(&host).ok_or("宿主准入状态不一致")?;
        Ok(match command {
            Command::Open { options } => {
                let engine = Session::new(self.dictionary, options);
                let state = engine.snapshot();
                owner.sessions.insert(session, (engine, self.settings));
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
                let entry = owner
                    .sessions
                    .get_mut(&session)
                    .ok_or("会话准入状态不一致")?;
                adopt(entry, self.options, self.settings)?;
                let engine = &mut entry.0;
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
    /// 可回收：非当前焦点且所有会话都无组合；回收后宿主下次按键重连并重新打开会话，不丢未上屏内容。
    pub fn idle(&self, host: u64) -> bool {
        self.active.is_none_or(|focus| focus.host != host)
            && self
                .hosts
                .get(&host)
                .is_some_and(|owner| owner.sessions.values().all(|(s, _)| !s.composing()))
    }
    /// 合计会话已达上限时由应用层回收空闲宿主，避免新宿主的 Open 被长期拒绝。
    pub fn full(&self) -> bool {
        self.hosts.values().map(|h| h.sessions.len()).sum::<usize>() >= MAX_SESSIONS
    }
    pub fn invalidate(&mut self) {
        self.failed = true;
        self.hosts.clear();
        self.active = None;
    }
}
/// 热重载的设置只在会话无组合时换上：组合中更换会改变宿主已显示的候选与编号。
fn adopt(entry: &mut (Session<'_>, u64), options: Options, settings: u64) -> Result<()> {
    if entry.1 != settings && !entry.0.composing() {
        entry.0.configure(options)?;
        entry.1 = settings;
    }
    Ok(())
}
