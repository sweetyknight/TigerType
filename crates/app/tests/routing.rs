//! 多宿主序号/焦点隔离和候选回送联测；合成数据，不注册或向真实应用写字。
use tigertype_app::{Result, dispatch::Dispatcher};
use tigertype_contracts::{
    Key, Options,
    candidate::{CandidateChoice, ScreenRect},
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
fn focus(epoch: u64) -> Command {
    Command::Focus {
        epoch,
        caret: Some(ScreenRect {
            left: -10,
            top: 20,
            right: -9,
            bottom: 40,
        }),
    }
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
    exchange(dispatch, host, 2, 1, focus(1))?;
    Ok(())
}
#[test]
fn multi_host_sequences_and_sessions_are_isolated_and_focus_switch_cancels_old_composition()
-> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    open(&mut dispatch, a)?;
    exchange(
        &mut dispatch,
        a,
        3,
        1,
        Command::Key {
            epoch: 1,
            key: Key::Character('u'),
        },
    )?;
    let old = dispatch.frame().unwrap();
    open(&mut dispatch, b)?;
    assert_eq!(
        exchange(
            &mut dispatch,
            a,
            4,
            1,
            Command::Key {
                epoch: 1,
                key: Key::Space
            }
        )?,
        Reply::Rejected(Rejection::NotFocused)
    );
    assert_eq!(
        dispatch.offer_selection(CandidateChoice {
            token: old.token,
            index: 0
        }),
        Err(Rejection::StaleSnapshot)
    );
    let Reply::Key(empty) = exchange(
        &mut dispatch,
        b,
        3,
        1,
        Command::Key {
            epoch: 1,
            key: Key::Space,
        },
    )?
    else {
        panic!("需要按键回复");
    };
    assert!(empty.commit.is_none());
    assert_eq!(
        exchange(&mut dispatch, b, 3, 1, Command::Clear)?,
        Reply::Rejected(Rejection::OutOfOrder)
    );
    exchange(&mut dispatch, a, 5, 1, focus(2))?;
    assert!(dispatch.frame().unwrap().state.composition.text.is_empty());
    assert_eq!(
        exchange(&mut dispatch, a, 6, 1, focus(1))?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    // 旧宿主的失焦通知不会清掉当前宿主 A 的焦点。
    exchange(
        &mut dispatch,
        b,
        4,
        1,
        Command::Focus {
            epoch: 2,
            caret: None,
        },
    )?;
    assert_eq!(dispatch.frame().unwrap().token.host, a);
    assert_eq!(
        exchange(
            &mut dispatch,
            a,
            7,
            1,
            Command::Key {
                epoch: 1,
                key: Key::Character('u')
            }
        )?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    assert!(dispatch.frame().unwrap().state.composition.text.is_empty());
    Ok(())
}
#[test]
fn candidate_view_click_is_delivered_to_original_host_once_and_only_on_explicit_claim() -> Result<()>
{
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let host = dispatch.add_host()?;
    open(&mut dispatch, host)?;
    exchange(
        &mut dispatch,
        host,
        3,
        1,
        Command::Key {
            epoch: 1,
            key: Key::Character('u'),
        },
    )?;
    let frame = dispatch.frame().unwrap();
    let mut view = tigertype_ui::candidate::View::default();
    let serial = view.update(Some(frame.clone()))?;
    let choice = view.choose(serial, 0).unwrap();
    assert_eq!(dispatch.offer_selection(choice), Ok(host));
    assert_eq!(dispatch.offer_selection(choice), Err(Rejection::Busy));
    assert_eq!(dispatch.frame().unwrap().state, frame.state); // 投递不是提交。
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            4,
            1,
            Command::TakeSelection { epoch: 2 }
        )?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    let Reply::Key(commit) = exchange(
        &mut dispatch,
        host,
        5,
        1,
        Command::TakeSelection { epoch: 1 },
    )?
    else {
        panic!("需要领取结果");
    };
    assert_eq!(commit.commit.as_deref(), Some("的"));
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            6,
            1,
            Command::TakeSelection { epoch: 1 }
        )?,
        Reply::Rejected(Rejection::StaleSnapshot)
    );
    assert!(view.choose(serial, 0).is_none());
    assert_eq!(
        dispatch.offer_selection(choice),
        Err(Rejection::StaleSnapshot)
    );
    Ok(())
}
#[test]
fn changed_state_blur_and_disconnect_invalidate_pending_clicks() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7)?;
    let host = dispatch.add_host()?;
    open(&mut dispatch, host)?;
    exchange(
        &mut dispatch,
        host,
        3,
        1,
        Command::Key {
            epoch: 1,
            key: Key::Character('u'),
        },
    )?;
    let choice = CandidateChoice {
        token: dispatch.frame().unwrap().token,
        index: 0,
    };
    dispatch.offer_selection(choice).unwrap();
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
            Command::TakeSelection { epoch: 1 }
        )?,
        Reply::Rejected(Rejection::StaleSnapshot)
    );
    let choice = CandidateChoice {
        token: dispatch.frame().unwrap().token,
        index: 0,
    };
    dispatch.offer_selection(choice).unwrap();
    exchange(
        &mut dispatch,
        host,
        6,
        1,
        Command::Focus {
            epoch: 2,
            caret: None,
        },
    )?;
    assert!(dispatch.frame().is_none());
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            7,
            1,
            Command::TakeSelection { epoch: 1 }
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
