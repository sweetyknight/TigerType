//! 单一管理通道的串行会话调度；由引擎专用线程持有，不创建线程或加载 DLL。

use crate::Result;
use std::collections::BTreeMap;
use tigerrime_contracts::protocol::{Command, Message, Rejection, Reply, Request, Response};
use tigerrime_engine::{Engine, Session};

const MAX_SESSIONS: usize = 64;

struct Admission {
    generation: u64,
    last_request: u64,
    last_session: u64,
}

impl Admission {
    fn check(
        &mut self,
        request: &Request,
        exists: bool,
        count: usize,
    ) -> std::result::Result<(), Rejection> {
        if request.route.generation != self.generation {
            return Err(Rejection::StaleGeneration);
        }
        if request.route.request_id <= self.last_request {
            return Err(Rejection::OutOfOrder);
        }
        // 接受序号后即消耗它，业务失败也不能重放可能已生效的按键。
        self.last_request = request.route.request_id;
        if matches!(request.command, Command::Open { .. }) {
            if request.route.session_id <= self.last_session {
                return Err(Rejection::SessionReused);
            }
            self.last_session = request.route.session_id;
            if count >= MAX_SESSIONS {
                return Err(Rejection::SessionLimit);
            }
        } else if !exists {
            return Err(Rejection::UnknownSession);
        }
        Ok(())
    }
}

/// 借用 Engine 保持线程亲和；断连时必须丢弃调度器，并由管理端结束该引擎代次。
/// ```compile_fail
/// fn requires_send<T: Send>() {}
/// requires_send::<tigerrime_app::dispatch::Dispatcher<'static>>();
/// ```
pub struct Dispatcher<'engine> {
    engine: &'engine Engine,
    sessions: BTreeMap<u64, Session<'engine>>,
    admission: Admission,
    failed: bool,
}

impl<'engine> Dispatcher<'engine> {
    pub fn new(engine: &'engine Engine, generation: u64) -> Result<Self> {
        if generation == 0 {
            return Err("引擎代次不能为零".into());
        }
        Ok(Self {
            engine,
            sessions: BTreeMap::new(),
            admission: Admission {
                generation,
                last_request: 0,
                last_session: 0,
            },
            failed: false,
        })
    }

    /// 只在引擎线程处理完整帧；不含管道等待，不能用于 TSF 回调或宣称满足 50ms。
    pub fn dispatch(&mut self, frame: &[u8]) -> Result<Vec<u8>> {
        let Message::Request(request) = tigerrime_ipc::decode(frame)? else {
            return Err("引擎通道只接受请求帧".into());
        };
        let reply = if self.failed {
            Reply::Rejected(Rejection::Unavailable)
        } else if let Err(reason) = self.admission.check(
            &request,
            self.sessions.contains_key(&request.route.session_id),
            self.sessions.len(),
        ) {
            Reply::Rejected(reason)
        } else {
            match self.execute(&request) {
                Ok(reply) => reply,
                Err(error) => {
                    self.invalidate();
                    // 保留原始引擎错误；不把状态不确定的失败包装为可重试成功。
                    return Err(error);
                }
            }
        };
        match tigerrime_ipc::encode(&Message::Response(Response {
            route: request.route,
            reply,
        })) {
            Ok(frame) => Ok(frame),
            Err(error) => {
                // 上屏数据可能已经从 Rime 消费，编码失败后同样停用，不静默丢弃后继续。
                self.invalidate();
                Err(error.into())
            }
        }
    }

    fn invalidate(&mut self) {
        self.failed = true;
        self.sessions.clear();
    }

    fn execute(&mut self, request: &Request) -> Result<Reply> {
        let id = request.route.session_id;
        Ok(match &request.command {
            Command::Open { schema } => {
                let session = self.engine.create_session(schema)?;
                let state = session.snapshot()?;
                self.sessions.insert(id, session);
                Reply::Opened(state)
            }
            Command::Key { keycode, modifiers } => Reply::Key(
                self.sessions
                    .get_mut(&id)
                    .ok_or("会话准入状态不一致")?
                    .process_key(*keycode, *modifiers)?,
            ),
            Command::Clear => {
                let session = self.sessions.get_mut(&id).ok_or("会话准入状态不一致")?;
                session.clear();
                Reply::Cleared(session.snapshot()?)
            }
            Command::Close => {
                self.sessions
                    .remove(&id)
                    .ok_or("会话准入状态不一致")?
                    .close()?;
                Reply::Closed
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigerrime_contracts::protocol::Route;

    fn request(request_id: u64, session_id: u64, command: Command) -> Request {
        Request {
            route: Route {
                generation: 7,
                request_id,
                session_id,
            },
            command,
        }
    }

    #[test]
    fn rejects_replay_old_generation_and_counter_wrap_without_advancing_on_stale_generation() {
        let mut gate = Admission {
            generation: 7,
            last_request: 0,
            last_session: 0,
        };
        let key = request(
            1,
            1,
            Command::Key {
                keycode: 32,
                modifiers: 0,
            },
        );
        assert_eq!(gate.check(&key, true, 1), Ok(()));
        assert_eq!(gate.check(&key, true, 1), Err(Rejection::OutOfOrder));
        let mut stale = request(u64::MAX, 1, Command::Clear);
        stale.route.generation = 6;
        assert_eq!(gate.check(&stale, true, 1), Err(Rejection::StaleGeneration));
        assert_eq!(gate.check(&request(2, 1, Command::Clear), true, 1), Ok(()));
        assert_eq!(
            gate.check(&request(u64::MAX, 1, Command::Close), true, 1),
            Ok(())
        );
        assert_eq!(gate.check(&key, true, 1), Err(Rejection::OutOfOrder));
    }

    #[test]
    fn capacity_and_closed_session_ids_cannot_be_reused_and_rejections_consume_sequence() {
        let mut gate = Admission {
            generation: 7,
            last_request: 0,
            last_session: 0,
        };
        let open = |sequence, session| {
            request(
                sequence,
                session,
                Command::Open {
                    schema: "tiger".into(),
                },
            )
        };
        assert_eq!(gate.check(&open(1, 10), false, MAX_SESSIONS - 1), Ok(()));
        assert_eq!(
            gate.check(&open(2, 11), false, MAX_SESSIONS),
            Err(Rejection::SessionLimit)
        );
        assert_eq!(
            gate.check(&open(2, 11), false, 0),
            Err(Rejection::OutOfOrder)
        );
        assert_eq!(
            gate.check(&open(3, 10), false, 0),
            Err(Rejection::SessionReused)
        );
        assert_eq!(
            gate.check(&open(4, 11), false, 0),
            Err(Rejection::SessionReused)
        );
        assert_eq!(gate.check(&open(5, 12), false, 0), Ok(()));
        assert_eq!(
            gate.check(&request(6, 10, Command::Clear), false, 1),
            Err(Rejection::UnknownSession)
        );
        assert_eq!(
            gate.check(&request(6, 12, Command::Clear), true, 1),
            Err(Rejection::OutOfOrder)
        );
    }
}
