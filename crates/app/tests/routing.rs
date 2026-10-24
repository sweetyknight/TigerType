//! 多宿主序号/焦点隔离与回收判定；合成数据，不注册或向真实应用写字。
use tigertype_app::{Result, dispatch::Dispatcher};
use tigertype_contracts::{
    InputState, Key, Options, OutputForm,
    protocol::{Command, Message, Rejection, Reply, Request, Route},
};
use tigertype_engine::{Data, Dictionary, Entry};
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
    host: u64,
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
    let frame = tigertype_ipc::encode(&Message::Request(request.clone()))?;
    Ok(tigertype_ipc::decode_response(&dispatch.dispatch(host, &frame)?, &request)?.reply)
}
fn focus(epoch: u64, focused: bool) -> Command {
    Command::Focus { epoch, focused }
}
fn key(epoch: u64, key: Key) -> Command {
    Command::Key { epoch, key }
}
fn open(dispatch: &mut Dispatcher<'_>, host: u64) -> Result<()> {
    assert!(matches!(
        exchange(
            dispatch,
            host,
            1,
            1,
            Command::Open {
                options: Options::default()
            }
        )?,
        Reply::Opened(_)
    ));
    exchange(dispatch, host, 2, 1, focus(1, true))?;
    Ok(())
}
fn state(reply: Reply) -> InputState {
    match reply {
        Reply::Key(outcome) => outcome.state,
        Reply::Focused(state) => state,
        other => panic!("需要状态回复：{other:?}"),
    }
}
#[test]
fn multi_host_sequences_and_sessions_are_isolated_and_focus_switch_cancels_old_composition()
-> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    open(&mut dispatch, a)?;
    let composing = state(exchange(
        &mut dispatch,
        a,
        3,
        1,
        key(1, Key::Character('u')),
    )?);
    assert!(composing.composing);
    open(&mut dispatch, b)?;
    assert_eq!(
        exchange(&mut dispatch, a, 4, 1, key(1, Key::Space))?,
        Reply::Rejected(Rejection::NotFocused)
    );
    // 焦点切走已清掉 A 的组合，旧快照选字不能上屏。
    assert_eq!(
        exchange(
            &mut dispatch,
            a,
            5,
            1,
            Command::Select {
                revision: composing.revision,
                index: 0,
                epoch: 1
            }
        )?,
        Reply::Rejected(Rejection::NotFocused)
    );
    let Reply::Key(empty) = exchange(&mut dispatch, b, 3, 1, key(1, Key::Space))? else {
        panic!("需要按键回复");
    };
    assert!(empty.commit.is_none());
    assert_eq!(
        exchange(&mut dispatch, b, 3, 1, Command::Clear)?,
        Reply::Rejected(Rejection::OutOfOrder)
    );
    assert!(
        state(exchange(&mut dispatch, a, 6, 1, focus(2, true))?)
            .composition
            .text
            .is_empty()
    );
    assert_eq!(
        exchange(&mut dispatch, a, 7, 1, focus(1, true))?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    // 旧宿主迟到的失焦不会清掉当前宿主 A 的焦点。
    exchange(&mut dispatch, b, 4, 1, focus(2, false))?;
    assert_eq!(
        exchange(&mut dispatch, a, 8, 1, key(1, Key::Character('u')))?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    assert!(
        state(exchange(
            &mut dispatch,
            a,
            9,
            1,
            key(2, Key::Character('u'))
        )?)
        .composing
    );
    Ok(())
}
#[test]
fn reclaim_queries_protect_focused_or_composing_hosts_and_track_session_capacity() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    open(&mut dispatch, a)?;
    assert!(!dispatch.idle(a));
    open(&mut dispatch, b)?;
    assert!(dispatch.idle(a));
    assert!(!dispatch.idle(b));
    exchange(&mut dispatch, b, 3, 1, key(1, Key::Character('u')))?;
    // 宿主自己失焦也清掉组合，之后才可回收；未知宿主永不作为回收对象。
    assert!(!state(exchange(&mut dispatch, b, 4, 1, focus(2, false))?).composing);
    assert!(dispatch.idle(b));
    assert!(!dispatch.idle(99));
    assert!(!dispatch.full());
    for id in 2..=63 {
        exchange(
            &mut dispatch,
            a,
            id + 1,
            id,
            Command::Open {
                options: Options::default(),
            },
        )?;
    }
    assert!(dispatch.full());
    exchange(&mut dispatch, a, 65, 2, Command::Close)?;
    assert!(!dispatch.full());
    dispatch.remove_host(a);
    assert!(!dispatch.idle(a));
    Ok(())
}
#[test]
fn stale_snapshot_blur_and_disconnect_reject_selection() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let host = dispatch.add_host()?;
    open(&mut dispatch, host)?;
    let old = state(exchange(
        &mut dispatch,
        host,
        3,
        1,
        key(1, Key::Character('u')),
    )?);
    exchange(
        &mut dispatch,
        host,
        4,
        1,
        Command::Configure {
            options: Options::default(),
        },
    )?;
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            5,
            1,
            Command::Select {
                revision: old.revision,
                index: 0,
                epoch: 1
            }
        )?,
        Reply::Rejected(Rejection::StaleSnapshot)
    );
    exchange(&mut dispatch, host, 6, 1, focus(2, false))?;
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            7,
            1,
            Command::Select {
                revision: old.revision + 1,
                index: 0,
                epoch: 2
            }
        )?,
        Reply::Rejected(Rejection::NotFocused)
    );
    dispatch.remove_host(host);
    assert_eq!(
        exchange(&mut dispatch, host, 8, 1, Command::Clear)?,
        Reply::Rejected(Rejection::UnknownHost)
    );
    assert!(dispatch.add_host()? > host);
    Ok(())
}
#[test]
fn global_capacity_old_generations_and_session_reuse_are_rejected() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let host = dispatch.add_host()?;
    for id in 1..=64 {
        assert!(matches!(
            exchange(
                &mut dispatch,
                host,
                id,
                id,
                Command::Open {
                    options: Options::default()
                }
            )?,
            Reply::Opened(_)
        ));
    }
    let other = dispatch.add_host()?;
    assert_eq!(
        exchange(
            &mut dispatch,
            other,
            1,
            1,
            Command::Open {
                options: Options::default()
            }
        )?,
        Reply::Rejected(Rejection::SessionLimit)
    );
    exchange(&mut dispatch, host, 65, 1, Command::Close)?;
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            66,
            1,
            Command::Open {
                options: Options::default()
            }
        )?,
        Reply::Rejected(Rejection::SessionReused)
    );
    for _ in 2..16 {
        dispatch.add_host()?;
    }
    assert!(dispatch.add_host().is_err());
    let old = Request {
        route: Route {
            generation: 6,
            request_id: 100,
            session_id: 2,
        },
        command: Command::Clear,
    };
    let frame = dispatch.dispatch(
        host,
        &tigertype_ipc::encode(&Message::Request(old.clone()))?,
    )?;
    assert_eq!(
        tigertype_ipc::decode_response(&frame, &old)?.reply,
        Reply::Rejected(Rejection::StaleGeneration)
    );
    dispatch.invalidate();
    assert_eq!(
        exchange(&mut dispatch, host, 101, 2, Command::Clear)?,
        Reply::Rejected(Rejection::Unavailable)
    );
    Ok(())
}
#[test]
fn reloaded_settings_wait_until_the_composition_ends() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let host = dispatch.add_host()?;
    open(&mut dispatch, host)?;
    assert!(
        state(exchange(
            &mut dispatch,
            host,
            3,
            1,
            key(1, Key::Character('u'))
        )?)
        .composing
    );
    let changed = Options {
        output_form: OutputForm::Traditional,
        auto_commit_four: true,
        ..Default::default()
    };
    dispatch.reconfigure(changed);
    // 组合中保持旧设置：宿主已显示的候选与编号不能被热重载改掉。
    let Reply::Key(committed) = exchange(&mut dispatch, host, 4, 1, key(1, Key::Space))? else {
        panic!("需要按键回复");
    };
    assert_eq!(committed.commit.as_deref(), Some("的"));
    // 组合在本键结束即换上新设置，回复随之把新外观带给候选窗。
    assert_eq!(committed.state.options, changed);
    let next = state(exchange(
        &mut dispatch,
        host,
        5,
        1,
        key(1, Key::Character('u')),
    )?);
    assert!(next.composing);
    assert_eq!(next.options, changed);
    // 重新取得焦点的回复也携带已换上的设置。
    assert_eq!(
        state(exchange(&mut dispatch, host, 6, 1, focus(2, true))?).options,
        changed
    );
    Ok(())
}
