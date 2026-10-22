//! 单线程会话路由：序号准入、代次隔离、一次提交及旧快照拒绝。
use crate::Result;
use std::collections::BTreeMap;
use tigerrime_contracts::protocol::{Command, Message, Rejection, Reply, Request, Response};
use tigerrime_engine::{Dictionary, Session};
const MAX_SESSIONS: usize = 64;

pub struct Dispatcher<'a> {
    dictionary: &'a Dictionary,
    sessions: BTreeMap<u64, Session<'a>>,
    generation: u64,
    last_request: u64,
    last_session: u64,
    failed: bool,
}
impl<'a> Dispatcher<'a> {
    pub fn new(dictionary: &'a Dictionary, generation: u64) -> Result<Self> {
        if generation == 0 {
            return Err("后台代次不能为零".into());
        }
        Ok(Self {
            dictionary,
            sessions: BTreeMap::new(),
            generation,
            last_request: 0,
            last_session: 0,
            failed: false,
        })
    }
    pub fn dispatch(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        let Message::Request(request) = tigerrime_ipc::decode(frame)? else {
            return Err("需要请求帧".into());
        };
        let reply = if self.failed {
            Reply::Rejected(Rejection::Unavailable)
        } else if let Err(reason) = self.admit(&request) {
            Reply::Rejected(reason)
        } else {
            match self.execute(&request) {
                Ok(reply) => reply,
                Err(error) => {
                    self.invalidate();
                    return Err(error);
                }
            }
        };
        // 状态改变后若编码失败，整条调度失效，不能重放状态不确定的按键。
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
    fn admit(&mut self, request: &Request) -> std::result::Result<(), Rejection> {
        if request.route.generation != self.generation {
            return Err(Rejection::StaleGeneration);
        }
        if request.route.request_id <= self.last_request {
            return Err(Rejection::OutOfOrder);
        }
        self.last_request = request.route.request_id;
        if matches!(request.command, Command::Open { .. }) {
            if request.route.session_id <= self.last_session {
                return Err(Rejection::SessionReused);
            }
            self.last_session = request.route.session_id;
            if self.sessions.len() >= MAX_SESSIONS {
                return Err(Rejection::SessionLimit);
            }
        } else if !self.sessions.contains_key(&request.route.session_id) {
            return Err(Rejection::UnknownSession);
        }
        Ok(())
    }
    fn execute(&mut self, request: &Request) -> Result<Reply> {
        let id = request.route.session_id;
        Ok(match request.command {
            Command::Open { options } => {
                let session = Session::new(self.dictionary, options);
                let state = session.snapshot();
                self.sessions.insert(id, session);
                Reply::Opened(state)
            }
            Command::Close => {
                self.sessions.remove(&id);
                Reply::Closed
            }
            _ => {
                let session = self.sessions.get_mut(&id).ok_or("会话准入状态不一致")?;
                match request.command {
                    Command::Key { key } => Reply::Key(session.process(key)?),
                    Command::Clear => Reply::Cleared(session.clear()?),
                    Command::Configure { options } => {
                        Reply::Configured(session.configure(options)?)
                    }
                    Command::Select { revision, index } => match session.select(revision, index) {
                        Ok(value) => Reply::Key(value),
                        Err(_) => Reply::Rejected(Rejection::StaleSnapshot),
                    },
                    _ => return Err("不一致的会话动作".into()),
                }
            }
        })
    }
    pub fn invalidate(&mut self) {
        self.failed = true;
        self.sessions.clear();
    }
}
