//! 一个有界收件箱、每次往返一个回复槽；取消用连接寿命位加合并唤醒，不因队列满而遗漏清理。
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Instant,
};
use tigertype_backend::{Dictionary, dispatch::Dispatcher};
use tigertype_contracts::{
    Options,
    protocol::{Command, Panel, Reply, Request, Response, Route},
};
use tigertype_input_client::{Channel, Error};
const CAPACITY: usize = 16;
type Alive = Arc<AtomicBool>;
type Answer<T> = SyncSender<Result<T, Error>>;
enum Message {
    Open {
        alive: Alive,
        deadline: Instant,
        answer: Answer<(u64, u64)>,
    },
    Request {
        host: u64,
        request: Request,
        deadline: Instant,
        answer: Answer<Response>,
    },
    Configure {
        options: Options,
        deadline: Instant,
        answer: Answer<()>,
    },
    Wake,
}
#[derive(Clone)]
pub struct Connector {
    sender: SyncSender<Message>,
    stopped: Alive,
}
pub struct Inbox {
    receiver: Receiver<Message>,
    stopped: Alive,
}
/// 不启动线程；组装方只启动一个引擎线程并让它独占 Inbox。
pub fn mailbox() -> (Connector, Inbox) {
    let (sender, receiver) = mpsc::sync_channel(CAPACITY);
    let stopped = Arc::new(AtomicBool::new(false));
    (
        Connector {
            sender,
            stopped: stopped.clone(),
        },
        Inbox { receiver, stopped },
    )
}
impl Connector {
    /// 供菜单读取线程是否已停止，不阻塞、不轮询文件或进程。
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    pub fn link(&self) -> Link {
        Link {
            connector: self.clone(),
            identity: None,
            last_request: 0,
        }
    }
    fn send(&self, message: Message, deadline: Instant) -> Result<(), Error> {
        if Instant::now() >= deadline || self.stopped.load(Ordering::Acquire) {
            return Err(Error::Transport);
        }
        self.sender.try_send(message).map_err(|_| Error::Transport)
    }
    /// 工作线程必须得到确认后才报告 Saved；主线程不等待配置文件 I/O。
    pub fn configure(&self, options: Options, deadline: Instant) -> Result<(), Error> {
        options.validate().map_err(|_| Error::Rejected)?;
        let (answer, receiver) = mpsc::sync_channel(1);
        self.send(
            Message::Configure {
                options,
                deadline,
                answer,
            },
            deadline,
        )?;
        receive(receiver, deadline)
    }
    /// 请求退出不等于线程已退出；组装方还须 join，不能把超时当作资源已释放。
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.wake();
    }
    fn wake(&self) {
        // 满队列已经保证引擎会醒；每次取消息前扫寿命位，因此清理不会被静默丢弃。
        let _ = self.sender.try_send(Message::Wake);
    }
}
struct Identity {
    host: u64,
    generation: u64,
    alive: Alive,
}
pub struct Link {
    connector: Connector,
    identity: Option<Identity>,
    last_request: u64,
}
impl Channel for Link {
    fn ensure(&mut self, deadline: Instant) -> Result<bool, Error> {
        if Instant::now() >= deadline || self.connector.stopped.load(Ordering::Acquire) {
            self.cancel();
            return Err(Error::Transport);
        }
        if self
            .identity
            .as_ref()
            .is_some_and(|id| id.alive.load(Ordering::Acquire))
        {
            return Ok(false);
        }
        self.cancel();
        let alive = Arc::new(AtomicBool::new(true));
        let (answer, receiver) = mpsc::sync_channel(1);
        let result = self
            .connector
            .send(
                Message::Open {
                    alive: alive.clone(),
                    deadline,
                    answer,
                },
                deadline,
            )
            .and_then(|()| receive(receiver, deadline));
        match result {
            Ok((host, generation)) => {
                self.identity = Some(Identity {
                    host,
                    generation,
                    alive,
                });
                Ok(true)
            }
            Err(error) => {
                // 握手已经排队也必须可取消，不能留下孤儿宿主占满槽位。
                alive.store(false, Ordering::Release);
                self.connector.wake();
                Err(error)
            }
        }
    }
    fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> Result<Reply, Error> {
        let result = (|| {
            let id = self
                .identity
                .as_ref()
                .filter(|id| id.alive.load(Ordering::Acquire))
                .ok_or(Error::Transport)?;
            self.last_request = self.last_request.checked_add(1).ok_or(Error::Exhausted)?;
            let route = Route {
                generation: id.generation,
                request_id: self.last_request,
                session_id: session,
            };
            let (answer, receiver) = mpsc::sync_channel(1);
            self.connector.send(
                Message::Request {
                    host: id.host,
                    request: Request { route, command },
                    deadline,
                    answer,
                },
                deadline,
            )?;
            let response = receive(receiver, deadline)?;
            if response.route != route {
                return Err(Error::Rejected);
            }
            Ok(response.reply)
        })();
        if result.is_err() {
            self.cancel();
        }
        result
    }
    fn show(&mut self, _: Panel, _: Instant) -> Result<(), Error> {
        // Mac 菜单直接在主线程唤起窗口，不把窗口动作假装成引擎确认。
        Err(Error::Rejected)
    }
    fn indicate(&mut self, _: bool, deadline: Instant) -> Result<(), Error> {
        // Mac 没有 Windows 双图标协调；仅检查连接寿命。
        self.ensure(deadline).map(|_| ())
    }
    fn cancel(&mut self) {
        if let Some(id) = self.identity.take() {
            id.alive.store(false, Ordering::Release);
            self.connector.wake();
        }
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn receive<T>(receiver: Receiver<Result<T, Error>>, deadline: Instant) -> Result<T, Error> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(Error::Transport)?;
    let result = receiver
        .recv_timeout(remaining)
        .map_err(|_| Error::Transport)?;
    // 即使槽中已有回复，唤醒时过期也不得把迟到提交交给主线程。
    if Instant::now() >= deadline {
        return Err(Error::Transport);
    }
    result
}
impl Inbox {
    pub fn run(
        self,
        dictionary: &Dictionary,
        generation: u64,
        options: Options,
    ) -> tigertype_backend::Result<()> {
        let mut dispatcher = Dispatcher::new(dictionary, generation, options)?;
        let mut hosts = BTreeMap::<u64, Alive>::new();
        while let Ok(message) = self.receiver.recv() {
            if self.stopped.load(Ordering::Acquire) {
                break;
            }
            hosts.retain(|host, alive| {
                if alive.load(Ordering::Acquire) {
                    true
                } else {
                    dispatcher.remove_host(*host);
                    false
                }
            });
            match message {
                Message::Open {
                    alive,
                    deadline,
                    answer,
                } => {
                    if !alive.load(Ordering::Acquire) || Instant::now() >= deadline {
                        continue;
                    }
                    if let Some(old) = dispatcher.reclaimable(hosts.len() < CAPACITY, |_| true) {
                        if let Some(alive) = hosts.remove(&old) {
                            alive.store(false, Ordering::Release);
                        }
                        dispatcher.remove_host(old);
                    }
                    let result = dispatcher
                        .add_host()
                        .map(|host| {
                            hosts.insert(host, alive.clone());
                            (host, generation)
                        })
                        .map_err(|_| Error::Rejected);
                    if answer.try_send(result).is_err() {
                        alive.store(false, Ordering::Release);
                        // 接收者可能已超时且 Wake 已先被消费，立即清理新宿主。
                        hosts.retain(|host, live| {
                            if live.load(Ordering::Acquire) {
                                true
                            } else {
                                dispatcher.remove_host(*host);
                                false
                            }
                        });
                    }
                }
                Message::Request {
                    host,
                    request,
                    deadline,
                    answer,
                } => {
                    if Instant::now() >= deadline {
                        continue;
                    }
                    let response = dispatcher.handle(host, request);
                    if !(Instant::now() < deadline && answer.try_send(Ok(response)).is_ok()) {
                        if let Some(alive) = hosts.remove(&host) {
                            alive.store(false, Ordering::Release);
                        }
                        dispatcher.remove_host(host);
                    }
                }
                Message::Configure {
                    options,
                    deadline,
                    answer,
                } => {
                    if Instant::now() < deadline {
                        dispatcher.reconfigure(options);
                        let _ = answer.try_send(Ok(()));
                    }
                }
                Message::Wake => {}
            }
        }
        for alive in hosts.into_values() {
            alive.store(false, Ordering::Release);
        }
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{thread, time::Duration};
    use tigertype_backend::{Data, Entry};
    use tigertype_contracts::Key;
    use tigertype_input_client::Host;
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }
    fn dictionary() -> Dictionary {
        Dictionary::new(Data {
            entries: vec![Entry {
                character: '虎',
                code: "ab".into(),
                weight: 1,
            }],
            common: vec!['虎'],
            ..Default::default()
        })
        .unwrap()
    }
    #[test]
    fn real_dispatch_roundtrip_reloads_cancels_reclaims_and_stops() {
        let (connector, inbox) = mailbox();
        let engine =
            thread::spawn(move || inbox.run(&dictionary(), 9, Options::default()).unwrap());
        let mut host = Host::new(connector.link());
        assert!(
            host.key(Key::Character('a'), deadline())
                .unwrap()
                .state
                .composing
        );
        host.key(Key::Character('b'), deadline()).unwrap();
        assert_eq!(
            host.key(Key::Space, deadline()).unwrap().commit.as_deref(),
            Some("虎")
        );
        let options = Options {
            page_size: 7,
            ..Default::default()
        };
        connector.configure(options, deadline()).unwrap();
        assert_eq!(
            host.key(Key::Character('a'), deadline())
                .unwrap()
                .state
                .options,
            options
        );
        host.close();
        assert_eq!(
            host.key(Key::Character('b'), deadline())
                .unwrap()
                .state
                .composition
                .text,
            "b"
        );
        // 超过容量的空闲宿主可以回收，当前有组合的宿主不能被回收。
        let mut links = Vec::new();
        for _ in 0..40 {
            let mut link = connector.link();
            link.ensure(deadline()).unwrap();
            links.push(link);
        }
        assert!(links[0].ensure(deadline()).unwrap());
        assert!(host.key(Key::Escape, deadline()).is_ok());
        connector.stop();
        engine.join().unwrap();
        assert!(!host.ready(deadline()));
    }
    #[test]
    fn full_queue_and_late_answer_never_succeed() {
        let (connector, inbox) = mailbox();
        for _ in 0..CAPACITY {
            assert!(connector.sender.try_send(Message::Wake).is_ok());
        }
        assert!(connector.link().ensure(deadline()).is_err());
        drop(inbox);
        assert!(connector.link().ensure(deadline()).is_err());
        let (answer, receiver) = mpsc::sync_channel(1);
        answer.send(Ok(123)).unwrap();
        assert_eq!(receive(receiver, Instant::now()), Err(Error::Transport));
    }
    #[test]
    fn uncertain_key_is_never_replayed_and_its_late_commit_has_no_receiver() {
        let (connector, inbox) = mailbox();
        let client = thread::spawn(move || {
            let mut link = connector.link();
            link.ensure(deadline()).unwrap();
            assert!(
                link.request(
                    1,
                    Command::Close,
                    Instant::now() + Duration::from_millis(10)
                )
                .is_err()
            );
            assert!(link.identity.is_none());
        });
        let Message::Open { answer, alive, .. } =
            inbox.receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("必须先握手");
        };
        answer.send(Ok((1, 1))).unwrap();
        let Message::Request {
            answer, request, ..
        } = inbox.receiver.recv_timeout(Duration::from_secs(2)).unwrap()
        else {
            panic!("必须只有一次请求");
        };
        client.join().unwrap();
        assert!(!alive.load(Ordering::Acquire));
        assert!(
            answer
                .try_send(Ok(Response {
                    route: request.route,
                    reply: Reply::Closed
                }))
                .is_err()
        );
        while let Ok(message) = inbox.receiver.try_recv() {
            assert!(matches!(message, Message::Wake));
        }
    }
    #[test]
    fn timed_out_handshake_is_cancelled_before_engine_starts() {
        let (connector, inbox) = mailbox();
        let mut link = connector.link();
        assert!(
            link.ensure(Instant::now() + Duration::from_millis(1))
                .is_err()
        );
        let engine =
            thread::spawn(move || inbox.run(&dictionary(), 1, Options::default()).unwrap());
        assert!(link.ensure(deadline()).unwrap());
        connector.stop();
        engine.join().unwrap();
    }
}
