//! 可执行边界策略；基于真实包名/解析图，而不是可被别名绕开的 import 字符串。
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

/// 精确的目录与包名对应表；分组目录不是 crate，不接受旧路径或占位成员。
pub const COMPONENTS: &[(&str, &str)] = &[
    ("tigertype-contracts", "crates/shared/contracts"),
    ("tigertype-engine", "crates/shared/engine"),
    ("tigertype-input-client", "crates/shared/input-client"),
    ("tigertype-ui-model", "crates/shared/ui-model"),
    ("tigertype-backend", "crates/shared/backend"),
    ("tigertype-windows-ipc", "crates/windows/ipc"),
    ("tigertype-windows-ui", "crates/windows/ui"),
    (
        "tigertype-windows-input-method",
        "crates/windows/input-method",
    ),
    ("tigertype-windows-app", "crates/windows/app"),
    ("tigertype-macos-input-method", "crates/macos/input-method"),
    ("tigertype-macos-ui", "crates/macos/ui"),
    ("tigertype-dev", "tools/dev"),
];
/// 直接依赖含 dev/build/平台条件；不能用别名、测试辅助或空壳绕开边界。
const RULES: &[(&str, &[&str])] = &[
    ("tigertype-contracts", &["serde"]),
    ("tigertype-engine", &["tigertype-contracts"]),
    ("tigertype-input-client", &["tigertype-contracts"]),
    ("tigertype-ui-model", &["tigertype-contracts"]),
    (
        "tigertype-backend",
        &[
            "tigertype-contracts",
            "tigertype-engine",
            "serde",
            "serde_json",
            "sha2",
            "toml",
            "windows-sys",
        ],
    ),
    (
        "tigertype-windows-ipc",
        &["tigertype-contracts", "serde", "serde_json", "windows-sys"],
    ),
    (
        "tigertype-windows-ui",
        &["tigertype-contracts", "tigertype-ui-model", "windows-sys"],
    ),
    (
        "tigertype-windows-input-method",
        &[
            "tigertype-contracts",
            "tigertype-input-client",
            "tigertype-windows-ipc",
            "windows",
            "windows-core",
        ],
    ),
    (
        "tigertype-windows-app",
        &[
            "tigertype-contracts",
            "tigertype-backend",
            "tigertype-windows-ipc",
            "tigertype-windows-ui",
            "serde",
            "serde_json",
            "windows-sys",
        ],
    ),
    (
        "tigertype-macos-input-method",
        &[
            "tigertype-contracts",
            "tigertype-input-client",
            "objc2",
            "objc2-foundation",
            "objc2-app-kit",
        ],
    ),
    (
        "tigertype-macos-ui",
        &[
            "tigertype-contracts",
            "tigertype-ui-model",
            "objc2",
            "objc2-foundation",
            "objc2-app-kit",
            "dispatch2",
        ],
    ),
    (
        "tigertype-dev",
        &[
            "tigertype-contracts",
            "tigertype-backend",
            "tigertype-windows-app",
            "tigertype-windows-ui",
            "tigertype-macos-input-method",
            "tigertype-macos-ui",
            "serde_json",
            "windows-sys",
            "objc2",
            "objc2-foundation",
            "objc2-app-kit",
        ],
    ),
];
/// 候选窗属于输入法，不可经共享 UI 模型或其他平台借道依赖引擎。
const FRONTENDS: &[&str] = &[
    "tigertype-windows-input-method",
    "tigertype-macos-input-method",
];
const VIEWS: &[&str] = &[
    "tigertype-ui-model",
    "tigertype-windows-ui",
    "tigertype-macos-ui",
];
const PURE: &[&str] = &[
    "tigertype-contracts",
    "tigertype-engine",
    "tigertype-input-client",
    "tigertype-ui-model",
];
/// 平台包：系统绑定、Objective-C 运行时与 libc。
fn platform(name: &str) -> bool {
    name == "windows"
        || name.starts_with("windows-")
        || name.starts_with("windows_")
        || name == "winapi"
        || name.starts_with("objc2")
        || name == "block2"
        || name == "dispatch2"
        || name.starts_with("core-foundation")
        || name == "libc"
}
/// backend 只许文件原语所需的 windows-sys（及其链接支撑）与 libc，不得到达界面或 COM 绑定。
fn file_primitive(name: &str) -> bool {
    matches!(name, "windows-sys" | "windows-link" | "libc")
        || name.starts_with("windows-targets")
        || name.starts_with("windows_")
}
/// 检查传递可达性，而不仅是 Cargo.toml 的直接引用。
fn forbidden(origin: &str, reached: &str) -> Option<&'static str> {
    let project_platform =
        reached.starts_with("tigertype-windows-") || reached.starts_with("tigertype-macos-");
    if PURE.contains(&origin) && (platform(reached) || project_platform) {
        return Some("纯共享层不得到达平台包");
    }
    if origin != "tigertype-dev" && reached == "tigertype-dev" {
        return Some("产品不得反向依赖开发工具");
    }
    if (origin.starts_with("tigertype-windows-") && reached.starts_with("tigertype-macos-"))
        || (origin.starts_with("tigertype-macos-") && reached.starts_with("tigertype-windows-"))
    {
        return Some("两平台不能互相依赖");
    }
    if origin == "tigertype-backend" && platform(reached) && !file_primitive(reached) {
        return Some("backend 只许文件原语平台包");
    }
    if !matches!(origin, "tigertype-dev" | "tigertype-windows-app")
        && (reached == "tigertype-backend" || FRONTENDS.contains(&reached))
    {
        return Some("只有平台组装与开发工具接入后台和输入法");
    }
    if origin == "tigertype-backend"
        && (project_platform || VIEWS.contains(&reached) || reached == "tigertype-input-client")
    {
        return Some("共享后台不得依赖传输、界面或宿主逻辑");
    }
    if VIEWS.contains(&origin)
        && matches!(
            reached,
            "tigertype-engine" | "tigertype-input-client" | "tigertype-windows-ipc"
        )
    {
        return Some("查字/设置视图只消费模型与契约");
    }
    if FRONTENDS.contains(&origin) && (reached == "tigertype-engine" || VIEWS.contains(&reached)) {
        return Some("输入法不依赖引擎或查字/设置视图");
    }
    None
}
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
            if current != origin
                && let Some(reason) = forbidden(name, current_name)
            {
                return Err(format!(
                    "传递架构违规（{reason}）：{name} 可达 {current_name}"
                ));
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

/// 路径与包名必须一致，防止平台源码又混回共享目录。
pub fn member_path(name: &str, directory: &Path) -> Result<()> {
    let expected = COMPONENTS
        .iter()
        .find(|(known, _)| *known == name)
        .ok_or_else(|| format!("未批准的工作区成员 {name}"))?
        .1;
    if directory != Path::new(expected) {
        return Err(format!(
            "组件目录不符：{name} 应位于 {expected}，实际 {}",
            directory.display()
        ));
    }
    Ok(())
}
pub fn layout(root: &Path, metadata: &Value) -> Result<()> {
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let members = array(metadata, "workspace_members")?;
    if members.len() != COMPONENTS.len() {
        return Err("工作区须完整登记批准的有实现组件".into());
    }
    for id in members {
        let package = array(metadata, "packages")?
            .iter()
            .find(|p| p["id"] == *id)
            .ok_or("成员缺少 package")?;
        let manifest = Path::new(string(package, "manifest_path")?)
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let relative = manifest
            .parent()
            .ok_or("清单没有父目录")?
            .strip_prefix(&root)
            .map_err(|_| "组件目录越过工作区")?;
        member_path(string(package, "name")?, relative)?;
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
    for name in ["contracts", "engine", "input-client", "ui-model"] {
        let path = root.join(format!("crates/shared/{name}/src/lib.rs"));
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
