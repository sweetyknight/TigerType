//! 每登录会话一后台：固定反应器/引擎线程，16 个预建请求管道，不按连接创建线程。
//! 宿主只凭 DACL 与同会话接纳，不打开宿主进程；断开由常驻读失败发现，始终保留至少一个空闲监听槽。
//! 查字/设置窗在同进程的固定 UI 线程，保存后的设置经交接槽热重载到引擎线程，输入路径不读盘。
use crate::{Result, data::Data, dispatch::Dispatcher, import, lookup::Controller};
use std::{
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tigertype_contracts::{
    Options,
    protocol::{Command, Message},
    ui::Reply as UiReply,
};
use tigertype_engine::Dictionary;
use tigertype_ipc::{
    FrameAssembler,
    pipe::{Listener, Pending, Pipe, Runtime, grant_app_container_query},
    service::HOST_SLOTS,
};
use tigertype_ui::ReplySender;
use windows_sys::Win32::{
    Foundation::WAIT_FAILED,
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
};

/// 查字工作线程保存设置后交给引擎线程的最新设置：只留最后一份，事件唤醒引擎，不排队、不轮询。
struct Reload {
    latest: Mutex<Option<Options>>,
    event: OwnedHandle,
}
impl Reload {
    fn new() -> Result<Self> {
        // SAFETY: 无命名、不可继承的自动复位事件；成功后立即交给 OwnedHandle 配对释放。
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Ok(Self {
            latest: Mutex::new(None),
            // SAFETY: 句柄刚创建，尚无其他所有者。
            event: unsafe { OwnedHandle::from_raw_handle(event) },
        })
    }
    fn set(&self, options: Options) -> Result<()> {
        *self.latest.lock().map_err(|_| "设置交接锁已损坏")? = Some(options);
        // SAFETY: self 持有事件句柄；失败必须报错，不能让已保存的设置静默不生效。
        if unsafe { SetEvent(self.event.as_raw_handle()) } == 0 {
            return Err(io::Error::last_os_error().into());
        }
        Ok(())
    }
    fn take(&self) -> Result<Option<Options>> {
        Ok(self.latest.lock().map_err(|_| "设置交接锁已损坏")?.take())
    }
}

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
            return Err("宿主回复队列已满".into());
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
                self.pipe.verify_client()?;
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
    end: Endpoint,
    host: Option<u64>,
    stopping: bool,
    // 最近一次收到帧的逻辑时钟，回收时选最久未用者；不读取系统时钟。
    used: u64,
}
impl Slot {
    fn bind(runtime: &Runtime, namespace: &str, index: usize) -> Result<Self> {
        Ok(Self {
            end: Endpoint::listen(runtime.bind(&format!("{namespace}-request-{index}"))?)?,
            host: None,
            stopping: false,
            used: 0,
        })
    }
    /// 已握手且只剩常驻空闲读取：无半帧、无在途回复；组合与焦点另由调度器判定。
    fn quiet(&self) -> bool {
        self.host.is_some()
            && !self.stopping
            && matches!(self.end.phase, Phase::Read)
            && self.end.deadline.is_none()
    }
    fn tick(
        &mut self,
        dispatch: &mut Dispatcher<'_>,
        generation: u64,
        options: Options,
        image: &Path,
        clock: &mut u64,
        show: Option<&ReplySender<()>>,
    ) -> Result<()> {
        let Some(event) = self.end.tick()? else {
            return Ok(());
        };
        let frame = match event {
            // 停止确认写完即结束：不再挂读取，避免对端先关闭使读取失败而丢失已确认的停止。
            Event::Written if self.stopping => return Ok(()),
            Event::Connected | Event::Written => return self.end.read(),
            Event::Frame(frame) => frame,
        };
        *clock += 1;
        self.used = *clock;
        let deadline = self.end.deadline.ok_or("请求缺少期限")?;
        let reply = match tigertype_ipc::decode(&frame)? {
            Message::Stop if self.host.is_some() => {
                // 停止命令仍打开调用进程核对映像；普通宿主不经过此路径，也不被打开。
                self.end.pipe.verify_client_image(image)?;
                self.stopping = true;
                tigertype_ipc::encode(&Message::Stopped)?
            }
            Message::Show if self.host.is_some() => {
                // 与停止同样核对调用进程映像；窗口由 UI 线程创建，这里只投递唤起信号。
                self.end.pipe.verify_client_image(image)?;
                show.ok_or("此后台未提供查字窗")?.send(())?;
                tigertype_ipc::encode(&Message::Shown)?
            }
            Message::Hello if self.host.is_none() => {
                let host = dispatch.add_host()?;
                self.host = Some(host);
                tigertype_ipc::encode(&Message::Welcome { generation, host })?
            }
            Message::Request(mut request) => {
                let host = self.host.ok_or("握手前收到输入请求")?;
                // 输入配置来自后台启动时已校验的设置，不在输入路径读盘。
                if matches!(request.command, Command::Open { .. }) {
                    request.command = Command::Open { options };
                }
                let frame = tigertype_ipc::encode(&Message::Request(request))?;
                dispatch.dispatch(host, &frame)?
            }
            _ => return Err("宿主请求类型不符".into()),
        };
        self.end.write(reply, deadline)
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
    clock: u64,
    reload: Arc<Reload>,
    // 正式后台才有查字 UI 线程；随 Server 释放而断开，UI 线程据此退出。
    show: Option<ReplySender<()>>,
}
impl<'a> Server<'a> {
    /// namespace 在正式入口固定为 backend；测试使用新建独立标签，不连接现用后台。
    pub fn new(dictionary: &'a Dictionary, namespace: &str, options: Options) -> Result<Self> {
        // 沙箱宿主须以受限查询打开后台核对映像；只改本进程对象 DACL，退出即失效。
        grant_app_container_query()?;
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
            clock: 0,
            reload: Arc::new(Reload::new()?),
            show: None,
        })
    }
    fn remove(&mut self, index: usize) {
        if let Some(slot) = self.slots[index].take() {
            if let Some(host) = slot.host {
                self.dispatch.remove_host(host);
            }
            // 立即断开，宿主下次发送前的非阻塞检查即可发现，不必等取消回调关闭句柄。
            slot.end.pipe.disconnect();
            drop(slot);
            // SAFETY: 即时失败未必产生内核完成包；只额外唤醒一次以补建监听，失败重建本身不自激。
            unsafe {
                SetEvent(self.runtime.completion_event().as_raw_handle());
            }
        }
    }
    /// 最后一个空槽被占用或会话达上限时，回收最久未用、非焦点、无组合且无未读字节的宿主。
    /// 待重建的槽位算作空槽，避免旧实例释放前重复回收。
    fn reclaim(&mut self) {
        let free = self
            .slots
            .iter()
            .any(|slot| slot.as_ref().is_none_or(|slot| !slot.end.connected));
        if free && !self.dispatch.full() {
            return;
        }
        let victim = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                let slot = slot.as_ref()?;
                (slot.quiet() && self.dispatch.idle(slot.host?)).then_some((slot.used, index))
            })
            .min();
        if let Some((_, index)) = victim {
            self.remove(index);
        }
    }
    pub fn run(mut self) -> Result<()> {
        let result = self.serve();
        self.slots.clear();
        self.owner.take();
        self.dispatch.invalidate();
        self.runtime
            .shutdown(Instant::now() + Duration::from_secs(2))?;
        result
    }
    fn serve(&mut self) -> Result<()> {
        loop {
            // 热重载先于本轮请求生效：保存完成之后发出的请求一定用上新设置。
            // 新会话随 Open 拿到新设置，已打开的会话等组合结束再换。
            if let Some(options) = self.reload.take()? {
                self.options = options;
                self.dispatch.reconfigure(options);
            }
            // 每次事件只访问固定 16 槽；空闲等待内核事件，不遍历词表、不忙轮询。
            for index in 0..HOST_SLOTS {
                if let Some(slot) = self.slots[index].as_mut() {
                    if slot
                        .tick(
                            &mut self.dispatch,
                            self.generation,
                            self.options,
                            &self.image,
                            &mut self.clock,
                            self.show.as_ref(),
                        )
                        .is_err()
                    {
                        self.remove(index);
                    } else if slot.stopping && matches!(slot.end.phase, Phase::Idle) {
                        // 停止确认写入完成后才关闭全部管道；不是用超时冒充线程退出。
                        return Ok(());
                    }
                } else if let Ok(slot) = Slot::bind(&self.runtime, &self.namespace, index) {
                    // 旧取消回调可能仍持有首实例；完成事件到来后才会再次尝试重建。
                    self.slots[index] = Some(slot);
                }
            }
            self.reclaim();
            let deadline = self
                .slots
                .iter()
                .flatten()
                .filter_map(|slot| slot.end.deadline)
                .min();
            let timeout = deadline.map_or(INFINITE, |end| {
                end.saturating_duration_since(Instant::now())
                    .as_nanos()
                    .div_ceil(1_000_000)
                    .min((u32::MAX - 1) as u128) as u32
            });
            let events = [
                self.runtime.completion_event().as_raw_handle(),
                self.reload.event.as_raw_handle(),
            ];
            // SAFETY: Runtime 与 Reload 持有两个事件；完成、热重载与最近期限共用一次等待，不等待宿主进程句柄。
            if unsafe { WaitForMultipleObjects(2, events.as_ptr(), 0, timeout) } == WAIT_FAILED {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
}

/// 前台线程即固定反应器/引擎线程；查字/设置另有固定 UI 线程与工作线程，三者共用只读词表，
/// 用 stop 命令停止。show 为真时启动即显示查字窗（用户唤起时后台尚未运行）。
pub fn run(package: &Path, directory: &Path, show: bool) -> Result<()> {
    let dictionary = Dictionary::new(import::load(package)?.data)?;
    // 受管目录租约归查字工作线程，在后台运行期间保持，退出后才释放。
    let data = if directory.try_exists()? {
        Data::open(directory)?
    } else {
        Data::create(directory)?
    };
    // 设置文件可能被外部改动；控制器载入时校验，不把偏蓝或不可读的配色发给宿主候选窗。
    let mut controller = Controller::new(&dictionary, data)?;
    let options = controller.options();
    let mut server = Server::new(&dictionary, "backend", options)?;
    let reload = server.reload.clone();
    let (show_sender, show_inbox) = tigertype_ui::mailbox()?;
    if show {
        show_sender.send(())?;
    }
    server.show = Some(show_sender);
    let (requests, receiver) = mpsc::sync_channel(1);
    let (replies, inbox) = tigertype_ui::mailbox()?;
    thread::scope(|scope| {
        let worker = thread::Builder::new()
            .name("tiger-lookup-engine".into())
            .spawn_scoped(scope, move || {
                while let Ok(request) = receiver.recv() {
                    let mut reply = controller.execute(request);
                    let failed = match &reply {
                        UiReply::Saved(Ok(options)) => reload.set(*options).err(),
                        _ => None,
                    };
                    if let Some(error) = failed {
                        reply = UiReply::Saved(Err(format!(
                            "设置已写入，但未能交给输入后台（{error}）；请重新启动后台"
                        )));
                    }
                    if replies.send(reply).is_err() {
                        break;
                    }
                }
            })?;
        let ui = thread::Builder::new()
            .name("tiger-lookup-ui".into())
            .spawn_scoped(scope, move || {
                // 首次唤起前不建窗口；后台停止时唤起邮箱断开，UI 线程随之退出。
                if show_inbox.receive().is_err() {
                    return Ok(());
                }
                tigertype_ui::run(requests, inbox, show_inbox, options)
            })?;
        // run 结束即释放 Server 与唤起邮箱：UI 退出并断开请求通道，工作线程随之结束。
        let result = server.run();
        // 真正 join 后才确认线程与目录租约已释放，不用超时冒充终止。
        let ui = ui.join().map_err(|_| "查字界面线程异常退出")?;
        worker.join().map_err(|_| "查字工作线程异常退出")?;
        result?;
        ui.map_err(Into::into)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;
    use tigertype_contracts::{OutputForm, protocol::Reply};
    use tigertype_engine::{Data as EngineData, Entry};
    use tigertype_ipc::service::Client;

    /// 真实管道与引擎线程在限时子进程内运行：任何一步卡住都由父测试到期终止，不会挂起整轮测试。
    #[test]
    fn show_requests_and_saved_settings_reach_a_running_backend() -> Result<()> {
        let status = crate::process::run(
            ProcessCommand::new(std::env::current_exe()?).args([
                "--ignored",
                "--exact",
                "backend::tests::show_and_reload_worker",
            ]),
            Duration::from_secs(20),
        )?;
        assert!(status.success());
        Ok(())
    }
    #[test]
    #[ignore = "只由限时父测试在子进程中运行，使用合成词表与独立命名空间"]
    fn show_and_reload_worker() -> Result<()> {
        let dictionary = Dictionary::new(EngineData {
            entries: vec![Entry {
                character: '的',
                code: "u".into(),
                weight: 1,
            }],
            common: vec!['的'],
            ..Default::default()
        })?;
        let namespace = format!("reload-test-{}", std::process::id());
        let mut server = Server::new(&dictionary, &namespace, Options::default())?;
        let reload = server.reload.clone();
        let (show, shown) = tigertype_ui::mailbox()?;
        server.show = Some(show);
        let changed = Options {
            output_form: OutputForm::Traditional,
            ..Default::default()
        };
        thread::scope(|scope| {
            let client = scope.spawn(move || {
                let deadline = || Instant::now() + Duration::from_secs(3);
                let runtime = Runtime::new().unwrap();
                let image = std::env::current_exe().unwrap();
                // 管理端唤起：后台核对映像后先把信号投进 UI 邮箱，再回复 Shown。
                Client::connect(&runtime, &image, &namespace, deadline())
                    .unwrap()
                    .show_lookup(deadline())
                    .unwrap();
                assert!(shown.try_receive().is_ok());
                // 保存交接之后发出的 Open 一定拿到新设置，无需重启后台。
                reload.set(changed).unwrap();
                let mut client = Client::connect(&runtime, &image, &namespace, deadline()).unwrap();
                let Reply::Opened(state) = client
                    .request(
                        1,
                        Command::Open {
                            options: Options::default(),
                        },
                        deadline(),
                    )
                    .unwrap()
                else {
                    panic!("需要 Opened 回复");
                };
                assert_eq!(state.options, changed);
                client.stop_backend(deadline()).unwrap();
            });
            server.run()?;
            client.join().map_err(|_| "客户端线程断言失败")?;
            Ok(())
        })
    }
}
