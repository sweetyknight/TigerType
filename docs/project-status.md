# 项目状态

更新：2026-10-22。范围见 [实施方案](implementation-plan.md)。

## 当前迁移
B 批（B1 最小 TIP 与统一按键会话、协议 v6；B2 宿主内候选窗、UIA/WinEvent、点击异步选字）已于提交 `3226587` 入库。本批进入 C 批第一步：实现 `DllRegisterServer`/`DllUnregisterServer` 注册入口与暂存目录流程文档。**注册入口从未在本机执行，未经任何真实宿主验收，仍不是可安装的输入法**。未注册、切换或安装输入法，未激活系统 TSF，测试不写系统或用户注册表、不在桌面显示窗口，未读取个人词库；本批不提交、不推送。

| 阶段 | 状态 |
|---|---|
| 文档治理 | 按最新授权删除旧文档/源码归档及迁移快照，只保留四份主文档 |
| 数据/引擎/基础协议/门禁 | 已实现并复验；协议为 v6，直接拒绝 v2–v5；无旧引擎运行依赖 |
| 独立查字/设置 | 已实现 Win32 原生窗口、分页学习资料和事务设置；可见交互/视觉与无障碍待验收 |
| 候选显示/点击 | B2 已实现宿主内自绘候选窗、WinEvent/UIA、点击异步会话；`preview-candidates` 与 ui 候选窗已删除；可见外观、读屏器与真实宿主未验收 |
| 多宿主后台 | A 批已实现：AC/低完整性 DACL、双侧认证（后台不开宿主进程）、单请求管道、懒连接重连一次、16 槽 LRU 回收、serve/stop |
| 沙箱宿主前提 | AppContainer/低完整性子进程可连接、Untrusted 被拒、均不能抢建实例，已实测；真实 UWP 宿主未验收 |
| DLL 驻留/关闭 | 已实现显式进程期 PIN 与资源排空；默认 EXE 限制保留，不支持热卸载 |
| TSF TIP | B 批已实现：DLL 导出与类工厂、`ITfTextInputProcessorEx`/按键/线程管理器 sink、会话内 IPC 的同步按键会话与异步点击会话、失焦取消；C 批注册入口已实现但未执行，真实宿主端到端未完成 |
| 安装和长期使用 | 未开始，须另行授权 |

## C 批第一步：注册入口（未执行）

- `crates/tsf/src/register.rs`：DLL 新增导出 `DllRegisterServer`/`DllUnregisterServer`。注册先校验 DLL 为绝对路径且同目录有 `tigertype.exe`，再写 HKLM\SOFTWARE\Classes\CLSID\{79A65161-8689-43AC-A57C-574C6A74922A}（名称、`InprocServer32`、`ThreadingModel=Apartment`），然后在单线程套间内以 `ITfInputProcessorProfileMgr::RegisterProfile` 登记简体中文 0x0804 语言配置 `{2097C544-B7D6-4F5C-9F4D-6DB1BE3EE319}`（“TigerType 虎码”，默认启用，暂无图标），以 `ITfCategoryMgr` 登记键盘 TIP、沉浸式、托盘三类别；任一步失败执行注销回滚。注销依次撤销配置、类别并删除 CLSID 子树（不存在视为已删除），单项失败不阻止其余步骤，返回第一个错误。套间若已由调用方以其他模式初始化则不重复反初始化；导出截住 panic。
- 暂存与位数：宿主只连接与 DLL 同目录的后台且核对映像路径，因此 x64/x86 DLL 放同一暂存目录（x86 改名 `tigertype_tsf_x86.dll`）共用一个后台，分别由 System32/SysWOW64 的 regsvr32 注册；README 记录构建、复制、对 `S-1-15-2-1` 授读/执行、注册、启动后台、注销与删除步骤，全部须用户逐步授权后手动执行。
- 测试（tsf 共 31 项，新增 3 项）：注册内容（键路径、默认值、`InprocServer32`、线程模型、语言、类别、配置 GUID 不同于 CLSID）；校验拒绝相对路径、缺 DLL、缺后台；注册表写入/删除在 `RegLoadAppKeyW` 加载的临时私有配置单元中往返非 ASCII 路径，删除可重复。确认测试后临时目录无残留。TSF 配置/类别调用会写 HKLM，未在测试中执行。
- DLL 现导出四个函数，x64/x86 均为未修饰导出名：`artifacts/tsf-c-register-dll-exports-20261022.log`、`artifacts/tsf-c-register-dll-exports-x86-20261022.log`。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **110 passed、6 ignored**（同前口径）；日志 `artifacts/tsf-c-register-check-final-20261022.log`。前一次运行期间仍在改代码，日志 `artifacts/tsf-c-register-check-20261022.log` 保留但不作结论。
- 注册入口本身未在本机运行，HKLM、TSF 配置与系统设置中的键盘列表均未改动。

## B2：宿主内候选窗、UIA 与点击选字

- 候选窗（`crates/tsf/src/candidate.rs`）：TSF 线程惰性创建、停用时销毁；窗口类以 TIP 模块注册（重复注册视为已存在，模块驻留不注销）；`WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW`、`WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE`；owner 取 `ITfContextView::GetWnd`，空则 `GetFocus`，x86 用 `SetWindowLongW` 改 owner。自绘九行内候选（序号+候选字+灰色注释，省略号截断）与提示行，系统颜色兼顾高对比度；未声明 DPI 感知，字号按 `GetDpiForWindow` 缩放；按组合矩形放在工作区内，下方不足翻到上方。窗口过程经本线程登记表以弱引用找回候选窗，窗口数据不存指针，panic 不穿过回调。
- 位置：按键/点击会话写入后以同一 cookie 取组合 `GetTextExt` 与视图窗口；无布局时回退本线程 `GetGUIThreadInfo` 光标，再无则隐藏。显示/刷新/隐藏发 `EVENT_OBJECT_IME_SHOW/CHANGE/HIDE`。
- UIA（`crates/tsf/src/uia.rs`）：根元素 AutomationId `IME_Candidate_Window`、List、宿主提供者补窗口属性；候选项 Name 为候选字、HelpText 为注释、ListItem，`ISelectionItemProvider::IsSelected` 对应高亮，运行时编号含展示编号，展示更新后旧元素返回 `UIA_E_ELEMENTNOTAVAILABLE`；显隐发 MenuOpened/MenuClosed、高亮项发 ElementSelected，只在有监听者时构造元素；声明 `UseComThreading`，只在 STA 上被调用；UIA 选择被拒绝。
- 点击：同行按下/抬起才投递私有消息（展示编号+序号）；`Popup::selectable` 绑定当前快照编号且同一展示只受理一个；`Editor::select` 以 `TF_ES_ASYNCDONTCARE` 排队，开始时才计算 50ms 预算，核对代次与组合文本（不要求光标在末端），终止的组合拒绝并把清空留给下一按键；会话内再核对展示编号后发 `Select`。宿主层：未打开/未聚焦/待清空/刚重连不发请求，`StaleSnapshot`/`NotFocused`/`StaleFocus` 视为未处理且保留连接，其余失败作废连接。
- 交换结论三态（跳过/丢失/写入）；写入阶段失败在同一会话内撤回仍一致的自有组合。停用时显式断开管道（`Link::close`），不依赖引用释放；测试中后台在停用后立即收到断开。
- 删除：`preview-candidates` 命令与 `app::candidate_preview`、`ui::candidate`（视图/邮箱/系统按钮弹窗）、contracts 的 `candidate` 模块（`ScreenRect`/`CandidateToken`/`CandidateFrame`/`CandidateChoice`），TSF 直接使用平台 `RECT`。
- 测试（tsf 共 28 项，新增 8 项）：组合矩形只在组合中报告、无布局仍写入；点击会话只写入完整的自有组合、跳过不作废、外部改写拒绝且不结束组合、终止后把清空留给下一键、排队预算从开始计算且失效后不执行；宿主选字只针对当前快照、拒绝保留连接；隐藏候选窗的布局/命中/同行点击投递/受理去重/展示编号；UIA 约定属性、导航、选中、运行时编号、命中与过期；进程内假后台端到端经真实窗口消息点击第二候选，后台收到 `Select{revision, index:1, epoch:2}`，旧展示编号的点击不发请求。全部测试保持候选窗隐藏，不移动真实鼠标。
- DLL 仍只导出两个函数，见 `artifacts/tsf-b2-dll-exports-20261022.log`、`artifacts/tsf-b2-dll-exports-x86-20261022.log`。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **107 passed、6 ignored**（同前口径；较 B1 新增 tsf 8 项，随删除去掉 ui 4 项、app 1 项）；日志 `artifacts/tsf-b2-check-20261022.log`。
- 未执行：可见候选窗外观、读屏器（讲述人/NVDA）对 UIA 与 WinEvent 的实际朗读、真实宿主的 owner/z-order（含 UWP 沉浸式）、逐显示器 DPI 宿主、宿主拒绝或延后排队点击会话时的行为。

## B1：最小 TIP 与统一按键会话

- 协议 v6：`InputState` 增加 `ascii_punctuation`；`InputState::consumes` 为 TSF 本地判定。引擎测试在 12 种可达前缀 × 两组选项 × 全部命名键/ASCII/若干非 ASCII 字符下核对其与 `handled` 完全一致（不只是子集）。编解码测试覆盖 v2–v5 头部拒绝及当前头下缺少该字段的 v5 状态正文拒绝。
- `tigertype_tsf.dll`（cdylib+rlib）：只导出 `DllGetClassObject`、`DllCanUnloadNow`（恒 `S_FALSE`），x64/x86 均为未修饰导出名，dumpbin 记录见 `artifacts/tsf-b1-dll-exports-20261022.log`、`artifacts/tsf-b1-dll-exports-x86-20261022.log`。新 CLSID `79A65161-8689-43AC-A57C-574C6A74922A`；类工厂拒绝聚合，实例只暴露 TIP/按键/线程管理器 sink。未实现注册入口（C 批）。
- 激活：首次激活创建进程期驻留 `Runtime`（静态单例，基础设施），以同一编号/cookie 配对登记与注销两个 sink，按键 sink 登记失败时回滚线程管理器 sink；重复激活拒绝。后台映像取 DLL 同目录 `tigertype.exe`，DLL 不读配置。
- 按键：`OnTestKeyDown`/`OnKeyDown` 先映射（Alt/AltGr 放行，`Ctrl+.`、组合中 `Ctrl+L`，单击 Shift 中英，上/左、下/右移动高亮，字符经 `ToUnicodeEx` 不改键盘状态取得）再按上次状态判定，不吃的键不发 IPC。吃下的键在 `TF_ES_SYNC` 会话内依次核对文档（只读、非空选区、迟到/失效回调都在调用后台前拒绝）→ 懒连接/Open/Focus/Clear/Key 共用 50ms 期限 → 写入；后台失败删除自有组合并放行原键，写入失败作废剩余组合且下次先 Clear，不重放。
- 焦点：文档焦点变化或按键上下文更换不发 IPC，旧编辑器失效并同步取消自有组合（锁被拒时排队一次只删一致组合的异步取消）；下一键先 `Focus`。后台以 `NotFocused`/`StaleFocus` 拒绝时重新报告焦点后补发一次。宿主终止组合或外部改写/光标移出时只结束旧组合、不改正文，后台先 `Clear` 再按当前插入点处理本键。编辑器不再跟踪快照编号（结果只在会话内取得），因此重连后引擎快照编号重置不影响写入。
- 测试（tsf 20 项）：按键映射与修饰键放行；宿主会话序列（新连接 Open/Focus、失焦重报、Clear、焦点被夺补发一次、传输失败不重放并以新会话重建、连接失败/Open 被拒不发键）；编辑会话（文档核对先于交换、外部改写与宿主终止、交换失败删除自有组合、未处理键不写、提交后清理失败不重放、同步/排队取消）；类工厂与导出；合成线程管理器的激活配对与回滚；未激活/无后台放行；进程内真实管道假后台端到端（两文档焦点切换、提交、停用取消组合并断开，后台收到的请求序列逐条核对）。测试不激活系统 TSF，使用独立命名空间，不连接正式后台。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **104 passed、6 ignored**（同前口径，不重复计入 DLL 探针子进程）；日志 `artifacts/tsf-b1-check-20261022.log`。ignored 与 A 批相同，不计入 passed。
- 未执行：注册与任何真实宿主（含 UWP、管理员、x86 宿主）、`OnTestKeyDown` 吃键而 `OnKeyDown` 放行时宿主是否把原键交还应用、50ms 端到端预算、按键 sink 与 `ToUnicodeEx` 在真实布局下的行为、读屏器与长期压力。

## A 批：传输与认证

- 协议 v5：删除 `Notification`、`TakeSelection`、焦点光标矩形和 `Busy` 拒绝；`Focus` 只带 `epoch` 与 `focused`。编解码测试覆盖 v2–v4 头部拒绝，以及伪造 v5 头携带旧通知/领取/光标正文的拒绝。
- 管道描述符改为 `D:P(登录 SID)(AC)S:(ML;;NW;;;LW)`，客户端权限仍不含创建实例；单元测试读回 DACL 两项 ACE、受保护标志和低完整性 NW 标签。后台启动时 `grant_app_container_query` 只向自身进程 DACL 追加 AC 受限查询（0x1000），测试确认幂等且只有该一项。
- 认证：后台接纳只核对 `GetNamedPipeClientSessionId`，不再打开宿主进程或令牌，删除 `Peer` 与宿主进程句柄等待；停止命令仍以受限查询打开调用进程核对映像。宿主核对管道所有者 SID、服务端会话和后台映像，打开后以 `PeekNamedPipe` 确认对端仍连通（防 PID 复用）。所有者不符、会话不符、映像不符、相对路径均有反例；无法在测试中创建另一账户，“其他用户抢注”只以替换本用户 SID 的方式覆盖拒绝分支，未做真实跨账户验收。
- 每槽一条请求管道，Hello 后立即 Welcome；反应器只等待完成事件与最近期限，不受 64 个等待对象上限约束。`Link::ensure` 实现宿主懒连接：发送前非阻塞 `PeekNamedPipe` 发现断开或未请求字节时，同一期限内重连一次并报告新连接；已写出请求失败不重放。
- 回收：无空槽（待重建槽算空槽）或合计会话满 64 时，回收最久未用、非焦点、无组合且只剩常驻空闲读取的宿主，并以 `DisconnectNamedPipe` 立即断开。跨进程测试连满 16 宿主：持有焦点与组合的 A 不被回收并成功上屏，最久未用的 B 被回收后重连（新宿主编号、旧会话为 UnknownSession）、重新打开会话并上屏，第 17 个宿主可连接并触发下一次回收。
- 修复开发中暴露的停止竞态：确认写完后立即重新挂读，若停止方已关闭，读取失败会把该槽当错误移除，停止丢失（15 次复跑 12 次超时）。改为确认写完即结束服务、不再挂读后，20 次复跑全部通过。
- 沙箱前提（`crates/ipc/src/pipe/sandbox_tests.rs`）：低完整性子进程（令牌复制后仅降低完整性）通过登录 SID 算出同名管道、核对后台映像并完成往返；Untrusted 级子进程打开管道得到 `ERROR_ACCESS_DENIED`；二者都不能为双实例专用对象抢建实例。10 次复跑全部通过。AppContainer 用例实测仅派生 SID 时 `CreateProcessW` 返回 `ERROR_FILE_NOT_FOUND`，须创建临时配置（当前用户注册表与 Packages 目录），已改为默认忽略、经授权显式运行。
- 2026-10-22 经用户授权运行该用例：x64、x86 各 1 passed，x64 另复跑 5 次全部通过。子进程自检令牌为 AppContainer 且完整性为 Low，由自身令牌的登录 SID 算出同名管道、被拒抢建实例，以受限查询打开父进程核对映像并完成往返，即**“沙箱令牌含登录 SID”“追加 AC 受限查询 ACE 后可核对后台映像”两项前提成立**。运行后确认 `%LOCALAPPDATA%\Packages` 与 AppContainer Mappings 注册表（130 项均有 Moniker）中无 `TigerType.Test.*` 残留，临时暂存目录已删除。日志 `artifacts/appcontainer-premise-20261022.log`。未做反例“无该 ACE 时 AC 无法查询后台”（授权在测试进程内全局生效，同进程无法隔离）；该用例仍为 ignored，不计入常规 passed。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **89 passed、6 ignored**（不重复计入 DLL 探针子进程）；日志 `artifacts/transport-auth-check-20261022.log`。新增 ignored 为 AppContainer 授权用例与沙箱子进程入口，均不计入 passed。
- 未执行：真实 UWP/管理员宿主、其他账户抢注、完整资源、50ms 端到端预算和长期压力。回收与宿主写入之间仍有窄竞态，按结果不确定处理。

## 方案修订批次

- 架构评审确认四个问题，已写入实施方案并分 A/B/C 三批修复；本批只改文档，未改代码：
  1. 尚无真实宿主验证：完整 TIP 未组装，协议与传输假设未经真实 TSF 检验。
  2. `Editor` 只用 `TF_ES_SYNC`（`crates/tsf/src/edit.rs`）；候选点击来自宿主消息循环而非按键回调，同步锁可能被拒（`TF_E_SYNCHRONOUS`）。
  3. 不支持 UWP/AppContainer：管道 DACL 只授予登录 SID 且无低完整性标签；客户端须 `OpenProcessToken` 读取后台令牌；候选窗在后台进程，无法与沉浸式宿主处于同一层级。推断后台读取管理员宿主令牌同样会失败，未实测。
  4. 宿主激活即占槽直到退出，16 槽容易耗尽；后台等待宿主进程句柄，受 `WaitForMultipleObjects` 64 对象上限约束。
- A 批已修复问题 3 的传输/认证部分与问题 4（见上）；问题 1、2 和宿主内候选窗属 B/C 批。
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

- 本节为 v4 时期记录：通知通道、双通道绑定、宿主进程句柄等待与后台候选窗已在 A 批删除，结论不再代表现行传输。
- 提交前于 2026-10-22 原样复跑统一入口：架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试通过，每个位数 **86 passed、4 ignored**，不重复计入子进程探针。沙箱目录权限失败与授权复跑成功证据分别保存为 `artifacts/precommit-check-20261022-sandbox.log`、`artifacts/precommit-check-20261022.log`；完整资源、可见 UI、真实 TSF 宿主及长期验收本次未复跑。
- 固定 16 对首实例管道及独立所有权租约，默认生产命名空间为 backend；一个固定反应器/引擎线程维护所有连接，无逐连接线程。协议 v4 增加 Hello/Welcome、候选/清屏通知及管理停止；客户端核对内核对端及预期映像，双通道绑定同一存活宿主。
- 增加异步 Pending 所有权 API：事件唤醒后领取结果，禁止同管道重叠操作；提前丢弃取消连接且仍保留内核缓冲。输入请求不重连或重放，握手、半帧、写回复和通知均有界；停机确认写完再清理连接。
- 实际限时子进程测试已通过：重复后台拒绝、错误映像拒绝、两个逻辑宿主的独立连接/会话、通知准确回到原宿主、一次候选领取提交，以及认证停止。测试 UI 只产生合成点击，不创建可见窗口；两个逻辑宿主位于同一个测试客户端进程，尚非真实多应用验收。
- 新增 tsf crate，实际实现 ITfEditSession / ITfCompositionSink（锁定 windows 0.62.2 / windows-core 0.62.2）。合成 COM vtable 文本宿主测试覆盖非 BMP、提交后续组合、取消保留已提交前缀、过期/重复/失焦/只读/非空选区/外部改写拒绝、提交后清理失败不重放，以及同步返回后迟到回调拒绝。清理失败保留“只可结束组合”的引用，不把已提交文字重新纳入可删除范围；旧组合的终止回调按组合代次隔离。
- 新依赖导致工作区特性合并重链接正在运行的 check.exe，首次门禁在 Windows 报 LNK1104；改为显式运行 lib/bin/integration 与 doctest，all-targets Clippy 和 DLL 探针保留，无测试跳过。原日志 `artifacts/backend-com-check-sandbox-20261022.log`；随后沙箱目录权限失败日志 `artifacts/backend-com-check-targets-sandbox-20261022.log` 保留，授权原样复跑成功。
- 候选显示使用单项最新值缓存，快速状态更新只合并画面，不合并按键或提交。统一入口最终通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，**每个位数 86 passed、4 ignored**，子进程探针输出不重复计数。最终日志 `artifacts/backend-com-check-final-20261022.log`；上次完整成功日志 `artifacts/backend-com-check-20261022.log` 保留。
- 尚未把 COM 编辑器接入系统 TIP 生命周期和通知唤醒桥，不宣称已能在真实应用打字。完整资源、真实多应用/管理员宿主、混合位数端到端、主动等待总预算和长期验收本批未执行；4 项 ignored 为完整资源及三种限时辅助宿主入口，不计入 passed。

## 候选路由基线

- 本节为 v3 时期记录：`TakeSelection`、焦点光标矩形和预览回送领取已在 A 批删除；`preview-candidates` 现以绑定快照的 `Select` 进程内选字。
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
- 下一步：C 批真实宿主验收——注册入口已就绪；须用户对 README 中暂存、授权 ACL、注册、启动后台、注销各步单独授权并手动执行，再于真实宿主（含 x86、管理员、UWP 开始菜单/设置搜索）验收，结束后注销并删除暂存目录。
- 注册入口风险：从未执行，HKLM 写入、`RegisterProfile`/`RegisterCategory` 的实际效果、回滚与注销的完整性、默认启用后系统设置中键盘列表的表现均未验证；语言配置无图标；重复注销时系统对未登记项可能报错。AppContainer 用例每次重跑仍须用户授权。独立窗口与候选窗的可见外观、字体/DPI、读屏器仍须验收；组件测试不是端到端输入法。
- B2 已知限制：候选窗用系统颜色与 Microsoft YaHei UI 15pt，未按 Win11Light 锁定配色、字体优先顺序、圆角或 PUA 字根字体；长注释以省略号截断。宿主无布局且无系统光标时不显示候选窗。点击排队会话若宿主始终不执行，同一展示的后续点击被挡住，直到下一次展示更新。UIA 未实现选择容器模式，`Select` 被拒；读屏器效果未实测。候选窗 owner 取自上下文视图或焦点窗口，跨线程 owner 的输入队列影响未验证。
- B1 已知限制：后台未运行时 `OnTestKeyDown` 仍按默认状态吃字母/标点，`OnKeyDown` 连接失败后放行；宿主是否在此情形把原键交还应用须在 C 批实测，若不交还需改判定策略。单击 Shift 判定不感知鼠标，Shift+点击选择后松开会切换中英。后台回收或重启后新会话恢复中文模式，不保留宿主侧英文模式。组合中按 Home/End/Delete 等未映射键会交给应用，下一键按外部改写处理。
- AppContainer 子进程测试已确认须创建并删除临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`）；C 批须对暂存目录授予 ALL APPLICATION PACKAGES 读取/执行，并由用户手动注册。两者都须在执行前单独授权。
- 放开低完整性后，同会话沙箱进程都能连接并占槽；回收只能缓解，同会话恶意代码造成的拒绝服务不在防护范围。后台须以非提升权限运行：提升启动时管道所有者为 Administrators，宿主会拒绝连接。微软提示绕过 AppContainer 规则的输入法可能被拦截；本方案只用显式 ACL 授权，该风险未完全排除。
- 独立工具退出等待当前工作真正结束；磁盘操作异常阻塞可能延迟进程退出，未实现强杀或假定超时即终止。设置冲突会保留外部文件与已确认选项，需关闭工具后重新打开读取外部版本。
- 默认 IPC 仍仅限 EXE；TIP 激活时显式选择进程期驻留。跨进程测试与合成 COM 宿主/线程管理器测试不能替代真实应用、管理员宿主、焦点切换和混合位数端到端故障验收；不以空壳 COM 导出或模拟键盘注入声称完成。
- DLL 首版驻留选择不可逆系统 PIN；必须等宿主退出才释放模块映像，不支持热更新/热卸载。管道关闭超时仍可能留有活跃引用，不能据此结束生命周期或释放关联资源。
- 字体覆盖、无障碍、真实应用、50ms 总预算和长期性能未验证。输入核心测试不代表 Windows 输入法验收。
