# TST-002：会话循环超时的定位与修复

2026-10-21。历史失败见 [引擎进程记录](engine-process.md)，当前验收以 [项目状态](project-status.md) 为准。

## 定位方法与边界

没有延长 120 秒引擎子进程限制，没有减少 1,000 次循环，没有更换 DLL、修改 Lua/学习开关、清除失败数据库或增加错误白名单。

在原循环中分别累计创建、重置选项、按键、清屏/快照、销毁耗时；每百次汇总当前探针自身的 `GetProcessTimes` / `GetProcessIoCounters`。所有循环仍真实创建和销毁 Rime 会话，不使用会话池冒充循环次数。报告新增 `session_timing_ms` / `process_usage`；阶段日志不包含按键内容。

这些计数是整个引擎探针进程的累计值，含诊断输出和 Rime 内部工作，不是只针对某个文件或单个线程的采样。循环前后取差，比较使用完全相同诊断格式的修复前后运行。

## 具体代码问题

Rust `Engine::create_session` 原先总是：

1. 调用 librime `create_session`。
2. 无条件调用 `select_schema`，即使新会话已经使用请求的方案。

锁定版本 1.17.0 对应提交 `33e78140250125871856cdc5b42ddc6a5fcd3cd4` 的官方源码说明第二步不是无副作用的检查：

- [service.cc](https://github.com/rime/librime/blob/33e78140250125871856cdc5b42ddc6a5fcd3cd4/src/rime/service.cc)：创建 Session 时已经构造 Engine。
- [engine.cc](https://github.com/rime/librime/blob/33e78140250125871856cdc5b42ddc6a5fcd3cd4/src/rime/engine.cc)：构造时已初始化默认方案、组件和选项；`ApplySchema` 会清理上下文并重新初始化组件/选项。
- [rime_api_impl.h](https://github.com/rime/librime/blob/33e78140250125871856cdc5b42ddc6a5fcd3cd4/src/rime_api_impl.h)：`RimeSelectSchema` 无同方案短路，直接 `ApplySchema(new Schema(schema_id))`。
- [switcher.cc](https://github.com/rime/librime/blob/33e78140250125871856cdc5b42ddc6a5fcd3cd4/src/rime/switcher.cc)：`SetActiveSchema` 更新最近方案/访问时间并调用用户配置 `Save()`。

这是适配层对“新会话已经完成初始化”的误用，不是更换/重编 librime 的理由。上述源码仅按锁定提交只读核对，没有作为新的运行依赖引入。

## 修复

- 在真正创建新会话后，用已验证且已有的 `get_status` 查询当前方案。
- 先复制方案 ID，再配对 `free_status`；仅在与请求方案**完整字节比较不相同**时调用 `select_schema`。
- 不缓存、假定或硬编码默认方案；默认方案变化时仍正确切换。只对新会话去掉冗余操作，不改变显式重载方案的 librime 语义。
- 查询、UTF-8 解码、切换失败均保持失败；状态 guard 和 Session guard 仍执行原有资源回收。
- 单元回归检查同方案零次切换、不同方案/同前缀正确切换、查询失败、解码失败、切换失败及状态释放顺序；不在测试父进程加载 DLL。

## 修复前后证据

修复前带诊断的完整套件本次也曾通过，说明旧问题不是每次固定死锁。其证据为 `artifacts/rust-engine-11636-1792597132172694500`：

| 1,000 次循环内的操作 | 修复前耗时 |
|---|---:|
| 创建会话（含原先无条件切换） | 45,194 ms |
| 选项 | 28 ms |
| 按键 | 248 ms |
| 清屏及快照 | 32 ms |
| 销毁 | 699 ms |

约 97.8% 的循环计时落在创建路径。不能再把该总耗时笼统归给按键计算或管道等待。

修复后第一轮完整套件通过，旧探针证据为 `artifacts/rust-engine-8408-1792597623952568300`。同口径比较：

| 指标 | 修复前 | 修复后首轮 |
|---|---:|---:|
| measure 阶段总耗时 | 49,663 ms | 38,634 ms |
| 1,000 次创建路径累计耗时 | 45,194 ms | 34,096 ms |
| 循环区间读操作增量 | 31,019 | 27,023 |
| 循环区间写操作增量 | 17,714 | 8,723 |

后续循环中默认方案已是 tiger，只需首轮真正切换一次：读操作减少 **3,996 = 999 × 4**，写操作减少 **8,991 = 999 × 9**。这与去掉 999 次同方案重建一致，是比单次墙钟加速更直接的因果证据。计数不是“写了多少个文件”，也不把每一笔写操作都归成一次 user.yaml 保存。

历史成功耗时本就波动较大，不能只凭这两次墙钟时间宣称固定加速比例。已确认并修复的是不必要的重建/配置保存开销；没有识别历史运行中外部调度或文件系统延迟的具体来源，也不声称所有环境因素已消失。

## 完整复验

修复后连续三轮执行同一完整命令，每轮均 **3 passed / 0 failed**，包括冷资源准备、40 个原生样例/1,000 次真实会话循环、真实管道及提交后故障/恢复验证：

```powershell
cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture
```

| 轮次 | 旧探针报告目录（artifacts/ 下） | measure 耗时 | 创建累计 | 循环读 / 写操作增量 |
|---|---|---:|---:|---:|
| 1 | `rust-engine-8408-1792597623952568300` | 38.634s | 34.096s | 27,023 / 8,723 |
| 2 | `rust-engine-11248-1792598300138855500` | 50.966s | 45.533s | 27,023 / 8,723 |
| 3 | `rust-engine-11748-1792598421320387900` | 39.120s | 34.518s | 27,023 / 8,723 |

每轮旧探针均返回 0、`session_cycles=1000`、`failed_cases=[]`、`native_errors=[]`。时间仍有波动，但同口径 I/O 操作减少量在三轮中完全一致；没有拿最快的一次冒充固定性能收益。

配套管道报告目录分别为 `rust-engine-8408-1792597687283809500`、`rust-engine-11248-1792598378052655000`、`rust-engine-11748-1792598486394174800`。正常/恢复代次错误为空，预期卡住/退出故障保持单列；没有更改故障退出码或验收断言。所有活动数据库仍在工作区外同名 `TigerRime-tests` 目录，失败证据未删除。

常规验证最终结果：

- `cargo fmt --all -- --check`：通过。
- `cargo clippy --workspace --all-targets --locked --offline -- -D warnings`：通过。
- `cargo test --workspace --locked --offline`：73 passed / 5 ignored，含文档测试。
- `cargo test --workspace --locked --offline --target i686-pc-windows-msvc`：70 passed / 1 ignored，含文档测试；不加载 x64 DLL。

普通测试中的 ignored 项未算作 passed；真实 DLL 均由上述显式限时原生命令验证。

## 同时捕获的独立目录撞名问题

常规测试曾在 `altered_runtime_calculator_is_rejected_without_rewriting_it` 创建合成目录时返回 `AlreadyExists (183)`，尚未进入该测试的资源校验。

共用 `resources::new_directory` 使用 `PID + SystemTime::now().as_nanos()` 命名，但系统时钟不保证并行读取不同。同一进程、同一前缀的 fixture 可以生成同名目录。

修复为短临界区内生成严格递增的数值 nonce；锁只覆盖编号计算，文件 I/O 在锁外。目录格式保持 `prefix-PID-nonce`，已有目录仍由 `create_dir` 拒绝，不重试、不覆盖、不接管旧数据。冻结/回拨时钟及 8 线程并发的确定性测试覆盖唯一性和计数耗尽。

此问题与 TST-002 的单线程原生循环超时不同，也不能据此倒推 TST-001 的历史管道权限失败已找到根因。

## 尚未覆盖

此次是会话创建路径与回归可靠性修复，不是长期稳定性、正式数据目录故障或 TSF 50ms 验收。LUA-002、自动重启监督、混合位数和 TSF/UI 的原有门槛不变。
