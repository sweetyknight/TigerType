//! 架构和文档门禁参加普通测试；外部命令失败时保留原始 stderr。
#[path = "support/governance.rs"]
mod policy;
use serde_json::{Value, json};
use std::process::Command;
use tigertype_app::{Result, TABLES, project_root};
use tigertype_backend::{import, resources};

#[test]
fn workspace_architecture_and_documentation() -> Result<()> {
    let root = project_root()?;
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked", "--offline"])
        .current_dir(&root)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    policy::architecture(&metadata)?;
    policy::documentation(&root)?;
    let git = Command::new("git")
        .args(["ls-files", "--cached", "-z"])
        .current_dir(&root)
        .output()?;
    assert!(
        git.status.success(),
        "{}",
        String::from_utf8_lossy(&git.stderr)
    );
    for path in std::str::from_utf8(&git.stdout)?
        .split('\0')
        .filter(|p| !p.is_empty())
    {
        policy::tracked_path(path)?;
    }
    let mut expected: Vec<String> = import::SOURCES
        .iter()
        .map(|name| format!("resources/{TABLES}/{name}"))
        .collect();
    let fonts: Value =
        serde_json::from_slice(&std::fs::read(root.join("resources/fonts.lock.json"))?)?;
    for font in fonts["fonts"].as_array().ok_or("字体清单缺少 fonts")? {
        let path = font["path"].as_str().ok_or("字体清单缺少 path")?;
        expected.push(format!("resources/fonts/{path}"));
    }
    expected.push("installer/ChineseSimplified.isl".into());
    let eol = Command::new("git")
        .args(["ls-files", "--eol", "--", "resources", "installer"])
        .current_dir(&root)
        .output()?;
    assert!(
        eol.status.success(),
        "{}",
        String::from_utf8_lossy(&eol.stderr)
    );
    policy::raw_resources(std::str::from_utf8(&eol.stdout)?, &expected)?;
    Ok(())
}

fn metadata() -> Value {
    json!({
        "workspace_members":["core"],
        "packages":[
            {"id":"core","name":"tigertype-contracts","dependencies":[{"name":"serde","rename":"innocent","kind":null,"target":null}]},
            {"id":"serde","name":"serde","dependencies":[]}
        ],
        "resolve":{"nodes":[{"id":"core","deps":[{"name":"innocent","pkg":"serde"}]},{"id":"serde","deps":[]}]}
    })
}
#[test]
fn dependency_policy_detects_alias_platform_dev_build_transitive_cycles_and_unknown_members() {
    assert!(policy::architecture(&metadata()).is_ok());
    for kind in [Value::Null, json!("dev"), json!("build")] {
        let mut bad = metadata();
        bad["packages"][0]["dependencies"][0] = json!({"name":"windows-sys","rename":"safe","kind":kind,"target":"cfg(target_os = \"other\")"});
        assert!(policy::architecture(&bad).is_err());
    }
    let mut bad = metadata();
    bad["packages"][1]["name"] = json!("windows-sys");
    assert!(policy::architecture(&bad).is_err());
    let mut bad = metadata();
    bad["resolve"]["nodes"][1]["deps"] = json!([{"pkg":"core"}]);
    assert!(policy::architecture(&bad).is_err());
    // 码表不再序列化，引擎只依赖 contracts；重新引入 serde 也须先改规则。
    let mut bad = metadata();
    bad["packages"][0]["name"] = json!("tigertype-engine");
    assert!(policy::architecture(&bad).is_err());
    let mut bad = metadata();
    bad["packages"][0]["name"] = json!("tigertype-unapproved");
    assert!(policy::architecture(&bad).is_err());
    let mut bad = metadata();
    bad["packages"][0]["name"] = json!("tigertype-tsf");
    bad["packages"][0]["dependencies"][0]["name"] = json!("tigertype-contracts");
    bad["packages"][1]["name"] = json!("tigertype-engine");
    assert!(policy::architecture(&bad).is_err());
}

/// 成员 origin 经直接依赖 via 传递到达名为 reached 的包。
fn reach(origin: &str, via: &str, reached: &str) -> Value {
    json!({
        "workspace_members":["a"],
        "packages":[
            {"id":"a","name":origin,"dependencies":[{"name":via,"rename":null,"kind":null,"target":null}]},
            {"id":"b","name":via,"dependencies":[]},
            {"id":"c","name":reached,"dependencies":[]}
        ],
        "resolve":{"nodes":[{"id":"a","deps":[{"name":via,"pkg":"b"}]},{"id":"b","deps":[{"pkg":"c"}]},{"id":"c","deps":[]}]}
    })
}
#[test]
fn layering_follows_the_multi_platform_rules() {
    // 允许：backend 经 windows-sys 到达链接支撑；app 组装 backend（及其下的 engine）；平台前端经 frontend 到 contracts。
    for (origin, via, reached) in [
        ("tigertype-backend", "windows-sys", "windows-link"),
        ("tigertype-app", "tigertype-backend", "tigertype-engine"),
        ("tigertype-tsf", "tigertype-frontend", "tigertype-contracts"),
        ("tigertype-imk", "tigertype-frontend", "tigertype-contracts"),
    ] {
        assert!(
            policy::architecture(&reach(origin, via, reached)).is_ok(),
            "{origin} -> {reached}"
        );
    }
    for (origin, via, reached) in [
        // 平台无关层不得到达任何平台包。
        ("tigertype-frontend", "tigertype-contracts", "windows-sys"),
        ("tigertype-frontend", "tigertype-contracts", "objc2"),
        ("tigertype-engine", "tigertype-contracts", "libc"),
        // backend 只许文件原语，且不碰传输、前端会话与界面。
        ("tigertype-backend", "windows-sys", "windows"),
        ("tigertype-backend", "tigertype-contracts", "objc2-app-kit"),
        ("tigertype-backend", "tigertype-ipc", "tigertype-contracts"),
        ("tigertype-backend", "tigertype-engine", "tigertype-ipc"),
        ("tigertype-backend", "tigertype-contracts", "tigertype-ui"),
        (
            "tigertype-backend",
            "tigertype-contracts",
            "tigertype-frontend",
        ),
        // 界面不碰引擎、后台、前端会话与传输。
        ("tigertype-ui", "tigertype-contracts", "tigertype-backend"),
        ("tigertype-ui", "tigertype-contracts", "tigertype-frontend"),
        // 平台前端不碰引擎、后台与界面，彼此不依赖。
        ("tigertype-tsf", "tigertype-ipc", "tigertype-backend"),
        ("tigertype-tsf", "tigertype-frontend", "tigertype-imk"),
        ("tigertype-tsf", "tigertype-frontend", "tigertype-engine"),
        ("tigertype-imk", "tigertype-frontend", "tigertype-ui"),
        ("tigertype-imk", "tigertype-contracts", "tigertype-backend"),
        ("tigertype-ui", "tigertype-contracts", "tigertype-imk"),
        // 只有 app 组装平台前端；组装层也不直接依赖 engine。
        ("tigertype-ipc", "tigertype-contracts", "tigertype-tsf"),
        ("tigertype-app", "tigertype-engine", "tigertype-contracts"),
    ] {
        assert!(
            policy::architecture(&reach(origin, via, reached)).is_err(),
            "{origin} -> {reached}"
        );
    }
}

#[test]
fn ui_cannot_call_engine_even_through_an_alias_or_transitive_dependency() {
    // UI 已加入工作区，必须继续拒绝绕过应用组装层直接查词表。
    let mut allowed = metadata();
    allowed["packages"][0]["name"] = json!("tigertype-ui");
    allowed["packages"][0]["dependencies"][0]["name"] = json!("tigertype-contracts");
    allowed["packages"][1]["name"] = json!("tigertype-contracts");
    assert!(policy::architecture(&allowed).is_ok());
    let mut direct = allowed.clone();
    direct["packages"][0]["dependencies"][0]["name"] = json!("tigertype-engine");
    assert!(policy::architecture(&direct).is_err());
    allowed["packages"][1]["name"] = json!("tigertype-engine");
    assert!(policy::architecture(&allowed).is_err());
}
#[test]
fn documentation_policy_detects_broken_links_and_forbidden_artifacts() -> Result<()> {
    let root =
        resources::new_directory(&std::env::temp_dir(), "tigertype-doc-check")?.canonicalize()?;
    std::fs::write(root.join("existing.md"), "测试目标")?;
    let file = root.join("source.md");
    assert!(policy::links(&root, &file, "[有效](existing.md)").is_ok());
    assert!(policy::links(&root, &file, "[失效](missing.md)").is_err());
    assert!(policy::links(&root, &file, "[空]()").is_err());
    assert!(policy::links(&root, &file, "[网页](https://example.com)").is_ok());
    // 反例：未跟踪、或仍按文本换行规范化的锁定资源都须被拒绝。
    let expected = ["resources/tiger/a.yaml".to_owned()];
    let raw = "i/crlf  w/crlf  attr/-text           \tresources/tiger/a.yaml\n";
    assert!(policy::raw_resources(raw, &expected).is_ok());
    assert!(policy::raw_resources("", &expected).is_err());
    let converted = "i/lf    w/crlf  attr/text=auto eol=lf \tresources/tiger/a.yaml\n";
    assert!(policy::raw_resources(converted, &expected).is_err());
    let other = "i/crlf  w/crlf  attr/-text           \tresources/tiger/a.yaml.bak\n";
    assert!(policy::raw_resources(other, &expected).is_err());
    for name in [
        "vendor/data.txt",
        "target/file.exe",
        "artifacts/report.json",
        ".cache/a",
        "data/a.userdb/b",
        "x.log",
        "x.userdb.txt",
    ] {
        assert!(policy::tracked_path(name).is_err());
    }
    assert!(policy::tracked_path("crates/engine/src/lib.rs").is_ok());
    Ok(())
}

#[test]
fn document_inventory_and_core_unsafe_policy_cannot_be_silently_removed() -> Result<()> {
    let root =
        resources::new_directory(&std::env::temp_dir(), "tigertype-doc-policy")?.canonicalize()?;
    std::fs::create_dir(root.join("docs"))?;
    for name in [
        "README.md",
        "AGENTS.md",
        "docs/implementation-plan.md",
        "docs/project-status.md",
    ] {
        std::fs::write(root.join(name), "# 合成文档")?;
    }
    for name in ["engine", "contracts", "frontend"] {
        let dir = root.join(format!("crates/{name}/src"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("lib.rs"), "#![forbid(unsafe_code)]")?;
    }
    assert!(policy::documentation(&root).is_ok());
    std::fs::write(root.join("crates/engine/src/lib.rs"), "// 删除了编译期约束")?;
    assert!(policy::documentation(&root).is_err());
    std::fs::write(
        root.join("crates/engine/src/lib.rs"),
        "#![forbid(unsafe_code)]",
    )?;
    std::fs::create_dir(root.join("docs/history"))?;
    assert!(policy::documentation(&root).is_err());
    Ok(())
}
