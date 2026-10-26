//! 固定槽位发现与宿主客户端；宿主在首个需要引擎的编辑会话内懒连接，不在输入回调启动后台。
use crate::{
    ClientState, Error, Message, Result, decode, encode,
    pipe::{Pipe, Runtime},
};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};
use tigertype_contracts::protocol::{Command, Panel, Reply, Request, Route};
pub const HOST_SLOTS: usize = 16;

pub struct Client {
    pipe: Pipe,
    state: ClientState,
    pub generation: u64,
    pub host: u64,
    sequence: u64,
    closed: bool,
}
impl Client {
    /// 仅管理工具可请求停机；服务端会打开调用进程核对映像，不把登录 SID 当作停机授权。
    /// 确认后在同一期限内等后台进程真正退出：后台确认后还要关管道、收线程，
    /// 安装程序随即检测文件占用并启动新后台，不能把确认当作已退出（后台在本进程时确认即返回）。
    pub fn stop_backend(&mut self, deadline: Instant) -> Result<()> {
        let process = self.pipe.server_process().inspect_err(|_| self.cancel())?;
        self.manage(Message::Stop, Message::Stopped, deadline)?;
        match process {
            Some(process) => crate::pipe::wait_exit(&process, deadline),
            None => Ok(()),
        }
    }
    /// 请求后台显示查字或设置窗：一问一答，不占用会话序号；成功后连接照常用于输入，失败即作废。
    /// 调用方应先放行前台切换，否则窗口可能停在后面。
    pub fn show(&mut self, panel: Panel, deadline: Instant) -> Result<()> {
        self.signal(Message::Show(panel), Message::Shown, deadline)
    }
    /// 报告本宿主线程是否持有输入焦点（任务栏正显示本输入法图标），后台据此隐藏或恢复通知区图标。
    pub fn indicate(&mut self, visible: bool, deadline: Instant) -> Result<()> {
        self.signal(Message::Indicator(visible), Message::Noted, deadline)
    }
    /// 与会话无关的一问一答：成功后连接照常用于输入，失败即作废。
    fn signal(&mut self, request: Message, expected: Message, deadline: Instant) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = (|| {
            self.pipe.send_frame(&encode(&request)?, deadline)?;
            if decode(&self.pipe.receive_frame(deadline)?)? != expected {
                return Err(Error::Invalid("信号回复不符"));
            }
            Ok(())
        })();
        if result.is_err() {
            self.cancel();
        }
        result
    }
    /// 管理请求一问一答后即作废连接，不与输入会话混用。
    fn manage(&mut self, request: Message, expected: Message, deadline: Instant) -> Result<()> {
        if self.closed {
            return Err(Error::Closed);
        }
        let result = (|| {
            self.pipe.send_frame(&encode(&request)?, deadline)?;
            if decode(&self.pipe.receive_frame(deadline)?)? != expected {
                return Err(Error::Invalid("管理请求回复不符"));
            }
            Ok(())
        })();
        self.cancel();
        result
    }
    pub fn connect(
        runtime: &Runtime,
        expected_image: &Path,
        namespace: &str,
        deadline: Instant,
    ) -> Result<Self> {
        for slot in 0..HOST_SLOTS {
            let pipe = runtime.connect(
                &format!("{namespace}-request-{slot}"),
                expected_image,
                deadline,
            );
            let mut pipe = match pipe {
                Ok(pipe) => pipe,
                // 未创建或已被占用的槽位立即失败，继续尝试下一个；不等待实例空闲。
                Err(Error::Io(error)) if matches!(error.raw_os_error(), Some(2 | 231)) => continue,
                Err(error) => return Err(error),
            };
            pipe.send_frame(&encode(&Message::Hello)?, deadline)?;
            let Message::Welcome { generation, host } = decode(&pipe.receive_frame(deadline)?)?
            else {
                return Err(Error::Invalid("缺少后台握手"));
            };
            return Ok(Self {
                pipe,
                state: ClientState::new(generation)?,
                generation,
                host,
                sequence: 0,
                closed: false,
            });
        }
        Err(Error::Busy)
    }
    /// 发送前的非阻塞核对：已断开或出现未请求的字节都使连接失效，绝不读取或解释那些字节。
    pub fn usable(&mut self) -> bool {
        if !self.closed && matches!(self.pipe.available(), Ok(0)) {
            return true;
        }
        self.cancel();
        false
    }
    pub fn request(&mut self, session: u64, command: Command, deadline: Instant) -> Result<Reply> {
        if self.closed {
            return Err(Error::Closed);
        }
        self.sequence = self.sequence.checked_add(1).ok_or(Error::Closed)?;
        let request = Request {
            route: Route {
                generation: self.generation,
                request_id: self.sequence,
                session_id: session,
            },
            command,
        };
        match self.pipe.transact(&mut self.state, request, deadline) {
            Ok(response) => Ok(response.reply),
            Err(error) => {
                // 已写出的请求结果不确定：连接作废，不在本层重连或重放。
                self.cancel();
                Err(error)
            }
        }
    }
    pub fn cancel(&mut self) {
        self.closed = true;
        self.state.cancel();
        self.pipe.cancellation().cancel();
    }
}

/// 宿主线程独占的懒连接；只有发送前确认连接不可用时才在同一期限内重连一次。
pub struct Link {
    image: PathBuf,
    namespace: String,
    client: Option<Client>,
}
impl Link {
    pub fn new(image: PathBuf, namespace: &str) -> Self {
        Self {
            image,
            namespace: namespace.into(),
            client: None,
        }
    }
    /// 返回 true 表示刚建立连接：宿主编号、请求序号与会话均为新值，调用方须先 Open/Focus 再发按键。
    /// 连接失败直接返回错误，由调用方放行原键；不在此循环重试。
    pub fn ensure(&mut self, runtime: &Runtime, deadline: Instant) -> Result<(&mut Client, bool)> {
        let fresh = !self.client.as_mut().is_some_and(Client::usable);
        if fresh {
            self.client = None;
            self.client = Some(Client::connect(
                runtime,
                &self.image,
                &self.namespace,
                deadline,
            )?);
        }
        Ok((self.client.as_mut().unwrap(), fresh))
    }
    /// 同一次 ensure 之后、同一期限内的后续请求使用；不做可用性检查，也不在此重连。
    pub fn client(&mut self) -> Option<&mut Client> {
        self.client.as_mut()
    }
    /// 宿主停用时立即释放管道，后台随即发现断开；不依赖其余引用何时释放。
    pub fn close(&mut self) {
        self.client = None;
    }
}
