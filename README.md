# TigerRime

面向个人使用的 Windows Rust 输入法前端项目，保留 librime，与小狼毫独立共存。

**当前完成可复用的 Rust 引擎适配、资源工具和原生测试，不是可安装的输入法。**
旧 Python 脚本及测试已删除；构建、资源准备和测试均不需要 Python。
完整范围见 [批准方案](docs/implementation-plan.md)，本批实现见 [Rust 引擎记录](docs/rust-engine.md)。

**当前原生稳定性回归未全通过：** 两次后续复跑在会话反复销毁/创建时捕获 `easy_english.userdb` 的 LevelDB I/O 错误。断言保持失败，根因尚未定位，不能用现用用户词典试用。

## 工作区

| crate | 已实现内容 |
|---|---|
| `contracts` | 拥有所有权的组合、候选和按键结果；无 Windows / librime 依赖 |
| `engine` | 固定 DLL 校验和动态加载、C ABI、单线程生命周期、会话、选项、配置、按键及快照 |
| `app` | Rust 资源准备 / 校验、限时测试子进程、公开样例与诊断结果 |

按依赖需要逐步加入其余 `ipc`、`tsf`、`ui`，不创建未实现的空壳入口。
TSF、生产 IPC、管理进程、候选窗、设置和安装尚未实现，不会注册或切换输入法。

## 构建与测试

需要 Windows x64、Rust 1.94.1、VS 2022 C++ 工具；资源解压需要 `C:\Program Files\7-Zip\7z.exe`。
版本由 `rust-toolchain.toml` / `Cargo.lock` 固定；资源和引擎由两个 JSON 清单锁定。

```powershell
Set-Location 'D:\Projects\TigerRime'
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo fmt --all -- --check

# 首次准备：只读原资源，下载校验后的官方发行包到本项目，不安装输入法。
cargo run --locked -p tigerrime-app -- prepare-resources 'D:\虎码秃版 小狼毫（Win）'
cargo run --locked -p tigerrime-app -- verify-resources

# 显式运行真实 DLL / 冷启动资源测试，使用全新的合成用户目录。
cargo test --locked -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture

# 基础代码的 x86 编译和测试，不代表已有 x86 TSF，也不加载 x64 DLL。
cargo test --workspace --locked --target i686-pc-windows-msvc
```

依赖已缓存时可追加 `--offline`。普通 `cargo test` 不需要下载 librime，也不加载真实 DLL；原生资源测试明确标为 ignored，需要上述显式命令。
默认测试覆盖错误路径、内存所有权、UTF-8 边界、资源保护及测试子进程的超时/失败回收。

## 原生探针与已知问题

真实测试在 `artifacts/rust-engine-*` 中生成 `deploy.json`、`engine.json`、日志及精确输入清单副本。
部署子进程限时 300 秒，输入测试子进程限时 120 秒，超时后终止并回收；父进程不加载引擎。
CLI 的 `probe-deploy` / `probe-engine` 是这些测试使用的内部工作模式，手工运行它们不自带外部监督。

20 个合成样例、100 次会话销毁、提交只消费一次和英文按键透传已验证。
**原计算器仍会在未完成表达式处报错**；`probe-engine` 保留诊断并返回 1。
`native_engine_outputs_and_known_lua_failure` 是精确复现这个已知问题的回归测试，不等于“所有功能无错误”。
该用例只容许这一条已知诊断，任何新错误或输出变化都会失败；安全修复后需同步更新此回归期望。
最近两次复跑确实因额外数据库错误失败；此前成功结果不能替代这一验收缺口。

测量只覆盖 Rust → librime → 拥有所有权的快照，不含 IPC、TSF 或绘制。
原十分钟 Python 采样仅作为 [历史记录](docs/baseline-results.md) 保留，不能冒充 Rust 性能数据；空闲资源与长时压力将使用 Rust 产品进程重测。

## 数据与授权

- `vendor/`、`.cache/`、`artifacts/`、`target/` 不入 Git；不会提交引擎二进制、原词表或用户数据库。
- 原虎码目录及 `D:\Projects\weasel` 只读；测试使用新建合成词典，不探测 `%APPDATA%\Rime`，不创建正式 `%APPDATA%\TigerRime`。
- 原 Lua 与学习开关保持不变，`dz_ci_filter` 仍禁用；不执行网址快捷指令，不安装字体。
- 下载工具仅访问锁定的官方 HTTPS 地址，校验失败不替换旧文件；失败暂存目录保留供诊断，不自动删除个人数据。
- 虎码包整体分发授权未确认，仅供个人本地使用，不公开发布资源或配置。没有遥测、自动更新或常驻工具。
