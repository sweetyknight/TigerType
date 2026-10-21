//! 使用固定公开样例的原生探针；调用方必须在限时子进程中运行。

use crate::{
    Result,
    resources::{self, Entry, Group, Lock},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    time::Instant,
};
use tigerrime_contracts::InputState;
use tigerrime_engine::{Engine, EnginePaths};

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
    comment_contains: Option<String>,
    comment_nonempty: Option<bool>,
}

pub fn new_workspace(root: &Path) -> Result<PathBuf> {
    resources::verify(root)?;
    let work = resources::new_directory(&root.join("artifacts"), "rust-engine")?;
    for directory in ["shared", "user", "logs"] {
        fs::create_dir(work.join(directory))?;
    }
    let mut tiger: Group = resources::read_json(&root.join("resources/tiger.lock.json"))?;
    tiger.files.retain(|entry| !entry.path.starts_with("01 "));
    resources::copy_entries(&root.join("vendor/tiger"), &work.join("user"), &tiger.files)?;
    let lock: Lock = resources::read_json(&root.join("dependencies.lock.json"))?;
    for (name, group) in [("prelude", lock.prelude), ("essay", lock.essay)] {
        resources::copy_entries(
            &root.join("vendor").join(name),
            &work.join("shared"),
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
        let target = work.join("shared").join(relative);
        fs::create_dir_all(target.parent().ok_or("缺少父目录")?)?;
        fs::copy(source, target)?;
        resources::verify_entry(
            &work.join("shared"),
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
    fs::write(
        work.join("test-workspace"),
        b"TigerRime synthetic data only\n",
    )?;
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

fn diagnostics(logs: &Path) -> Result<Vec<String>> {
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

fn measure(engine: &Engine, fixtures: Fixtures) -> Result<Value> {
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
    for (schema, expected) in [("tiger", false), ("tigress", false), ("PY_c", true)] {
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
    // 所有路径都走同一个会话 guard；主动 close 校验销毁返回值。
    for _ in 0..100 {
        let mut session = engine.create_session("tiger")?;
        reset_options(&mut session)?;
        session.process_key('u' as i32, 0)?;
        session.clear();
        if !session.snapshot()?.composition.text.is_empty() {
            return Err("清屏后仍有组合文本".into());
        }
        session.close()?;
    }
    let mut session = engine.create_session("tiger")?;
    reset_options(&mut session)?;
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
    Ok(
        json!({"cases": results, "failed_cases": failed_cases, "session_cycles": 100,
        "commit_once": true, "ascii_passthrough": true,
        "latency_ms": {"scope": "Rust process_key + owned context/status/commit; no IPC/TSF/UI", "samples": samples.len(),
            "p50": samples[(samples.len() * 50).div_ceil(100) - 1],
            "p95": samples[(samples.len() * 95).div_ceil(100) - 1],
            "p99": samples[(samples.len() * 99).div_ceil(100) - 1], "max": samples.last()} }),
    )
}

pub fn run(root: &Path, work: &Path, deploy: bool) -> Result<()> {
    resources::reject_links(work)?;
    let work = work.canonicalize()?;
    if work.parent() != Some(root.join("artifacts").canonicalize()?.as_path())
        || !work.join("test-workspace").is_file()
    {
        return Err("探针仅允许本项目新建的隔离测试目录".into());
    }
    let paths = EnginePaths {
        shared: work.join("shared"),
        user: work.join("user"),
        logs: work.join("logs"),
    };
    for path in [&paths.shared, &paths.user, &paths.logs] {
        resources::reject_links(path)?;
    }
    let dll = root.join("vendor/librime/dist/lib/rime.dll");
    let start = Instant::now();
    let mut report = if deploy {
        Engine::deploy(&dll, &paths)?;
        json!({"version": tigerrime_engine::VERSION, "deploy_seconds": start.elapsed().as_secs_f64()})
    } else {
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
        result
    };
    let errors = diagnostics(&paths.logs)?;
    report["native_errors"] = json!(errors);
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
    fn fixture_expectations_are_not_silently_ignored() {
        let fixtures: Fixtures =
            serde_json::from_str(include_str!("../../../fixtures/engine-smoke.json")).unwrap();
        assert_eq!(fixtures.cases.len(), 20);
        for case in fixtures.cases {
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
}
