//! 单通道在途请求状态；不拥有系统句柄，cancel 不能替代传输层的 CancelIoEx。

use crate::Message;
use crate::{Error, FrameAssembler, Result, decode_response, encode};
use std::time::{Duration, Instant};
use tigertype_contracts::protocol::{Request, Response};

struct Pending {
    request: Request,
    deadline: Instant,
    frame: FrameAssembler,
}

enum State {
    Idle,
    Pending(Pending),
    Closed,
}

pub struct ClientState {
    generation: u64,
    last_request: u64,
    state: State,
}

impl ClientState {
    pub fn new(generation: u64) -> Result<Self> {
        if generation == 0 {
            return Err(Error::Invalid("引擎代次不能为零"));
        }
        Ok(Self {
            generation,
            last_request: 0,
            state: State::Idle,
        })
    }

    /// 截止时间须在连接前创建；连接、发送和收取各分片只能共用 remaining()。
    /// 队列容量为一个在途请求，Busy 不覆盖它；本层从不自行重试按键。
    pub fn begin(&mut self, request: Request, deadline: Instant) -> Result<Vec<u8>> {
        match self.state {
            State::Closed => return Err(Error::Closed),
            State::Pending(_) => {
                self.remaining()?;
                return Err(Error::Busy);
            }
            State::Idle => {}
        }
        if request.route.generation != self.generation
            || request.route.request_id <= self.last_request
        {
            return Err(Error::Invalid("请求代次不符或序号未递增"));
        }
        if Instant::now() >= deadline {
            self.cancel();
            return Err(Error::TimedOut);
        }
        let frame = encode(&Message::Request(request.clone()))?;
        self.last_request = request.route.request_id;
        self.state = State::Pending(Pending {
            request,
            deadline,
            frame: FrameAssembler::default(),
        });
        // 编码时间也计入同一预算；超时后不能将帧交给传输层发送。
        self.remaining()?;
        Ok(frame)
    }

    pub fn remaining(&mut self) -> Result<Duration> {
        self.remaining_at(Instant::now())
    }

    fn remaining_at(&mut self, now: Instant) -> Result<Duration> {
        match &self.state {
            State::Pending(pending) => {
                if now < pending.deadline {
                    return Ok(pending.deadline - now);
                }
                self.cancel();
                Err(Error::TimedOut)
            }
            State::Idle => Err(Error::Invalid("当前没有在途请求")),
            State::Closed => Err(Error::Closed),
        }
    }

    /// 只在完整、匹配且未过期时交付一次。EOF/写失败须由传输层调用 cancel。
    pub fn receive(&mut self, bytes: &[u8]) -> Result<Option<Response>> {
        if let Err(error) = self.remaining() {
            // 空闲时的主动回复也视为协议失步，不能留到下一请求重新解释。
            self.cancel();
            return Err(error);
        }
        let result = self.receive_pending(bytes);
        if result.is_err() {
            self.cancel();
        }
        result
    }

    fn receive_pending(&mut self, bytes: &[u8]) -> Result<Option<Response>> {
        let State::Pending(pending) = &mut self.state else {
            return Err(Error::Closed);
        };
        let Some(frame) = pending.frame.push(bytes)? else {
            self.remaining()?;
            return Ok(None);
        };
        let response = decode_response(&frame, &pending.request)?;
        // 接收和 JSON 校验都不能刷新截止时间，校验期间到期的提交也不交付。
        self.remaining()?;
        self.state = State::Idle;
        Ok(Some(response))
    }

    /// 取消后不可复用；丢弃组合与恢复代次由 app 决策，不在这里重放或重新连接。
    pub fn cancel(&mut self) {
        self.state = State::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_contracts::{
        InputState, KeyOutcome,
        protocol::{Command, Reply, Route},
    };

    fn key_request(id: u64) -> Request {
        Request {
            route: Route {
                generation: 7,
                request_id: id,
                session_id: 1,
            },
            command: Command::Key {
                epoch: 1,
                key: tigertype_contracts::Key::Space,
                prepare: tigertype_contracts::protocol::Prepare::Keep,
            },
        }
    }

    fn reply(request: &Request) -> Vec<u8> {
        encode(&Message::Response(Response {
            route: request.route,
            reply: Reply::Key(KeyOutcome {
                handled: true,
                commit: Some("的".into()),
                state: InputState::default(),
            }),
        }))
        .unwrap()
    }

    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(10)
    }

    #[test]
    fn fragmented_reply_is_delivered_once_and_busy_does_not_replace_pending() {
        let mut client = ClientState::new(7).unwrap();
        let request = key_request(1);
        client.begin(request.clone(), deadline()).unwrap();
        assert!(matches!(
            client.begin(key_request(2), deadline()),
            Err(Error::Busy)
        ));
        let frame = reply(&request);
        for byte in &frame[..frame.len() - 1] {
            assert!(
                client
                    .receive(std::slice::from_ref(byte))
                    .unwrap()
                    .is_none()
            );
        }
        let response = client.receive(&frame[frame.len() - 1..]).unwrap().unwrap();
        assert!(
            matches!(response.reply, Reply::Key(outcome) if outcome.commit.as_deref() == Some("的"))
        );
        assert!(client.receive(&frame).is_err());
        assert!(matches!(
            client.begin(request, deadline()),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn phases_share_one_absolute_deadline_without_sleeping_or_renewal() {
        let mut client = ClientState::new(7).unwrap();
        let end = deadline();
        let request = key_request(1);
        client.begin(request.clone(), end).unwrap();
        assert_eq!(
            client
                .remaining_at(end - Duration::from_millis(50))
                .unwrap(),
            Duration::from_millis(50)
        );
        let frame = reply(&request);
        assert!(client.receive(&frame[..5]).unwrap().is_none());
        assert_eq!(
            client
                .remaining_at(end - Duration::from_millis(10))
                .unwrap(),
            Duration::from_millis(10)
        );
        assert!(matches!(client.remaining_at(end), Err(Error::TimedOut)));
        assert!(matches!(client.receive(&frame[5..]), Err(Error::Closed)));
        assert!(matches!(
            client.begin(request, deadline()),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn expired_start_cancellation_and_disconnect_never_accept_late_commits() {
        let request = key_request(1);
        let mut client = ClientState::new(7).unwrap();
        assert!(matches!(
            client.begin(request.clone(), Instant::now()),
            Err(Error::TimedOut)
        ));
        assert!(matches!(
            client.receive(&reply(&request)),
            Err(Error::Closed)
        ));
        for prefix in [0, 8, 32] {
            let mut client = ClientState::new(7).unwrap();
            client.begin(request.clone(), deadline()).unwrap();
            let frame = reply(&request);
            assert!(client.receive(&frame[..prefix]).unwrap().is_none());
            client.cancel();
            client.cancel();
            assert!(matches!(
                client.receive(&frame[prefix..]),
                Err(Error::Closed)
            ));
            assert!(matches!(client.remaining(), Err(Error::Closed)));
        }
    }

    #[test]
    fn reply_for_an_older_request_poisoning_a_new_exchange_is_not_delivered() {
        let mut client = ClientState::new(7).unwrap();
        let first = key_request(1);
        let old_frame = reply(&first);
        client.begin(first.clone(), deadline()).unwrap();
        assert!(client.receive(&old_frame).unwrap().is_some());
        assert!(client.begin(first, deadline()).is_err());
        let mut wrong_generation = key_request(2);
        wrong_generation.route.generation = 6;
        assert!(client.begin(wrong_generation, deadline()).is_err());
        client.begin(key_request(2), deadline()).unwrap();
        assert!(client.receive(&old_frame).is_err());
        assert!(matches!(
            client.receive(&reply(&key_request(2))),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn a_complete_reply_arriving_after_the_deadline_is_rejected_without_polling_first() {
        let mut client = ClientState::new(7).unwrap();
        let request = key_request(1);
        client.begin(request.clone(), deadline()).unwrap();
        // 直接注入已过期的时点，避免用 sleep 将调度抖动当成正确性断言。
        let State::Pending(pending) = &mut client.state else {
            panic!("请求应处于在途状态");
        };
        pending.deadline = Instant::now();
        assert!(matches!(
            client.receive(&reply(&request)),
            Err(Error::TimedOut)
        ));
        assert!(matches!(
            client.begin(key_request(2), deadline()),
            Err(Error::Closed)
        ));
    }

    #[test]
    fn malformed_coalesced_and_wrong_kind_replies_close_the_channel() {
        let request = key_request(1);
        let frame = reply(&request);
        let mut joined = frame.clone();
        joined.extend_from_slice(&frame);
        let wrong_kind = encode(&Message::Response(Response {
            route: request.route,
            reply: Reply::Closed,
        }))
        .unwrap();
        for bad in [vec![0; crate::HEADER_LEN], joined, wrong_kind] {
            let mut client = ClientState::new(7).unwrap();
            client.begin(request.clone(), deadline()).unwrap();
            assert!(client.receive(&bad).is_err());
            assert!(matches!(client.receive(&frame), Err(Error::Closed)));
        }
    }

    #[test]
    fn numbering_never_wraps_and_invalid_unsent_requests_do_not_break_an_idle_client() {
        assert!(ClientState::new(0).is_err());
        let mut client = ClientState::new(7).unwrap();
        let mut invalid = key_request(1);
        invalid.route.session_id = 0;
        assert!(client.begin(invalid, deadline()).is_err());
        let last = key_request(u64::MAX);
        client.begin(last.clone(), deadline()).unwrap();
        assert!(client.receive(&reply(&last)).unwrap().is_some());
        assert!(client.begin(key_request(1), deadline()).is_err());
    }
}
