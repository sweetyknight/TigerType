# ENG-001：LevelDB I/O 回归的定位与测试隔离修复

日期：2026-10-21。本文保留 I/O 修复阶段的验证结果；之后 LUA-001 也已修复，当前待办统一见 `project-status.md`，计算器修复见 `calculator-fix.md`。

## 结论与范围

原测试把活动 LevelDB 数据库放在源码工作区 `artifacts/` 内。独立 Win32 程序在同一区域也能复现文件替换失败，完全不需要加载 librime；工作区外的同盘对照与系统临时目录对照则连续通过。

本次修复的是**原生测试的数据隔离缺陷**：活动数据改在系统临时目录，源码工作区仅保留报告；四轮各 1,000 次会话循环均未再出现数据库错误。
没有修改 librime、LevelDB、原 Lua 或学习开关，没有加延时/重试，没有忽略新增错误。

证据支持“工作区路径相关的文件替换干扰”。**没有识别出具体占用进程或驱动，不能归咎于 Codex、杀毒软件、索引服务或 Rust FFI，也不能宣称修复了 Windows/LevelDB 内部缺陷。** 后续正式数据目录仍须验证外部占用与权限故障处理。

## 缩小复现

原错误为 `Error opening db 'easy_english': IO error: ... .dbtmp`，日志中的 Windows 文本已是问号，不能仅凭文本猜错误码。

发行依赖的版本文件记录 LevelDB 1.23.0。[对应 Windows 实现](https://github.com/google/leveldb/blob/1.23/util/env_windows.cc#L538-L565) 对 CURRENT 执行 `MoveFileA`，目标已存在时再执行 `ReplaceFileA`。

新增 `crates/app/examples/leveldb_file_io.rs`：

- 只在新建合成目录中独占写入 16 字节的 MANIFEST 名称，sync、关闭句柄后替换 CURRENT。
- `leveldb` 模式复现上述文件操作；`move` 模式作 `MoveFileExW` 对照。
- 首次错误立即退出，保存数值错误码，不重试、不加载 rime.dll、不操作实际数据库。
- `project` / `target` / `cache` / `temp` 是受限位置选项，不接受任意现用数据目录。

### 本机对照结果

| 位置 / 方式 | 结果 |
|---|---|
| 工作区 artifacts / LevelDB 方式，沙箱执行 | 第 141 次失败，MoveFileA=183、ReplaceFileA=1175 |
| 工作区 artifacts / LevelDB 方式，当前用户执行 | 第 128 次及第 19 次失败，ReplaceFileA=1175 |
| 工作区 artifacts / MoveFileExW | 第 1,508 次失败，错误 5（拒绝访问） |
| 同工作区 target / LevelDB 方式 | 第 1,142 次失败，1175 |
| 同工作区 .cache / LevelDB 方式 | 第 614 次失败，1175 |
| 系统临时目录 / LevelDB 方式 | 10,000 次通过 |
| 系统临时目录 / MoveFileExW | 10,000 次通过 |
| 同 D 盘、TigerRime 工作区外 / LevelDB 方式 | 10,000 次通过 |

D 盘为本地 NTFS，调查时剩余空间约 668 GB。对照不支持“磁盘空间不足”或“只在沙箱失败”的解释。
1175 的原始 Windows 文本为“无法删除要被替换的文件。”；故障点在替换 CURRENT，不是候选字符串复制。
这直接证明最小复现的错误码，不意味着已逐条捕获原 librime 日志中每一次失败的数值错误码。

只读 Restart Manager 查询在失败后的查询时刻返回空列表；不能排除瞬时占用或其他文件系统观察/过滤行为。没有终止其他进程、关闭防护、改变权限或系统设置。

部分对照目录：

- `artifacts/file-io-41644-1792585506104455800`：工作区内 1175。
- `artifacts/file-io-50728-1792585506998512200`：MoveFileExW 错误 5。
- `target/file-io-33892-1792585829480437700`、`.cache/file-io-45676-1792585834268981800`：同工作区对照失败。
- `C:\Users\hello\AppData\Local\Temp\file-io-46416-1792585623317888400`：LevelDB 方式通过。
- `C:\Users\hello\AppData\Local\Temp\file-io-35216-1792585674750048700`：MoveFileExW 通过。
- `D:\Projects\file-io-52528-1792585900092603300`：经明确许可创建的工作区外同盘对照，通过；仅含合成 CURRENT 文件。

## 修复内容

- `probe::new_workspace` 在项目中只建立报告目录；活动的 shared/user/logs 放在系统临时目录 `TigerRime-tests/<报告目录名>`。
- `runtime_paths` 校验报告根目录、v2 标记、生成的目录名、运行数据标记以及符号链接/目录联接；不接受任意数据库路径，不接管旧测试目录。
- 如果 TMP/TEMP 落在源码工作区内，明确拒绝，不静默回退到有问题的位置。
- 子进程的工作目录改为隔离数据的 user 目录；报告记录 `user_data_dir`、`native_log_dir`，stdout/stderr 仍写入 artifacts。
- 失败与成功的合成运行数据均保留，不自动删除失败数据库；旧失败目录没有修改。
- 会话循环由 100 加强到 1,000；新增英文 `enable_user_dict=true` 的显式断言，避免未来通过关闭学习“修复”测试。
- 当时保持 300 秒部署 / 120 秒引擎测试截止时间，以及只允许原计算器那一条已知诊断的严格断言；后续 LUA-001 修复已移除此例外。

## 复验

在四个全新目录运行 `native_engine_outputs_and_known_lua_failure`，每轮 20 个功能输出断言、1,000 次会话循环、一次性提交与英文透传均通过：

| 报告目录（artifacts 下） | 结果 |
|---|---|
| `rust-engine-46428-1792586417095709100` | 通过，无 LevelDB 错误 |
| `rust-engine-40468-1792586489162247200` | 通过，无 LevelDB 错误 |
| `rust-engine-48884-1792586569437139700` | 通过，无 LevelDB 错误 |
| `rust-engine-42408-1792586838718318700` | 最终增加英文学习断言后通过，无 LevelDB 错误 |

活动数据分别保留在 `C:\Users\hello\AppData\Local\Temp\TigerRime-tests\<报告目录名>`。四轮均只出现原计算器的已知中间态错误，探针依旧返回 1；回归测试通过不代表该 Lua 问题已修复。

fmt、Clippy、常规 Cargo 测试以及 x86 基础测试通过。新增目录名与旧工作区拒绝测试，保留库边界、所有权与超时测试。
这不是两小时/八小时压力或真实宿主验收，也未解决所有可能的 Windows 文件占用故障。

## 复现命令

```powershell
# 独立文件操作诊断：project 模式保留故障证据，可能以 1 退出。
cargo run --locked --offline -p tigerrime-app --example leveldb_file_io -- leveldb 1000 project
cargo run --locked --offline -p tigerrime-app --example leveldb_file_io -- leveldb 10000 temp

# 修复后的引擎回归；新建活动数据，不读取任何现用词典。
cargo test --locked --offline -p tigerrime-app --test native -- --ignored --exact native_engine_outputs_without_errors --nocapture
```

当前命令同时验证计算器补丁，要求零原生错误。若再次出现数据库错误，保留 engine.json / stderr / 临时数据目录，按严格失败处理；不要删除词典、关闭学习、放宽错误断言或无限重试。
