//! 引擎线程独占的多宿主路由与回收策略；宿主标识由通道所有者给出（跨进程为认证后的连接），不接受正文冒认宿主。
//! 类型化收发：跨进程的编解码与协议版本在 ipc，进程内通道直接传 Request/Response。
use crate::Result;
use std::collections::BTreeMap;
use tigertype_contracts::{
    Options,
    protocol::{Command, Prepare, Rejection, Reply, Request, Response},
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
    // 最近一次往来的逻辑时钟，回收时选最久未用者；不读取系统时钟。
    used: u64,
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
    // 当前设置及其版本；版本 0 为启动时设置，每次热重载递增。
    options: Options,
    settings: u64,
    clock: u64,
}
impl<'a> Dispatcher<'a> {
    /// options 为启动时已校验的设置；宿主不读盘，会话一律以此（及其后的热重载）打开。
    pub fn new(dictionary: &'a Dictionary, generation: u64, options: Options) -> Result<Self> {
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
            options,
            settings: 0,
            clock: 0,
        })
    }
    /// 设置热重载：新会话立即使用；已打开会话在下一次请求时交给引擎，无组合即换上，组合中等本次组合结束（含顶屏）再换。
    pub fn reconfigure(&mut self, options: Options) {
        self.options = options;
        self.settings += 1;
    }
    /// 只在通道认证成功、建立宿主时分配；断开后必须 remove_host，不能复用编号。
    pub fn add_host(&mut self) -> Result<u64> {
        if self.failed || self.hosts.len() >= MAX_HOSTS {
            return Err("后台不可用或宿主数超限".into());
        }
        self.last_host = self.last_host.checked_add(1).ok_or("宿主编号耗尽")?;
        self.clock += 1;
        self.hosts.insert(
            self.last_host,
            Host {
                sessions: BTreeMap::new(),
                last_request: 0,
                last_session: 0,
                last_focus: 0,
                used: self.clock,
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
    /// 宿主的会话外往来（唤起窗口、图标报告等）同样算作使用，推迟其回收。
    pub fn touch(&mut self, host: u64) {
        self.clock += 1;
        if let Some(owner) = self.hosts.get_mut(&host) {
            owner.used = self.clock;
        }
    }
    /// 引擎内部失败使整个调度失效：本请求与之后的请求都回复不可用，宿主随之取消未上屏组合、放行原键。
    pub fn handle(&mut self, host: u64, request: Request) -> Response {
        self.touch(host);
        let reply = if self.failed {
            Reply::Rejected(Rejection::Unavailable)
        } else if let Err(reason) = self.admit(host, &request) {
            Reply::Rejected(reason)
        } else {
            self.execute(host, &request).unwrap_or_else(|_| {
                self.invalidate();
                Reply::Rejected(Rejection::Unavailable)
            })
        };
        Response {
            route: request.route,
            reply,
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
        if matches!(
            request.command,
            Command::Key {
                prepare: Prepare::Open,
                ..
            }
        ) {
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
        if let Command::Key {
            epoch,
            prepare: prepare @ (Prepare::Open | Prepare::Focus),
            ..
        } = request.command
        {
            // 可拒绝的代次检查先于新建/清空，拒绝的按键没有处理。
            if epoch == 0 || epoch <= self.hosts[&host].last_focus {
                return Ok(Reply::Rejected(Rejection::StaleFocus));
            }
            if prepare == Prepare::Open {
                self.open(host, session);
            }
            self.focus(host, session, epoch)?;
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
                    Command::Key { key, prepare, .. } => {
                        // 已确认焦点/代次后才清空，省掉中间快照和一次往返。
                        if prepare == Prepare::Clear {
                            engine.clear()?;
                        }
                        Reply::Key(engine.process(key)?)
                    }
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
    /// 新会话总是使用后台当前设置；宿主不读盘，也无从携带选项。
    fn open(&mut self, host: u64, session: u64) {
        self.hosts.get_mut(&host).unwrap().sessions.insert(
            session,
            (Session::new(self.dictionary, self.options), self.settings),
        );
    }
    /// 取得焦点：取消其他会话未上屏的组合，清空本会话并采用新设置。
    fn focus(&mut self, host: u64, session: u64, epoch: u64) -> Result<()> {
        self.hosts.get_mut(&host).unwrap().last_focus = epoch;
        if let Some(old) = self.active.replace(Focus {
            host,
            session,
            epoch,
        }) && (old.host != host || old.session != session)
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
        let entry = self
            .hosts
            .get_mut(&host)
            .unwrap()
            .sessions
            .get_mut(&session)
            .unwrap();
        entry.0.clear()?;
        adopt(entry, self.options, self.settings)?;
        Ok(())
    }
    /// 可回收：非当前焦点且所有会话都无组合；回收后宿主下次请求重连并重新打开会话，不丢未上屏内容。
    pub fn idle(&self, host: u64) -> bool {
        self.active.is_none_or(|focus| focus.host != host)
            && self
                .hosts
                .get(&host)
                .is_some_and(|owner| owner.sessions.values().all(|(s, _)| !s.composing()))
    }
    /// 合计会话已达上限：新宿主的 Open 会被拒绝，须先回收空闲宿主。
    pub fn full(&self) -> bool {
        self.hosts.values().map(|h| h.sessions.len()).sum::<usize>() >= MAX_SESSIONS
    }
    /// 回收策略（各平台共用）：已无空闲接入位（`spare` 为假）或合计会话达上限时，在空闲（见 [`Self::idle`]）
    /// 且平台另行认可（`quiet`：如传输无半帧、无在途回复、不持有任务栏图标）的宿主中选最久未用者。
    pub fn reclaimable(&self, spare: bool, quiet: impl Fn(u64) -> bool) -> Option<u64> {
        if spare && !self.full() {
            return None;
        }
        self.hosts
            .iter()
            .filter(|&(&host, _)| self.idle(host) && quiet(host))
            .min_by_key(|(_, owner)| owner.used)
            .map(|(&host, _)| host)
    }
    pub fn invalidate(&mut self) {
        self.failed = true;
        self.hosts.clear();
        self.active = None;
    }
}
/// 每个设置版本只交给会话一次；组合中由引擎暂存，不改宿主已显示的候选与编号。
fn adopt(entry: &mut (Session<'_>, u64), options: Options, settings: u64) -> Result<()> {
    if entry.1 != settings {
        entry.0.adopt(options)?;
        entry.1 = settings;
    }
    Ok(())
}
