# 项目状态

更新：2026-10-22。范围见 [实施方案](implementation-plan.md)。

## 当前迁移
本批按用户要求将产品统一更名为 TigerType，覆盖文档、包名、程序、窗口、IPC、探针导出和数据目录标记，不保留旧名兼容层；工作区目录重命名与本批验证结果见下。**完整 TIP DLL（类工厂、激活/按键/焦点 sink 与宿主通知桥）尚未组装，仍不是可安装的输入法**。未注册、切换或安装输入法，未读取个人词库；本批不提交、不推送。

| 阶段 | 状态 |
|---|---|
| 文档治理 | 按最新授权删除旧文档/源码归档及迁移快照，只保留四份主文档 |
| 数据/引擎/基础协议/门禁 | 已实现并复验；无旧引擎运行依赖 |
| 独立查字/设置 | 已实现 Win32 原生窗口、分页学习资料和事务设置；可见交互/视觉与无障碍待验收 |
| 候选显示/点击 | 后台进程内预览窗与回送/一次领取已实现；方案已改为宿主进程内候选窗（B 批），现有实现届时删除 |
| 多宿主后台 | 已实现固定槽位发现、内核身份/映像认证、独立通知、serve/stop；尚不支持 AppContainer 宿主，按新方案改造（A 批） |
| DLL 驻留/关闭 | 已实现显式进程期 PIN 与资源排空；默认 EXE 限制保留，不支持热卸载 |
| TSF COM 编辑 | 已实现同步编辑会话、组合 sink 和自有范围保护；完整 TIP 服务、点击所需的异步编辑会话及混合位数端到端尚未完成 |
| 安装和长期使用 | 未开始，须另行授权 |

## 方案修订批次

- 架构评审确认四个问题，已写入实施方案并分 A/B/C 三批修复；本批只改文档，未改代码：
  1. 尚无真实宿主验证：完整 TIP 未组装，协议与传输假设未经真实 TSF 检验。
  2. `Editor` 只用 `TF_ES_SYNC`（`crates/tsf/src/edit.rs`）；候选点击来自宿主消息循环而非按键回调，同步锁可能被拒（`TF_E_SYNCHRONOUS`）。
  3. 不支持 UWP/AppContainer：管道 DACL 只授予登录 SID 且无低完整性标签；客户端须 `OpenProcessToken` 读取后台令牌；候选窗在后台进程，无法与沉浸式宿主处于同一层级。推断后台读取管理员宿主令牌同样会失败，未实测。
  4. 宿主激活即占槽直到退出，16 槽容易耗尽；后台等待宿主进程句柄，受 `WaitForMultipleObjects` 64 对象上限约束。
- 依据：[微软 IME 要求](https://learn.microsoft.com/en-us/windows/apps/design/input/input-method-editor-requirements)（候选窗须为宿主 owned window、与应用同一 z-order 层级，UIA 约定、IME WinEvent、沉浸式类别）和 [Crashpad 管道描述符](https://chromium.googlesource.com/crashpad/crashpad/+/21a20ef8adf3949de8dd65758a16f83aab344b3c/util/win/registration_protocol_win.cc)（ALL APPLICATION PACKAGES 授权与完整性标签）。两项平台前提（沙箱令牌含登录 SID；追加 ACE 后沙箱客户端可受限查询后台进程）尚未实测，A 批首先验证。
- 文档修改后，统一入口于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **86 passed、4 ignored**（不重复计入 DLL 探针子进程）；日志 `artifacts/plan-revision-check-20261022.log`。代码未变，结果只说明文档与现有实现均未破坏门禁，不是新方案的验证。

## 更名批次验证

- 已同步六个 crate 的依赖/调用方、Cargo.lock、架构白名单与反例、DLL 导出/加载符号、测试环境变量及固定协议字节（`TTYP`，结构版本仍为 v4）；受管目录使用新所有权标记，旧目录拒绝接管。原始资源、历史日志和 Git 历史不改写；旧构建缓存不是新名称的验收证据。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **86 passed、4 ignored**，不重复计入 DLL 探针子进程。日志 `artifacts/tigertype-rename-check-20261022.log`。首次格式检查失败已按 rustfmt 修正，随后沙箱目录保护测试返回 `Access is denied`，授权原样复跑通过；失败日志分别为 `artifacts/tigertype-rename-check-sandbox-20261022.log`、`artifacts/tigertype-rename-check-formatted-sandbox-20261022.log`。
- 用户已完成根目录重命名，实际确认工作区为 `D:\Projects\TigerType`，旧路径已不存在，新目录不是链接。源码和文档无旧产品名残留；本批产品与工作区更名完成，未提交或推送。
- 新路径首次复验中 DLL 驻留测试报 `Os { code: 267, kind: NotADirectory, message: "目录名称无效。" }`：移动后的构建缓存仍含旧编译期绝对路径。默认架构清理后 x86 缓存仍复现；按默认、x86/x64 目标清理本项目六个包的构建产物后重建通过，无需改源码或放宽测试。失败日志 `artifacts/tigertype-new-path-check-20261022.log`、`artifacts/tigertype-new-path-check-final-20261022.log`；清理记录 `artifacts/tigertype-new-path-clean-20261022.log`、`artifacts/tigertype-new-path-clean-targets-20261022.log`。
- 新路径统一入口已实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **86 passed、4 ignored**，不重复计入探针子进程；日志 `artifacts/tigertype-new-path-check-verified-20261022.log`。新路径重新构建并执行 `tigertype.exe --help` 通过，日志 `artifacts/tigertype-new-path-cli-20261022.log`。
- 完整资源、可见 UI、真实 TSF 宿主和长期验收未执行；原有功能限制不因更名解除。
- CLI 冒烟命令 `cargo run --locked --offline -p tigertype-app -- --help` 已构建并执行 `tigertype.exe`，帮助标题为 TigerType；日志 `artifacts/tigertype-cli-help-20261022.log`。

## 跨进程后台与 COM 编辑基线

- 提交前于 2026-10-22 原样复跑统一入口：架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试通过，每个位数 **86 passed、4 ignored**，不重复计入子进程探针。沙箱目录权限失败与授权复跑成功证据分别保存为 `artifacts/precommit-check-20261022-sandbox.log`、`artifacts/precommit-check-20261022.log`；完整资源、可见 UI、真实 TSF 宿主及长期验收本次未复跑。
- 固定 16 对首实例管道及独立所有权租约，默认生产命名空间为 backend；一个固定反应器/引擎线程维护所有连接，无逐连接线程。协议 v4 增加 Hello/Welcome、候选/清屏通知及管理停止；客户端核对内核对端及预期映像，双通道绑定同一存活宿主。
- 增加异步 Pending 所有权 API：事件唤醒后领取结果，禁止同管道重叠操作；提前丢弃取消连接且仍保留内核缓冲。输入请求不重连或重放，握手、半帧、写回复和通知均有界；停机确认写完再清理连接。
- 实际限时子进程测试已通过：重复后台拒绝、错误映像拒绝、两个逻辑宿主的独立连接/会话、通知准确回到原宿主、一次候选领取提交，以及认证停止。测试 UI 只产生合成点击，不创建可见窗口；两个逻辑宿主位于同一个测试客户端进程，尚非真实多应用验收。
- 新增 tsf crate，实际实现 ITfEditSession / ITfCompositionSink（锁定 windows 0.62.2 / windows-core 0.62.2）。合成 COM vtable 文本宿主测试覆盖非 BMP、提交后续组合、取消保留已提交前缀、过期/重复/失焦/只读/非空选区/外部改写拒绝、提交后清理失败不重放，以及同步返回后迟到回调拒绝。清理失败保留“只可结束组合”的引用，不把已提交文字重新纳入可删除范围；旧组合的终止回调按组合代次隔离。
- 新依赖导致工作区特性合并重链接正在运行的 check.exe，首次门禁在 Windows 报 LNK1104；改为显式运行 lib/bin/integration 与 doctest，all-targets Clippy 和 DLL 探针保留，无测试跳过。原日志 `artifacts/backend-com-check-sandbox-20261022.log`；随后沙箱目录权限失败日志 `artifacts/backend-com-check-targets-sandbox-20261022.log` 保留，授权原样复跑成功。
- 候选显示使用单项最新值缓存，快速状态更新只合并画面，不合并按键或提交。统一入口最终通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，**每个位数 86 passed、4 ignored**，子进程探针输出不重复计数。最终日志 `artifacts/backend-com-check-final-20261022.log`；上次完整成功日志 `artifacts/backend-com-check-20261022.log` 保留。
- 尚未把 COM 编辑器接入系统 TIP 生命周期和通知唤醒桥，不宣称已能在真实应用打字。完整资源、真实多应用/管理员宿主、混合位数端到端、主动等待总预算和长期验收本批未执行；4 项 ignored 为完整资源及三种限时辅助宿主入口，不计入 passed。

## 候选路由基线

- 协议 v3：按键/选字增加焦点代次，新增 Focus/TakeSelection；同步更新编解码、固定帧、管道测试和调用方，直接拒绝 v2。光标矩形支持负坐标，拒绝倒置/越界矩形和零焦点代次。
- `Dispatcher` 改为 16 宿主、合计 64 会话的隔离调度；宿主编号只从连接所有者传入，不在正文中传输。覆盖相同请求/会话编号的宿主隔离、失焦/旧焦点按键、旧快照/重复点击、状态变化、断连、容量和代次拒绝；尚未把此 API 接到真实多宿主管道服务。
- 新增 `preview-candidates`：固定引擎线程与原生 UI 线程，通过容量一的事件邮箱显示最多九候选；不激活窗口、标准按钮、工作区边缘夹取，旧窗口通知附展示编号。点击只回送意图，由原预览宿主领取一次并打印最终选择，不向系统宿主上屏。
- 统一入口最终通过架构/文档、fmt、Clippy，**x64 / x86 各 75 passed、3 ignored**，子进程探针输出不重复计数。日志 `artifacts/candidate-routing-check-verified-20261022.log`；覆盖路由、隐藏候选窗真实 WM_COMMAND 的展示编号、高位/耗尽编号、负坐标/底边定位和无效预览参数。
- 初次沙箱权限失败记录在 `artifacts/candidate-routing-check-sandbox-20261022.log`。授权复跑暴露 x86 `SetWindowLongPtrW` 为 i32 别名，改为按目标位数存取 u32 位型后通过；原失败日志 `artifacts/candidate-routing-check-20261022.log` 保留。补边界测试时的 Clippy 初始化问题也已修正，失败日志 `artifacts/candidate-routing-check-release-20261022.log` 保留；未跳过门禁。
- 未打开可见预览或接管桌面。当前候选使用系统控件、15pt Microsoft YaHei UI，长注释明确省略；这不等于完成锁定 Win11Light 配色/圆角、完整字体优先顺序、PUA 覆盖或读屏器验收。

## DLL 驻留/关闭基线

- 默认 `Runtime::new` 保持 DLL 拒绝；新增 `new_process_resident`，仅供激活阶段显式选择进程期驻留。无 DllMain、无注册入口、无后台自动启动。
- 新增 `shutdown`：原子关闭新连接准入，用内核事件等待全部管道、取消令牌和在途操作释放；输入路径不引入生命周期锁等待。超时不恢复准入、不强制释放；支持后续再次等候。排空不宣称线程已终止或模块已卸载。
- x64 定向 IPC 测试通过（日志 `artifacts/dll-residency-x64-20261022.log`）；新覆盖未释放取消令牌时关闭超时、关闭后不再准入、取消 accept/空闲读取后的排空。独立限时子进程加载实际 Rust cdylib，检查默认 EXE 拒绝、释放宿主加载引用后模块仍驻留、80 次超时 accept 与重复排空。子进程测试输出不重复计入父测试 passed。
- 统一入口最终通过架构/文档、fmt、Clippy，**x64 / x86 各 69 passed、3 ignored**（不重复计入父测试调用的探针子进程输出）；日志 `artifacts/dll-residency-check-final-20261022.log`。新增多等候者唤醒测试确认最后一个持有者释放后不会漏通知。ignored 为完整资源、管道辅助入口、DLL 辅助宿主，均未计入 passed。
- 沙箱内受管目录测试失败，授权原样复跑通过；未跳过保护测试。失败证据 `artifacts/dll-residency-check-sandbox-20261022.log`。未把探针等同于 TSF，未宣称 DLL 可热卸载、50ms 宿主回调预算或真实应用安全性已验收。

## 独立窗口交互基线

- 已补齐设置窗 Enter 保存/Esc 关闭、按钮 Enter 动作、下拉选择和 IMM 组合确认优先通路。增加异步回复的本窗口焦点恢复；窗口隐藏、其他应用前台或用户已经切到别的控件时不抢焦点。
- 修复重复唤起设置丢草稿、保存回复重置查字选区/滚动位置、查询提示覆盖设置错误的问题；错误类型或无在途动作的回复现在使 UI 返回故障，不再继续使用不确定通道。
- 定向 UI 测试 **8 passed**，日志 `artifacts/lookup-interaction-unit-20261022.log`。统一入口 `cargo run --locked --offline -p tigertype-app --example check` 已实际通过架构/文档、fmt、Clippy，**x64 / x86 各 65 passed、2 ignored**；ignored 未计入 passed。最终日志 `artifacts/lookup-interaction-check-20261022.log`。测试仅使用隐藏合成控件，不接管桌面、不碰剪贴板。
- 沙箱内受管目录测试仍返回 `Access is denied`；授权原样复跑成功，未跳过测试或放宽路径检查。失败证据 `artifacts/lookup-interaction-check-sandbox-20261022.log`。
- 上次可见验收已实际观察 `han` 查询与学习资料的系统可访问性树，但在设置操作前由用户 Escape 中止；不能据此认定键盘、字体或无障碍验收完成。该次完整资源复验 **1 passed**：`artifacts/lookup-visible-resources-20261022.log`、`artifacts/rust-tiger-21880-1792642076832168700/result.json`；本批没有重复运行完整资源。
- 字根 PUA/非 BMP 字体覆盖、完整字体优先顺序、跨屏 DPI、真实 IME 交互和读屏器仍未验收；新焦点/键盘逻辑的自动测试不替代实机验证。

## 独立窗口初版基线

- 新增 `ui` crate，只依赖 contracts 和既有 windows-sys；架构反例覆盖 UI 直接、别名和传递调用 engine。新增 CLI `lookup-window`，显式目录不接管其他应用数据。
- UI/工作线程各固定一个，请求与回复各容量一；系统消息/内核事件唤醒，无定时轮询、逐查询建线程或跨线程窗口指针。一次仅一个在途查询/保存，旧页不能接到新文本，保存成功后才更新已确认选项。
- 定向合成测试已通过：应用库 10 passed、UI 库 5 passed；覆盖设置保存与重开、并发冲突/坏配置保护、64 字限制、非 BMP/缺资料、旧页拒绝、错误/错配回复、邮箱满/断连，以及隐藏原生控件创建/销毁、消息泵查询和线程退出通知。原始日志：`artifacts/native-lookup-unit-20261022.log`。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 最终复验通过：架构/文档、fmt、Clippy，**x64 / x86 各 62 passed、2 ignored**。ignored 为完整资源测试和管道辅助入口，未计入 passed。最终日志：`artifacts/native-lookup-check-final-20261022.log`；前次成功日志：`artifacts/native-lookup-check-20261022.log`；沙箱权限失败日志：`artifacts/native-lookup-check-sandbox-20261022.log`。
- 测试未显示窗口、不读写剪贴板、不加载个人资源。标准控件、15pt 字体、Tab/助记键和 DPI 重排已编码；未验证可见窗口、跨屏 DPI、读屏器、字根字体覆盖或 Win11Light 视觉还原。
- 开发过程：沙箱内目录保护测试返回 `Access is denied`，授权原样复跑；新增测试初用空词表，被引擎合法性校验拒绝，改为合法合成词表后通过，没有放宽引擎/路径保护。首次编译的控件常量导入及 SAFETY 注释位置问题已修正。

## 上一批验证（离线核心基线）

- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 最终通过：架构/文档、fmt、Clippy，以及 **x64 / x86 各 54 passed、2 ignored**；ignored 分别为完整资源测试和限时管道辅助入口，未计入 passed。日志：`artifacts/rust-tiger-check-20261022.log`。
- 完整资源命令 `cargo test --locked --offline -p tigertype-app --test locked_resources -- --ignored --nocapture` 最终 **1 passed、0 failed**，10.35 秒。报告：`artifacts/rust-tiger-52408-1792606116488222200/result.json`；前一次同条件成功报告也保留。
- 9 项锁定文本导入 117,662 条单字编码、17,047 条单字拼音、99,142 条学习资料；逐条验证可输入编码，当前拆分完整码均能在码表找到。验证简码/全码、ü/v、非 BMP、转换后字形自身编码、包损坏拒绝及禁止覆盖旧包；未运行 DLL、Lua 或 OpenCC。
- 核心支持静态候选、常用字过滤、两种四码上屏方式、分页/选重、独立查字、多音字、学习信息、简繁多映射与去重；查询不修改会话。协议 v2 使用逻辑按键、配置和带版本选字；焦点/光标及宿主接入尚待 TSF 阶段。
- settings.json 复用受管目录、排他租约、冲突检测、事务备份和安全替换；验证只使用合成临时目录，正式 APPDATA 入口未开放。

### 开发过程失败与修复

- 完整资源首次失败：把 `align_type: center` 当作像素整数。改为独立对齐字段并增加正反例，原失败报告 `artifacts/rust-tiger-34796-1792605396858217500/result.json` 保留；未改资源/hash 或放宽解析。
- 沙箱内临时目录保护测试返回 `Access is denied`，统一命令在失败处停止；授权原样复跑通过，未绕过路径保护，也未证明所有环境权限问题已消失。
- x86 的 `dispatch-*.exe` 未执行即返回 740。同 SHA256 二进制改名后两项测试通过，测试文件改为 `routing.rs`；最终完整 x86 门禁通过，无跳过或管理员运行要求。

## 风险与下一步
- LevelDB/Lua/独立引擎问题随组件退役退出范围，不宣称本批修复。
- 下一步按实施方案第 4 步 A 批改造传输与认证，先实测两项 AppContainer 前提；随后 B 批组装最小 TIP 与宿主内候选窗，连接 EditReport.committed 的不可重放语义与失败清理。独立窗口的可见交互、字体/DPI/无障碍仍须验收；组件测试不是端到端输入法。
- AppContainer 子进程测试可能须创建并删除临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`）；C 批须对暂存目录授予 ALL APPLICATION PACKAGES 读取/执行，并由用户手动注册。两者都须在执行前单独授权。
- 放开低完整性后，同会话沙箱进程都能连接并占槽；回收只能缓解，同会话恶意代码造成的拒绝服务不在防护范围。微软提示绕过 AppContainer 规则的输入法可能被拦截；本方案只用显式 ACL 授权，该风险未完全排除。
- 独立工具退出等待当前工作真正结束；磁盘操作异常阻塞可能延迟进程退出，未实现强杀或假定超时即终止。设置冲突会保留外部文件与已确认选项，需关闭工具后重新打开读取外部版本。
- 默认 IPC 仍仅限 EXE；显式驻留入口与关闭排空已加入，尚未组装完整 TIP。跨进程测试与合成 COM 宿主测试不能替代真实应用、管理员宿主、焦点切换和混合位数端到端故障验收；不以空壳 COM 导出或模拟键盘注入声称完成。
- DLL 首版驻留选择不可逆系统 PIN；必须等宿主退出才释放模块映像，不支持热更新/热卸载。管道关闭超时仍可能留有活跃引用，不能据此结束生命周期或释放关联资源。
- 字体覆盖、无障碍、真实应用、50ms 总预算和长期性能未验证。输入核心测试不代表 Windows 输入法验收。
