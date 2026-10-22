//! 每登录会话一后台：固定反应器/引擎线程与 UI 线程，16 对预建管道，不按连接创建线程。
use crate::{Result, data::Data, dispatch::Dispatcher, import};
use std::{
    os::windows::io::{AsHandle, AsRawHandle},
    path::Path,
    sync::mpsc::TryRecvError,
    time::{Duration, Instant},
};
use tigerrime_contracts::{
    Options,
    candidate::{CandidateChoice, CandidateFrame},
    protocol::{Command, Message},
};
use tigerrime_engine::Dictionary;
use tigerrime_ipc::{
    FrameAssembler,
    pipe::{Listener, Pending, Pipe, Runtime},
    service::HOST_SLOTS,
};
use tigerrime_ui::{Inbox, candidate::FrameSender};
use windows_sys::Win32::{
    Foundation::WAIT_FAILED,
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Threading::{INFINITE, SetEvent, WaitForMultipleObjects},
};

#[derive(Clone, Copy)]
enum Phase {
    Accept,
    Read,
    Write,
    Idle,
}
struct Endpoint {
    pipe: Pipe,
    pending: Option<Pending>,
    phase: Phase,
    frame: FrameAssembler,
    deadline: Option<Instant>,
    connected: bool,
}
enum Event {
    Connected,
    Frame(Vec<u8>),
    Written,
}
impl Endpoint {
    fn listen(listener: Listener) -> Result<Self> {
        let (pipe, pending) = listener.start_accept()?;
        Ok(Self {
            pipe,
            pending: Some(pending),
            phase: Phase::Accept,
            frame: FrameAssembler::default(),
            deadline: None,
            connected: false,
        })
    }
    fn read(&mut self) -> Result<()> {
        self.phase = Phase::Read;
        self.pending = Some(self.pipe.start_read()?);
        Ok(())
    }
    fn write(&mut self, frame: Vec<u8>, deadline: Instant) -> Result<()> {
        if self.pending.is_some() {
            return Err("宿主通知/回复队列已满".into());
        }
        if Instant::now() >= deadline {
            return Err("回复已过期".into());
        }
        self.phase = Phase::Write;
        self.deadline = Some(deadline);
        self.pending = Some(self.pipe.start_write(frame)?);
        Ok(())
    }
    fn tick(&mut self) -> Result<Option<Event>> {
        if self.deadline.is_some_and(|end| Instant::now() >= end) {
            return Err("宿主握手或半帧超时".into());
        }
        let Some(pending) = self.pending.as_mut() else {
            return Ok(None);
        };
        let Some((mut bytes, count)) = pending.try_complete()? else {
            return Ok(None);
        };
        self.pending.take();
        match self.phase {
            Phase::Accept => {
                self.pipe.authenticate_client()?;
                self.connected = true;
                self.phase = Phase::Idle;
                self.deadline = Some(Instant::now() + Duration::from_secs(2));
                Ok(Some(Event::Connected))
            }
            Phase::Read => {
                if count == 0 {
                    return Err("宿主断连".into());
                }
                self.deadline
                    .get_or_insert_with(|| Instant::now() + Duration::from_millis(50));
                if let Some(frame) = self.frame.push(&bytes[..count])? {
                    self.frame = FrameAssembler::default();
                    self.phase = Phase::Idle;
                    Ok(Some(Event::Frame(frame)))
                } else {
                    self.read()?;
                    Ok(None)
                }
            }
            Phase::Write => {
                if count == 0 {
                    return Err("管道写入为零".into());
                }
                if count < bytes.len() {
                    bytes.drain(..count);
                    self.pending = Some(self.pipe.start_write(bytes)?);
                    Ok(None)
                } else {
                    self.phase = Phase::Idle;
                    self.deadline = None;
                    Ok(Some(Event::Written))
                }
            }
            Phase::Idle => Err("在途操作状态不一致".into()),
        }
    }
}
struct Slot {
    request: Endpoint,
    notice: Endpoint,
    hello: bool,
    host: Option<u64>,
    stopping: bool,
}
impl Slot {
    fn bind(runtime: &Runtime, namespace: &str, index: usize) -> Result<Self> {
        // 两个名字都成功占用后才提交异步 accept，避免半对重建不断取消自身并触发忙循环。
        let request = runtime.bind(&format!("{namespace}-request-{index}"))?;
        let notice = runtime.bind(&format!("{namespace}-notice-{index}"))?;
        Ok(Self {
            request: Endpoint::listen(request)?,
            notice: Endpoint::listen(notice)?,
            hello: false,
            host: None,
            stopping: false,
        })
    }
    fn tick(
        &mut self,
        dispatch: &mut Dispatcher<'_>,
        generation: u64,
        options: Options,
        image: &Path,
    ) -> Result<()> {
        if let Some(event) = self.request.tick()? {
            match event {
                Event::Connected | Event::Written => self.request.read()?,
                Event::Frame(frame) => match tigerrime_ipc::decode(&frame)? {
                    Message::Stop if self.host.is_some() => {
                        self.request.pipe.peer()?.verify_image(image)?;
                        self.request.write(
                            tigerrime_ipc::encode(&Message::Stopped)?,
                            self.request.deadline.ok_or("停机缺少期限")?,
                        )?;
                        self.stopping = true;
                    }
                    Message::Hello if self.host.is_none() && !self.hello => self.hello = true,
                    Message::Request(mut request) => {
                        let host = self.host.ok_or("握手前收到输入请求")?;
                        // 输入配置来自后台启动时已校验的设置，不在输入路径读盘。
                        if matches!(request.command, Command::Open { .. }) {
                            request.command = Command::Open { options };
                        }
                        let frame = tigerrime_ipc::encode(&Message::Request(request))?;
                        let reply = dispatch.dispatch(host, &frame)?;
                        self.request
                            .write(reply, self.request.deadline.ok_or("请求缺少期限")?)?;
                    }
                    _ => return Err("宿主请求类型不符".into()),
                },
            }
        }
        self.notice.tick()?;
        if self.request.connected && self.notice.connected {
            if !self
                .request
                .pipe
                .peer()?
                .same_process(self.notice.pipe.peer()?)?
            {
                return Err("请求和通知通道宿主不一致".into());
            }
            if self.hello && self.host.is_none() {
                let host = dispatch.add_host()?;
                self.host = Some(host);
                self.notice.deadline = None;
                self.request.write(
                    tigerrime_ipc::encode(&Message::Welcome { generation, host })?,
                    self.request.deadline.ok_or("握手缺少期限")?,
                )?;
            }
        }
        Ok(())
    }
}

pub struct Server<'a> {
    runtime: Runtime,
    owner: Option<Listener>,
    slots: Vec<Option<Slot>>,
    dispatch: Dispatcher<'a>,
    generation: u64,
    namespace: String,
    options: Options,
    image: std::path::PathBuf,
}
impl<'a> Server<'a> {
    /// namespace 在正式入口固定为 backend；测试使用新建独立标签，不连接现用后台。
    pub fn new(dictionary: &'a Dictionary, namespace: &str, options: Options) -> Result<Self> {
        let runtime = Runtime::new()?;
        // 所有权管道永不接收业务，也不在连接更换时释放；首实例约束就是每登录会话排他租约。
        let owner = runtime.bind(&format!("{namespace}-owner"))?;
        let mut generation = 0u64;
        // SAFETY: 系统随机数写入固定八字节，代次不复用，不作为身份认证替代物。
        if unsafe {
            BCryptGenRandom(
                std::ptr::null_mut(),
                (&mut generation as *mut u64).cast(),
                8,
                BCRYPT_USE_SYSTEM_PREFERRED_RNG,
            )
        } < 0
            || generation == 0
        {
            return Err("无法生成后台代次".into());
        }
        let mut slots = Vec::new();
        for index in 0..HOST_SLOTS {
            slots.push(Some(Slot::bind(&runtime, namespace, index)?));
        }
        Ok(Self {
            runtime,
            owner: Some(owner),
            slots,
            dispatch: Dispatcher::new(dictionary, generation)?,
            generation,
            namespace: namespace.into(),
            options,
            image: std::env::current_exe()?,
        })
    }
    fn remove(&mut self, index: usize) {
        if let Some(slot) = self.slots[index].take() {
            if let Some(host) = slot.host {
                self.dispatch.remove_host(host);
            }
            drop(slot);
            // SAFETY: 即时失败未必产生内核完成包；只额外唤醒一次以补建监听，失败重建本身不自激。
            unsafe {
                SetEvent(self.runtime.completion_event().as_raw_handle());
            }
        }
    }
    fn notify(&mut self, host: u64, message: Message) {
        if let Some(index) = self
            .slots
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.host == Some(host)))
        {
            let result = tigerrime_ipc::encode(&message)
                .map_err(Into::into)
                .and_then(|frame| {
                    self.slots[index]
                        .as_mut()
                        .unwrap()
                        .notice
                        .write(frame, Instant::now() + Duration::from_millis(50))
                });
            // 通知无法交付就断开该宿主，不能让点击留在未知状态或在后来重放。
            if result.is_err() {
                self.remove(index);
            }
        }
    }
    pub fn run(mut self, frames: FrameSender, choices: Inbox<CandidateChoice>) -> Result<()> {
        let result = self.serve(&frames, &choices);
        self.slots.clear();
        self.owner.take();
        self.dispatch.invalidate();
        self.runtime
            .shutdown(Instant::now() + Duration::from_secs(2))?;
        result
    }
    fn serve(&mut self, frames: &FrameSender, choices: &Inbox<CandidateChoice>) -> Result<()> {
        let mut shown: Option<CandidateFrame> = None;
        loop {
            // 每次事件只访问固定 16 槽；空闲等待内核事件，不遍历词表、不忙轮询。
            for index in 0..HOST_SLOTS {
                if let Some(slot) = self.slots[index].as_mut() {
                    if slot
                        .tick(
                            &mut self.dispatch,
                            self.generation,
                            self.options,
                            &self.image,
                        )
                        .is_err()
                        || (slot.request.connected && !slot.request.pipe.peer()?.is_alive()?)
                    {
                        self.remove(index);
                    } else if slot.stopping && matches!(slot.request.phase, Phase::Read) {
                        // 停止确认写入完成后才关闭全部管道；不是用超时冒充线程退出。
                        return Ok(());
                    }
                } else {
                    // 旧取消回调可能仍持有首实例；完成事件到来后才会再次尝试重建。
                    if let Ok(slot) = Slot::bind(&self.runtime, &self.namespace, index) {
                        self.slots[index] = Some(slot);
                    }
                }
            }
            match choices.try_receive() {
                Ok(choice) => {
                    if let Ok(host) = self.dispatch.offer_selection(choice) {
                        self.notify(
                            host,
                            Message::Notification {
                                token: choice.token,
                                reset: false,
                            },
                        );
                    } else {
                        shown = None;
                    }
                }
                Err(TryRecvError::Disconnected) => return Ok(()),
                Err(TryRecvError::Empty) => {}
            }
            let current = self.dispatch.frame();
            if current != shown {
                if let Some(old) = &shown
                    && current.as_ref().is_none_or(|new| {
                        (new.token.host, new.token.session, new.token.focus)
                            != (old.token.host, old.token.session, old.token.focus)
                    })
                {
                    self.notify(
                        old.token.host,
                        Message::Notification {
                            token: old.token,
                            reset: true,
                        },
                    );
                }
                frames.send(current.clone())?;
                shown = current;
            }
            let mut handles = vec![
                self.runtime.completion_event().as_raw_handle(),
                choices.event().as_raw_handle(),
            ];
            let mut deadline = None;
            for slot in self.slots.iter().flatten() {
                if slot.request.connected {
                    handles.push(slot.request.pipe.peer()?.as_handle().as_raw_handle());
                }
                for end in [slot.request.deadline, slot.notice.deadline]
                    .into_iter()
                    .flatten()
                {
                    deadline = Some(deadline.map_or(end, |old: Instant| old.min(end)));
                }
            }
            let timeout = deadline.map_or(INFINITE, |end| {
                end.saturating_duration_since(Instant::now())
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min((u32::MAX - 1) as u128) as u32
            });
            // SAFETY: 所有句柄由此 Server/Inbox 持有；完成、用户关闭、宿主退出及有限期限共用一次等待。
            if unsafe { WaitForMultipleObjects(handles.len() as u32, handles.as_ptr(), 0, timeout) }
                == WAIT_FAILED
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
}

pub fn run(package: &Path, directory: &Path) -> Result<()> {
    let dictionary = Dictionary::new(import::load(package)?.data)?;
    let data = if directory.try_exists()? {
        Data::open(directory)?
    } else {
        Data::create(directory)?
    };
    let options = data
        .settings()?
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()?
        .unwrap_or_default();
    let (frames, inbox) = tigerrime_ui::candidate::mailbox()?;
    let (choices, received) = tigerrime_ui::mailbox()?;
    let worker = std::thread::Builder::new()
        .name("tiger-backend".into())
        .spawn(move || -> std::result::Result<(), String> {
            let _data = data;
            let work = || -> Result<()> {
                Server::new(&dictionary, "backend", options)?.run(frames, received)
            };
            work().map_err(|e| e.to_string())
        })?;
    let result = tigerrime_ui::candidate::run(inbox, choices, "停止后台");
    worker.join().map_err(|_| "后台线程异常退出")??;
    result.map_err(Into::into)
}
