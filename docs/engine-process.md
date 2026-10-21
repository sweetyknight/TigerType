# 真实引擎进程、受控启动与故障回归

2026-10-21。接续 [Windows 管道](windows-pipe.md)，本批首次把真实 librime 接到管道上。当前仍是合成数据探针链路，不是常驻管理程序或可安装输入法。

本文保留该批开发时的失败与复验记录；后续 TST-002 已定位到适配层同方案重复重建并修复，三轮完整套件通过，见 [会话超时修复](session-timeout.md)。

## 已实现的链路

`不加载 DLL 的父测试 → EngineProcess / ClientState → 认证命名管道 → 独立 EXE 主线程 → Dispatcher → librime`

- `app::job::JobChild` 显式指定可执行文件，正确引用 Windows 参数；以 `CREATE_SUSPENDED` 创建子进程，加入专用 Job 后才恢复主线程。Job 设置 `KILL_ON_JOB_CLOSE`，不被子进程继承。
- 通过 `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` 只继承父进程查询句柄、私有自动复位事件及三个标准 I/O 句柄。父进程句柄只授予查询/同步权限，不把测试中的 PID 环境变量作为身份凭据。
- 子模式校验继承标志、句柄类型和唯一所有权后，使用已有 `Peer` 核对管道对端。原始句柄接管集中在 unsafe 启动边界，并明确要求仅在子进程入口执行一次。
- 父端先完成管道连接，再等待“引擎已初始化”事件；就绪等待同时观察进程退出，不把已退出的子进程继续当成等待中的引擎。启动阶段共享 30 秒预算，不在输入路径启动或读取配置。
- `probe-engine-pipe` 复用隔离工作区校验、锁定 DLL 和现有 `Engine` / `Dispatcher`。Engine 只在子进程主线程创建一次；会话与所有 Rime 调用都在该线程串行执行，I/O 完成线程不调用 Rime。
- 父端每代维护递增请求号，消息仍经过已有长度、身份、代次和回复校验。错误直接返回并关闭在途状态，不重放；终止与回收由输入路径之外的调用方执行。
- 父端正常关闭管道后，子端释放 Dispatcher 的会话，再 finalize 引擎并写诊断结果。异常退出、超时或未显式 finish 时，Job 所有者负责终止和有界回收。
- 新子模式只有受限的合成数据入口；不读取正式 APPDATA、不注册/切换输入法。报告和日志使用 `create_new`，不覆盖已有证据。

## 故障注入与证据边界

默认模式不注入故障。两个故障模式只用于限时原生父测试：

1. `StallAfterCommit`：先让真实引擎产生拥有所有权的提交，再通知父测试命中故障点，随后暂停回复。父端请求应到期、旧通道应拒绝后续请求，最后由 Job 终止子进程。
2. `ExitAfterCommit`：在同一位置通知故障点后，以退出码 23 直接结束进程，不运行 Rust Drop / Rime finalize，模拟突然崩溃。

就绪事件自动复位；父端已经消费初次就绪通知，后续信号才是命中提交后故障点的证据。测试不能仅凭“某处超时”判定故障注入成功。

正常代次与恢复代次要求退出 0、`native_errors: []`。注入代次的非零退出是**预期故障验证**，单独记在 `pipe-regression.json`，不计作正常引擎成功退出；留下的空 `pipe-2.json` / `pipe-3.json` 表示被强制终止或直接退出，不能当成完整报告。

测试最后使用新进程和新代次，重开同一份新建合成数据库，验证提交仍可用且原生错误为空。没有删除失败数据、修改 DLL / 学习开关 / Lua 资源，也没有在同一进程重新初始化 librime。

## 覆盖范围

- 无 DLL 的常规测试：参数引用、非法启动句柄、显式继承与就绪事件、提前退出、Job 终止、所有者 Drop 回收、无效 EXE，以及拒绝覆盖既有日志。x64/x86 均运行。
- 真实管道正常链路：两个会话、提交仅消费一次、清屏/关闭，以及 tiger / tigress / PY_c 三方案中的计算器错误后恢复。
- 真实管道故障链路：提交后卡住、提交后退出、失败后的通道不可复用、新进程/代次重开合成数据库；父测试始终不加载 rime.dll。
- 原有 40 个样例和 1,000 次循环仍在独立原生回归中执行，未改小数量或扩大原生错误白名单。

## 本批首次原生结果

首次完整原生命令为 **2 passed / 1 failed**：冷资源与新增管道测试通过，旧 `native_engine_outputs_without_errors` 在 120 秒限时内未结束。

- 旧探针失败目录：`artifacts/rust-engine-41908-1792595289539820100`。部署退出正常、原生错误为空；引擎阶段超时，stdout/stderr 为空，未形成 `engine.json`，不能确定停在哪个步骤或归因于新管道。
- 新链路成功目录：`artifacts/rust-engine-41908-1792595455672305000`。正常代次处理 48 个请求、退出 0 且错误为空；卡住代次以 1 终止、退出代次返回 23，两次均确认命中故障点且未交付回复；新代次处理 4 个请求并正常退出，错误为空。
- 失败目录保留。随后只为旧探针增加阶段边界和每百次已关闭会话的汇总日志（无按键内容、不逐键或逐会话记录），以同一 120 秒限时和 1,000 次循环复验；不把诊断增强称作根因修复。

## 复验结果与当前门槛

**该批验证时：新增链路通过，但完整原生验收未全绿。** 后续结论见文首的会话超时修复记录。

| 验证 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过；首次新增 Job 测试中一处宏内 SAFETY 注释位置不符，已修正复验 |
| `cargo test --workspace --locked --offline` | 70 passed / 5 ignored，含文档测试 |
| `cargo test --workspace --locked --offline --target i686-pc-windows-msvc` | 67 passed / 1 ignored，含文档测试 |
| 完整显式原生命令，两轮 | 两轮均 2 passed / 1 failed；旧原生探针超时，冷资源与新管道故障回归通过 |
| 单独复验 `native_engine_outputs_without_errors` | 1 passed；40 样例、1,000 次循环及原有恢复断言通过，原生错误为空 |
| 加强第二会话组合断言后单独复验 `native_engine_pipe_faults_and_recovery` | 1 passed；正常及恢复代次退出 0、原生错误为空，两个注入故障均命中并被隔离 |

原生命令均保留锁定资源校验，并在授权后使用新合成目录执行：

```powershell
cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture
cargo test --locked --offline -p tigerrime-app --test native native_engine_outputs_without_errors -- --ignored --exact --nocapture
cargo test --locked --offline -p tigerrime-app --test native native_engine_pipe_faults_and_recovery -- --ignored --exact --nocapture
```

- 第二轮旧探针超时证据：`artifacts/rust-engine-21064-1792595729200823100`。最后阶段为 `session-cycles`，进入时相对 measure 开始约 11.679 秒；不能由此断定是死锁还是处理变慢。
- 第二轮管道成功证据：`artifacts/rust-engine-21064-1792595893175413800`。
- 旧探针单项成功证据：`artifacts/rust-engine-24240-1792596136583145500`。新增汇总显示持续推进到 1,000 次，measure 阶段约 83.287 秒，最终正常退出；命令整体时间包含准备/部署，不能与单引擎子进程的 120 秒预算混用。
- 加强后新管道成功证据：`artifacts/rust-engine-8768-1792596468320295100`，正常 48 个请求、恢复 4 个请求。活动数据均在同名系统临时 `TigerRime-tests` 目录，旧失败目录未删除。

该批针对旧超时的 CPU/I/O 计数查询曾被沙箱拒绝，授权后子进程已退出，未取得计数。当时只确认整套与单项运行结果不稳定，未将原因归给 CPU、杀毒软件、文件 I/O 或新管道代码。没有增加延时/重试、缩减循环、延长预算或调整白名单。后续在子进程内采集计数定位并修复冗余路径，见 [定位与复验](session-timeout.md)。

## 尚未完成

- 本批实现的是受控启动和生命周期基础，不是生产重启监督器。没有自动重启，因此也没有实现每分钟最多三次的自动重启策略、单登录实例发现或完整管理进程。
- Job 尚未设置内存预算：完整功能峰值还未测定，不能任意设置数值后宣称达到资源门槛。也尚未验证父进程突然死亡、子孙进程和所有 Job 失败路径的完整故障矩阵。
- 子模式有 120 秒诊断寿命，普通请求测试限时 5 秒，提交后卡住测试限时 100ms；这些都不是 TSF 50ms 主动等待或端到端延迟验收。Job Drop 的回收等待最多 2 秒，不能放进 TSF 输入回调。
- 数据恢复测试还不能替代英文学习写入中的崩溃、权限/占用失败、部署回退或长期稳定性验收。
- 混合位数宿主、可卸载 TSF DLL 驻留保护、TSF/UI、LUA-002 与长期使用门槛均未完成；不得接入个人词典试用。
