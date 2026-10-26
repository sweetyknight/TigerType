//! 可执行边界策略；基于真实包名/解析图，而不是可被别名绕开的 import 字符串。
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

const RULES: &[(&str, &[&str])] = &[
    ("tigertype-contracts", &["serde"]),
    ("tigertype-engine", &["tigertype-contracts"]),
    (
        "tigertype-ipc",
        &["tigertype-contracts", "serde_json", "windows-sys"],
    ),
    ("tigertype-ui", &["tigertype-contracts", "windows-sys"]),
    (
        "tigertype-tsf",
        &[
            "tigertype-contracts",
            "tigertype-ipc",
            "windows-sys",
            "windows",
            "windows-core",
        ],
    ),
    (
        "tigertype-app",
        &[
            "tigertype-contracts",
            "tigertype-engine",
            "tigertype-ipc",
            "tigertype-ui",
            "serde",
            "serde_json",
            "sha2",
            "toml",
            "windows-sys",
        ],
    ),
];
type Result<T> = std::result::Result<T, String>;
fn array<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>> {
    value[key]
        .as_array()
        .ok_or_else(|| format!("metadata 缺少数组 {key}"))
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("metadata 缺少字符串 {key}"))
}
pub fn architecture(metadata: &Value) -> Result<()> {
    let packages = array(metadata, "packages")?;
    let by_id: BTreeMap<_, _> = packages
        .iter()
        .map(|p| Ok((string(p, "id")?, p)))
        .collect::<Result<_>>()?;
    let members = array(metadata, "workspace_members")?;
    if members.is_empty() {
        return Err("工作区不得为空".into());
    }
    for id in members {
        let id = id.as_str().ok_or("成员编号不是字符串")?;
        let package = by_id.get(id).ok_or("成员缺少 package")?;
        let name = string(package, "name")?;
        let allowed = RULES
            .iter()
            .find(|(known, _)| *known == name)
            .ok_or_else(|| format!("未批准的工作区成员 {name}"))?
            .1;
        for dependency in array(package, "dependencies")? {
            // 不按 kind/target 过滤，dev、build、非本机条件依赖也必须遵守。
            let actual = string(dependency, "name")?;
            if !allowed.contains(&actual) {
                return Err(format!(
                    "架构违规：{name} -> {actual}（含别名/平台/测试依赖）"
                ));
            }
        }
    }
    let nodes = array(&metadata["resolve"], "nodes")?;
    let graph: BTreeMap<_, _> = nodes
        .iter()
        .map(|node| {
            let edges = array(node, "deps")?
                .iter()
                .map(|edge| string(edge, "pkg"))
                .collect::<Result<Vec<_>>>()?;
            Ok((string(node, "id")?, edges))
        })
        .collect::<Result<_>>()?;
    for id in members {
        let origin = id.as_str().ok_or("成员编号非法")?;
        let name = string(by_id[origin], "name")?;
        let mut pending = vec![origin];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            if !seen.insert(current) {
                continue;
            }
            let current_name = string(by_id.get(current).ok_or("解析图引用未知包")?, "name")?;
            let forbidden = matches!(
                (name, current_name),
                ("tigertype-tsf", "tigertype-engine" | "tigertype-ui")
                    | (
                        "tigertype-ui",
                        "tigertype-engine" | "tigertype-tsf" | "tigertype-ipc"
                    )
            );
            let platform_core = matches!(name, "tigertype-contracts" | "tigertype-engine")
                && (current_name == "windows"
                    || current_name.starts_with("windows-")
                    || current_name == "winapi");
            if forbidden || platform_core {
                return Err(format!("传递架构违规：{name} 可达 {current_name}"));
            }
            if ["librime", "rime", "mlua", "rlua", "opencc", "libloading"].contains(&current_name) {
                return Err(format!("产品依赖包含退役运行库 {current_name}"));
            }
            for edge in graph.get(current).ok_or("解析图缺少节点")? {
                if *edge == origin {
                    return Err(format!("依赖环回到 {name}"));
                }
                pending.push(edge);
            }
        }
    }
    Ok(())
}

pub fn tracked_path(path: &str) -> Result<()> {
    let path = path.replace('\\', "/").to_lowercase();
    if path.split('/').any(|part| {
        ["vendor", ".cache", "artifacts", "target", ".git"].contains(&part)
            || part.ends_with(".userdb")
    }) || path.ends_with(".log")
        || path.ends_with(".userdb.txt")
    {
        return Err(format!("禁止入库的资源/产物：{path}"));
    }
    Ok(())
}

/// 锁定资源以原始字节入库才能换机重新打包：`git ls-files --eol` 中每个应有路径都须已跟踪且为 attr/-text，
/// 否则添加时 CRLF 被改写，本机工作区仍正确、换机检出后 SHA256 才不符。
pub fn raw_resources(listing: &str, expected: &[String]) -> Result<()> {
    for path in expected {
        let info = listing
            .lines()
            .find_map(|line| line.split_once('\t').filter(|(_, p)| p == path))
            .ok_or_else(|| format!("锁定资源未入库：{path}"))?
            .0;
        if !info.split_whitespace().any(|field| field == "attr/-text") {
            return Err(format!("锁定资源须标记 -text，禁止换行转换：{path}"));
        }
    }
    Ok(())
}

pub fn links(root: &Path, file: &Path, text: &str) -> Result<()> {
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        // shortcut: 只解析仓库采用的内联文件链接；引入引用式链接时须扩展解析并补反例。
        for tail in line.split("](").skip(1) {
            let end = tail.find(')').ok_or("Markdown 链接缺少右括号")?;
            let target = &tail[..end];
            if target.starts_with("https://")
                || target.starts_with("http://")
                || target.starts_with("mailto:")
                || target.starts_with('#')
            {
                continue;
            }
            let path = target.split('#').next().unwrap().trim_matches(['<', '>']);
            if path.is_empty() {
                return Err("空文档链接".into());
            }
            let resolved = file.parent().ok_or("文档没有父目录")?.join(path);
            if !resolved.exists() {
                return Err(format!("失效链接：{} -> {target}", file.display()));
            }
            if !resolved
                .canonicalize()
                .map_err(|e| e.to_string())?
                .starts_with(root)
            {
                return Err(format!("文档链接越过仓库：{target}"));
            }
        }
    }
    Ok(())
}

pub fn documentation(root: &Path) -> Result<()> {
    let main = [
        "README.md",
        "AGENTS.md",
        "docs/implementation-plan.md",
        "docs/project-status.md",
    ];
    for name in main {
        let path = root.join(name);
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        links(root, &path, &text)?;
    }
    for item in fs::read_dir(root.join("docs")).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        if !["implementation-plan.md", "project-status.md"]
            .iter()
            .any(|name| path.file_name().is_some_and(|n| n == *name))
        {
            return Err(format!(
                "新增主文档须先明确职责并更新清单：{}",
                path.display()
            ));
        }
    }
    for name in ["contracts", "engine"] {
        let path = root.join(format!("crates/{name}/src/lib.rs"));
        let source = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        if !source
            .lines()
            .any(|line| line.trim() == "#![forbid(unsafe_code)]")
        {
            return Err(format!("{name} 必须由编译器禁止 unsafe"));
        }
    }
    Ok(())
}
