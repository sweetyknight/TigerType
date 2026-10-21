# Windows 命名管道边界与验证

2026-10-21。本文记录首批真实 Windows 管道传输；后续真实 librime、显式继承启动与 Job 接入见 [进程记录](engine-process.md)。总体进度以 [项目状态](project-status.md) 为准。

## 已实现

`ipc::pipe` 仅在 Windows 编译，复用已锁定的 windows-sys 0.61.2，没有新增版本或第三方依赖。

- `Runtime` 在 EXE 启动时创建一次。所有连接共用一个自定义 Windows 线程池，完成线程上限为 1；每个运行时最多保留 64 个管道对象，包括取消尚未完成的对象。不按连接或按键新建线程。
- 本机名称由当前登录 SID、Windows 会话 ID 和受限标签生成；不接受任意路径、远程主机、分隔符或 NUL。创建时启用 `FILE_FLAG_FIRST_PIPE_INSTANCE`、单实例和 `PIPE_REJECT_REMOTE_CLIENTS`。
- 受保护 DACL 只授权当前登录 SID，读写权限排除 `FILE_CREATE_PIPE_INSTANCE`（与 `FILE_APPEND_DATA` 同值）。不修改用户、目录或其他程序的权限。
- `Peer::from_process` 复制可信的现有进程句柄（如 `Child`），而非稍后凭 PID 重开；双方在传输正文前核对内核报告的进程 ID、Windows 会话 ID、登录 SID，以及该预期进程是否仍存活。不信任消息自报的身份。
- 客户端使用 `SECURITY_IDENTIFICATION`，不给服务端可执行模拟操作的客户端令牌。连接只尝试已存在的本机实例；不存在/忙立即返回错误，不同步启动引擎、不忙轮询或自行重试。
- `accept` / `send_frame` / `receive_frame` / `transact` 使用 OVERLAPPED I/O。连接、每次分片读写和回复解码共用调用方给出的绝对截止时间。
- `transact` 复用 `ClientState`：单在途请求、匹配身份和类型、一次性交付；任何传输错误使客户端状态和管道失效，不重放按键、不接收旧提交。
- `Cancellation` 可跨线程通知 `CancelIoEx`。即使取消发生在提交前后的狭窄竞态中，也会再次取消已提交的操作；最终交付前再次检查取消和截止时间。

## 取消后的所有权

每个操作在固定地址持有自己的 OVERLAPPED、16 KiB 分片缓冲区和管道引用，并给完成回调保留一份 Arc。提交即完成和正常挂起均交给线程池回调；立即失败或 `ERROR_PIPE_CONNECTED` 则撤销通知并收回回调引用。

等待方超时只请求取消并返回，**不等待取消收尾、不释放内核仍可能访问的缓冲区**。回调只向容量为 1 的 channel 投递完成结果，不做 JSON、引擎或 UI 工作，也不阻塞；接收者已退出时直接回收。回调捕获 Rust panic，禁止穿过 Windows ABI。只有最后一个操作引用释放后才关闭注册与管道，线程池在最后引用/回调结束后释放。

如果 Windows 永远不完成某个 I/O，相关对象仍会保留，但受运行时 64 对象上限约束，不靠无限等待或提前释放制造“回收成功”。未来生产监督仍需终止故障进程；本批没有实现该监督器。

### 当前 EXE 限制

完成回调的内存所有权不能自动保证 **DLL 代码**不会卸载。当前 `Runtime::new` 查询回调所在模块，只允许主 EXE，明确拒绝可卸载 DLL。接入 TSF DLL 前必须实现模块驻留/受控卸载协议并验证，不能删除入口检查来绕过。

## 验证

普通 Cargo 测试即运行真实管道测试，不加载 librime：

- 同进程真实管道大段非 BMP / Emoji 回复，覆盖客户端先连接的特殊分支。
- 挂起 accept、无数据读取、只收到半个头部、对端不读造成写入挂起，均能到期返回；测试用完成通知确认操作持有的管道最终被释放。
- 跨线程取消、断连、超长头部、失效后禁止复用、实例冲突及 64 对象容量。
- 直接读回内核 DACL：保护位、唯一允许 ACE、登录 SID、权限掩码均匹配；允许两实例的专用测试管道仍拒绝同登录身份创建第二实例。
- 限时辅助子进程真实往返；同登录 SID 但错误服务端/客户端进程均在正文处理前拒绝。父测试最多等子进程 15 秒，失败后终止并回收。
- x64 和 x86 各自运行以上测试；**尚不是 x86 客户端 → x64 服务端混合配对验收**。辅助进程通过测试环境变量定位父进程，仅为测试启动方式，不能直接作为生产发现协议。

| 命令 | 最终结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过 |
| `cargo test --workspace --locked --offline` | 63 passed / 4 ignored，含文档测试 |
| `cargo test --workspace --locked --offline --target i686-pc-windows-msvc` | 60 passed / 1 ignored，含文档测试 |
| `cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture` | 2 passed，40 样例和 1,000 会话循环、旧协议/分片原生回归保持零错误 |

`pipe_worker` 的 ignored 标记只表示内部子进程入口；实际调用由三个默认父测试限时完成，不把单独 ignored 项算作 passed。

原生成功记录：`artifacts/rust-engine-50452-1792593138712469300`；冷资源目录：`artifacts/rust-resources-50452-1792593116263538600`。活动数据库继续使用源码外的 `TigerRime-tests/rust-engine-50452-1792593138712469300`。此次真实引擎探针仍是内存协议通路，**没有把 librime 接到新管道上**，不能合并成引擎跨进程验收结论。

### 开发过程中的失败与复验

1. 初次编译发现 windows-sys 的 PTP 类型为 `isize`，以及内部字段可见性不匹配；修正后构建通过。Clippy 两处宏内 SAFETY 注释位置不符，也已修正，没有关闭检查。
2. 第一次完整测试中，ACL 负向断言错误地用 `CreateFileW` 申请带 `FILE_APPEND_DATA` 的客户端访问来检验“创建实例”权限，本机返回成功，断言 `客户端意外取得创建实例权限` 失败。**没有扩大 DACL 或删除安全断言**：改用允许两个实例的专用管道，实际调用 `CreateNamedPipeW` 创建第二实例；得到 `ERROR_ACCESS_DENIED (5)`，同时保留读回 DACL 的精确检查。
3. 后续一次沙箱内 x86 完整测试的 `authenticated_child_round_trip_uses_real_process_boundaries` 返回 `Access is denied`，当时未标明具体调用阶段。授权后同命令通过。随后将测试名称从时间戳改为进程内原子序号，排除并行用例名称碰撞，并增加 runtime/bind/spawn/peer/accept/receive 阶段信息；沙箱内三轮管道测试及最终完整 x64/x86 回归通过。**这不能证明原始失败由名称碰撞或沙箱导致，根因仍待复现定位**；没有加入生产重试、测试自动重试或错误忽略。

## 本批验证时未完成的部分

- 尚无产品管理/引擎消息循环、单登录实例发现、可信启动组装、Job Object、重启限流或真正的引擎崩溃恢复。
- 未验证跨账户/登录会话的实际攻击进程、管理员/普通完整性混合宿主、远程 SMB 接入、混合位数端到端链路，以及可卸载 TSF DLL。当前 ACL/身份边界测试不是完整安全审计。
- 测试中的短期限只检验超时行为和回收，不是 TSF 50ms 总预算或 P95/P99 性能验收；未采集长期资源数据。
- 下一批先让 app 的独立管理/引擎模式使用这条通道并加入故障注入，再做 TSF 和候选窗。LUA-002、真实数据目录故障处理及长期验收仍未过门槛，不能接个人词典试用。

以上是首批管道验证的范围；后续已接入受控真实引擎消息循环和 Job 生命周期，见 [进程记录](engine-process.md)；TST-002 的后续修复与完整复验见 [会话超时修复](session-timeout.md)。仍未完成生产管理/监督。
