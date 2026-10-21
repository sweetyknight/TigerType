# Rust 引擎适配与验证

2026-10-21；本文记录首批 Rust 适配提交 `402737c`。当前状态见 [项目状态](project-status.md)，后续 LevelDB 调查与修复见 [专题记录](leveldb-io.md)。

**提交时状态：Rust 常规测试和 Clippy 通过，但原生稳定性回归未全通过。** 初始两次引擎测试通过，之后两次独立目录复跑出现新增 LevelDB I/O 错误；以下保留当时证据，不用后续成功记录覆盖历史失败。

## 变更范围

- 新建 Cargo workspace，仅添加已有实际实现的 `contracts`、`engine`、`app`。其余三个 crate 留给相应 IPC / TSF / UI 开发，不做空壳。
- 替换旧的四个 Python 工具与三个 Python 测试文件，删除其缓存，更新所有当前入口文档。
- 保留 66 项资源校验清单、20 个公开功能样例、完整批准方案及注明工具/口径的历史数据。
- 无注册表操作、输入法切换、字体安装或个人数据库访问。

## 可复用边界

`engine::Engine::start` 和 `Engine::deploy` 显式接受 DLL 与数据目录；不会探测 APPDATA 或启动外部程序。
加载前检查固定 SHA256，文件以只读共享方式保持打开，避免校验到加载之间被覆盖；DLL 依赖仅从 DLL 所在目录和 System32 搜索。
不静态链接 `rime.dll`，资源准备和父测试进程不会加载引擎。

ABI 声明只保留 `rime_get_api` 到 `get_version` 的所需前缀，所有使用的函数槽检查长度和非空。
已与锁定的官方头文件逐槽比对 73 个字段；x64 `get_version` 偏移 584，Traits / Context / Status 大小分别为 96 / 88 / 56。
版本固定 1.17.0，运行时检查 Lua 模块；DLL 校验值与根依赖清单也有一致性测试。

- 引擎及会话不能跨线程，`Engine` 的 `!Send` 约束有编译失败测试。
- glog / librime 含进程级全局状态，成功 setup 后同一进程不允许再初始化；恢复必须重启进程。
- 会话借用引擎，不能比引擎活得更久；主动 close 消费所有权，Drop 不会重复销毁。
- Context、Commit、Status、Config 均在成功获得后立即交给 RAII guard；转换失败或 Rust unwind 也释放。
- 返回的是 Rust String / Vec 快照，UI 或 TSF 不会得到 Rime 指针。
- UTF-8 长度、字符边界、选择范围、高亮索引、候选数量和文本容量均在复制时检查。保留 UTF-8 字节位置，TSF 阶段再转换为 UTF-16。
- 按键仍使用 librime 的 X11 keycode / modifier 语义；未消费与失败区分，提交是一次性消费操作。

FFI 假设已校验 DLL 返回符合 C ABI 的有效内存；边界检查不是防御内存损坏 DLL 的沙箱。
直接的 Rime 调用可能卡死，因此禁止把适配放进应用进程的 TSF 回调。当前以限时测试子进程隔离，生产管道、Job Object 和恢复监督尚待实现。

## 测试与证据

已执行：

1. `cargo fmt --all -- --check`
2. `cargo clippy --workspace --all-targets --offline -- -D warnings`
3. `cargo test --workspace --offline`：22 项单元测试、2 项子进程失败/超时测试、1 项编译失败文档测试；真实资源测试显式跳过。
4. `cargo test --workspace --locked --offline --target i686-pc-windows-msvc`：21 项单元测试和编译失败文档测试通过。x64 DLL 测试不在 x86 上运行。
5. `cargo run --locked --offline -p tigerrime-app -- prepare-resources 'D:\虎码秃版 小狼毫（Win）'`：原资源、副本及已有依赖复验通过，无新增下载。
6. `cargo test --locked --offline -p tigerrime-app --test native -- --ignored --skip timeout_worker --test-threads=1 --nocapture`：该轮冷启动资源和引擎两个测试通过；随后两次单独复跑引擎测试失败，见下文。不能概括为原生验收通过。

冷启动资源测试从锁定缓存，在全新目录导入、解压和校验，未依赖旧 Python 生成的 vendor 布局才能成功：

`D:\Projects\TigerRime\artifacts\rust-resources-37468-1792583748778843400`

完整引擎测试目录（新建用户数据，父进程未加载 rime.dll）：

`D:\Projects\TigerRime\artifacts\rust-engine-37468-1792583762124585900`

20 个输出样例通过，学习开关保持 tiger / tigress 关闭、PY_c 开启，九候选保持不变。
另验证 100 次会话创建/清屏/关闭、提交只消费一次、英文键透传、重复初始化拒绝，以及非 BMP / Emoji 字符串所有权。

本次 Debug 原生探针有 4,000 个预热逐键样本：P50 0.0196 ms、P95 0.0825 ms、P99 0.1241 ms、最大 0.5197 ms。
起止点为 Rust `Session::process_key` 调用前后，含 Context / Status / Commit 的获取与复制；**不是正式输入法端到端延迟，也没有与旧程序做对照。**

## 已知缺陷没有被伪装成通过

原 `calculator_translator.lua:400` 的不完整表达式会产生 `LuaTranslation::Next error(2)` 和 `attempt to call a nil value`。
`probe-engine` 将这条诊断写入 `engine.json` 并返回退出码 1。
原生集成测试要求准确复现这一条已知错误，并拒绝任何额外错误或候选失败；它证明回归检测有效，不证明问题已修复。

### 当时复跑发现的数据库错误

保持同一代码、同一固定资源和严格断言，在全新目录重新运行引擎测试：

- `artifacts/rust-engine-46604-1792584258662528200`：除计算器外，10 条 `Error opening db 'easy_english': IO error: ... .dbtmp`。
- `artifacts/rust-engine-43680-1792584560498155200`：除计算器外，2 条同类错误。

两次均在 100 次会话循环附近出现；最终输出断言仍通过，但严格的原生诊断检查失败。具体的 Windows 错误文本在原日志中已是问号，不据此猜测错误码。
测试进程已退出，涉及的都是新建合成词典；原配置及现用词典未参与测试，失败目录和 `lost` 数据保留以便进一步分析。

已核对固定提交的 [rime_api_impl.h](https://github.com/rime/librime/blob/33e78140250125871856cdc5b42ddc6a5fcd3cd4/src/rime_api_impl.h)：`RimeFinalize` 会先等待维护线程，再停止服务、清理注册表和卸载模块；Rust 没有遗漏在 finalize 之前/之后所需的这项回收。
`UserDictionary::Load` 打开失败时会尝试安排恢复任务；这解释了为什么应保留测试数据，但不能证明最初 I/O 失败的原因。

尚不能归因于 Rust 适配、上游库或环境文件占用。没有关闭英文学习、添加任意延迟、静默重试、忽略错误或更换锁定 DLL 来制造通过结果。
这是一项后续开发前必须继续定位的稳定性缺口，本次提交是可复用适配与诊断基础，不是可安装输入法的交付。

## 尚未实现 / 未验证

- Rust 空闲 CPU / 内存 / 线程的十分钟复测、两小时压力和八小时使用；旧程序对照仍缺失。
- 生产协议、50ms 总预算、可取消命名管道、身份验证、管理进程、Job Object、故障恢复和迟到提交丢弃。
- TSF、候选窗、设置、可回退部署、安装和各种宿主兼容性；x86 编译测试不等于完成 x86 TSF。
- Lua 安全改造、网址异步启动、字体回退及高级功能边界。
- 新下载的网络路径本轮未重跑，使用既有锁定缓存；下载调用保留 HTTPS 限制、大小/hash 校验及 190 秒子进程截止时间。7-Zip 列表/解压分别限时 60/120 秒。

当时的下一步是定位数据库 I/O 错误；当前后续动作统一维护在 `project-status.md`，避免历史文档与当前待办冲突。
