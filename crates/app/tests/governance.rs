//! 架构和文档门禁参加普通测试；外部命令失败时保留原始 stderr。
#[path = "support/governance.rs"]
mod policy;
use serde_json::{Value, json};
use std::process::Command;
use tigerrime_app::{Result, project_root, resources};

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
    Ok(())
}

fn metadata() -> Value {
    json!({
        "workspace_members":["core"],
        "packages":[
            {"id":"core","name":"tigerrime-engine","dependencies":[{"name":"serde","rename":"innocent","kind":null,"target":null}]},
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
    let mut bad = metadata();
    bad["packages"][0]["name"] = json!("tigerrime-unapproved");
    assert!(policy::architecture(&bad).is_err());
    let mut bad = metadata();
    bad["packages"][0]["name"] = json!("tigerrime-tsf");
    bad["packages"][0]["dependencies"][0]["name"] = json!("tigerrime-contracts");
    bad["packages"][1]["name"] = json!("tigerrime-engine");
    assert!(policy::architecture(&bad).is_err());
}

#[test]
fn ui_cannot_call_engine_even_through_an_alias_or_transitive_dependency() {
    // UI 已加入工作区，必须继续拒绝绕过应用组装层直接查词表。
    let mut allowed = metadata();
    allowed["packages"][0]["name"] = json!("tigerrime-ui");
    allowed["packages"][0]["dependencies"][0]["name"] = json!("tigerrime-contracts");
    allowed["packages"][1]["name"] = json!("tigerrime-contracts");
    assert!(policy::architecture(&allowed).is_ok());
    let mut direct = allowed.clone();
    direct["packages"][0]["dependencies"][0]["name"] = json!("tigerrime-engine");
    assert!(policy::architecture(&direct).is_err());
    allowed["packages"][1]["name"] = json!("tigerrime-engine");
    assert!(policy::architecture(&allowed).is_err());
}
#[test]
fn documentation_policy_detects_broken_links_and_forbidden_artifacts() -> Result<()> {
    let root =
        resources::new_directory(&std::env::temp_dir(), "tigerrime-doc-check")?.canonicalize()?;
    std::fs::write(root.join("existing.md"), "测试目标")?;
    let file = root.join("source.md");
    assert!(policy::links(&root, &file, "[有效](existing.md)").is_ok());
    assert!(policy::links(&root, &file, "[失效](missing.md)").is_err());
    assert!(policy::links(&root, &file, "[空]()").is_err());
    assert!(policy::links(&root, &file, "[网页](https://example.com)").is_ok());
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
        resources::new_directory(&std::env::temp_dir(), "tigerrime-doc-policy")?.canonicalize()?;
    std::fs::create_dir(root.join("docs"))?;
    for name in [
        "README.md",
        "AGENTS.md",
        "docs/implementation-plan.md",
        "docs/project-status.md",
    ] {
        std::fs::write(root.join(name), "# 合成文档")?;
    }
    for name in ["engine", "contracts"] {
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
