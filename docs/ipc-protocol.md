# 阶段 2：有界协议、在途请求与串行会话调度

2026-10-21。本文记录有界编解码、分片收帧、客户端在途状态与引擎侧调度。后续已有 [Windows 管道传输](windows-pipe.md) 和 [受控真实引擎进程](engine-process.md)，尚非完整产品链路；当前总体状态见 [项目状态](project-status.md)。

## 实现范围

- `contracts` 定义拥有所有权的 `Route`、`Request`、`Response` 与错误码，复用已有 `InputState` / `KeyOutcome`；仅增加工作区已锁定的 serde 依赖。
- `ipc` 的协议/状态模块负责发送前和接收后的验证、受限序列化、分片组装、回复身份匹配及客户端在途状态；这些模块不调用 Windows 或引擎。后续 `pipe` 模块单独条件依赖 Windows，整个 crate 仍不依赖引擎。
- `app::dispatch::Dispatcher` 借用现有 `Engine`，在其专用线程上执行创建、按键、清屏和关闭，不产生额外线程或初始化另一个引擎。
- 原资源、Lua 补丁、40 个功能样例、学习开关及 DLL 锁定值不变。测试父进程仍不加载 DLL。

## 帧格式与边界

固定 12 字节头，整数均为小端：

| 偏移 | 字段 |
|---|---|
| 0..4 | ASCII `TRIM` |
| 4..6 | u16 协议版本，当前为 1 |
| 6..8 | u16 保留位，必须为 0 |
| 8..12 | u32 UTF-8 JSON 正文长度，不含头，1 至 1,048,576 字节 |

传输层接入时必须先读头并调用 `frame_len`，然后才按验证过的长度分配正文；`FrameAssembler` 已实现先收头校验、再按实际分片增长正文。`decode` 只接收完整的一帧，拒绝截断或拼接多帧。`encode` 在写入过程中限制长度，不先生成任意大的正文。

- 文本上限沿用引擎的每字段 64 KiB，候选页最多 100 项；整帧预算可能先于单字段预算耗尽，超限返回错误而非截断候选或提交。
- 检查 UTF-8 组合位置、选择区间、页大小及高亮索引。页码不超过 u32；固定线格式和数值边界在 x64/x86 使用同一组测试，不传递本机内存布局。
- JSON 必填字段、未知命令、重复字段和超范围数字均拒绝；方案标识限 1–64 字节 ASCII 字母、数字、下划线、连字符，不接受路径。
- `keycode` / `modifiers` 保持 librime 的 i32 X11 语义，Windows 键盘映射不在本批范围内。

## 身份和调度规则

这是**单一可信管理端 → 引擎端**的协议，不直接给每个宿主各开一个 Dispatcher。后续管理端须将多个宿主身份映射为这个通道内的会话编号。

1. `generation`、`request_id`、`session_id` 都为非零 u64。管理端负责换代，并在每一代中递增分配请求和会话编号；计数到顶不能回绕复用。
2. 旧代次在执行前拒绝，不推进当前代次序号。当前代次请求序号必须严格递增；即便业务请求被拒绝，其序号也已消耗，不能重放。
3. 新会话编号必须大于此前尝试创建的编号。关闭或容量拒绝后也不复用，避免迟到请求作用于另一会话。最多保留 64 个活动会话，超限显式返回 `SessionLimit`。
4. 回复回显三个编号。`decode_response` 仅作无状态身份和类型校验；客户端使用下述 `ClientState` 维护在途状态，在成功/超时后移除请求，不继续使用旧请求接收提交。
5. 引擎执行或回复编码失败时返回原始错误、清理会话并停用 Dispatcher；之后只回复 `Unavailable`，不重新执行或重放按键。传输层接入后，对外部写失败、断连和超时也必须终止当前代次。
6. Dispatcher 的借用和编译失败测试保留 `Engine` / `Session` 的线程亲和；所有退出路径由现有 Session guard 释放。进程级故障恢复仍必须重启引擎，不得在同一进程再次初始化 librime。

## 分片与客户端在途状态

- `FrameAssembler` 每个实例只交付一帧，允许头和正文按任意字节切分。未知长度时最多先复制 12 字节，不按不可信长度预分配；错误头、超限、尾随字节、拼接多帧会使实例失效并释放缓存，不猜测边界或偷偷跳过字节。
- `ClientState::begin(request, deadline)` 同一时刻只允许一个在途请求，返回待发送帧；第二个请求返回 `Busy`，不覆盖第一个。正常交付后才恢复空闲，序号仍必须递增，不能回绕或重放。
- `deadline` 是调用方在连接前建立的绝对 `Instant`，连接/发送/收帧只能共用 `remaining()`。接收分片、解码和校验不会续期，编码和解码后再次检查到期；在途请求过期后关闭，禁止用新的时间预算继续旧请求。
- `receive` 仅在完整、身份和类型均匹配、未到期时返回一次回复；之后空闲状态收到重复回复、下一请求收到上一请求的回复，均关闭通道而不交付。等待时只返回 `None`，不自行循环、等待或重试。
- `cancel()` 丢弃在途请求与部分帧并永久关闭该状态对象；随后的合法迟到提交也返回 `Closed`。传输层必须在 EOF、写失败、取消或断连时通知它，恢复/换代由 `app` 决定。
- **ClientState 本身不是内核取消实现。** 其 `cancel()` 不会调用 `CancelIoEx`。后续已新增的 `pipe::Cancellation` 才执行系统取消，并持有 OVERLAPPED/缓冲区直到完成；两者由管道 `transact` 在失败时联动，具体边界见 [Windows 管道记录](windows-pipe.md)。

## 验证口径

- 常规协议测试：固定字节 x64/x86 互通格式、全部命令/回复及非 BMP / Emoji 所有权、逐字节截断、错误头/版本/长度、畸形 JSON、字段边界、编码膨胀与回复错配。
- 调度准入测试：重复/倒序请求、旧代次、计数到顶、会话容量、关闭后复用、拒绝后的序号消费；这些测试不加载 DLL。
- 原生探针经完整帧往返验证两个会话隔离、提交只消费一次、清屏/关闭、重复提交请求与旧代次拒绝，结果单列 `protocol_dispatch`。沿用 300 秒部署 / 120 秒引擎子进程限制，活动数据库仍为源码外新建合成目录。
- 分片/客户端测试覆盖所有拆分位置、逐字节输入、最大帧、超大声明长度、Busy 不覆盖、同一截止时间剩余量、到期后完整回复、取消后剩余分片、重复/错配回复及序号耗尽。不使用 sleep 推断时间性能。
- 原生探针单列 `protocol_client_delivery`：请求按 3 字节、回复按 7 字节分片；交付第一次真实提交后，确认引擎已产生第二次提交，再取消客户端并要求拒绝该结果。诊断请求期限为 5 秒，仅验证逻辑，不是 50ms 或跨进程延迟测量。
- 原有 40 个功能断言、1,000 次会话循环、Lua 错误后恢复与零原生错误要求不降低。原有延迟采样仍只计 Rust → librime，不包含协议回归。

### 首批协议/调度验证记录

| 命令 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过；首轮故意构造倒置区间的测试写法触发 Clippy，改为显式起止变量后复验通过 |
| `cargo test --workspace --locked --offline` | 42 passed / 3 ignored，含 2 个编译失败文档测试 |
| `cargo test --workspace --locked --offline --target i686-pc-windows-msvc` | 39 passed，含文档测试；不加载 x64 DLL |
| `cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture` | 授权重跑后 2 passed，40 样例、1,000 会话循环及协议调度均通过，原生错误为空 |

原生命令在沙箱内首轮两项都返回 Win32 `Access is denied`，未视为通过；在授权后以相同命令执行，未绕过资源校验或改动数据权限。冷资源准备报告目录为 `artifacts/rust-resources-48856-1792590934015763100`；原生成功证据为 `artifacts/rust-engine-48856-1792590955813117600` 中的 `deploy.json`、`engine.json` 及 stdout/stderr。活动数据位于系统临时目录 `TigerRime-tests/rust-engine-48856-1792590955813117600`，未删除旧失败证据。

### 后续分片/在途状态验证记录

同日继续实现后：

| 命令 | 结果 |
|---|---|
| `cargo fmt --all -- --check` | 通过 |
| `cargo clippy --workspace --all-targets --locked --offline -- -D warnings` | 通过；首轮新增探针嵌套分支触发 `collapsible_if`，合并条件后复验通过 |
| `cargo test --workspace --locked --offline` | 52 passed / 3 ignored，含 2 个编译失败文档测试 |
| `cargo test --workspace --locked --offline --target i686-pc-windows-msvc` | 49 passed，含文档测试 |
| `cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture` | 沙箱外授权执行，2 passed；`protocol_client_delivery` 为 true，原生错误为空 |

本次冷资源目录为 `artifacts/rust-resources-36632-1792591696310992600`，原生成功证据在 `artifacts/rust-engine-36632-1792591714500676400`。活动数据仍在系统临时目录 `TigerRime-tests/rust-engine-36632-1792591714500676400`；不删除或覆盖上一批证据。

本节保留前两批验证结果；后续的管道 ACL/身份、实际 I/O 和取消测试见 [Windows 管道记录](windows-pipe.md)。产品消息循环、监督、TSF/UI、50ms 总预算和长期验收仍未完成，不能连接个人词典试用。
