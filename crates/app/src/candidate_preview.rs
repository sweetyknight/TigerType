//! 可见候选联调入口：真实引擎/路由/窗口/点击领取闭环，只向控制台打印最终选择，不向宿主上屏。
use crate::{Result, dispatch::Dispatcher, import};
use std::path::Path;
use tigertype_contracts::{
    Key, Options,
    candidate::ScreenRect,
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
    let (choices, received) = tigertype_ui::mailbox()?;
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
                        caret: Some(ScreenRect {
                            left: 200,
                            top: 200,
                            right: 201,
                            bottom: 224,
                        }),
                    },
                )?;
                for key in code.chars() {
                    exchange(
                        &mut dispatch,
                        host,
                        &mut sequence,
                        Command::Key {
                            epoch: 1,
                            key: Key::Character(key),
                        },
                    )?;
                }
                let frame = dispatch.frame().ok_or("预览焦点丢失")?;
                if frame.state.candidates.is_empty() {
                    return Err("此编码没有候选，未打开窗口".into());
                }
                frames.send(Some(frame))?;
                if let Ok(choice) = received.receive() {
                    if dispatch
                        .offer_selection(choice)
                        .map_err(|e| format!("点击被拒绝：{e:?}"))?
                        != host
                    {
                        return Err("预览宿主不符".into());
                    }
                    let reply = exchange(
                        &mut dispatch,
                        host,
                        &mut sequence,
                        Command::TakeSelection {
                            epoch: choice.token.focus,
                        },
                    )?;
                    let Reply::Key(outcome) = reply else {
                        return Err("选择未被原宿主领取".into());
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
