//! 每登录会话一后台：固定反应器/引擎线程，16 个预建请求管道，不按连接创建线程。
//! 宿主只凭 DACL 与同会话接纳，不打开宿主进程；断开由常驻读失败发现，始终保留至少一个空闲监听槽。
use crate::{Result, data::Data, dispatch::Dispatcher, import};
use std::{
    os::windows::io::AsRawHandle,
    path::Path,
    time::{Duration, Instant},
};
use tigertype_contracts::{
    Options,
    protocol::{Command, Message},
};
use tigertype_engine::Dictionary;
use tigertype_ipc::{
    FrameAssembler,
    pipe::{Listener, Pending, Pipe, Runtime, grant_app_container_query},
    service::HOST_SLOTS,
};
use windows_sys::Win32::{
    Foundation::WAIT_FAILED,
    Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom},
    System::Threading::{INFINITE, SetEvent, WaitForSingleObject},
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
            // SAFETY: Runtime 持有完成事件；完成与最近期限共用一次等待，不等待宿主进程句柄。
            if unsafe {
                WaitForSingleObject(self.runtime.completion_event().as_raw_handle(), timeout)
            } == WAIT_FAILED
            {
                return Err(std::io::Error::last_os_error().into());
            }
        }
    }
}

/// 前台线程即固定反应器/引擎线程；不显示后台候选窗，用 stop 命令停止。
pub fn run(package: &Path, directory: &Path) -> Result<()> {
    let dictionary = Dictionary::new(import::load(package)?.data)?;
    // 受管目录租约在后台运行期间保持，退出后才释放。
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
    let result = Server::new(&dictionary, "backend", options)?.run();
    drop(data);
    result
}
