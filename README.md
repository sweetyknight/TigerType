# TigerRime

Windows Rust 专用虎码单字输入法项目：单字输入、拼音查字学习、简繁候选与原生 Win11Light 外观，不兼容 Rime 生态。

- [实施方案](docs/implementation-plan.md)：批准范围、架构、验收。
- [项目状态](docs/project-status.md)：实际已实现、缺陷和验证；不要把规划当成可用功能。
- [开发约束](AGENTS.md)：代码、数据保护和交付流程。

## 构建和检查
需要 Windows x64、锁定 Rust 1.94.1、MSVC 工具链与 i686-pc-windows-msvc 目标。首次准备缓存可运行 `cargo fetch --locked`；之后离线检查，不自动下载原词表或引擎。

```powershell
cargo run --locked --offline -p tigerrime-app --example check
cargo test --workspace --locked --offline
```

完整资源和真实应用验收另见状态。普通测试使用合成数据，不加载 librime、不安装输入法。

## 离线核心工具

先将批准的原资源按锁定清单准备在本项目 `vendor/tiger`；工具只读取所需的 9 项文本，不自动探测现用输入法目录。

```powershell
# 输出必须为新文件；已有包不会被覆盖。以下路径按本仓库位置填写。
cargo run --locked --offline -p tigerrime-app -- import-resources 'D:\Projects\TigerRime\.cache\tiger-v1.json'
cargo run --locked --offline -p tigerrime-app -- verify-package 'D:\Projects\TigerRime\.cache\tiger-v1.json'
cargo run --locked --offline -p tigerrime-app -- lookup 'D:\Projects\TigerRime\.cache\tiger-v1.json' 'han'
cargo run --locked --offline -p tigerrime-app -- lookup 'D:\Projects\TigerRime\.cache\tiger-v1.json' '漢'
# 核心诊断仅输出 JSON 状态，不向其他应用上屏。
cargo run --locked --offline -p tigerrime-app -- demo-input 'D:\Projects\TigerRime\.cache\tiger-v1.json' 'unid '
# 完整锁定资源单列验收，不包含在普通测试的 passed 中。
cargo test --locked --offline -p tigerrime-app --test locked_resources -- --ignored --nocapture
```

CLI 查询输出第一页；独立查字窗口、候选窗和 TSF 的完成情况以项目状态为准。生成包的摘要用于损坏检测，不是发布者签名；不从不可信来源取得数据包。

## 数据边界
原资源只读并锁定来源/hash；不读取个人词库，不修改现用输入法。正式数据独立使用 `%APPDATA%\TigerRime`，开放情况见状态。

`vendor/`、`.cache/`、`artifacts/`、`target/` 与数据库不入 Git；资源整体授权未确认，仅限个人本地使用，不公开发布。
