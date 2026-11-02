//! 多宿主序号/焦点隔离与回收策略；合成数据，类型化收发，不经编解码、不向真实应用写字。
use tigertype_backend::{Result, dispatch::Dispatcher};
use tigertype_contracts::{
    InputState, Key, Options, OutputForm, Theme,
    protocol::{Command, Prepare, Rejection, Reply, Request, Route},
};
use tigertype_engine::{Data, Dictionary, Entry};
fn dictionary() -> Dictionary {
    Dictionary::new(Data {
        entries: vec![Entry {
            character: '的',
            code: "u",
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
    let response = dispatch.handle(host, request.clone());
    // 回复只回送请求的路由，宿主据此丢弃不匹配的回复。
    assert_eq!(response.route, request.route);
    Ok(response.reply)
}
fn input(epoch: u64, prepare: Prepare, key: Key) -> Command {
    Command::Key {
        epoch,
        prepare,
        key,
    }
}
fn key(epoch: u64, key: Key) -> Command {
    input(epoch, Prepare::Keep, key)
}
/// 以序号 1、代次 1 开会话并取得焦点；无组合时的空格不改状态。
fn open(dispatch: &mut Dispatcher<'_>, host: u64) -> Result<()> {
    assert!(matches!(
        exchange(dispatch, host, 1, 1, input(1, Prepare::Open, Key::Space))?,
        Reply::Key(_)
    ));
    Ok(())
}
fn state(reply: Reply) -> InputState {
    match reply {
        Reply::Key(outcome) => outcome.state,
        other => panic!("需要状态回复：{other:?}"),
    }
}

#[test]
fn prepared_keys_share_admission_focus_snapshot_and_reload_rules() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    let first = state(exchange(
        &mut dispatch,
        a,
        1,
        1,
        input(1, Prepare::Open, Key::Character('u')),
    )?);
    assert!(first.composing && first.candidates[0].text == "的");
    // 拒绝重复会话/旧代次必须先于清空，原快照仍然可以选字。
    for (sequence, command, reason) in [
        (
            2,
            input(2, Prepare::Open, Key::Space),
            Rejection::SessionReused,
        ),
        (
            3,
            input(1, Prepare::Focus, Key::Space),
            Rejection::StaleFocus,
        ),
        (
            4,
            input(2, Prepare::Clear, Key::Space),
            Rejection::StaleFocus,
        ),
    ] {
        assert_eq!(
            exchange(&mut dispatch, a, sequence, 1, command)?,
            Reply::Rejected(reason)
        );
    }
    assert!(
        matches!(exchange(&mut dispatch, a, 5, 1, Command::Select { revision: first.revision, index: 0, epoch: 1 })?, Reply::Key(value) if value.commit.as_deref() == Some("的"))
    );
    exchange(
        &mut dispatch,
        a,
        6,
        1,
        input(1, Prepare::Keep, Key::Character('u')),
    )?;
    let options = Options {
        page_size: 3,
        ..Default::default()
    };
    dispatch.reconfigure(options);
    let cleared = state(exchange(
        &mut dispatch,
        a,
        7,
        1,
        input(1, Prepare::Clear, Key::Character('u')),
    )?);
    assert_eq!(
        cleared.composition.text, "u",
        "清空与本键一起执行，不能累加旧编码"
    );
    assert_eq!(cleared.options, options, "清空组合时采用新设置");
    exchange(
        &mut dispatch,
        b,
        1,
        1,
        input(1, Prepare::Open, Key::Character('u')),
    )?;
    assert_eq!(
        exchange(&mut dispatch, a, 8, 1, input(1, Prepare::Clear, Key::Space))?,
        Reply::Rejected(Rejection::NotFocused)
    );
    let restored = state(exchange(
        &mut dispatch,
        a,
        9,
        1,
        input(2, Prepare::Focus, Key::Character('u')),
    )?);
    assert_eq!(restored.composition.text, "u");
    assert_eq!(
        exchange(&mut dispatch, b, 2, 1, key(1, Key::Space))?,
        Reply::Rejected(Rejection::NotFocused)
    );
    // 新会话的旧代次不能创建残留对象或抢走现有焦点。
    assert_eq!(
        exchange(&mut dispatch, a, 10, 2, input(1, Prepare::Open, Key::Space))?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    assert_eq!(
        exchange(
            &mut dispatch,
            a,
            11,
            2,
            input(3, Prepare::Focus, Key::Space)
        )?,
        Reply::Rejected(Rejection::UnknownSession)
    );
    assert!(
        matches!(exchange(&mut dispatch, a, 12, 1, key(2, Key::Space))?, Reply::Key(value) if value.commit.as_deref() == Some("的"))
    );
    Ok(())
}
#[test]
fn multi_host_sequences_and_sessions_are_isolated_and_focus_switch_cancels_old_composition()
-> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
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
        exchange(&mut dispatch, b, 3, 1, key(1, Key::Space))?,
        Reply::Rejected(Rejection::OutOfOrder)
    );
    assert!(
        state(exchange(
            &mut dispatch,
            a,
            6,
            1,
            input(2, Prepare::Focus, Key::Space)
        )?)
        .composition
        .text
        .is_empty()
    );
    assert_eq!(
        exchange(&mut dispatch, a, 7, 1, input(1, Prepare::Focus, Key::Space))?,
        Reply::Rejected(Rejection::StaleFocus)
    );
    // 已取代的代次不能再输入，当前代次照常。
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
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    open(&mut dispatch, a)?;
    assert!(!dispatch.idle(a));
    open(&mut dispatch, b)?;
    assert!(dispatch.idle(a));
    assert!(!dispatch.idle(b));
    exchange(&mut dispatch, b, 3, 1, key(1, Key::Character('u')))?;
    // 其他宿主取得焦点即清掉旧组合，之后才可回收；未知宿主永不作为回收对象。
    exchange(&mut dispatch, a, 3, 1, input(2, Prepare::Focus, Key::Space))?;
    assert!(dispatch.idle(b));
    assert!(!dispatch.idle(99));
    assert!(!dispatch.full());
    for id in 2..=63 {
        exchange(
            &mut dispatch,
            a,
            id + 2,
            id,
            input(id + 1, Prepare::Open, Key::Space),
        )?;
    }
    assert!(dispatch.full());
    exchange(&mut dispatch, a, 66, 2, Command::Close)?;
    assert!(!dispatch.full());
    dispatch.remove_host(a);
    assert!(!dispatch.idle(a));
    Ok(())
}
#[test]
fn stale_snapshot_focus_loss_and_disconnect_reject_selection() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
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
    let other = dispatch.add_host()?;
    open(&mut dispatch, other)?;
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            7,
            1,
            Command::Select {
                revision: old.revision + 1,
                index: 0,
                epoch: 1
            }
        )?,
        Reply::Rejected(Rejection::NotFocused)
    );
    dispatch.remove_host(host);
    assert_eq!(
        exchange(&mut dispatch, host, 8, 1, key(1, Key::Space))?,
        Reply::Rejected(Rejection::UnknownHost)
    );
    assert!(dispatch.add_host()? > other);
    Ok(())
}
#[test]
fn global_capacity_old_generations_and_session_reuse_are_rejected() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
    let host = dispatch.add_host()?;
    for id in 1..=64 {
        assert!(matches!(
            exchange(
                &mut dispatch,
                host,
                id,
                id,
                input(id, Prepare::Open, Key::Character('u'))
            )?,
            Reply::Key(_)
        ));
    }
    let other = dispatch.add_host()?;
    assert_eq!(
        exchange(
            &mut dispatch,
            other,
            1,
            1,
            input(1, Prepare::Open, Key::Space)
        )?,
        Reply::Rejected(Rejection::SessionLimit)
    );
    // 被拒的开会话不改动现有焦点会话：最后一个会话的组合仍可上屏。
    assert!(
        matches!(exchange(&mut dispatch, host, 65, 64, key(64, Key::Space))?, Reply::Key(value) if value.commit.as_deref() == Some("的"))
    );
    exchange(&mut dispatch, host, 66, 1, Command::Close)?;
    assert_eq!(
        exchange(
            &mut dispatch,
            host,
            67,
            1,
            input(65, Prepare::Open, Key::Space)
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
        command: key(64, Key::Space),
    };
    assert_eq!(
        dispatch.handle(host, old).reply,
        Reply::Rejected(Rejection::StaleGeneration)
    );
    dispatch.invalidate();
    assert_eq!(
        exchange(&mut dispatch, host, 101, 2, key(64, Key::Space))?,
        Reply::Rejected(Rejection::Unavailable)
    );
    Ok(())
}
#[test]
fn reloaded_settings_wait_until_the_composition_ends() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
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
    // 重新取得焦点会清空组合，其按键回复也携带已换上的设置。
    assert_eq!(
        state(exchange(
            &mut dispatch,
            host,
            6,
            1,
            input(2, Prepare::Focus, Key::Space)
        )?)
        .options,
        changed
    );
    Ok(())
}
#[test]
fn sessions_open_with_current_backend_settings() -> Result<()> {
    let dictionary = dictionary();
    let started = Options {
        theme: Theme::Rose,
        ..Default::default()
    };
    let mut dispatch = Dispatcher::new(&dictionary, 7, started)?;
    let host = dispatch.add_host()?;
    // 宿主不读盘也不携带选项；会话以后台启动时的设置打开。
    let opened = state(exchange(
        &mut dispatch,
        host,
        1,
        1,
        input(1, Prepare::Open, Key::Space),
    )?);
    assert_eq!(opened.options, started);
    // 热重载之后打开的会话立即用上新设置。
    let changed = Options {
        page_size: 7,
        ..started
    };
    dispatch.reconfigure(changed);
    let opened = state(exchange(
        &mut dispatch,
        host,
        2,
        2,
        input(2, Prepare::Open, Key::Space),
    )?);
    assert_eq!(opened.options, changed);
    Ok(())
}
#[test]
fn reclaim_picks_the_least_recently_used_idle_host_only_when_capacity_runs_out() -> Result<()> {
    let dictionary = dictionary();
    let mut dispatch = Dispatcher::new(&dictionary, 7, Options::default())?;
    let a = dispatch.add_host()?;
    let b = dispatch.add_host()?;
    let c = dispatch.add_host()?;
    let any = |_| true;
    // 仍有空闲接入位且会话未满：不回收。
    assert_eq!(dispatch.reclaimable(true, any), None);
    // 接入位用尽：选最久未用者；会话外往来同样推迟回收。
    assert_eq!(dispatch.reclaimable(false, any), Some(a));
    dispatch.touch(a);
    assert_eq!(dispatch.reclaimable(false, any), Some(b));
    // 平台不认可（传输未空闲或持有任务栏图标）的宿主跳过。
    assert_eq!(dispatch.reclaimable(false, |host| host != b), Some(c));
    // 当前焦点与组合中的宿主不回收；请求同样计为使用。
    open(&mut dispatch, b)?;
    exchange(&mut dispatch, b, 3, 1, key(1, Key::Character('u')))?;
    assert_eq!(dispatch.reclaimable(false, any), Some(c));
    assert_eq!(dispatch.reclaimable(false, |host| host != c), Some(a));
    assert_eq!(dispatch.reclaimable(false, |host| host == b), None);
    // 合计会话达上限时即使还有空闲接入位也要回收。C 开会话会取走焦点，B 随后重新取得焦点并组合。
    for id in 2..=64 {
        exchange(
            &mut dispatch,
            c,
            id - 1,
            id,
            input(id, Prepare::Open, Key::Space),
        )?;
    }
    exchange(
        &mut dispatch,
        b,
        4,
        1,
        input(2, Prepare::Focus, Key::Character('u')),
    )?;
    assert!(dispatch.full());
    assert_eq!(dispatch.reclaimable(true, any), Some(a));
    dispatch.remove_host(a);
    assert_eq!(dispatch.reclaimable(true, any), Some(c));
    Ok(())
}
