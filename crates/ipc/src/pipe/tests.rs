//! 真实本机管道与取消回归；测试线程模拟对端，不进入产品线程模型。

use super::*;
use crate::Message;
use std::sync::atomic::AtomicUsize;
use std::sync::mpsc;
use tigertype_contracts::{
    InputState, KeyOutcome,
    protocol::{Command, Reply, Route},
};

fn label() -> String {
    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
    format!(
        "test-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    )
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn pair(runtime: &Runtime) -> Result<(Pipe, Pipe)> {
    let name = label();
    let listener = runtime.bind(&name)?;
    // 特意先连接再 accept，覆盖 ERROR_PIPE_CONNECTED 的无完成包分支。
    let client = runtime.connect(&name, &std::env::current_exe()?, deadline())?;
    let server = listener.accept(deadline())?;
    Ok((client, server))
}

fn request() -> Request {
    Request {
        route: Route {
            generation: 1,
            request_id: 1,
            session_id: 1,
        },
        command: Command::Key {
            epoch: 1,
            key: tigertype_contracts::Key::Space,
            prepare: tigertype_contracts::protocol::Prepare::Keep,
        },
    }
}

#[test]
fn synchronous_io_does_not_wait_for_a_busy_completion_thread() -> Result<()> {
    use windows_sys::Win32::System::Threading::*;
    // 只阻塞测试运行时自己的完成线程，确定性模拟调度延迟，不压满用户 CPU。
    struct Gate {
        entered: mpsc::SyncSender<()>,
        release: mpsc::Receiver<()>,
    }
    unsafe extern "system" fn hold(
        _: PTP_CALLBACK_INSTANCE,
        context: *mut std::ffi::c_void,
        _: PTP_WORK,
    ) {
        // SAFETY: Box<Gate> 由测试持有到 WaitForThreadpoolWorkCallbacks 完成。
        let gate = unsafe { &*context.cast::<Gate>() };
        let _ = gate.entered.try_send(());
        let _ = gate.release.recv_timeout(Duration::from_secs(5));
    }
    struct Work(PTP_WORK, mpsc::SyncSender<()>);
    impl Drop for Work {
        fn drop(&mut self) {
            let _ = self.1.try_send(());
            // SAFETY: 测试只创建这一个工作项，先放行再等退出，最后回收注册。
            unsafe {
                WaitForThreadpoolWorkCallbacks(self.0, 0);
                CloseThreadpoolWork(self.0);
            }
        }
    }
    let runtime = Runtime::new()?;
    let (mut client, mut server) = pair(&runtime)?;
    let (entered, ready) = mpsc::sync_channel(1);
    let (release, released) = mpsc::sync_channel(1);
    let mut gate = Box::new(Gate {
        entered,
        release: released,
    });
    let environment = TP_CALLBACK_ENVIRON_V3 {
        Version: 3,
        Pool: runtime.0.pool,
        CallbackPriority: TP_CALLBACK_PRIORITY_NORMAL,
        Size: size_of::<TP_CALLBACK_ENVIRON_V3>() as u32,
        ..Default::default()
    };
    // SAFETY: Gate 地址稳定且有效至工作项回收；只使用本测试的私有线程池。
    let work =
        unsafe { CreateThreadpoolWork(Some(hold), (&mut *gate as *mut Gate).cast(), &environment) };
    assert!(work != 0);
    let work = Work(work, release);
    // SAFETY: 新建工作项保持存活，Drop 配对等待和关闭。
    unsafe {
        SubmitThreadpoolWork(work.0);
    }
    ready.recv_timeout(Duration::from_secs(2)).unwrap();
    let frame = crate::encode(&Message::Request(request()))?;
    // 先挂起读取，再由写入让内核完成；回收线程仍被挡住，正常异步完成也必须可直接领取。
    let mut pending = server.start_read()?;
    assert!(pending.try_complete()?.is_none());
    client.send_frame(&frame, Instant::now() + Duration::from_millis(50))?;
    let (bytes, count) = pending.try_complete()?.expect("内核完成不能等待线程池转交");
    assert_eq!(&bytes[..count], frame);
    drop(pending);
    for _ in 0..32 {
        let end = Instant::now() + Duration::from_millis(50);
        client.send_frame(&frame, end)?;
        assert_eq!(server.receive_frame(end)?, frame);
    }
    // 同步完成尚未领取时发生取消，仍不得交付结果。
    let mut pending = client.start_write(frame)?;
    client.cancellation().cancel();
    assert!(matches!(pending.try_complete(), Err(Error::Cancelled)));
    drop(pending);
    drop(work);
    drop(client);
    drop(server);
    runtime.shutdown(deadline())
}

// 新的空闲读没有请求期限，测试异常也必须取消，避免 scope 在 panic 时等待永不结束的读。
struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[test]
fn asynchronous_read_rejects_overlap_and_drop_cancels_without_freeing_kernel_memory() -> Result<()>
{
    let runtime = Runtime::new()?;
    let (mut client, server) = pair(&runtime)?;
    let mut pending = client.start_read()?;
    assert!(matches!(client.start_read(), Err(Error::Busy)));
    assert!(pending.try_complete()?.is_none());
    drop(pending);
    assert!(matches!(client.start_read(), Err(Error::Closed)));
    drop(client);
    drop(server);
    runtime.shutdown(deadline())?;
    Ok(())
}

#[test]
fn shutdown_timeout_is_not_drain_and_cannot_reopen_admission() -> Result<()> {
    let runtime = Runtime::new()?;
    let listener = runtime.bind(&label())?;
    let cancellation = listener.cancellation();
    assert!(matches!(
        runtime.shutdown(Instant::now() + Duration::from_millis(5)),
        Err(Error::TimedOut)
    ));
    assert!(matches!(runtime.bind(&label()), Err(Error::Closed)));
    drop(listener);
    // 取消令牌本身也持有管道，不能只看没有输入请求就声称资源释放。
    assert!(matches!(
        runtime.shutdown(Instant::now()),
        Err(Error::TimedOut)
    ));
    drop(cancellation);
    runtime.shutdown(deadline())?;
    runtime.shutdown(Instant::now())?;
    assert!(matches!(runtime.bind(&label()), Err(Error::Closed)));
    Ok(())
}

#[test]
fn shutdown_event_wakes_all_waiters_after_the_last_owner_drops() -> Result<()> {
    let runtime = Runtime::new()?;
    let listener = runtime.bind(&label())?;
    std::thread::scope(|scope| -> Result<()> {
        let (done, observed) = mpsc::sync_channel(2);
        let mut workers = Vec::new();
        for _ in 0..2 {
            let done = done.clone();
            let runtime = &runtime;
            workers.push(scope.spawn(move || {
                let result = runtime.shutdown(deadline());
                done.send(result).unwrap();
            }));
        }
        assert!(matches!(
            observed.recv_timeout(Duration::from_millis(20)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        drop(listener);
        // 两个等候者都被同一手动事件唤醒，后续 shutdown 不重置它或恢复准入。
        for _ in 0..2 {
            observed.recv_timeout(Duration::from_secs(2)).unwrap()?;
        }
        for worker in workers {
            worker.join().unwrap();
        }
        runtime.shutdown(Instant::now())
    })
}

#[test]
fn shutdown_drains_cancelled_accept_and_idle_read_without_polling() -> Result<()> {
    let runtime = Runtime::new()?;
    let listener = runtime.bind(&label())?;
    let cancellation = listener.cancellation();
    std::thread::scope(|scope| -> Result<()> {
        let worker = scope.spawn(move || listener.accept(deadline()));
        cancellation.cancel();
        drop(cancellation);
        assert!(matches!(
            worker.join().unwrap(),
            Err(Error::Closed | Error::Cancelled)
        ));
        runtime.shutdown(deadline())
    })?;
    let runtime = Runtime::new()?;
    let (client, mut server) = pair(&runtime)?;
    let cancellation = server.cancellation();
    std::thread::scope(|scope| -> Result<()> {
        let worker = scope.spawn(move || server.receive_frame_idle(Duration::from_secs(1)));
        cancellation.cancel();
        drop(cancellation);
        drop(client);
        assert!(matches!(
            worker.join().unwrap(),
            Err(Error::Closed | Error::Cancelled)
        ));
        runtime.shutdown(deadline())
    })
}

#[test]
fn idle_receive_does_not_spend_frame_budget_before_first_bytes() -> Result<()> {
    let runtime = Runtime::new()?;
    let (mut client, mut server) = pair(&runtime)?;
    std::thread::scope(|scope| -> Result<()> {
        let _cancel = CancelOnDrop(server.cancellation());
        let (done, observed) = mpsc::sync_channel(1);
        let worker = scope.spawn(move || {
            let result = server.receive_frame_idle(Duration::from_millis(30));
            let _ = done.send(());
            result
        });
        assert!(matches!(
            observed.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        client.send_frame(&crate::encode(&Message::Request(request()))?, deadline())?;
        observed.recv_timeout(Duration::from_secs(2)).unwrap();
        let (frame, _) = worker.join().unwrap()?;
        assert_eq!(decode(&frame)?, Message::Request(request()));
        Ok(())
    })
}

#[test]
fn idle_receive_cancels_before_or_during_wait_and_observes_eof() -> Result<()> {
    let runtime = Runtime::new()?;
    for before in [true, false] {
        let (_client, mut server) = pair(&runtime)?;
        std::thread::scope(|scope| {
            let cancel = CancelOnDrop(server.cancellation());
            if before {
                cancel.0.cancel();
            }
            let worker = scope.spawn(move || server.receive_frame_idle(Duration::from_secs(1)));
            drop(cancel);
            assert!(matches!(
                worker.join().unwrap(),
                Err(Error::Closed | Error::Cancelled)
            ));
        });
    }
    let (client, mut server) = pair(&runtime)?;
    drop(client);
    assert!(matches!(
        server.receive_frame_idle(Duration::from_secs(1)),
        Err(Error::Io(_))
    ));
    Ok(())
}

#[test]
fn idle_receive_partial_frame_has_one_deadline_and_invalid_budgets_are_rejected() -> Result<()> {
    let runtime = Runtime::new()?;
    let (mut client, mut server) = pair(&runtime)?;
    for budget in [Duration::ZERO, Duration::from_secs(6)] {
        assert!(matches!(
            server.receive_frame_idle(budget),
            Err(Error::Invalid(_))
        ));
    }
    let frame = crate::encode(&Message::Request(request()))?;
    client.operate(Kind::Write, frame[..5].to_vec(), deadline())?;
    assert!(matches!(
        server.receive_frame_idle(Duration::from_millis(30)),
        Err(Error::TimedOut)
    ));
    assert!(matches!(
        server.receive_frame_idle(Duration::from_secs(1)),
        Err(Error::Closed)
    ));
    Ok(())
}

#[test]
fn real_pipe_round_trip_copies_large_unicode_reply_and_matches_identity() -> Result<()> {
    let runtime = Runtime::new()?;
    let (mut client, mut server) = pair(&runtime)?;
    std::thread::scope(|scope| -> Result<()> {
        let worker = scope.spawn(move || -> Result<()> {
            let message = decode(&server.receive_frame(deadline())?)?;
            assert_eq!(message, Message::Request(request()));
            server.send_frame(
                &crate::encode(&Message::Response(Response {
                    route: request().route,
                    reply: Reply::Key(KeyOutcome {
                        handled: true,
                        commit: Some("𠀀😀".repeat(4096)),
                        state: InputState::default(),
                    }),
                }))?,
                deadline(),
            )
        });
        let mut state = ClientState::new(1)?;
        let reply = client.transact(&mut state, request(), deadline())?;
        assert!(
            matches!(reply.reply, Reply::Key(outcome) if outcome.commit == Some("𠀀😀".repeat(4096)))
        );
        worker.join().unwrap()?;
        Ok(())
    })
}

#[test]
fn pending_accept_times_out_and_callback_releases_its_pipe() -> Result<()> {
    let runtime = Runtime::new()?;
    let mut listener = runtime.bind(&label())?;
    let (sender, receiver) = mpsc::sync_channel(1);
    Arc::get_mut(&mut listener.0.io).unwrap().on_drop = Some(sender);
    let result = listener.accept(Instant::now() + Duration::from_millis(30));
    assert!(matches!(result, Err(Error::TimedOut)));
    // 等完成回调，而不是忙轮询；调用方返回后，I/O 持有的资源仍必须最终释放。
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    Ok(())
}

#[test]
fn timed_out_read_and_partial_frame_are_closed_and_reclaimed() -> Result<()> {
    let runtime = Runtime::new()?;
    for partial in [false, true] {
        let (mut client, mut server) = pair(&runtime)?;
        if partial {
            let frame = crate::encode(&Message::Request(request()))?;
            client.operate(Kind::Write, frame[..5].to_vec(), deadline())?;
        }
        let (sender, receiver) = mpsc::sync_channel(1);
        Arc::get_mut(&mut server.io).unwrap().on_drop = Some(sender);
        assert!(matches!(
            server.receive_frame(Instant::now() + Duration::from_millis(30)),
            Err(Error::TimedOut)
        ));
        assert!(matches!(
            server.receive_frame(deadline()),
            Err(Error::Closed)
        ));
        drop(server);
        receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    }
    Ok(())
}

#[test]
fn external_cancel_wakes_a_pending_read_and_does_not_deliver_a_reply() -> Result<()> {
    let runtime = Runtime::new()?;
    let (_client, mut server) = pair(&runtime)?;
    let cancellation = server.cancellation();
    std::thread::scope(|scope| {
        let worker = scope.spawn(move || server.receive_frame(deadline()));
        cancellation.cancel();
        cancellation.cancel();
        assert!(matches!(
            worker.join().unwrap(),
            Err(Error::Cancelled | Error::Closed)
        ));
    });
    Ok(())
}

#[test]
fn stalled_write_times_out_and_keeps_buffer_alive_until_completion() -> Result<()> {
    let runtime = Runtime::new()?;
    let (mut client, _server) = pair(&runtime)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    Arc::get_mut(&mut client.io).unwrap().on_drop = Some(sender);
    // 对端不读取，写入超过管道缓冲区的数据强制进入挂起路径。
    assert!(matches!(
        client.operate(
            Kind::Write,
            vec![0; CHUNK * 4],
            Instant::now() + Duration::from_millis(30)
        ),
        Err(Error::TimedOut)
    ));
    drop(client);
    receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    Ok(())
}

#[test]
fn rejects_oversized_header_disconnect_and_cancelled_transact_without_retry() -> Result<()> {
    let runtime = Runtime::new()?;
    let (mut client, mut server) = pair(&runtime)?;
    let mut invalid = crate::encode(&Message::Request(request()))?;
    invalid[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    client.operate(Kind::Write, invalid[..12].to_vec(), deadline())?;
    assert!(matches!(
        server.receive_frame(deadline()),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        server.receive_frame(deadline()),
        Err(Error::Closed)
    ));
    drop(server);
    assert!(client.receive_frame(deadline()).is_err());

    let (mut client, _server) = pair(&runtime)?;
    let mut state = ClientState::new(1)?;
    assert!(matches!(
        client.transact(
            &mut state,
            request(),
            Instant::now() + Duration::from_millis(30)
        ),
        Err(Error::TimedOut)
    ));
    assert!(matches!(state.receive(&[]), Err(Error::Closed)));
    assert!(matches!(
        client.receive_frame(deadline()),
        Err(Error::Closed)
    ));
    Ok(())
}

#[test]
fn names_instances_and_runtime_capacity_are_bounded() -> Result<()> {
    let runtime = Runtime::new()?;
    for name in ["", "../other", "a\\b", "a:b", "x\0y", &"a".repeat(65)] {
        assert!(runtime.bind(name).is_err());
    }
    let name = label();
    let first = runtime.bind(&name)?;
    assert!(runtime.bind(&name).is_err());
    drop(first);
    let mut listeners = Vec::new();
    for _ in 0..MAX_PIPES {
        listeners.push(runtime.bind(&label())?);
    }
    assert!(matches!(runtime.bind(&label()), Err(Error::Busy)));
    drop(listeners);
    assert!(runtime.bind(&label()).is_ok());
    Ok(())
}
