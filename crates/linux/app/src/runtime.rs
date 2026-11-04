//! 固定引擎/工作线程组装；加载和设置事务不在 IBus 主线程或逐键路径执行。
use crate::channel::{self, Connector};
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use tigertype_backend::{Dictionary, import, lookup::Controller, storage::Data};
use tigertype_contracts::{
    Options,
    ui::{Reply, Request},
};
struct Work {
    request: Request,
    complete: Box<dyn FnOnce(Reply) + Send>,
}
#[derive(Clone)]
pub struct Worker(SyncSender<Option<Work>>);
fn failed(request: &Request, message: &str) -> Reply {
    match request {
        Request::Lookup { .. } => Reply::Lookup(Err(message.into())),
        _ => Reply::Saved(Err(message.into())),
    }
}
impl Worker {
    /// UI 调用永不阻塞；满队列也明确回复失败，不静默丢弃保存动作。
    pub fn submit(&self, request: Request, complete: impl FnOnce(Reply) + Send + 'static) {
        if let Err(error) = self.0.try_send(Some(Work {
            request,
            complete: Box::new(complete),
        })) {
            let (mpsc::TrySendError::Full(work) | mpsc::TrySendError::Disconnected(work)) = error;
            let work = work.unwrap();
            (work.complete)(failed(&work.request, "工作线程不可用或队列已满"));
        }
    }
}
pub struct Runtime {
    pub connector: Connector,
    pub ready: Arc<OnceLock<Result<Options, String>>>,
    pub worker: Worker,
    engine: JoinHandle<()>,
    work: JoinHandle<()>,
}
impl Runtime {
    pub fn start(tables: PathBuf, root: PathBuf) -> std::io::Result<Self> {
        Self::load(move || {
            let data = if root.try_exists()? {
                Data::open(&root)?
            } else {
                Data::create(&root)?
            };
            Ok((
                Arc::new(Dictionary::new(import::load(&tables)?.data()?)?),
                data,
            ))
        })
    }
    /// 租约从引擎加载期直接移交工作线程，不曾释放再重新争抢；词表只读共享。
    fn load(
        loader: impl FnOnce() -> tigertype_backend::Result<(Arc<Dictionary>, Data)> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (connector, inbox) = channel::mailbox();
        let ready = Arc::new(OnceLock::new());
        let status = ready.clone();
        let (loaded, receive) = mpsc::sync_channel(1);
        let (initial, options) = mpsc::sync_channel::<Result<Options, String>>(1);
        let (worker, jobs) = mpsc::sync_channel::<Option<Work>>(1);
        let engine = thread::Builder::new()
            .name("TigerType-engine".into())
            .spawn(move || {
                let result = (|| -> Result<(), String> {
                    let (dictionary, data) = loader().map_err(|e| e.to_string())?;
                    loaded
                        .send((dictionary.clone(), data))
                        .map_err(|_| "工作线程未启动")?;
                    let options = options.recv().map_err(|_| "工作线程加载失败")??;
                    let _ = status.set(Ok(options));
                    inbox
                        .run(&dictionary, 1, options)
                        .map_err(|e| e.to_string())
                })();
                if let Err(error) = result {
                    let _ = status.set(Err(error));
                }
            })?;
        let wake = connector.clone();
        let work = thread::Builder::new()
            .name("TigerType-worker".into())
            .spawn(move || {
                let Ok((dictionary, data)) = receive.recv() else {
                    return;
                };
                let mut controller = match Controller::new(&dictionary, data) {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = initial.send(Err(error.to_string()));
                        return;
                    }
                };
                if initial.send(Ok(controller.options())).is_err() {
                    return;
                }
                while let Ok(Some(work)) = jobs.recv() {
                    let mut reply = controller.execute(work.request);
                    if let Reply::Saved(Ok(options)) = &reply {
                        // 文件已事务保存，只有引擎确认接收热重载后才给 UI 回 Saved。
                        if wake
                            .configure(*options, Instant::now() + Duration::from_secs(2))
                            .is_err()
                        {
                            reply = Reply::Saved(Err(
                                "配置已写盘，但引擎未确认热重载；请重新启动输入法".into(),
                            ));
                        }
                    }
                    (work.complete)(reply);
                }
                wake.stop();
            });
        let work = match work {
            Ok(work) => work,
            Err(error) => {
                connector.stop();
                let _ = engine.join();
                return Err(error);
            }
        };
        Ok(Self {
            connector,
            ready,
            worker: Worker(worker),
            engine,
            work,
        })
    }
    pub fn shutdown(self) {
        self.connector.stop();
        // 退出期允许等待固定工作线程；显式停止消息不依赖 UI 是否还持有发送端克隆。
        let _ = self.worker.0.send(None);
        let _ = self.work.join();
        let _ = self.engine.join();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tigertype_backend::{Data as EngineData, Entry, resources};
    use tigertype_contracts::Key;
    use tigertype_input_client::Host;
    #[test]
    fn full_or_disconnected_work_queue_returns_an_explicit_failure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let worker = Worker(sender);
        worker.submit(Request::ReloadSettings, |_| {});
        let (tx, rx) = mpsc::sync_channel(1);
        worker.submit(Request::ReloadSettings, move |reply| {
            tx.send(reply).unwrap();
        });
        assert!(matches!(rx.recv().unwrap(), Reply::Saved(Err(_))));
        drop(receiver);
        let (tx, rx) = mpsc::sync_channel(1);
        worker.submit(
            Request::Lookup {
                query: "虎".into(),
                page: 0,
            },
            move |reply| {
                tx.send(reply).unwrap();
            },
        );
        assert!(matches!(rx.recv().unwrap(), Reply::Lookup(Err(_))));
    }
    #[test]
    fn real_worker_preserves_lease_saves_and_reloads_before_reply() {
        let root = resources::new_directory(
            &std::env::temp_dir().canonicalize().unwrap(),
            "tigertype-linux-runtime",
        )
        .unwrap()
        .join("data");
        let path = root.clone();
        let runtime = Runtime::load(move || {
            Ok((
                Arc::new(Dictionary::new(EngineData {
                    entries: vec![Entry {
                        character: '虎',
                        code: "ab",
                        weight: 1,
                    }],
                    common: vec!['虎'],
                    ..Default::default()
                })?),
                Data::create(&path)?,
            ))
        })
        .unwrap();
        let (tx, rx) = mpsc::sync_channel(1);
        runtime.worker.submit(
            Request::SaveSettings(Options {
                page_size: 7,
                ..Default::default()
            }),
            move |reply| {
                tx.send(reply).unwrap();
            },
        );
        assert!(
            matches!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), Reply::Saved(Ok(options)) if options.page_size == 7)
        );
        assert!(Data::open(&root).is_err());
        let mut host = Host::new(runtime.connector.link());
        assert_eq!(
            host.key(Key::Character('a'), Instant::now() + Duration::from_secs(2))
                .unwrap()
                .state
                .options
                .page_size,
            7
        );
        runtime.shutdown();
        assert!(Data::open(&root).is_ok());
    }
    #[test]
    fn failed_loader_disconnects_worker_and_reports_reason() {
        let runtime = Runtime::load(|| Err("合成加载失败".into())).unwrap();
        let ready = runtime.ready.clone();
        runtime.shutdown();
        assert_eq!(ready.get().unwrap().as_ref().unwrap_err(), "合成加载失败");
    }
}
