//! 受控合成目录的真实引擎管道模式；尚不是常驻管理进程或重启监督器。

use crate::{
    Result,
    dispatch::Dispatcher,
    job::{self, Bootstrap, JobChild},
    probe,
};
use serde_json::json;
use std::{
    ffi::OsString,
    fs::OpenOptions,
    os::windows::io::AsHandle,
    path::Path,
    time::{Duration, Instant},
};
use tigerrime_contracts::protocol::{Command, Message, Reply, Request, Response, Route};
use tigerrime_engine::Engine;
use tigerrime_ipc::{
    ClientState,
    pipe::{Peer, Pipe, Runtime},
};
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    None,
    StallAfterCommit,
    ExitAfterCommit,
}
impl Fault {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "none" => Ok(Self::None),
            "stall-after-commit" => Ok(Self::StallAfterCommit),
            "exit-after-commit" => Ok(Self::ExitAfterCommit),
            _ => Err("未知的探针故障模式".into()),
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::StallAfterCommit => "stall-after-commit",
            Self::ExitAfterCommit => "exit-after-commit",
        }
    }
}

pub struct EngineProcess {
    pipe: Option<Pipe>,
    client: ClientState,
    child: JobChild,
    generation: u64,
    sequence: u64,
}

impl EngineProcess {
    /// 必须在输入路径之外调用；仅接受已经部署的隔离合成工作区。
    pub fn start(
        runtime: &Runtime,
        binary: &Path,
        root: &Path,
        work: &Path,
        generation: u64,
        fault: Fault,
    ) -> Result<Self> {
        let client = ClientState::new(generation)?;
        let paths = probe::runtime_paths(root, work)?;
        let work = work.canonicalize()?;
        let label = format!("engine-{}-{generation}", std::process::id());
        let listener = runtime.bind(&label)?;
        let args = [
            OsString::from("probe-engine-pipe"),
            work.as_os_str().to_owned(),
            label.clone().into(),
            generation.to_string().into(),
            fault.name().into(),
        ];
        // 启动单独给 30 秒预算；不把加载 DLL/配置算作已经满足输入回调 50ms。
        let deadline = Instant::now() + Duration::from_secs(30);
        let child = JobChild::spawn(
            binary,
            &args,
            &paths.user,
            &work.join(format!("pipe-{generation}.stdout.log")),
            &work.join(format!("pipe-{generation}.stderr.log")),
        )?;
        let pipe = listener.accept(Peer::from_process(child.as_handle())?, deadline)?;
        child.wait_signal(deadline)?;
        Ok(Self {
            pipe: Some(pipe),
            client,
            child,
            generation,
            sequence: 0,
        })
    }

    pub fn request(
        &mut self,
        session: u64,
        command: Command,
        deadline: Instant,
    ) -> Result<Response> {
        self.sequence = self.sequence.checked_add(1).ok_or("请求序号耗尽")?;
        let request = Request {
            route: Route {
                generation: self.generation,
                request_id: self.sequence,
                session_id: session,
            },
            command,
        };
        // 错误立即返回，由输入路径外的调用方决定终止；不在这里同步回收或重放。
        Ok(self.pipe.as_mut().ok_or("引擎通道已关闭")?.transact(
            &mut self.client,
            request,
            deadline,
        )?)
    }

    pub fn finish(mut self, deadline: Instant) -> Result<u32> {
        drop(self.pipe.take());
        self.child.wait(deadline)
    }

    pub fn wait_fault_point(&self, deadline: Instant) -> Result<()> {
        self.child.wait_signal(deadline)
    }

    pub fn wait_exit(&self, deadline: Instant) -> Result<u32> {
        self.child.wait(deadline)
    }

    pub fn terminate(mut self, deadline: Instant) -> Result<u32> {
        self.client.cancel();
        drop(self.pipe.take());
        self.child.terminate()?;
        self.child.wait(deadline)
    }
}

fn disconnected(error: &tigerrime_ipc::Error) -> bool {
    matches!(error, tigerrime_ipc::Error::Io(error) if error.kind() == std::io::ErrorKind::UnexpectedEof
        || matches!(error.raw_os_error().map(|code| code as u32), Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA | ERROR_PIPE_NOT_CONNECTED)))
}

/// 内部模式只在 JobChild 显式继承的父子关系中运行；不注册输入法、不访问正式词典。
/// # Safety
/// 仅在子进程入口调用一次；parent/event 的有效值必须是尚未被 Rust 接管的继承句柄。
pub unsafe fn run(
    root: &Path,
    work: &Path,
    label: &str,
    generation: u64,
    fault: Fault,
    parent: usize,
    event: usize,
) -> Result<()> {
    // SAFETY: 入口保证唯一接管，Bootstrap 校验原始句柄的继承标志和类型。
    let Bootstrap { parent, event } = unsafe { Bootstrap::take(parent, event)? };
    let paths = probe::runtime_paths(root, work)?;
    let report = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(work.join(format!("pipe-{generation}.json")))?;
    let runtime = Runtime::new()?;
    let mut pipe = runtime.connect(label, parent, Instant::now() + Duration::from_secs(30))?;
    // 诊断工作模式有总寿命；正式常驻循环与监督策略仍待 app 组装。
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut processed = 0u64;
    let result: Result<()> = (|| {
        let engine = Engine::start(&root.join("vendor/librime/dist/lib/rime.dll"), &paths)?;
        let mut dispatcher = Dispatcher::new(&engine, generation)?;
        job::signal(&event)?;
        loop {
            let frame = match pipe.receive_frame(deadline) {
                Ok(frame) => frame,
                Err(error) if disconnected(&error) => break,
                Err(error) => return Err(error.into()),
            };
            let response = dispatcher.dispatch(&frame)?;
            processed += 1;
            if fault != Fault::None
                && matches!(tigerrime_ipc::decode(&response)?,
                Message::Response(Response { reply: Reply::Key(ref outcome), .. }) if outcome.commit.is_some())
            {
                // 只有真实引擎已经产生提交才通知父测试；不以“碰巧超时”冒充命中故障点。
                job::signal(&event)?;
                match fault {
                    Fault::StallAfterCommit => std::thread::sleep(Duration::from_secs(60)),
                    Fault::ExitAfterCommit => std::process::exit(23),
                    Fault::None => unreachable!(),
                }
            }
            pipe.send_frame(&response, deadline)?;
        }
        // dispatcher 先销毁全部 Session，再由 Engine guard finalize；断连不继续接受新通道。
        Ok(())
    })();
    let errors = probe::diagnostics(&paths.logs)?;
    serde_json::to_writer_pretty(
        report,
        &json!({
            "generation": generation, "requests_processed": processed, "fault": fault.name(),
            "user_data_dir": paths.user, "native_errors": errors,
            "error": result.as_ref().err().map(ToString::to_string),
            "scope": "parent -> named pipe -> isolated librime worker; no TSF/UI"
        }),
    )?;
    result?;
    if !errors.is_empty() {
        return Err("管道引擎存在原生错误，不能判定为通过".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_real_disconnects_are_normal_shutdowns() {
        for code in [ERROR_BROKEN_PIPE, ERROR_NO_DATA, ERROR_PIPE_NOT_CONNECTED] {
            assert!(disconnected(
                &std::io::Error::from_raw_os_error(code as i32).into()
            ));
        }
        assert!(!disconnected(&tigerrime_ipc::Error::TimedOut));
        assert!(!disconnected(&std::io::Error::from_raw_os_error(5).into()));
        assert!(Fault::parse("stall").is_err());
    }
}
