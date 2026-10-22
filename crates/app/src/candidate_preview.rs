//! 进程内候选联调入口：真实引擎/路由/窗口/点击选字闭环，只向控制台打印最终选择，不向宿主上屏。
//! 宿主内候选窗（B 批）落地前保留；候选快照取自按键回复，位置固定，不经过跨进程协议。
use crate::{Result, dispatch::Dispatcher, import};
use std::path::Path;
use tigertype_contracts::{
    Key, Options,
    candidate::{CandidateChoice, CandidateFrame, CandidateToken, ScreenRect},
    protocol::{Command, Message, Reply, Request, Route},
};
use tigertype_engine::Dictionary;

fn exchange(
    dispatch: &mut Dispatcher<'_>,
    host: u64,
    sequence: &mut u64,
    command: Command,
) -> Result<Reply> {
    *sequence = sequence.checked_add(1).ok_or("预览序号耗尽")?;
    let request = Request {
        route: Route {
            generation: 1,
            request_id: *sequence,
            session_id: 1,
        },
        command,
    };
    let response = dispatch.dispatch(
        host,
        &tigertype_ipc::encode(&Message::Request(request.clone()))?,
    )?;
    Ok(tigertype_ipc::decode_response(&response, &request)?.reply)
}
pub fn run(package: &Path, code: &str) -> Result<()> {
    if code.is_empty() || code.len() > 4 || !code.bytes().all(|c| c.is_ascii_lowercase()) {
        return Err("候选预览只接受 1–4 位小写虎码".into());
    }
    let dictionary = Dictionary::new(import::load(package)?.data)?;
    let code = code.to_owned();
    let (frames, inbox) = tigertype_ui::candidate::mailbox()?;
    let (choices, received) = tigertype_ui::mailbox::<CandidateChoice>()?;
    let worker = std::thread::Builder::new()
        .name("tiger-candidate-preview".into())
        .spawn(move || -> std::result::Result<(), String> {
            let work = || -> Result<()> {
                let mut dispatch = Dispatcher::new(&dictionary, 1)?;
                let host = dispatch.add_host()?;
                let mut sequence = 0;
                exchange(
                    &mut dispatch,
                    host,
                    &mut sequence,
                    Command::Open {
                        options: Options::default(),
                    },
                )?;
                exchange(
                    &mut dispatch,
                    host,
                    &mut sequence,
                    Command::Focus {
                        epoch: 1,
                        focused: true,
                    },
                )?;
                let mut state = None;
                for key in code.chars() {
                    let Reply::Key(outcome) = exchange(
                        &mut dispatch,
                        host,
                        &mut sequence,
                        Command::Key {
                            epoch: 1,
                            key: Key::Character(key),
                        },
                    )?
                    else {
                        return Err("预览按键被拒绝".into());
                    };
                    state = Some(outcome.state);
                }
                let state = state.ok_or("预览没有按键结果")?;
                if state.candidates.is_empty() {
                    return Err("此编码没有候选，未打开窗口".into());
                }
                let frame = CandidateFrame {
                    token: CandidateToken {
                        host,
                        generation: 1,
                        session: 1,
                        focus: 1,
                        revision: state.revision,
                    },
                    caret: ScreenRect {
                        left: 200,
                        top: 200,
                        right: 201,
                        bottom: 224,
                    },
                    state,
                };
                frames.send(Some(frame.clone()))?;
                if let Ok(choice) = received.receive() {
                    if choice.token != frame.token {
                        return Err("点击快照与预览不符".into());
                    }
                    // 选字绑定快照编号，旧点击由引擎拒绝而不是按当前页重解释。
                    let Reply::Key(outcome) = exchange(
                        &mut dispatch,
                        host,
                        &mut sequence,
                        Command::Select {
                            revision: choice.token.revision,
                            index: choice.index,
                            epoch: 1,
                        },
                    )?
                    else {
                        return Err("选择被拒绝".into());
                    };
                    println!(
                        "候选预览选择：{}（未向任何应用上屏）",
                        outcome.commit.ok_or("选择没有产生结果")?
                    );
                }
                dispatch.remove_host(host);
                Ok(())
            };
            work().map_err(|e| e.to_string())
        })?;
    let result = tigertype_ui::candidate::run(inbox, choices, "关闭预览");
    // 关闭候选窗先断开选择发送端，再等待固定工作线程真正退出。
    worker.join().map_err(|_| "候选预览线程异常退出")??;
    result.map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_preview_codes_fail_before_any_package_read_or_window_creation() {
        // 故意不提供数据包，确保非法输入先失败且测试不创建可见窗口。
        for code in ["", "A", "123", "abcde", "汉字"] {
            assert!(
                run(Path::new("unused-package"), code)
                    .unwrap_err()
                    .to_string()
                    .contains("1–4 位小写虎码")
            );
        }
    }
}
