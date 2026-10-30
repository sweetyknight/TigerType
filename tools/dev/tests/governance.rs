//! 架构和文档门禁参加普通测试；外部命令失败时保留原始 stderr。
#[path = "support/governance.rs"]
mod policy;
use serde_json::{Value, json};
use std::process::Command;
use tigertype_backend::{TABLES, import, resources};
use tigertype_dev::{Result, project_root};

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
    policy::layout(&root, &metadata)?;
    // 用实际 metadata 构造反例：不能漏登记组件，也不能把正确包名挂到错误清单路径。
    let mut incomplete = metadata.clone();
    incomplete["workspace_members"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert!(policy::layout(&root, &incomplete).is_err());
    let mut misplaced = metadata.clone();
    let first = misplaced["workspace_members"][0].clone();
    let package = misplaced["packages"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|p| p["id"] == first)
        .unwrap();
    package["manifest_path"] = json!(root.join("Cargo.toml"));
    assert!(policy::layout(&root, &misplaced).is_err());
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
    expected.push("installer/windows/ChineseSimplified.isl".into());
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
    bad["packages"][0]["name"] = json!("tigertype-windows-input-method");
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
    // 允许：backend 经 windows-sys 到达链接支撑；app 组装 backend（及其下的 engine）；平台前端经 input-client 到 contracts。
    for (origin, via, reached) in [
        ("tigertype-backend", "windows-sys", "windows-link"),
        ("tigertype-backend", "libc", "libc"),
        (
            "tigertype-macos-app",
            "tigertype-backend",
            "tigertype-engine",
        ),
        (
            "tigertype-macos-app",
            "tigertype-macos-input-method",
            "objc2-input-method-kit",
        ),
        (
            "tigertype-windows-app",
            "tigertype-backend",
            "tigertype-engine",
        ),
        (
            "tigertype-windows-input-method",
            "tigertype-input-client",
            "tigertype-contracts",
        ),
        (
            "tigertype-macos-input-method",
            "tigertype-input-client",
            "tigertype-contracts",
        ),
    ] {
        assert!(
            policy::architecture(&reach(origin, via, reached)).is_ok(),
            "{origin} -> {reached}"
        );
    }
    for (origin, via, reached) in [
        // 平台无关层不得到达任何平台包。
        (
            "tigertype-input-client",
            "tigertype-contracts",
            "windows-sys",
        ),
        ("tigertype-input-client", "tigertype-contracts", "objc2"),
        (
            "tigertype-macos-app",
            "tigertype-backend",
            "tigertype-windows-app",
        ),
        (
            "tigertype-macos-input-method",
            "tigertype-contracts",
            "tigertype-macos-app",
        ),
        ("tigertype-engine", "tigertype-contracts", "libc"),
        // backend 只许文件原语，且不碰传输、前端会话与界面。
        ("tigertype-backend", "windows-sys", "windows"),
        ("tigertype-backend", "tigertype-contracts", "objc2-app-kit"),
        (
            "tigertype-backend",
            "tigertype-windows-ipc",
            "tigertype-contracts",
        ),
        (
            "tigertype-backend",
            "tigertype-engine",
            "tigertype-windows-ipc",
        ),
        (
            "tigertype-backend",
            "tigertype-contracts",
            "tigertype-windows-ui",
        ),
        (
            "tigertype-backend",
            "tigertype-contracts",
            "tigertype-input-client",
        ),
        // 界面不碰引擎、后台、前端会话与传输。
        (
            "tigertype-windows-ui",
            "tigertype-contracts",
            "tigertype-backend",
        ),
        (
            "tigertype-windows-ui",
            "tigertype-contracts",
            "tigertype-input-client",
        ),
        // 平台前端不碰引擎、后台与界面，彼此不依赖。
        (
            "tigertype-windows-input-method",
            "tigertype-windows-ipc",
            "tigertype-backend",
        ),
        (
            "tigertype-windows-input-method",
            "tigertype-input-client",
            "tigertype-macos-input-method",
        ),
        (
            "tigertype-windows-input-method",
            "tigertype-input-client",
            "tigertype-engine",
        ),
        (
            "tigertype-macos-input-method",
            "tigertype-input-client",
            "tigertype-windows-ui",
        ),
        (
            "tigertype-macos-input-method",
            "tigertype-contracts",
            "tigertype-backend",
        ),
        (
            "tigertype-windows-ui",
            "tigertype-contracts",
            "tigertype-macos-input-method",
        ),
        // 平台 app 与合成工具组装组件；产品组装层不直接依赖 engine。
        (
            "tigertype-windows-ipc",
            "tigertype-contracts",
            "tigertype-windows-input-method",
        ),
        (
            "tigertype-windows-app",
            "tigertype-engine",
            "tigertype-contracts",
        ),
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
    allowed["packages"][0]["name"] = json!("tigertype-windows-ui");
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
    assert!(policy::tracked_path("crates/shared/engine/src/lib.rs").is_ok());
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
    for name in ["engine", "contracts", "input-client", "ui-model"] {
        let dir = root.join(format!("crates/shared/{name}/src"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("lib.rs"), "#![forbid(unsafe_code)]")?;
    }
    assert!(policy::documentation(&root).is_ok());
    std::fs::write(
        root.join("crates/shared/engine/src/lib.rs"),
        "// 删除了编译期约束",
    )?;
    assert!(policy::documentation(&root).is_err());
    std::fs::write(
        root.join("crates/shared/engine/src/lib.rs"),
        "#![forbid(unsafe_code)]",
    )?;
    std::fs::create_dir(root.join("docs/history"))?;
    assert!(policy::documentation(&root).is_err());
    Ok(())
}

/// 目录规则与依赖规则分别校验：包名正确也不能放回旧的混合目录。
#[test]
fn component_paths_match_platform_and_responsibility() {
    use std::path::Path;
    for (name, directory) in policy::COMPONENTS {
        assert!(policy::member_path(name, Path::new(directory)).is_ok());
    }
    for (name, directory) in [
        (
            "tigertype-windows-input-method",
            "crates/shared/input-method",
        ),
        ("tigertype-macos-ui", "crates/windows/ui"),
        ("tigertype-input-client", "crates/frontend"),
        ("tigertype-dev", "crates/windows/app"),
        ("tigertype-unapproved", "crates/shared/unapproved"),
    ] {
        assert!(policy::member_path(name, Path::new(directory)).is_err());
    }
}

#[test]
fn split_components_reject_tool_back_edges_cross_platform_and_shared_platform_dependencies() {
    for (origin, via, reached) in [
        (
            "tigertype-windows-ui",
            "tigertype-ui-model",
            "tigertype-contracts",
        ),
        (
            "tigertype-macos-ui",
            "tigertype-ui-model",
            "tigertype-contracts",
        ),
        (
            "tigertype-dev",
            "tigertype-windows-app",
            "tigertype-backend",
        ),
        ("tigertype-dev", "tigertype-macos-ui", "objc2"),
    ] {
        assert!(policy::architecture(&reach(origin, via, reached)).is_ok());
    }
    for (origin, via, reached) in [
        (
            "tigertype-windows-app",
            "tigertype-contracts",
            "tigertype-dev",
        ),
        (
            "tigertype-windows-app",
            "tigertype-contracts",
            "tigertype-macos-ui",
        ),
        (
            "tigertype-macos-ui",
            "tigertype-contracts",
            "tigertype-windows-ui",
        ),
        ("tigertype-ui-model", "tigertype-contracts", "windows-sys"),
        ("tigertype-ui-model", "tigertype-contracts", "objc2"),
        (
            "tigertype-backend",
            "tigertype-engine",
            "tigertype-windows-ui",
        ),
        (
            "tigertype-windows-input-method",
            "tigertype-input-client",
            "tigertype-ui-model",
        ),
        (
            "tigertype-macos-ui",
            "tigertype-ui-model",
            "tigertype-engine",
        ),
    ] {
        assert!(
            policy::architecture(&reach(origin, via, reached)).is_err(),
            "{origin} -> {reached}"
        );
    }
    // 产品的测试/构建依赖同样不能借开发工具形成 app -> dev -> app 环。
    for kind in [Value::Null, json!("dev"), json!("build")] {
        let mut bad = metadata();
        bad["packages"][0]["name"] = json!("tigertype-windows-app");
        bad["packages"][0]["dependencies"][0] = json!({"name":"tigertype-dev", "rename":"helper", "kind":kind, "target":"cfg(target_os = \"macos\")"});
        assert!(policy::architecture(&bad).is_err());
    }
}
