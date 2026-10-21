//! 使用固定公开样例的原生探针；调用方必须在限时子进程中运行。

use crate::{
    Result,
    resources::{self, Entry, Lock},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tigerrime_contracts::InputState;
use tigerrime_engine::{Engine, EnginePaths};

const WORKSPACE_MARKER: &[u8] = b"TigerRime isolated synthetic workspace v2\n";
const SESSION_CYCLES: usize = 1_000;

fn runtime_name(work: &Path) -> Result<&str> {
    let name = work
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("缺少测试目录名")?;
    let (process, nonce) = name
        .strip_prefix("rust-engine-")
        .and_then(|suffix| suffix.split_once('-'))
        .ok_or("不是生成的引擎测试目录")?;
    process.parse::<u32>()?;
    nonce.parse::<u128>()?;
    Ok(name)
}

fn runtime_base(root: &Path) -> Result<PathBuf> {
    let base = std::env::temp_dir().join("TigerRime-tests");
    resources::reject_links(&base)?;
    let base = base.canonicalize()?;
    // 活动数据库必须离开被观察的源码工作区；TMP/TEMP 被改到工作区内时明确拒绝。
    if base.starts_with(root.canonicalize()?) {
        return Err("测试临时目录不能位于项目工作区内".into());
    }
    Ok(base)
}

/// 根据受限报告目录派生临时运行数据，不接受任意数据库路径。
pub fn runtime_paths(root: &Path, work: &Path) -> Result<EnginePaths> {
    resources::reject_links(work)?;
    let work = work.canonicalize()?;
    if work.parent() != Some(root.join("artifacts").canonicalize()?.as_path())
        || fs::read(work.join("test-workspace"))? != WORKSPACE_MARKER
    {
        return Err("探针只允许新建的隔离测试报告目录".into());
    }
    let name = runtime_name(&work)?;
    let data = runtime_base(root)?.join(name);
    resources::reject_links(&data)?;
    if fs::read(data.join("test-workspace"))? != name.as_bytes() {
        return Err("测试数据与报告目录不匹配".into());
    }
    let paths = EnginePaths {
        shared: data.join("shared"),
        user: data.join("user"),
        logs: data.join("logs"),
    };
    for path in [&paths.shared, &paths.user, &paths.logs] {
        resources::reject_links(path)?;
    }
    resources::verify_runtime_tiger(&paths.user)?;
    Ok(paths)
}

#[derive(Deserialize)]
struct Fixtures {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    schema: String,
    input: String,
    #[serde(default)]
    options: BTreeMap<String, bool>,
    first: Option<String>,
    contains: Option<String>,
    commit: Option<String>,
    page_size: Option<usize>,
    min_candidates: Option<usize>,
    max_candidates: Option<usize>,
    comment_contains: Option<String>,
    comment_nonempty: Option<bool>,
}

pub fn new_workspace(root: &Path) -> Result<PathBuf> {
    resources::verify(root)?;
    let work = resources::new_directory(&root.join("artifacts"), "rust-engine")?;
    let base = std::env::temp_dir().join("TigerRime-tests");
    resources::reject_links(&base)?;
    fs::create_dir_all(&base)?;
    let data = runtime_base(root)?.join(runtime_name(&work)?);
    // create_dir 拒绝复用已有目录；不复制、共享或删除任何现用用户数据库。
    fs::create_dir(&data)?;
    for directory in ["shared", "user", "logs"] {
        fs::create_dir(data.join(directory))?;
    }
    let patch = resources::stage_tiger(root, &data.join("user"))?;
    fs::write(
        work.join("runtime-patches.json"),
        serde_json::to_vec_pretty(&[patch])?,
    )?;
    let lock: Lock = resources::read_json(&root.join("dependencies.lock.json"))?;
    for (name, group) in [("prelude", lock.prelude), ("essay", lock.essay)] {
        resources::copy_entries(
            &root.join("vendor").join(name),
            &data.join("shared"),
            &group.files,
        )?;
    }
    let files: Vec<Entry> = resources::read_json(&root.join("vendor/librime/files.json"))?;
    for entry in files
        .iter()
        .filter(|entry| entry.path.starts_with("deps/share/opencc/"))
    {
        let source = root.join("vendor/librime").join(&entry.path);
        let relative = entry
            .path
            .strip_prefix("deps/share/")
            .ok_or("OpenCC 路径不符")?;
        let target = data.join("shared").join(relative);
        fs::create_dir_all(target.parent().ok_or("缺少父目录")?)?;
        fs::copy(source, target)?;
        resources::verify_entry(
            &data.join("shared"),
            &Entry {
                path: relative.to_owned(),
                size: entry.size,
                sha256: entry.sha256.clone(),
                url: None,
            },
        )?;
    }
    // 留下精确输入清单副本，防止后来更改源码换行或锁文件导致无法追溯。
    for (source, target) in [
        ("dependencies.lock.json", "dependencies.lock.json"),
        ("resources/tiger.lock.json", "tiger.lock.json"),
        ("fixtures/engine-smoke.json", "engine-smoke.json"),
    ] {
        fs::copy(root.join(source), work.join(target))?;
    }
    fs::write(data.join("test-workspace"), runtime_name(&work)?.as_bytes())?;
    fs::write(work.join("test-workspace"), WORKSPACE_MARKER)?;
    Ok(work)
}

fn check(case: &Case, state: &InputState, commit: &str) -> Vec<&'static str> {
    let mut failures = Vec::new();
    if case
        .first
        .as_ref()
        .is_some_and(|v| state.candidates.first().map(|c| &c.text) != Some(v))
    {
        failures.push("首候选不符");
    }
    if case
        .contains
        .as_ref()
        .is_some_and(|v| !state.candidates.iter().any(|c| &c.text == v))
    {
        failures.push("候选缺失");
    }
    if case.commit.as_ref().is_some_and(|v| v != commit) {
        failures.push("上屏文字不符");
    }
    if case.page_size.is_some_and(|v| v != state.page_size) {
        failures.push("页大小不符");
    }
    if case
        .min_candidates
        .is_some_and(|v| state.candidates.len() < v)
    {
        failures.push("候选数量不足");
    }
    if case
        .max_candidates
        .is_some_and(|v| state.candidates.len() > v)
    {
        failures.push("意外产生候选");
    }
    if case
        .comment_contains
        .as_ref()
        .is_some_and(|v| !state.candidates.iter().any(|c| c.comment.contains(v)))
    {
        failures.push("注释缺失");
    }
    if case.comment_nonempty == Some(true) && state.candidates.iter().all(|c| c.comment.is_empty())
    {
        failures.push("注释为空");
    }
    failures
}

pub fn native_errors(log: &str) -> Vec<&str> {
    log.lines()
        .filter(|line| {
            let bytes = line.as_bytes();
            bytes.len() > 9
                && matches!(bytes[0], b'E' | b'F')
                && bytes[1..9].iter().all(u8::is_ascii_digit)
                && bytes[9] == b' '
        })
        .collect()
}

pub(crate) fn diagnostics(logs: &Path) -> Result<Vec<String>> {
    let mut errors = BTreeSet::new();
    for file in fs::read_dir(logs)? {
        let path = file?.path();
        if path.is_file() {
            let bytes = fs::read(path)?;
            let log = String::from_utf8_lossy(&bytes);
            errors.extend(native_errors(&log).into_iter().map(str::to_owned));
        }
    }
    Ok(errors.into_iter().collect())
}

fn reset_options(session: &mut tigerrime_engine::Session<'_>) -> Result<()> {
    for option in [
        "ascii_mode",
        "pinyin",
        "emoji_cn",
        "chaifen",
        "simplification",
        "charset_comment_filter",
        "udpf_switch",
    ] {
        session.set_option(option, false)?;
    }
    session.set_option("extended_char", true)?;
    Ok(())
}

fn check_dispatch(engine: &Engine) -> Result<()> {
    use crate::dispatch::Dispatcher;
    use tigerrime_contracts::protocol::{Command, Message, Rejection, Reply, Request, Route};

    fn exchange(dispatcher: &mut Dispatcher<'_>, request: Request) -> Result<Reply> {
        let frame = tigerrime_ipc::encode(&Message::Request(request.clone()))?;
        let frame = dispatcher.dispatch(&frame)?;
        Ok(tigerrime_ipc::decode_response(&frame, &request)?.reply)
    }
    let request = |request_id, session_id, command| Request {
        route: Route {
            generation: 7,
            request_id,
            session_id,
        },
        command,
    };
    let key = |keycode: char| Command::Key {
        keycode: keycode as i32,
        modifiers: 0,
    };
    let mut dispatcher = Dispatcher::new(engine, 7)?;
    // 内存帧往返不包含任何管道等待，不并入下方原有的库内延迟统计。
    for (id, schema) in [(1, "tiger"), (2, "tigress")] {
        if !matches!(exchange(&mut dispatcher, request(id, id, Command::Open { schema: schema.into() }))?,
            Reply::Opened(state) if state.schema_id == schema)
        {
            return Err("协议创建会话或方案不符".into());
        }
    }
    if dispatcher.dispatch(&[0; tigerrime_ipc::HEADER_LEN]).is_ok() {
        return Err("协议接受了无效帧".into());
    }
    if !matches!(exchange(&mut dispatcher, request(3, 1, key('u')))?,
        Reply::Key(outcome) if outcome.handled && outcome.commit.is_none()
            && outcome.state.candidates.first().is_some_and(|candidate| candidate.text == "的"))
    {
        return Err("协议按键快照不符".into());
    }
    if !matches!(exchange(&mut dispatcher, request(4, 2, key('d')))?, Reply::Key(outcome) if outcome.commit.is_none())
    {
        return Err("第二会话发生意外提交".into());
    }
    let mut stale = request(u64::MAX, 1, key(' '));
    stale.route.generation = 6;
    if exchange(&mut dispatcher, stale)? != Reply::Rejected(Rejection::StaleGeneration)
        || exchange(&mut dispatcher, request(3, 1, key('u')))?
            != Reply::Rejected(Rejection::OutOfOrder)
    {
        return Err("协议未拒绝旧代次或重复请求".into());
    }
    if !matches!(exchange(&mut dispatcher, request(5, 1, key(' ')))?,
        Reply::Key(outcome) if outcome.commit.as_deref() == Some("的"))
    {
        return Err("会话之间相互污染或提交丢失".into());
    }
    if exchange(&mut dispatcher, request(5, 1, key(' ')))? != Reply::Rejected(Rejection::OutOfOrder)
        || !matches!(exchange(&mut dispatcher, request(6, 1, key(' ')))?, Reply::Key(outcome) if outcome.commit.is_none())
    {
        return Err("协议重复接受提交或未消费上次提交".into());
    }
    if !matches!(exchange(&mut dispatcher, request(7, 2, Command::Clear))?,
        Reply::Cleared(state) if state.composition.text.is_empty() && state.candidates.is_empty())
    {
        return Err("协议清屏失败".into());
    }
    for (sequence, session, command, expected) in [
        (8, 1, Command::Close, Reply::Closed),
        (9, 1, key('u'), Reply::Rejected(Rejection::UnknownSession)),
        (
            10,
            1,
            Command::Open {
                schema: "tiger".into(),
            },
            Reply::Rejected(Rejection::SessionReused),
        ),
        (11, 2, Command::Close, Reply::Closed),
    ] {
        if exchange(&mut dispatcher, request(sequence, session, command))? != expected {
            return Err("关闭会话的释放或身份校验失败".into());
        }
    }
    Ok(())
}

fn check_client_delivery(engine: &Engine) -> Result<()> {
    use crate::dispatch::Dispatcher;
    use tigerrime_contracts::protocol::{Command, Reply, Request, Route};
    use tigerrime_ipc::{ClientState, Error, FrameAssembler};

    let request = |request_id, command| Request {
        route: Route {
            generation: 8,
            request_id,
            session_id: 1,
        },
        command,
    };
    let key = |keycode: char| Command::Key {
        keycode: keycode as i32,
        modifiers: 0,
    };
    let mut client = ClientState::new(8)?;
    let mut dispatcher = Dispatcher::new(engine, 8)?;
    for (sequence, command) in [
        (
            1,
            Command::Open {
                schema: "tiger".into(),
            },
        ),
        (2, key('u')),
        (3, key(' ')),
        (4, key('u')),
        (5, key(' ')),
    ] {
        // 此处只验证原生数据通路，5 秒诊断期限不冒充 TSF 的 50ms 验收。
        let request = request(sequence, command);
        let frame = client.begin(request.clone(), Instant::now() + Duration::from_secs(5))?;
        let mut incoming = FrameAssembler::default();
        let mut assembled = None;
        for bytes in frame.chunks(3) {
            if let Some(frame) = incoming.push(bytes)? {
                assembled = Some(frame);
            }
        }
        let response = dispatcher.dispatch(&assembled.ok_or("分片请求未完成")?)?;
        if sequence == 5 {
            // 引擎已消费第二次提交；取消后即使收到合法结果也不能再次交付给宿主。
            if !matches!(tigerrime_ipc::decode_response(&response, &request)?.reply,
                Reply::Key(outcome) if outcome.commit.as_deref() == Some("的"))
            {
                return Err("取消测试未产生预期的原生提交".into());
            }
            client.cancel();
            if !matches!(client.receive(&response), Err(Error::Closed)) {
                return Err("取消后仍接受了原生迟到提交".into());
            }
            continue;
        }
        let mut delivered = None;
        for bytes in response.chunks(7) {
            if let Some(response) = client.receive(bytes)?
                && delivered.replace(response).is_some()
            {
                return Err("同一原生回复被交付两次".into());
            }
        }
        match delivered.ok_or("分片原生回复未交付")?.reply {
            Reply::Opened(state) if sequence == 1 && state.schema_id == "tiger" => {}
            Reply::Key(outcome) if sequence == 3 && outcome.commit.as_deref() == Some("的") => {}
            Reply::Key(outcome) if matches!(sequence, 2 | 4) && outcome.commit.is_none() => {}
            _ => return Err("分片原生回复内容不符".into()),
        }
    }
    // 模拟断连时结束该调度器，Session guard 释放未主动 close 的会话。
    drop(dispatcher);
    Ok(())
}

fn measure(engine: &Engine, fixtures: Fixtures) -> Result<Value> {
    // 只记录阶段与批次汇总，不输出按键内容；超时未生成 JSON 时也保留推进位置。
    let started = Instant::now();
    eprintln!("native-probe: fixtures");
    let mut results = Vec::new();
    let mut failed_cases = Vec::new();
    for case in fixtures.cases {
        let mut session = engine.create_session(&case.schema)?;
        reset_options(&mut session)?;
        for (name, enabled) in &case.options {
            session.set_option(name, *enabled)?;
        }
        let mut state = session.snapshot()?;
        let mut commit = String::new();
        for key in case.input.chars() {
            let outcome = session.process_key(key as i32, i32::from(key.is_ascii_uppercase()))?;
            if let Some(text) = outcome.commit {
                commit.push_str(&text);
            }
            state = outcome.state;
        }
        let failures = check(&case, &state, &commit);
        if !failures.is_empty() {
            failed_cases.push(case.id.clone());
        }
        results.push(json!({"id": case.id, "failures": failures, "commit": commit,
            "preedit": state.composition.text, "page_size": state.page_size, "schema_id": state.schema_id,
            "candidates": state.candidates.iter().map(|candidate| json!({"text": candidate.text, "comment": candidate.comment, "label": candidate.label})).collect::<Vec<_>>() }));
        session.close()?;
    }
    eprintln!(
        "native-probe: options-commit-recovery elapsed_ms={}",
        started.elapsed().as_millis()
    );
    // 英文用户词典必须保持启用，防止用关闭学习来绕过 I/O 回归。
    for (schema, expected) in [
        ("tiger", false),
        ("tigress", false),
        ("PY_c", true),
        ("easy_english", true),
    ] {
        if engine.config_bool(schema, "translator/enable_user_dict")? != expected {
            return Err(format!("学习开关变化: {schema}").into());
        }
    }
    if engine.config_int("tiger", "menu/page_size")? != 9 {
        return Err("九候选配置不符".into());
    }
    let mut session = engine.create_session("tiger")?;
    reset_options(&mut session)?;
    session.process_key('u' as i32, 0)?;
    let committed = session.process_key(' ' as i32, 0)?;
    if committed.commit.as_deref() != Some("的") || session.take_commit()?.is_some() {
        return Err("提交丢失或重复消费".into());
    }
    session.set_option("ascii_mode", true)?;
    if session.process_key('a' as i32, 0)?.handled {
        return Err("英文透传按键被吞掉".into());
    }
    session.close()?;
    // 同一会话在错误表达式之后仍可正常计算，避免只验证新建会话而遗漏残留状态。
    for schema in ["tiger", "tigress", "PY_c"] {
        let mut session = engine.create_session(schema)?;
        reset_options(&mut session)?;
        for key in "=sin()".chars() {
            if session.process_key(key as i32, 0)?.commit.is_some() {
                return Err("错误表达式意外提交".into());
            }
        }
        session.clear();
        let mut state = session.snapshot()?;
        for key in "=1+2".chars() {
            state = session.process_key(key as i32, 0)?.state;
        }
        if !state
            .candidates
            .iter()
            .any(|candidate| candidate.text == "1+2=3")
        {
            return Err(format!("计算器错误后未恢复: {schema}").into());
        }
        session.close()?;
    }
    eprintln!(
        "native-probe: session-cycles elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let mut cycle_times = [Duration::ZERO; 5];
    eprintln!("native-probe: process-usage {}", process_usage()?);
    // 所有路径都走同一个会话 guard；主动 close 校验销毁返回值。
    for cycle in 1..=SESSION_CYCLES {
        let step = Instant::now();
        let mut session = engine.create_session("tiger")?;
        cycle_times[0] += step.elapsed();
        let step = Instant::now();
        reset_options(&mut session)?;
        cycle_times[1] += step.elapsed();
        let step = Instant::now();
        session.process_key('u' as i32, 0)?;
        cycle_times[2] += step.elapsed();
        let step = Instant::now();
        session.clear();
        if !session.snapshot()?.composition.text.is_empty() {
            return Err("清屏后仍有组合文本".into());
        }
        cycle_times[3] += step.elapsed();
        let step = Instant::now();
        session.close()?;
        cycle_times[4] += step.elapsed();
        // 每百次已关闭会话才记一条诊断；不减少总次数，不为逐键路径增加日志。
        if cycle % 100 == 0 {
            eprintln!(
                "native-probe: sessions_completed={cycle} elapsed_ms={} cycle_ms={} process={}",
                started.elapsed().as_millis(),
                cycle_timing(&cycle_times),
                process_usage()?
            );
        }
    }
    let mut session = engine.create_session("tiger")?;
    reset_options(&mut session)?;
    eprintln!(
        "native-probe: latency elapsed_ms={}",
        started.elapsed().as_millis()
    );
    let mut samples = Vec::with_capacity(4000);
    for iteration in 0..520 {
        for input in ["u", "uni", "d", "kry"] {
            for key in input.chars() {
                let start = Instant::now();
                let _ = session.process_key(key as i32, 0)?;
                if iteration >= 20 {
                    samples.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            session.clear();
        }
    }
    samples.sort_by(f64::total_cmp);
    session.close()?;
    eprintln!(
        "native-probe: protocol elapsed_ms={}",
        started.elapsed().as_millis()
    );
    check_dispatch(engine)?;
    check_client_delivery(engine)?;
    eprintln!(
        "native-probe: measured elapsed_ms={}",
        started.elapsed().as_millis()
    );
    Ok(
        json!({"cases": results, "failed_cases": failed_cases, "session_cycles": SESSION_CYCLES,
        "commit_once": true, "ascii_passthrough": true, "calculator_recovery": true, "protocol_dispatch": true,
        "session_timing_ms": cycle_timing(&cycle_times), "process_usage": process_usage()?,
        "protocol_client_delivery": true,
        "latency_ms": {"scope": "Rust process_key + owned context/status/commit; no IPC/TSF/UI", "samples": samples.len(),
            "p50": samples[(samples.len() * 50).div_ceil(100) - 1],
            "p95": samples[(samples.len() * 95).div_ceil(100) - 1],
            "p99": samples[(samples.len() * 99).div_ceil(100) - 1], "max": samples.last()} }),
    )
}

fn cycle_timing(times: &[Duration; 5]) -> Value {
    json!({"create": times[0].as_millis(), "options": times[1].as_millis(),
        "key": times[2].as_millis(), "clear_snapshot": times[3].as_millis(), "close": times[4].as_millis()})
}

#[cfg(windows)]
fn process_usage() -> Result<Value> {
    use windows_sys::Win32::{
        Foundation::FILETIME,
        System::Threading::{
            GetCurrentProcess, GetProcessIoCounters, GetProcessTimes, IO_COUNTERS,
        },
    };
    let mut created = FILETIME::default();
    let mut exited = FILETIME::default();
    let mut kernel = FILETIME::default();
    let mut user = FILETIME::default();
    let mut io = IO_COUNTERS::default();
    // SAFETY: 仅查询当前探针自己的计数，不打开其他进程；输出结构均在栈上有效。
    unsafe {
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        ) == 0
            || GetProcessIoCounters(GetCurrentProcess(), &mut io) == 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    let millis = |time: FILETIME| {
        ((u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)) / 10_000
    };
    Ok(json!({"user_ms": millis(user), "kernel_ms": millis(kernel),
        "read_ops": io.ReadOperationCount, "write_ops": io.WriteOperationCount,
        "read_bytes": io.ReadTransferCount, "write_bytes": io.WriteTransferCount}))
}

#[cfg(not(windows))]
fn process_usage() -> Result<Value> {
    Ok(Value::Null)
}

pub fn run(root: &Path, work: &Path, deploy: bool) -> Result<()> {
    let paths = runtime_paths(root, work)?;
    eprintln!("native-probe: validated-workspace");
    let work = work.canonicalize()?;
    let dll = root.join("vendor/librime/dist/lib/rime.dll");
    let start = Instant::now();
    let mut report = if deploy {
        Engine::deploy(&dll, &paths)?;
        json!({"version": tigerrime_engine::VERSION, "deploy_seconds": start.elapsed().as_secs_f64()})
    } else {
        eprintln!("native-probe: initializing-engine");
        let engine = Engine::start(&dll, &paths)?;
        if !matches!(
            Engine::start(&dll, &paths),
            Err(tigerrime_engine::Error::AlreadyInitialized)
        ) {
            return Err("允许了重复初始化".into());
        }
        let fixtures: Fixtures = resources::read_json(&work.join("engine-smoke.json"))?;
        let result = measure(&engine, fixtures)?;
        drop(engine);
        eprintln!("native-probe: finalized");
        result
    };
    let errors = diagnostics(&paths.logs)?;
    report["native_errors"] = json!(errors);
    report["user_data_dir"] = json!(paths.user);
    report["native_log_dir"] = json!(paths.logs);
    fs::write(
        work.join(if deploy { "deploy.json" } else { "engine.json" }),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if !errors.is_empty() {
        return Err("原生诊断存在错误，详见隔离目录中的 JSON 和日志；不能判定为全绿".into());
    }
    if report["failed_cases"]
        .as_array()
        .is_some_and(|cases| !cases.is_empty())
    {
        return Err("功能样例断言失败".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_diagnostics_keep_step_order_and_query_only_own_process() {
        let times = [1, 2, 3, 4, 5].map(Duration::from_millis);
        assert_eq!(
            cycle_timing(&times),
            json!({"create": 1, "options": 2, "key": 3, "clear_snapshot": 4, "close": 5})
        );
        #[cfg(windows)]
        {
            let usage = process_usage().unwrap();
            for field in [
                "user_ms",
                "kernel_ms",
                "read_ops",
                "write_ops",
                "read_bytes",
                "write_bytes",
            ] {
                assert!(usage[field].as_u64().is_some(), "{field}");
            }
        }
    }

    #[test]
    fn fixture_expectations_are_not_silently_ignored() {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../../../fixtures/engine-smoke.json")).unwrap();
        assert_eq!(fixtures.cases.len(), 40);
        for case in fixtures.cases {
            if case.max_candidates == Some(0) {
                assert!(check(&case, &InputState::default(), "").is_empty());
                assert!(!check(&case, &InputState::default(), "unexpected commit").is_empty());
                let mut state = InputState::default();
                state
                    .candidates
                    .push(tigerrime_contracts::Candidate::default());
                assert!(!check(&case, &state, "").is_empty());
                continue;
            }
            assert!(
                !check(&case, &InputState::default(), "").is_empty(),
                "{}",
                case.id
            );
        }
    }

    #[test]
    fn detects_native_errors_not_info() {
        assert_eq!(
            native_errors("I20261021 info\nE20261021 error\nF20261021 fatal"),
            ["E20261021 error", "F20261021 fatal"]
        );
    }

    #[test]
    fn unknown_fixture_expectation_is_an_error() {
        assert!(
            serde_json::from_str::<Case>(r#"{"id":"x","schema":"tiger","input":"u","frist":"的"}"#)
                .is_err()
        );
    }

    #[test]
    fn runtime_name_cannot_choose_an_arbitrary_directory() {
        assert_eq!(
            runtime_name(Path::new("rust-engine-42-12345")).unwrap(),
            "rust-engine-42-12345"
        );
        for value in [
            "Rime",
            "rust-engine-other-user",
            "rust-engine-42-../Rime",
            "rust-engine-42-123-extra",
            "rust-engine-4294967296-1",
        ] {
            assert!(runtime_name(Path::new(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn existing_or_legacy_workspace_is_not_adopted() {
        let root = resources::new_directory(&std::env::temp_dir(), "tigerrime-path-tests").unwrap();
        let work = resources::new_directory(&root.join("artifacts"), "rust-engine").unwrap();
        fs::write(
            work.join("test-workspace"),
            b"TigerRime synthetic data only\n",
        )
        .unwrap();
        assert!(runtime_paths(&root, &work).is_err());
        let existing = root.join("existing-user-data");
        fs::create_dir(&existing).unwrap();
        fs::write(existing.join("test-workspace"), WORKSPACE_MARKER).unwrap();
        assert!(runtime_paths(&root, &existing).is_err());
        assert_eq!(
            fs::read(existing.join("test-workspace")).unwrap(),
            WORKSPACE_MARKER
        );
    }
}
