//! 真实协议与纯 Rust 核心联测；不借助旧 DLL 的历史结果。
//! 使用 routing 文件名：x86 的 dispatch 测试名触发了启动提升要求，同字节改名后可正常运行。
use tigerrime_app::{Result, dispatch::Dispatcher};
use tigerrime_contracts::{
    Key, Options,
    protocol::{Command, Message, Rejection, Reply, Request, Route},
};
use tigerrime_engine::{Data, Dictionary, Entry};

fn dictionary() -> Dictionary {
    Dictionary::new(Data {
        entries: vec![Entry {
            character: '的',
            code: "u".into(),
            weight: 1,
        }],
        common: vec!['的'],
        ..Default::default()
    })
    .unwrap()
}
fn exchange(
    dispatch: &mut Dispatcher<'_>,
    sequence: u64,
    session: u64,
    command: Command,
) -> Result<Reply> {
    let request = Request {
        route: Route {
            generation: 7,
            request_id: sequence,
            session_id: session,
        },
        command,
    };
    let frame = tigerrime_ipc::encode(&Message::Request(request.clone()))?;
    Ok(tigerrime_ipc::decode_response(&dispatch.dispatch(&frame)?, &request)?.reply)
}
#[test]
fn sessions_commits_replays_and_stale_selection_are_isolated() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    exchange(
        &mut dispatch,
        1,
        1,
        Command::Open {
            options: Options::default(),
        },
    )?;
    exchange(
        &mut dispatch,
        2,
        2,
        Command::Open {
            options: Options::default(),
        },
    )?;
    let Reply::Key(first) = exchange(
        &mut dispatch,
        3,
        1,
        Command::Key {
            key: Key::Character('u'),
        },
    )?
    else {
        panic!("需要按键回复")
    };
    let Reply::Key(empty) = exchange(&mut dispatch, 4, 2, Command::Key { key: Key::Space })? else {
        panic!("需要按键回复")
    };
    assert!(empty.commit.is_none());
    exchange(
        &mut dispatch,
        5,
        1,
        Command::Configure {
            options: Options::default(),
        },
    )?;
    assert_eq!(
        exchange(
            &mut dispatch,
            6,
            1,
            Command::Select {
                revision: first.state.revision,
                index: 0
            }
        )?,
        Reply::Rejected(Rejection::StaleSnapshot)
    );
    let Reply::Key(commit) = exchange(&mut dispatch, 7, 1, Command::Key { key: Key::Space })?
    else {
        panic!("需要按键回复")
    };
    assert_eq!(commit.commit.as_deref(), Some("的"));
    assert_eq!(
        exchange(&mut dispatch, 7, 1, Command::Key { key: Key::Space })?,
        Reply::Rejected(Rejection::OutOfOrder)
    );
    exchange(&mut dispatch, 8, 1, Command::Close)?;
    assert_eq!(
        exchange(
            &mut dispatch,
            9,
            1,
            Command::Open {
                options: Options::default()
            }
        )?,
        Reply::Rejected(Rejection::SessionReused)
    );
    dispatch.invalidate();
    assert_eq!(
        exchange(&mut dispatch, 10, 2, Command::Clear)?,
        Reply::Rejected(Rejection::Unavailable)
    );
    Ok(())
}
#[test]
fn admission_caps_sessions_and_rejects_old_generation() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    for id in 1..=64 {
        assert!(matches!(
            exchange(
                &mut dispatch,
                id,
                id,
                Command::Open {
                    options: Options::default()
                }
            )?,
            Reply::Opened(_)
        ));
    }
    assert_eq!(
        exchange(
            &mut dispatch,
            65,
            65,
            Command::Open {
                options: Options::default()
            }
        )?,
        Reply::Rejected(Rejection::SessionLimit)
    );
    let old = Request {
        route: Route {
            generation: 6,
            request_id: 100,
            session_id: 1,
        },
        command: Command::Clear,
    };
    let result = dispatch.dispatch(&tigerrime_ipc::encode(&Message::Request(old.clone()))?)?;
    assert_eq!(
        tigerrime_ipc::decode_response(&result, &old)?.reply,
        Reply::Rejected(Rejection::StaleGeneration)
    );
    Ok(())
}
