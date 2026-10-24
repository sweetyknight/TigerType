//! 每登录会话一后台：固定反应器/引擎线程，16 个预建请求管道，不按连接创建线程。
//! 宿主只凭 DACL 与同会话接纳，不打开宿主进程；断开由常驻读失败发现，始终保留至少一个空闲监听槽。
//! 查字/设置窗在同进程的固定 UI 线程，保存后的设置经交接槽热重载到引擎线程，输入路径不读盘。
//! 持有输入焦点的宿主线程会报告任务栏正显示本输入法图标，此时经另一交接槽让 UI 线程隐藏通知区图标。
use crate::{Result, data::Data, dispatch::Dispatcher, import, lookup::Controller};
use std::{
    os::windows::io::AsRawHandle,
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use tigertype_contracts::{
    Options,
    protocol::{Command, Message, Panel},
    ui::Reply as UiReply,
};
use tigertype_engine::Dictionary;
use tigertype_ipc::{
    FrameAssembler,
    pipe::{Listener, Pending, Pipe, Runtime, grant_app_container_query},
    service::HOST_SLOTS,
};
use tigertype_ui::{Latest, ReplySender};
use windows_sys::Win32::{
    Foundation::WAIT_FAILED,
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Threading::{INFINITE, SetEvent, WaitForMultipleObjects},
};

/// 交给后台 UI 线程的信号：唤起查字/设置窗，与通知区图标显隐。正式后台才有 UI 线程，测试可接入核对。
/// 任务栏输入法图标的持有者是最近报告取得输入焦点的宿主：有持有者时任务栏已显示本输入法图标，
/// 通知区图标隐藏，同一时刻只显示一个；旧版宿主从不报告，通知区图标照常可用。
#[derive(Default)]
struct Ui {
    // 随 Server 释放而断开，UI 线程据此退出。
    show: Option<ReplySender<Panel>>,
    holder: Option<u64>,
    hidden: bool,
    taskbar: Option<Arc<Latest<bool>>>,
}
impl Ui {
    /// 后到的取得覆盖先前持有者；失去只清除自己，与他人取得乱序到达时不会误显通知区图标。
    fn report(&mut self, host: u64, visible: bool) {
        if visible {
            self.holder = Some(host);
        } else if self.holder == Some(host) {
            self.holder = None;
        }
        self.publish();
    }
    /// 显隐只影响图标：交接失败不中断输入服务，下次事件再同步。
    fn publish(&mut self) {
        let hidden = self.holder.is_some();
        if hidden != self.hidden
            && let Some(taskbar) = &self.taskbar
            && taskbar.set(hidden).is_ok()
        {
            self.hidden = hidden;
        }
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
        ui: &mut Ui,
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
            Message::Show(panel) if self.host.is_some() => {
                // 宿主图标菜单与管理端都可唤起：只投递信号、不改数据，因此不核对映像；窗口由 UI 线程创建。
                ui.show.as_ref().ok_or("此后台未提供查字窗")?.send(panel)?;
                tigertype_ipc::encode(&Message::Shown)?
            }
            Message::Indicator(visible) if self.host.is_some() => {
                // 先交给 UI 线程再确认，宿主收到确认时图标显隐已经提交。
                ui.report(self.host.ok_or("握手前收到图标报告")?, visible);
                tigertype_ipc::encode(&Message::Noted)?
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
    reload: Arc<Latest<Options>>,
    ui: Ui,
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
            reload: Arc::new(Latest::new()?),
            ui: Ui::default(),
        })
    }
    fn remove(&mut self, index: usize) {
        if let Some(slot) = self.slots[index].take() {
            if let Some(host) = slot.host {
                self.dispatch.remove_host(host);
                // 宿主停用或断开等同失去输入焦点。
                self.ui.report(host, false);
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
    /// 最后一个空槽被占用或会话达上限时，回收最久未用、非焦点、无组合且无未读字节的宿主；
    /// 持有任务栏图标的宿主不回收，否则通知区图标会与任务栏图标同时出现。
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
                let host = slot.host?;
                (slot.quiet() && self.dispatch.idle(host) && self.ui.holder != Some(host))
                    .then_some((slot.used, index))
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
                            &mut self.ui,
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
            // 补交此前交接失败的图标显隐。
            self.ui.publish();
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
                self.reload.event().as_raw_handle(),
            ];
            // SAFETY: Runtime 与热重载交接槽持有两个事件；完成、热重载与最近期限共用一次等待，不等待宿主进程句柄。
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
    let settings_path = controller.settings_path();
    let options = controller.options();
    let mut server = Server::new(&dictionary, "backend", options)?;
    let reload = server.reload.clone();
    let (show_sender, show_inbox) = tigertype_ui::mailbox()?;
    if show {
        show_sender.send(Panel::Lookup)?;
    }
    server.ui.show = Some(show_sender);
    let taskbar = Arc::new(Latest::new()?);
    server.ui.taskbar = Some(taskbar.clone());
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
                // 启动即挂通知区图标，查字/设置窗首次唤起才建；后台停止时唤起邮箱断开，UI 线程随之退出。
                tigertype_ui::run(
                    requests,
                    inbox,
                    show_inbox,
                    &taskbar,
                    &settings_path,
                    options,
                )
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
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

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
        server.ui.show = Some(show);
        let taskbar = Arc::new(Latest::new()?);
        server.ui.taskbar = Some(taskbar.clone());
        let changed = Options {
            output_form: OutputForm::Traditional,
            ..Default::default()
        };
        thread::scope(|scope| {
            let client = scope.spawn(move || {
                let deadline = || Instant::now() + Duration::from_secs(3);
                let runtime = Runtime::new().unwrap();
                let image = std::env::current_exe().unwrap();
                // 宿主图标菜单唤起设置页：后台先把目标窗口投进 UI 邮箱再回复 Shown，连接继续用于输入。
                let mut client = Client::connect(&runtime, &image, &namespace, deadline()).unwrap();
                client.show(Panel::Settings, deadline()).unwrap();
                assert_eq!(shown.try_receive(), Ok(Panel::Settings));
                // 宿主线程取得输入焦点：确认前已让 UI 线程隐藏通知区图标。
                client.indicate(true, deadline()).unwrap();
                assert_eq!(taskbar.take(), Ok(Some(true)));
                // 另一宿主线程取得后，先前持有者迟到的“失去”不会让图标重现。
                let mut other = Client::connect(&runtime, &image, &namespace, deadline()).unwrap();
                other.indicate(true, deadline()).unwrap();
                client.indicate(false, deadline()).unwrap();
                assert_eq!(taskbar.take(), Ok(None));
                // 持有者断开等同失去：通知区图标恢复。事件可能残留旧信号，按值判断，最多等三次。
                drop(other);
                let restored = (0..3).any(|_| {
                    // SAFETY: 交接槽在本闭包期间有效，等待有明确上限。
                    unsafe { WaitForSingleObject(taskbar.event().as_raw_handle(), 3000) };
                    taskbar.take() == Ok(Some(false))
                });
                assert!(restored);
                // 保存交接之后发出的 Open 一定拿到新设置，无需重启后台。
                reload.set(changed).unwrap();
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
