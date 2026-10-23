# 项目状态

更新：2026-10-23。范围见 [实施方案](implementation-plan.md)。

## 当前迁移
B 批与 C 批注册入口已分别于提交 `3226587`、`7fb5a95` 入库（未推送）。C 批暂存、ACL 加固与两种位数注册已完成（注册由用户以管理员执行），第一轮真实宿主验收（Notepad++ x64）发现三个问题，本批已修复但**修复版尚未重新部署**：后台离线时吞键、宿主终止组合后候选窗残留、候选窗高亮为蓝色。x86、管理员、UWP 宿主未验收；仍不是可安装的输入法。未读取个人词库。

2026-10-23 按用户批准的设计稿重做全部界面（候选窗、查字窗、设置窗），新增五种配色主题（含自定义四基色）与候选字体切换，协议升为 v7；必跑检查通过，**未提交、未部署，真机视觉未验收**。

2026-10-23 同日实现安装包（Inno Setup），查字/设置窗并入后台进程、保存后热重载；已生成 `TigerType-Setup-0.1.0.exe`，**未在任何电脑上安装验收，未提交**。

| 阶段 | 状态 |
|---|---|
| 文档治理 | 按最新授权删除旧文档/源码归档及迁移快照，只保留四份主文档 |
| 数据/引擎/基础协议/门禁 | 已实现并复验；协议为 v9，直接拒绝 v2–v8；无旧引擎运行依赖 |
| 查字/设置 | 10-23 并入后台进程的固定 UI 线程，与输入法同时可用，保存后热重载（真实候选窗中的生效未实测）；同日加任务栏输入法图标入口，单击/右键菜单直接打开设置页（见下节）；同日按新设计稿重做：字列表加详情卡、开关/分段单选设置卡片、主题与候选字体、自定义四基色本地与后台双重校验；隐藏窗口测试覆盖主题像素与布局，真机视觉与无障碍待验收 |
| 候选显示/点击 | 宿主内自绘候选窗、WinEvent/UIA、点击异步会话已在 Notepad++ 实测可用；10-23 重设计为主题配色、圆角高亮、对齐注释列与页脚（未部署）；读屏器与其他宿主未验收 |
| 多宿主后台 | A 批已实现：AC/低完整性 DACL、双侧认证（后台不开宿主进程）、单请求管道、懒连接重连一次、16 槽 LRU 回收、serve/stop |
| 沙箱宿主前提 | AppContainer/低完整性子进程可连接、Untrusted 被拒、均不能抢建实例，已实测；真实 UWP 宿主未验收 |
| DLL 驻留/关闭 | 已实现显式进程期 PIN 与资源排空；默认 EXE 限制保留，不支持热卸载 |
| TSF TIP | 已注册两种位数，Notepad++ x64 第一轮验收通过主要按键；10-23 安装版在新进程只出英文（键盘开闭状态未打开）已修复；输入法图标右键无菜单（未处理 `OnClick(右键)`）**已修复，未部署、真机未验收**；23:12 核对 19 个已载入进程中 18 个仍是 v7 旧 DLL、与 v9 后台不通，只出英文；其他输入法候选框消失根因未定（见下节）；x86/管理员/UWP 未验收 |
| 安装包 | 10-23 已实现并生成安装包；安装、升级、卸载均未实测 |
| 长期使用 | 未开始 |

## 微软候选框消失：TextInputHost 主线程忙等（2026-10-24 00:00，只读取证，未改代码）

- 用户报告微软候选框消失仍未修复。只读检查发现 TextInputHost（pid 35772，10-23 13:29:57 启动）的线程 33580 持续占满一个核：累计 CPU 约 14,620 秒，其中用户态仅 744 秒，余者在内核态；按 97–99% 的速率倒推，约从 10-23 19:55 前后开始。进程未判定挂起，能响应同步消息。证据与原始采样存 `artifacts/textinputhost-spin-20261024.log`。
- 采样方法：自写只读采样器，每次暂停该线程读取用户态寄存器与栈后立即恢复，按模块导出名归类（本机无 cdb/WinDbg、无符号）。120/120 次都停在 `win32u!NtUserPeekMessage`；寄存器显示调用为 `PeekMessage(&msg, NULL, WM_PAINT, WM_PAINT, …)`；栈为 XAML → `Windows.UI`（CoreDispatcher）→ 窗口过程 → `edgehtml.dll`（旧版 EdgeHTML），所处理窗口为同线程的 `Internet Explorer_Hidden`。即 EdgeHTML 在一次消息处理里进入只取重绘消息的忙等循环，再没出来：该线程只处理重绘与外部同步消息，投递给它的其他消息得不到处理。
- 该线程还拥有 TSF 封送窗口 `CicMarshalWnd` 与 XAML 消息/计时器窗口；微软候选框所在的 CoreWindow（203C2）属另一线程 34084（CPU 正常）。23:21、23:35、23:49 用户打字时候选框弹出/收起事件仍成对出现，所以该卡死**不是**“候选框完全不出”的充分条件；它与“时有时无、组字中途消失”是否相关、以及触发源（EdgeHTML 在 TextInputHost 里通常只用于表情/GIF、剪贴板历史等面板）都未确认。TigerType DLL 不在 TextInputHost 中，TigerType 不向其窗口发消息；本轮未发现两者之间的通道。
- 已排除：必跑检查中隐藏候选窗未配对的 `EVENT_OBJECT_IME_SHOW`（23:34 运行后候选框仍正常）；测试与实验弹出的真实菜单（23:41 实验后 23:49 仍正常）；指针事件风暴（指针静止时忙等照旧）。
- 对照实验（00:22–00:25，用户同意重启 TextInputHost）：用户在 Claude 中用微软拼音打字，旧进程照常发出 6 次候选框弹出/收起事件（窗口 203C2），用户看不到候选框；用户停手 3 秒后脚本结束旧进程，系统 1 秒内拉起新进程（pid 956），用户再次打字即看到候选框。新进程无任何线程忙等。结论：**微软候选框消失的直接原因是 TextInputHost 进入坏状态（主线程卡在 EdgeHTML 忙等），重启即恢复**；“发出弹出事件但看不到”说明候选窗口仍在走显示流程，只是未呈现到屏幕。失效时窗口的隐身/位置状态没记到：候选窗口及其父窗口在系统专用层，`EnumWindows` 枚举不到，脚本只按枚举结果读取。日志 `artifacts/textinputhost-restart-trial-20261024.log`、`artifacts/textinputhost-monitor-20261024.log`。
- 触发源仍未确认。时间关联（只是关联）：忙等约 19:45–20:00 开始；Codex 应用（用户所说的“ChatGPT”，包 `OpenAI.Codex`）的电脑操控组件 `codex-computer-use-swift.exe` 自 17:58 起常驻；19:44 有 Claude 电脑操控向记事本注入按键的验收；10-22、10-23 两次候选框失效也都发生在电脑操控注入按键之后。00:18:20–00:18:24 另有名为 `location` 的短命进程连续 20 次各弹一个 `#32770` 对话框抢走前台（约每 130–250ms 一次），磁盘上未找到该程序。TigerType DLL 不在 TextInputHost 中，也不向其窗口发消息，本轮未发现 TigerType 到 TextInputHost 的通道。
- 已升级并重启只读记录器（`monitor2.log`）：增加前台程序完整路径、候选窗口弹出时其与父窗口的隐身状态和位置、每 2 秒物理/注入按键**次数**（不记键值），用于下一次忙等开始时对照当时的操控工具、程序与 TigerType 活动。
- 失效时的临时恢复：结束 `TextInputHost.exe`，系统会在下次输入时自动重启（用户此前也这样恢复过）。TigerType 不做自动重启（项目不引入看门狗，也不应自动结束系统进程）。

## 输入法图标右键菜单修复（2026-10-23，未部署）

- 依据下节已确认的根因，经用户要求修复：`OnClick(TF_LBI_CLK_RIGHT)` 先在单次回调预算内确认后台并取当前配色，再在右键位置弹出“设置/查字”菜单，选择后走与左键相同的唤起路径；后台不可用时两项置灰并附原因说明。所有者为本线程临时建的零尺寸隐藏窗口（`WS_EX_NOACTIVATE`，自绘消息须回到本模块，不交宿主窗口处理），用完即毁，不激活、不改宿主焦点；菜单项自绘，选中为主题高亮底加强调条、置灰项用次要文字色且不高亮，高对比度沿用系统颜色，文字随项保存供读屏器。`InitMenu`/`OnMenuSelect` 留空（未声明菜单样式，系统不调用）。复用候选窗的配色、字体与绘制函数（改为 crate 内可见），tsf 的 `windows` 依赖增开 `Win32_UI_Controls` 特性（自绘结构）。
- 测试改走真机入口：先向本线程投递按键，再调用 `OnClick(右键)`，让系统菜单模态循环真实运行并吃掉按键，返回后断言按键已全部取走（证明菜单弹出过），另设 3 秒 `EndMenu` 定时器防挂住、全局锁避免并行测试同时弹菜单。覆盖：`↓ Enter` 唤起设置页、`↓ ↓ Enter` 唤起查字窗、`Esc` 不唤起、离线时 `↓ Enter` 收起菜单但不唤起（实测 Enter 落在置灰项上会直接收起菜单）、左键直接打开设置页；服务测试经进程内假后台核对右键与左键依次送达 `Settings/Lookup/Settings`、无新的 `Open`，并断言按钮样式不含 `TF_LBI_STYLE_BTN_MENU`。菜单内容（自绘、编号、文字、置灰与离线说明）与各主题下选中/置灰项的像素读回另有测试。反例：把右键分支临时改回不处理，右键测试即失败（“菜单没有取走预置按键”），随后恢复。删除了合成 `ITfMenu` 与直接调用 `InitMenu` 的旧测试。测试时菜单会在屏幕左上角一闪而过，README 已注明。
- 统一入口于 2026-10-23 通过：架构/文档 5 passed，fmt、Clippy 无告警，x64、x86 各 **133 passed、8 ignored**；日志 `artifacts/indicator-right-click-check-20261023.log`。首次运行仅因新代码未格式化失败（`artifacts/indicator-right-click-check-fmt-fail-20261023.log`），格式化后通过。
- 未验证：部署后真实任务栏右键（须安装新版并重开程序；已载入旧 DLL 的程序仍无菜单）；任务栏取得前台时菜单点外部能否收起；浮动语言栏；读屏器朗读；Win10 外观。

## 右键打不开窗口、只出英文与候选框消失复查（2026-10-23 23:10–23:35，只读，未改代码）

- 用户报告：右键选中仍打不开窗口；仍不断出现无法输入中文、阻拦其他输入法候选框。电脑操控授权被拒，本轮只做只读查询、代码与文档核对，并以 WinEvent 记录器（前台、菜单弹出/收起、后台窗口显隐、全桌面 IME 事件；不记按键）取证，记录存 `artifacts/recheck-events-20261023.log`。
- 输入法图标右键（根因已确认）：TSF 只对样式含 `TF_LBI_STYLE_BTN_MENU` 的按钮调用 `InitMenu`/`OnMenuSelect`（[TF_LBI_STYLE 常量](https://learn.microsoft.com/en-us/windows/win32/tsf/tf-lbi-style--constants)）；`indicator.rs` 只声明 `BTN_BUTTON`，系统右键时调用的是 `OnClick(TF_LBI_CLK_RIGHT)`，而 `OnClick` 只处理左键、注释误以为任务栏会走 `InitMenu`。小狼毫（`WeaselTSF/LanguageBar.cpp`）与 Mozc（`tip_lang_bar_menu.cc`）都在 `OnClick(右键)` 里自行 `TrackPopupMenuEx(TPM_NONOTIFY|TPM_RETURNCMD)` 弹菜单再按结果处理。`service_tests` 直接调用 `InitMenu`/`OnMenuSelect`，测试通过但真机路径不存在；README 原写“单击/右键效果相同”不实，已更正。
- 通知区图标右键：代码路径（版本 4 `WM_CONTEXTMENU` → 前置隐藏窗口 → `TrackPopupMenuEx` → 投递 `OPEN` → `present`）未发现缺陷，后台与资源管理器同为中完整性，图标已登记在任务栏上（`Shell_NotifyIconGetRect` 成功）；但此前只用测试进程投递的合成消息验证过，**真实右键仍未实测**。23:12 时后台查字窗与设置窗均已创建且隐藏。
- 只出英文（当前状态已确认）：23:12 枚举载入 `tigertype_tsf.dll` 的 19 个进程，只有 23:08 新开的 Notepad++ 映像为 1,003,520 字节（v9）；资源管理器、Chrome、jie、GLKVM 为 974,848 字节（13:12 首装版，v7，且无键盘开闭修复），ToDesk、SearchHost、Docker、WhatsApp、VS Code、Claude、ChatGPT、ApplicationFrameHost 等 14 个为 978,944 字节（14:02 版，v7）。后台为 v9 且直接拒绝 v2–v8，这 18 个进程里 TigerType 每键握手失败后放行原键，只能出英文，开始菜单搜索（SearchHost）与资源管理器也在其中。结构原因：DLL 进程期驻留且已映射文件不能替换，协议严格相等，今天 5 次部署 3 次升协议，每次部署都使所有已开程序失效直到重开或注销；失败时静默放行、无任何提示。
- 只出英文（新 DLL 中可复发的风险，未实测）：单击 Shift 切换中英不感知鼠标（Shift+点击选择会切换），且输入法图标始终画同一个“虎”、不随中英状态更新，切到英文后用户看不出；中英状态按每个连接独立，重连后恢复中文，各程序表现不一致。
- 其他输入法候选框（仍未定因）：23:21 用户在 ChatGPT 用微软拼音输入时，TextInputHost 的候选框（“Windows 输入体验”）菜单弹出/收起事件 21 对、成对出现，此刻正常；TextInputHost 自 13:29 未重启。复核上一节的原始记录 `artifacts/candidate-watch-20261023.log`：每个新开的实验记事本中**第一次组字都没有候选框打开事件**（微软拼音 18:32:11、18:34:28、18:34:44、18:34:57、18:35:07；TigerType 18:25:41 同样），第二次常在打开 10–16ms 后自行关闭，与上一节“复查候选框均正常弹出”的结论不符，该结论作废；这些都在注入按键下出现，不能区分是 TigerType、注入方式还是新窗口首次组字本身。代码中 TigerType 停用后不再持有任何 sink，跨进程只剩候选窗的 IME WinEvent/UIA 事件；发现一处可疑路径：候选窗是宿主窗口的被拥有窗口，宿主关闭该窗口时系统连带销毁候选窗，不会发出 `EVENT_OBJECT_IME_HIDE`，此后本线程的 `Popup` 句柄失效、TigerType 自己的候选窗在该线程再不显示（直到重新激活）。此前“未配对 IME_SHOW 不是触发条件”的排除只用了从未可见的测试窗口，不覆盖可见窗口被连带销毁的情形。
- 本轮只改 README 与本文档；统一入口复跑通过（退出码 0），日志 `artifacts/recheck-check-20261023.log`。
- 未验证：真实右键通知区图标与输入法图标（需真实右键）；按物理键盘做的对照（新记事本只用微软拼音 / TigerType 组字后切微软拼音 / 关闭拥有候选窗的窗口后再用微软拼音），各自以记录器核对候选框弹出事件。

## 通知区常驻图标（2026-10-23）

- 用户要求：设置须能在任何地方右键打开，不能只在特定软件里可用。任务栏输入法图标只在前台程序正在用 TigerType 时出现（焦点在桌面/资源管理器、当前是其他输入法、或程序仍载旧 DLL 时都没有入口），因此在后台加常驻通知区图标，与输入法图标并存。
- 实现（`crates/ui/src/native/tray.rs`）：后台固定 UI 线程启动即建隐藏顶层窗口并登记图标，不新开线程；查字/设置窗仍等首次唤起（唤起信号或图标操作）才建。登录早期资源管理器未就绪而登记失败不算故障，收到 `TaskbarCreated` 广播（资源管理器就绪或重启）即删旧再挂。版本 4 回调：单击或 Enter（`NIN_SELECT`/`NIN_KEYSELECT`）打开设置页，右键或菜单键（`WM_CONTEXTMENU`）弹出“设置/查字”菜单；菜单项自绘（主题底色、高亮与强调条，高对比度用系统色），同时保留文字供读屏器。选择后投递线程消息，由事件循环 `present`；被点击时本进程获准切前台。图标与 TSF 输入法图标同样式，HICON 在登记期间保持，换新或退出时释放；后台退出时删除图标。回调不分配（栈上 UTF-16 缓冲）。
- 测试：只建隐藏窗口、不登记图标，核对单击/Enter 打开设置页、鼠标移动不打开、菜单投递的查字与唤起信号都交给首次唤起，唤起邮箱断开即返回；图标位图角透明、中心不透明、无蓝色。右键弹出的菜单是系统模态循环，不在常规测试中运行。
- 统一入口于 2026-10-23 通过：架构/文档 5 passed，fmt、Clippy 无告警，x64、x86 各 **130 passed、8 ignored**；日志 `artifacts/tray-check-20261023.log`。
- 部署（21:18）：打包日志 `artifacts/tray-package-20261023.log`，安装包 SHA256 `afccd994…dd6c65ba`；以普通身份启动、安装程序自行提权（`artifacts/tray-install-20261023.log`），已装 `tigertype.exe` 与 release 产物哈希一致；后台 `stop` 后经桌面外壳在 explorer 内重启（17 个管道）。
- 实测（21:18–21:21，电脑操控；任务栏与溢出区只授权左键）：当前输入法为微软五笔、TigerType 未在用时，Win11 把新图标收在“^”溢出区，单击其中的“虎”图标打开设置页，`GetForegroundWindow` 读回“TigerType 设置”。右键无法经电脑操控点击，改由测试进程向后台自己的图标窗口投递与版本 4 右键相同的回调（`WM_CONTEXTMENU`，锚点取溢出区图标位置），菜单弹在锚点上方，白底“设置/查字”两项，悬停为浅琥珀底加橙色强调条、无蓝色；点“查字”打开查字窗。首次点“查字”时查字窗成了前台却未显示：我经外壳重启后台时传了显示参数 0，系统把进程第一个无所有者窗口的首次 `ShowWindow` 换成隐藏；第二次正常显示。安装脚本 `[Run]` 未用 `runhidden`，登录自启与开始菜单也按正常显示启动，用户路径不受影响；今后重启后台改用显示参数 1。
- 未验证：资源管理器对真实右键/菜单键是否按版本 4 投递 `WM_CONTEXTMENU`（Shell 标准行为，电脑操控无法右键任务栏）、登录时资源管理器晚于后台就绪后由 `TaskbarCreated` 补挂、资源管理器重启后补挂、高 DPI 图标清晰度、读屏器朗读菜单、深色主题下菜单外框与自绘项的协调、Win10。

## 任务栏输入法图标设置入口（2026-10-23）

- 用户要求：必须有设置页和界面操作入口（此前只能开始菜单→查字窗→“设置”按钮，输入法本身没有入口）。用户在“输入法图标菜单”与“托盘图标”间按资源占用取舍：图标菜单不在后台常驻任何东西，只在已激活 TigerType 的宿主里多一个按钮对象，选用它。
- 实现：TIP 激活时向 `ITfLangBarItemMgr` 登记 `GUID_LBI_INPUTMODE` 按钮（`crates/tsf/src/indicator.rs`），停用时注销；登记失败照常激活。图标每次现画（琥珀主题强调色圆角底、白色“虎”，圆角外透明），不读盘。单击打开设置页；右键菜单“设置”“查字”，弹出前以 50ms 预算确认后台可连接，连不上则两项置灰并附“后台未运行：从开始菜单打开 TigerType 查字与设置”。选择后先 `AllowSetForegroundWindow(ASFW_ANY)`，再经本宿主已有连接发 `Show(Panel)`；唤起不占会话，成功后连接照常用于输入，失败作废连接、收起候选，下一键重建会话。
- 协议 v9：`Message::Show` 携带 `Panel::{Lookup, Settings}`，直接拒绝 v2–v8。后台接受已握手连接的唤起、**不再核对调用进程映像**（只投递信号、不改数据；停止仍核对），目标窗口经 UI 邮箱交给 UI 线程。`Client::show_lookup` 改为 `Client::show(panel)`，成功不作废连接；`lookup-window` 用 `Panel::Lookup`。
- 设置页独立显示：UI 线程按目标窗口显示查字窗或设置页（`present`），设置页可在查字窗隐藏时单独打开，重复唤起保留草稿；设置窗加 `WS_EX_APPWINDOW` 以便有自己的任务栏按钮（被拥有窗口默认没有，被挡住后找不回）；关闭设置页时仅在查字窗可见时才把焦点还给查询框，不激活隐藏的查字窗。
- 测试：Host 唤起成功不打断组合、失败作废连接且下一键重开会话（`host::tests`）；合成线程管理器加语言栏管理器，核对激活登记一个输入模式按钮、停用注销（释放顺序加 `indicator`），后台不在时菜单三项全置灰，后台在时菜单两项可选，菜单“设置”“查字”与左键单击经同一连接依次送达 `Settings/Lookup/Settings` 且请求记录中无新的 Open（`service_tests`）；图标位图读回：角透明、中心不透明、全部不透明像素蓝分量不高于红绿（`indicator::tests`）；后台测试改为同一连接先唤起设置页再 `Open` 拿到热重载设置；未接 UI 的后台拒绝唤起设置页（反例）；编解码覆盖两种目标窗口。设置页单独显示未写隐藏窗口测试（`present` 会显示并前置窗口，测试不建可见窗口）。
- 统一入口于 2026-10-23 通过：架构/文档 5 passed，fmt、Clippy 无告警，x64、x86 各 **128 passed、8 ignored**；日志 `artifacts/indicator-check-20261023.log`。
- 部署（20:25）：打包日志 `artifacts/indicator-package-20261023.log`，安装包 SHA256 `f190ec8d…079081be4`；静默升级成功（`artifacts/indicator-install-20261023.log`），已装 `tigertype_tsf.dll` 与 release 产物哈希一致。本次以 `Start-Process -Verb RunAs` 直接提权启动安装程序，安装程序没有未提权的原实例，`runasoriginaluser` 因而**以提权身份**启动了后台：管道所有者为 Administrators，普通宿主与 `stop` 均拒绝连接（“管道所有者不是当前用户”），且它占着本会话的管道名。经用户 UAC 确认后以提权 `taskkill` 结束，再经桌面外壳 `IShellWindows.FindWindowSW(SWC_DESKTOP)` 取 explorer 进程内的 `Shell.ShellExecute` 启动 `serve`：父进程 explorer、会话 1、非提权、17 个管道。用户正常双击安装包时由安装程序自行提权，不经此路径；但从已提权的终端运行安装包会复现同一问题（见风险）。
- 部署后各已打开程序（Chrome、VS Code、Claude、ChatGPT、WhatsApp、Windows Terminal、资源管理器等 21 个进程）仍载入 v7/v8 DLL，与 v9 后台协议不符，只能放行原键（只出英文），须重新打开才载入新 DLL。
- 记事本实测（20:39–20:42，经用户授权电脑操控；任务栏归资源管理器，只授权到左键单击级）：新开的记事本载入新 DLL（映像 1,003,520 字节），Win+空格切到 TigerType 后任务栏通知区出现琥珀色“虎”图标，悬停提示“TigerType 虎码：单击打开设置，右键更多”。按键注入 `sr` 出候选“1 发”（`type` 以 Unicode 注入的字符不经输入法，须逐键注入）。单击图标：设置页单独弹出在记事本之上，查字窗未出现，任务栏多出设置页按钮；`GetForegroundWindow` 读回“TigerType 设置”、属后台进程，即**设置页确实到了前台**。点“取消”后焦点回记事本、查字窗未被激活；再次单击图标，设置页再次到前台；设置页开着时记事本内仍正常出字。测试标签页不保存关闭，用户原有标签页未动。
- 实测发现设置页副标题仍写“保存后重新启动后台生效”（热重载之前的旧文案），已改为“保存后即时生效，无需重启后台”。统一入口复跑通过，结果同上（`artifacts/indicator-check-final-20261023.log`）。
- 重新部署（21:03，经用户要求）：打包日志 `artifacts/subtitle-package-20261023.log`，安装包 SHA256 `7d3dcdd3…43b3588b`；这次以普通身份启动安装程序、由其自行申请 UAC，日志 `artifacts/subtitle-install-20261023.log`，安装程序先停掉旧后台，再以**未提权**原用户拉起新后台，已装 `tigertype.exe` 与 release 产物哈希一致。该后台仍是本会话进程的后代，`stop` 后经桌面外壳在 explorer 内重启：父进程 explorer、17 个管道。记事本（仍载 20:25 版 DLL，同为 v9）中单击图标，设置页显示新副标题。约 10 秒后读前台窗口为另一程序的远程桌面 RemoteApp 窗口（`msrdc`），设置页仍在最上层显示；前台被谁切走未查明，与上一轮两次读回设置页为前台的结果并存。
- 未验证：右键菜单（电脑操控对任务栏只允许左键，须用户手动点）、后台停止时置灰菜单的显示、图标在高 DPI 下的清晰度、x86/UWP/管理员宿主中的图标、读屏器对菜单的朗读、旧版浮动语言栏中的右键（只实现了任务栏的 `InitMenu` 路径）。左键单击打开设置页而不是切换中英文，是本批的取舍，待用户体验后确认。

## 删除候选窗提示行（2026-10-23）

- 用户要求：空码等情况下候选窗里的“空码已满…”“暂无候选…”提示框不美观，按主流做法去掉。微软拼音、搜狗、小狼毫都不弹文字提示，无候选时编码只留在组合下划线里；用户原小狼毫虎码方案为 `auto_clear: max_length`（满码空码时后码顶上清屏）。
- 实现：协议升为 v8，删除 `InputState.notice`（直接拒绝 v2–v7）；引擎删掉全部提示文字，满四码空码时下一字母清掉空码、不上屏并开始新编码，空码空格/选重、越界翻页、组合中反引号、拼音超过 6 字母均静默忽略且组合不变；TSF 仅在有候选时显示候选窗，删掉提示行、图标及其两种颜色。
- 测试：引擎“满码空码被下一键顶掉、空码空格与选重不上屏”替换原提示断言；协议对超长 `commit_preview` 的拒绝替换原超长提示用例；候选窗测试核对无提示行时窗口高度恰为候选行加内边距。
- 统一入口于 2026-10-23 通过：架构/文档 5 passed，fmt、Clippy 无告警，x64、x86 各 **126 passed、8 ignored**；日志 `artifacts/no-notice-check-20261023.log`；补记部署与实测后复跑结果相同（`artifacts/no-notice-check-final-20261023.log`）。
- 部署（19:42，经用户要求）：打包日志 `artifacts/no-notice-package-20261023.log`，安装包 SHA256 `7c594fc0…0b0bcf`；经 `Shell.Application.ShellExecute` 静默安装成功（`artifacts/no-notice-install-20261023.log`），程序目录三个文件与 release 产物哈希一致。安装程序以原用户拉起的后台**仍挂在本会话作业对象内**（`Shell.Application` 在调用进程内执行，未经 explorer），故 `stop` 后改由桌面外壳视图 `FindWindowSW(SWC_DESKTOP)` 取得 explorer 进程内的 `IShellDispatch` 执行 `serve`：新后台父进程为 explorer、会话 1、17 个管道，且不在本会话作业内。
- 记事本实测（19:44，经电脑操控按键注入，截图 `artifacts/no-notice-notepad-20261023/`）：新进程载入新 DLL，`sr` 出“发”；`smpw` 后按空格、`=` 均无候选窗、无提示、未上屏，再按 `s` 组合变为 `s`（候选“说”），空码被顶掉且未上屏；`ecp` 无候选窗；`sr` 单页按 `=`、`-` 只显示候选；拼音反查 `` `yi `` 首页按 `-`、翻到末页（第 35 页）后再按 `=` 均只显示候选与页脚。结束时文档为空、标签页未修改，测试标签页已关闭。
- 未验证：其他宿主（Chrome、VS Code 等）与 x86 宿主；读屏器；拼音超过 6 字母的真机表现。安装前已打开、仍载入 v7 DLL 的程序须重新打开才能出字。

## 安装版只出英文与其他输入法候选框消失排查（2026-10-23）

- 用户报告：用过 TigerType 后微软拼音/五笔仍会候选框消失、只能盲打；装上安装版后 TigerType 在安装前后打开的程序里都只出英文。
- 只出英文（已定位并修复，**修复版未部署**）：安装版后台以普通权限运行，探针按 TIP 同一序列以 50ms 预算连接、Open/Focus/按键全部成功（`sr` 出“发”，往返 <1ms），排除后台与传输。跨进程以 `WM_IME_CONTROL` 读取：新开的记事本中 TigerType 已加载，但输入上下文开闭状态为 0（关闭），用过微软输入法的窗口均为 1；从外部把该记事本置为打开后，同一窗口立即能输入“发”。根因：TigerType 激活时从不打开 `GUID_COMPARTMENT_KEYBOARD_OPENCLOSE`，新线程管理器初值为空即关闭，TSF 不把按键交给 TIP；此前在 Notepad++ 中可用，是因为该线程先用过微软输入法、状态已是打开（小狼毫在激活时同样会打开并监听该状态）。修复：激活时打开，激活期间监听，被宿主或开关热键关闭即重新打开，停用时注销 sink；中英文仍只由单击 Shift 切换（`Ctrl+Space` 等开关热键因此不再关闭 TigerType）。合成线程管理器新增开闭状态容器，测试核对首次打开、被关闭后重开、停用后不再干预、已打开时不重复写入。Claude、Notepad++ 等安装前已打开的程序仍驻留暂存版 DLL，其暂存后台已停止，只能放行原键，须重新打开这些程序。
- 修复版部署（14:02，经安装包静默升级，日志 `artifacts/keyboard-open-install-20261023.log`，安装包 SHA256 `1c52c6bb…aa2724`）：新 DLL 映像 978944 字节，旧安装版 974848、暂存版 827392。16:30 核对各进程所载版本：只有之后新开的字符映射表与此后才激活 TigerType 的 VS Code 载入新版；ChatGPT、Chrome、资源管理器、jie、GLKVM、记事本仍是旧安装版，Claude、Notepad++ 仍是暂存版。DLL 进程期驻留，安装不能替换已载入的映像，用户此时“只有记事本能用”与此一致（记事本能用是因为曾被外部置为打开）。Chromium（Claude、ChatGPT、Chrome、VS Code 均基于它）激活 TSF 用普通 `Activate`、不设开闭状态，旧 DLL 在其中只要 TigerType 先激活就必然只出英文。新 DLL 在记事本以外宿主中的真实打字尚未验证：探针进程无窗口焦点时 TSF 不载入 TIP，两次申请电脑操控均被拒；已启动前台记录器（`artifacts/foreground-ime-watch-20261023.log`，记录前台进程所载 DLL 版本、开闭状态与是否连上后台）等待下一次手动测试。
- 17:44–18:02 记录：用户只重启了 Claude（换上新版）；TigerType 在 Chrome、ChatGPT（均为旧版）与 WhatsApp（新版）中连上过后台，即按键已送达 TigerType 预判；17:58 重开的 ChatGPT 聚焦数分钟从未载入 TigerType（该进程无签名/映像加载限制，排除拒载），会话当前输入法读回为微软五笔（`artifacts/active-tip-watch-20261023.log`）。TigerType 不在用户输入法列表（`HKCU\Control Panel\International\User Profile\zh-Hans-CN` 与 CTF SortOrder 只有微软拼音、五笔），仅因 HKLM 默认启用而出现在 Win+空格 切换列表：注销暂存 DLL 时被移出用户列表，安装包重新注册后未加回（安装完成页要求用户在设置中添加）。连上后台的宿主里是否仍出英文、各应用打字时实际选中的输入法，尚待记录器取证。
- 18:02–18:16 用户在设置中加回 TigerType 后于 Chrome、VS Code、ChatGPT、Claude 测试仍只出英文：记录期间无任何宿主连上后台、后台读计数为零，核实为**后台进程不在运行**（无 `tigertype.exe`、无管道、无崩溃事件）。14:02 的后台由会话内启动的安装包以原用户身份拉起，挂在 Claude Code 的作业对象下，17:51 用户重启 Claude 时随旧会话一并被结束；之后所有宿主连不上后台，只能放行原键。这是本次部署方式造成的，正常从资源管理器运行安装包或登录自启不经该作业。18:18 先 `stop` 掉一个仍挂在作业下的后台，再经资源管理器外壳的 `ShellExecute` 以 `serve` 重启后台（父进程为外壳 explorer，会话 1，17 个管道），探针出字正常；随后 Chrome 连上后台、后台读 +6，说明按键经预判、编辑会话与引擎交换。后台停止时 TigerType 只静默放行原键、没有任何提示，用户只能看到“只出英文”。
- 18:20 用户确认后台正常时 Chrome、VS Code、ChatGPT、Claude 均能出虎码。
- 其他输入法候选框消失（对照实验，记录 `artifacts/candidate-watch-20261023.log`）：全局订阅 UIA 菜单事件，确认微软输入法候选框由 TextInputHost 发出 `IME_Candidate_Window`（“Microsoft 候选项 UI”）打开/关闭事件，可不截图判定其是否出现；同时以低级钩子只统计物理/注入输入次数。结论与证据：①TextInputHost 自 13:29 重启后到 18:24 未再重启，其间 TigerType 经历暂存版/旧版/新版、后台停止、键盘关闭与反复互切，用户 18:24 物理打字时微软拼音候选框正常，TextInputHost 从未载入 TigerType，正常使用 TigerType 不是触发条件。②经用户授权向脚本新开的记事本注入按键（每批先核对前台，不符即中止）：微软拼音组字中切到 TigerType 再切回、TigerType 组字中切到微软拼音、候选框显示时关窗、组字中切走焦点，复查候选框均正常弹出。③仅用微软拼音、无 TigerType 参与：按人手速度（键间 120ms）注入 5+5 轮，候选框全部正常；一次性连发 30 轮，7 次在弹出约 10ms 后自行收起且未再弹出（该次组字全程无候选框，即“只能盲打”）；混合连发（拼音、空格上屏、退格、Esc、组字中途关窗/开窗）共弹出 245 次，124 次自行收起。即**快速/成批到达的按键会让微软输入法候选框在组字中途消失**，与 TigerType 无关；本次只复现到逐次组字的消失，“持续失效直到重启 TextInputHost”未复现。④本机输入来源：用户未敲键盘时仍每秒出现 1–2 次注入鼠标点击并切换前台，键盘输入显示为物理（存在虚拟 HID 设备），结合 ToDesk 窗口标题 `Mac-K2M9R2`、UAC 弹窗与电脑操控授权均无法在本机操作，判断用户经远程工具操作本机；另有 Codex 电脑操控组件在运行，此前的验收也用 Claude 电脑操控注入按键。这些来源的按键都可能成批到达，与“用 TigerType 测试后出现”的相关性由此而来。实验窗口已全部结束，会话输入法已切回 TigerType，记录器已停止。
- ChatGPT 进程下运行着 `codex-computer-use-swift.exe`（Codex 电脑操控组件，会注入输入），与“电脑操控注入按键后微软拼音候选框消失”的公开报告同类，列为其他输入法候选框消失的主要嫌疑，未做对照实验。
- 候选框消失（**根因未确定**）：用户确认当时已处于坏状态，重启 TextInputHost 后恢复。排除项：记录全桌面 IME WinEvent 后，单测对从未可见的候选窗发出 `IME_SHOW` 且销毁时没有 `IME_HIDE`（每次必跑检查都会如此），之后微软输入法仍正常，未配对事件不是触发条件；之后用户用键盘在新记事本中以 TigerType 输入并上屏，事件成对，微软输入法仍正常。仍未排除：计算机操控/远程控制工具注入按键（公开报告的同类现象，本机另有 ToDesk、GLKVM 与其他自动化程序在运行），以及 TigerType 留下的“键盘关闭”状态与微软输入法切换的交互（修复后不再出现该状态）。本会话请求电脑操控授权被拒，未做注入对照实验。
- 统一入口于 2026-10-23 通过，每个位数 **126 passed、8 ignored**（新增开闭状态测试 1 项）；日志 `artifacts/keyboard-open-check-20261023.log`。首次运行仅因测试代码未格式化失败，格式化后复跑通过。

## 安装包与查字窗并入后台（2026-10-23）

- 用户要求：做成安装包以便在各电脑安装；选用 Inno Setup；查字/设置窗须与输入法同时打开使用，保存设置后热重载而非暂停或重启后台。
- 打包工具（用户批准下载）：Inno Setup 6.7.3 取自官方 GitHub 发布，SHA256 `9c73c3ba…66e97b732` 与发布摘要一致、签名有效（Pyrsys B.V.），以 `/CURRENTUSER` 静默装到 `%LOCALAPPDATA%\Programs\Inno Setup 6`（未用管理员），日志 `artifacts/inno-setup-install-20261023.log`。官方安装包不含简体中文，取同一标签的 `Files/Languages/Unofficial/ChineseSimplified.isl`（`7d544b9b…a236e1278`）放 `vendor/inno`，打包驱动核对摘要。AGENTS 记录此为唯一非 Rust 工具例外。
- 查字并入后台：删除独立 `lookup-window <包> <目录>` 入口与 `lookup::run`；`backend::run` 在同进程起固定查字工作线程（借用引擎线程的只读词表，独占受管目录租约）与固定 UI 线程（首次唤起才建窗）。协议 v7 加入管理消息 `Show`/`Shown`（v7 未部署，不另升版），后台核对调用进程映像后把唤起信号投入 UI 邮箱；未接 UI 的后台拒绝唤起。关闭主窗只隐藏（连同设置窗与草稿）；后台停止时唤起邮箱断开，UI 与工作线程退出并 join。`lookup-window`（无参数）请本会话后台显示查字窗，先 `AllowSetForegroundWindow(ASFW_ANY)`；无后台时本进程即以安装版位置成为后台并立即显示。
- 热重载：工作线程保存成功后把设置放入只留最新一份的交接槽并置事件；引擎线程等待完成事件与该事件，每轮先取交接槽再处理请求。新会话随 `Open` 用新设置；调度器给已打开会话记设置版本，在无组合的下一次请求（含焦点）时 `configure`，组合中保持旧设置以免改掉宿主已显示的候选与编号。交接失败时向设置窗报告“已写入但未交给后台”。
- 入口：`serve` 省略路径即用安装版位置（程序目录 `tiger-v1.json`、`%APPDATA%\TigerType`）。`tigertype.exe` 改为 GUI 子系统，无标准句柄时附加父控制台；无控制台时 `serve`/`lookup-window` 失败弹错误框，`stop` 等不弹窗（供安装程序隐藏调用）。`.cargo/config.toml` 让两种 MSVC 目标静态链接 CRT；dumpbin 核对三个 release 产物均无 `VCRUNTIME140`/`api-ms-win-crt-*` 依赖、子系统为 Windows GUI（`artifacts/installer-dependents-20261023.log`）。
- 安装包：`installer/tigertype.iss` 与 `cargo run --locked --offline -p tigertype-app --example package`（打包驱动示例设 `test = true`，其测试纳入必跑检查）。驱动构建 x64/x86 release、核对 `vendor/fonts` 与锁定清单及脚本中的字体行一致、核对语言文件摘要、由 `vendor/tiger` 重新生成数据包（117,662 条编码）到新建目录，ISCC 编译后只删该文件与空目录；ISCC 不认 `\\?\` 前缀，驱动对本地盘符路径去掉前缀（首次因此编译失败）。产物 `target\package\TigerType-Setup-0.1.0.exe`，16,084,223 字节，SHA256 `cb0c3f32ef24bcff6df25ec211b82205a3af31226033040e1cc6b6cbfbca6499`；打包记录 `artifacts/installer-package-20261023.log` 列出七个打包文件。安装行为见 README“安装包”。
- 冒烟（未安装、未注册）：release 后台以新建目录 `.cache\install-smoke-20261023` 启动，`lookup-window` 退出码 0，后台进程出现“TigerType 查字”窗口；本工具进程不在前台，`AllowSetForegroundWindow` 无效，窗口未到前台（用户从开始菜单点击时的前台切换未实测）；`stop` 退出码 0，后台 2 秒内退出，错误输出为空（`artifacts/install-smoke-*-20261023.log`）。
- 新增测试：调度器热重载等组合结束（`routing.rs`）；后台在限时子进程内经真实管道验证唤起信号进入 UI 邮箱、交接后的 `Open` 拿到新设置（`backend::tests`）；未接 UI 的后台拒绝唤起（`backend_process.rs` 反例）；关闭主窗只隐藏、唤起邮箱断开才退出（ui 隐藏窗口测试，用 `WM_QUIT` 证明 CLOSE 未提前返回）；脚本与字体清单同步（正反例）及路径前缀处理。
- 未验证：在任何电脑上安装（HKLM 注册、DLL 注册顺序、全机字体、Run 自启、`runasoriginaluser` 启动后台）、升级改名与重启删除、卸载（提升权限的 `stop` 能否被接受）、安装向导中文显示、从开始菜单唤起时窗口前置、终端直接运行时的控制台输出、热重载在真实候选窗中的效果、在查字窗里用 TigerType 输入（TIP 载入后台进程自身）、Win10。
- 统一入口于 2026-10-23 通过，每个位数 **125 passed、8 ignored**（新增 ignored 为限时父测试专用的后台子进程用例）；文档定稿并补上首次唤起前置后复跑同样通过，日志 `artifacts/installer-check-final-20261023.log`（首轮 `artifacts/installer-check-20261023.log`）。冒烟所用程序早于首次唤起前置的修改。
- 本机迁移：本机仍有 C 批暂存注册（`C:\TigerType-Staging`，CLSID 相同）；安装前须按 README 停止暂存后台并注销暂存 DLL，否则两处注册与后台互相冲突。README 删除了被安装包取代的手动暂存/更新步骤。

## C 批暂存（2026-10-22，经用户授权执行）

- release 构建 `tigertype-app`、`tigertype-tsf`（x64）与 `tigertype-tsf`（x86）通过，日志 `artifacts/staging-release-build-20261022.log`。
- 确认 `C:\TigerType-Staging` 原不存在后新建，复制 `tigertype.exe`（x64，PE 0x8664）、`tigertype_tsf.dll`（x64，0x8664）与 x86 DLL 改名 `tigertype_tsf_x86.dll`（0x014C）；SHA256 依次为 `5E0035AB…2BF809`、`E4021C2A…20D702`、`37E4D95A…F2D865`，与构建产物一致。`icacls … /grant *S-1-15-2-1:(OI)(CI)(RX)` 成功，三文件均继承该 ACE；日志 `artifacts/staging-acl-grant-20261022.log`、`artifacts/staging-acl-verify-20261022.log`。
- 目录原继承 `C:\` 默认的 Authenticated Users 修改权；注册后 DLL 会加载进所有使用 TSF 的进程（含管理员进程），可被非提升进程替换而成提权途径。经用户授权按 README 加固：`/inheritance:r` 后只授 Administrators、SYSTEM 完全控制与 Users、ALL APPLICATION PACKAGES 读/执行。核验三文件只继承这四项、无 Authenticated Users，当前非提升身份以写方式打开三文件均被拒（`UnauthorizedAccessException`），哈希未变；日志 `artifacts/staging-acl-harden-20261022.log`、`artifacts/staging-acl-harden-verify-20261022.log`。
- 残留风险：目录所有者仍是当前用户，所有者隐含修改 DACL 的权限，同用户恶意进程可先改权限再替换 DLL。彻底消除须以管理员把所有者改为 Administrators，或加 OWNER RIGHTS 只读项；均未授权、未执行。加固后删除暂存目录需要管理员。
- 以锁定资源生成 `.cache\tiger-v1.json` 并校验通过：117,662 条编码、17,047 条单字拼音、99,142 条学习资料，与完整资源验收一致；日志 `artifacts/staging-package-20261022.log`。
- 用户首次在管理员 cmd 中按 README 原 PowerShell 写法注册失败（RegSvr32“找不到指定的模块”）：cmd 不认单引号，引号成了路径一部分，DLL 未加载、`DllRegisterServer` 未执行，系统未改动。只读核对两种位数 DLL 依赖（user32、ole32、oleaut32、uiautomationcore、`VCRUNTIME140.dll`、UCRT 等）在 System32/SysWOW64 均存在；README 注册命令已改为 cmd/PowerShell 通用的双引号。部署注意：DLL 动态依赖 VC++ 运行库，无该运行库的机器无法加载，安装阶段须静态链接 CRT 或随附运行库。
- 用户随后以双引号、管理员身份注册两种位数成功。只读核验（`artifacts/registration-verify-20261022.log`）：HKLM CLSID 的 64 位视图 `InprocServer32` 指向 `tigertype_tsf.dll`、32 位视图指向 `tigertype_tsf_x86.dll`，均为 `Apartment`；`CTF\TIP\{CLSID}` 下 0x804 配置 `{2097C544…}` 描述“TigerType 虎码”、`Enable=1`，键盘/沉浸式/托盘三类别齐全（该键两视图内容一致）。
- 以当前非提升身份从 `C:\TigerType-Staging\tigertype.exe serve` 启动后台，新建受管数据目录 `.cache\backend-data`；确认进程映像为暂存路径，所有权管道与 16 个请求管道已建立。后台输出 `artifacts/acceptance-backend-20261022.log`。
- 用户在系统设置添加键盘并在 Notepad++ 中切换到 TigerType。

## C 批第一轮真实宿主验收（Notepad++ x64，2026-10-22）

- 方式：经用户授权以 computer-use 向 Notepad++ 注入按键并截图；文本以状态栏 UTF-8 字节长度与列号核对。
- 通过：组合内嵌显示与候选窗跟随组合矩形；空格上屏首选；四码 `srid` 出“发”，第五码顶屏；Esc、Tab 清空；退格删码；拼音反查 `` `han `` 九候选带编码注释；`=`/`-` 与 PageDown 翻页、↑↓ 移动高亮；数字、`;`、`'` 选重；未组合时 `,` `.` 与双引号配对出全角；单击 Shift 中英切换；`Ctrl+.` 英文标点；组合中 `Ctrl+L` 显示学习信息；回车上屏原码；鼠标点击第二候选经异步会话上屏；未组合时 Esc 放行给 Notepad++；后台停止再启动后自动重连恢复。
- 问题 1（严重，已修复待部署）：后台停止后按 `a`、`b`、`,` 全部丢失（长度不变）。证实宿主（Notepad++/CUAS）不会交还“`OnTestKeyDown` 吃下、`OnKeyDown` 放行”的原键。修复：预判前确认后台可用（已有连接以非阻塞检查复用，否则 50ms 期限内只做握手），连不上即不吃键；Shift 抬起同理。
- 问题 2（已修复待部署）：组合中点击行首，宿主结束组合（`u` 留作正文，属宿主行为），但候选窗不收起。修复：组合 sink 收到宿主终止时通知收起候选窗。
- 问题 3（已修复待部署）：用户要求界面不得有任何蓝色；候选窗高亮原用系统选中色（蓝）。修复：固定中性灰配色（背景 F9F9F9、高亮行 DCDCDC、正文 1A1A1A、注释 707070），仅用户开启高对比度时用系统颜色；测试核对默认配色 R=G=B。查字/设置窗的标准控件选中色仍为系统蓝，尚未处理。
- 观察：Notepad++ 自带“输入时自动补全”会在上屏后弹出其蓝色建议列表，非 TigerType 界面。
- 用户报告用过后其他输入法（微软拼音/五笔）候选不显示、只能盲打。只读排查：TextInputHost、ctfmon 均在运行且未加载 TigerType DLL（TigerType 已加载于 claude、Code、explorer、notepad++、WhatsApp、WindowsTerminal 等获得过焦点的进程）。公开报告（Claude computer-use 在 Windows 注入按键后微软拼音候选窗消失，重启 TextInputHost.exe 即恢复）与现象一致，倾向由本轮 computer-use 注入按键引起；尚未通过重启 TextInputHost 并手动复测确认，不能排除 TigerType 相关。
- 修复后统一入口于 2026-10-22 通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **113 passed、6 ignored**（同前口径，tsf 新增 4 项）；日志 `artifacts/acceptance-fix-check-final-20261022.log`。首次运行因测试中常量断言被 Clippy 拒绝失败，日志 `artifacts/acceptance-fix-check-20261022.log` 保留。修复版 release DLL（x64 `6322B084…`、x86 `7160A3AA…`）已构建，日志 `artifacts/acceptance-fix-release-20261022.log`；后台程序无改动。
- 未执行：修复版部署与复测、x86 宿主、管理员宿主、UWP（开始菜单/设置搜索）、读屏器、其他输入法在手动输入下的复核。

## C 批第二轮验收（修复版，2026-10-23）

- 用户以管理员按 README 改名旧 DLL 并复制修复版，重启 TextInputHost。只读核验：暂存目录 x64/x86 DLL 哈希为 `6322B084…`/`7160A3AA…`，与构建一致，旧文件改名保留（`*.old1.dll`），新文件继承加固后的 ACL。替换前已启动的进程（含未真正退出的 Notepad++ 5912）仍映射旧版，故改在替换后新开的 Windows 记事本（商店版 x64）中复测；用户在其中切换到 TigerType。
- 配色：像素读回候选窗边框 `D6D6D6`、高亮行 `DCDCDC`，无蓝色（压缩截图中浅灰几乎不可见）。
- 组合终止：组合中点击编辑区别处，可见的 TigerType 候选窗数量为 0，`u` 留作正文。
- 后台离线：`stop` 后按 `a`、`b`、`,`，三键原样进入记事本（`uab,`），不再丢键；重新 `serve` 后下一键自动重连，`sr` 出“发”并上屏。
- x86 宿主：`SysWOW64\notepad.exe` 被系统转到 x64 商店版（多开了一个空窗口）；改用 32 位 `SysWOW64\charmap.exe`，其“复制字符”框中组合 `u` 出候选、空格上屏“的”；32 位 PowerShell 读回该进程加载 `tigertype_tsf_x86.dll`（替换后启动），与 x64 后台正常通信。
- 本轮仍以 computer-use 注入按键，可能再次影响 TextInputHost（见上一轮观察），验收后须重启 TextInputHost 并手动复核微软拼音/五笔。
- 未执行：管理员宿主（UIPI 阻止注入，须用户手动）、UWP 开始菜单/设置搜索（外壳不在授权范围，须用户手动）、读屏器、长时使用；查字/设置窗的系统蓝色选中色已在下一节处理。

## 查字窗去蓝与阿拉伯数字标号（2026-10-23）

- 用户要求：选中文字不得为系统蓝色；选字标号改用阿拉伯数字。
- 标号：引擎生成的候选标号由 `㊀…㊈` 改为 `1…9`，与数字键选重对应；引擎测试核对每页标号从 1 起。标号由后台生成，须替换暂存目录的 `tigertype.exe` 后生效，TIP DLL 无需更换。
- 查字/设置窗（未加载 v6 控件清单，经典控件本身无蓝色）：两个文本框（查询框、只读资料框）子类化，系统 EDIT 用系统选中色画完后，在同一回调内按字符位置以中性灰（背景 `DCDCDC`、文字 `1A1A1A`）重画仍可见的选区，期间隐藏插入符；选择、复制与键盘行为不变。“输出字形”下拉框改为自绘（保留字符串供键盘导航与读屏器），选中项为灰色，随 DPI 调整项高。仅用户开启高对比度时沿用系统颜色。
- 测试：隐藏窗口中核对两个文本框已换用重画过程；把跨行选区画到内存位图读像素，选中的“乙丙/丁戊”为 `DCDCDC`、未选中的“甲/己”保持白色；下拉选中项为灰、未选项为窗口底色；编译期核对选区配色 R=G=B。与既有隐藏窗口测试合并执行（窗口类同名，不能并行注册）。
- 统一入口于 2026-10-23 通过，每个位数 **113 passed、6 ignored**（同前口径）；日志 `artifacts/no-blue-labels-check-final-20261023.log`。首次运行时用于实机核对的 debug 查字窗占用 `target\debug\tigertype.exe`，Cargo 无法重链接而失败，关闭后复跑通过，失败日志 `artifacts/no-blue-labels-check-20261023.log` 保留。release `tigertype.exe`（`DA2EECE5…`）已由用户以管理员复制到暂存目录，后台以新版重新启动（普通权限，管道 17 个就绪）。替换前已启动的程序（Claude、VS Code、资源管理器、Notepad++、Windows Terminal）仍驻留旧 DLL、候选窗高亮仍为蓝色，须完全退出重开或注销后生效。
- 实机：查字窗打开时当前输入法为微软五笔，为避免注入按键影响 TextInputHost，未自动输入与拖选；可见效果待用户手动核对。窗口标题栏/边框由系统 DWM 绘制，若用户开启“在标题栏和窗口边框上显示强调色”会显示强调色，不属应用内绘制，未处理。

## 界面重设计、配色主题与候选字体（2026-10-23）

- 用户批准设计预览稿（画布 `TigerType 界面设计预览`），选定：候选字体默认霞鹜文楷、保留楷体可切换，编码字体 Cascadia Mono，窗口文字微软雅黑 UI；要求多套配色可切换、全部无蓝、不影响性能。
- 契约：`Options` 增加 `theme`（琥珀/墨/竹/夜/自定义）、`custom`（背景、文字、高亮、强调四基色，其余按比例混色推导）与 `candidate_font`；`Options::validate` 拒绝超出 RGB、偏蓝（蓝分量高于红或绿）与任一文字对比度低于 4.5:1。设置窗保存前本地校验，后台写入与载入 `settings.json` 时再校验，失败不落盘；旧 `settings.json` 缺新字段会被拒绝（开发期不兼容），须删除后重新保存。协议 v7，直接拒绝 v2–v6。外观随 `InputState` 交给候选窗，DLL 不读盘。自定义主题契约与 app 校验由分叉会话完成，其余由本会话完成，两边按文件分工未交叉写入。
- 候选窗：圆角高亮加左侧指示条、注释列对齐、页脚（页码与“拼音”标签）只在可翻页或拼音反查时出现；纯 GDI，内存位图双缓冲、库存 DC 画刷/画笔改色（逐键不新建画刷），三种字体按 DPI 与候选字体缓存，GDI 静默替换缺失字体时按实际字体名回退；Win11 由 DWM 画圆角与主题色边框，Win10 退回直角自绘外框。未增加线程、定时器、读盘或 IPC。
- 查字/设置：不启用系统控件主题；父窗口自绘底色、卡片与输入框外框，按钮/开关/分段单选子类化自绘（保留 BUTTON 复选/单选语义供键盘与读屏器），列表与字形格自绘；查字改为字列表加详情卡（田字格字形、编码点、四项资料为可选中的只读 EDIT），“复制本页资料”经隐藏 EDIT 标准复制；设置为七行卡片并新增“取消”。Win11 标题栏与边框改用主题色，深色主题用浅色标题按钮。拖动缩放结束时补发一次布局（系统模态循环会吞掉投递的布局消息）。
- 测试：契约逐个核对各主题无蓝与对比度（含系统蓝、青、偏蓝灰反例）；候选窗在内存位图读回窗口底色、高亮底色与指示条像素，核对 0xRRGGBB→COLORREF 字节序、页脚出现条件与缺失字体回退；查字/设置核对选区为主题选区色、选中分段为强调色、最小尺寸布局不越界、主题/字体/自定义色往返与偏蓝/格式错误被拒、保存回复换上新主题与字体但不重写资料。本机临时探测确认霞鹜文楷、楷体、Cascadia Mono 均被正确识别（未入库）。
- 统一入口于 2026-10-23 通过，每个位数 **118 passed、6 ignored**；日志 `artifacts/ui-redesign-check-20261023.log`。首次运行因分叉会话写入的契约测试未格式化失败，第二次因 Clippy 多余类型转换与缺少 SAFETY 注释失败，修正后通过；失败日志未单独保留。
- 未验证：候选窗真机视觉（Win11 圆角/阴影/边框色、Win10 直角外观）、子类化按钮覆盖重画是否闪烁、读屏器、多显示器、UWP 宿主中当前用户字体的可见性、生僻字与字根字形覆盖；设置仍须重新启动后台才用于输入；拖动缩放过程中控件要到松手后才重排。查字/设置窗的实机复核见下节。

## 查字/设置窗实机复核与随包字体（2026-10-23）

- 实机复核（Win11、150% 缩放，debug 查字工具，新建数据目录 `.cache\ui-redesign-lookup`）：查询文字经 `WM_SETTEXT` 填入、按钮以 `BM_CLICK` 或鼠标触发，未向五笔注入按键；截图按物理像素保存为 `artifacts/ui-visual-{lookup,settings}-{before,after}-20261023.png`。暖色底、卡片、输入框强调底边、琥珀主按钮、列表高亮与指示条、田字格字形、开关与分段单选、主题色标题栏均与设计稿一致。
- 复核发现并修复：设置窗行标题/说明控件编号与“输出字形”说明、“原字”单选重号，致“自定义配色”行标题不显示、“原字”字号变小（重新编号，并在隐藏窗口测试中断言两窗控件编号唯一）；查字窗最小尺寸也作用于固定尺寸的设置窗，把它撑宽（只限制可缩放窗口）；初始尺寸未按 DPI 换算，150% 下被夹到最小尺寸并出现列表滚动条（按窗口 DPI 设初始尺寸）；对话框管理器随焦点改写 `BS_DEFPUSHBUTTON`，点击后“查询”变成次按钮外观（主按钮按编号认定）；EDIT 换字体按字号重设边距，“拆分”值比其他资料右移（统一清零）。助记下划线按系统规则在最后输入为键盘时显示，已补 `UIS_INITIALIZE` 初始化，不再强改。
- 随包字体（用户批准下载）：霞鹜文楷屏幕阅读版与 TumanPUA 取自虎码官网盘 `huma.ysepan.com` 的“虎码方案必装字体 2026.08.15.7z”（SHA256 `d80340c8…cabcb7`；命令行到其下载域的 TLS 连接被重置，改由内置浏览器下载后移入独立目录，只解出两个字体与说明）；说明写明其中文楷为融合虎码字根版，与本机已装文件哈希一致，故选它而非上游 v1.530（上游 `LXGWWenKaiGBScreen.ttf` 已按发布方 SHA256 `0df4009a…f1e503` 下载核对，留作备选，未入清单）。Cascadia Mono 取自官方 `CascadiaCode-2407.24.zip`（`e67a68ee…0c8474`）中的 `ttf/CascadiaMono.ttf`。三者大小与 SHA256 锁定在 `resources/fonts.lock.json`，文件在 `vendor/fonts`（不入 Git）。楷体与微软雅黑 UI 为微软系统字体，不随包分发。
- `install-fonts <目录>`：先校验全部来源（大小、SHA256、拒绝链接与路径逃逸）再动手；复制到 `%LOCALAPPDATA%\Microsoft\Windows\Fonts`，已有同名同内容则复用、内容不同则拒绝且不覆盖；任一登记项已指向该文件即不重复登记（本机文楷以本地化名称登记过），否则写当前用户字体登记；新装或新登记的字体经 `AddFontResourceW` 加入本会话并广播 `WM_FONTCHANGE`（挂起窗口最多等 1 秒）。测试用合成字体与私有注册表配置单元：首装复制并登记、重装不改动、不覆盖用户同名文件、来源被改动则一个都不装、路径逃逸被拒；单列的完整资源测试核对 `vendor/fonts` 三个真实字体与清单一致（已通过）。经用户要求于本机执行 `install-fonts`：霞鹜文楷、TumanPUA 已存在且内容一致、已登记，未改动；Cascadia Mono 新复制到用户字体目录（`714804` 字节，SHA256 与清单一致）并登记为 `Cascadia Mono (TrueType)`；随即再运行一次，三者均“已存在，未改动”。本机 `C:\Windows\Fonts` 另有系统自带的另一版 Cascadia Mono（同族名、内容不同），两者并存，由系统择一使用。换电脑后的可用性与 UWP 宿主可见性未实测。
- 统一入口于 2026-10-23 通过，每个位数 **120 passed、7 ignored**（新增字体安装与清单测试，及单列的 vendor 字体核对）；日志 `artifacts/ui-fonts-check-20261023.log`，复核修复后的上一轮通过日志 `artifacts/ui-redesign-visual-fix-check-20261023.log`。

## C 批第一步：注册入口

- `crates/tsf/src/register.rs`：DLL 新增导出 `DllRegisterServer`/`DllUnregisterServer`。注册先校验 DLL 为绝对路径且同目录有 `tigertype.exe`，再写 HKLM\SOFTWARE\Classes\CLSID\{79A65161-8689-43AC-A57C-574C6A74922A}（名称、`InprocServer32`、`ThreadingModel=Apartment`），然后在单线程套间内以 `ITfInputProcessorProfileMgr::RegisterProfile` 登记简体中文 0x0804 语言配置 `{2097C544-B7D6-4F5C-9F4D-6DB1BE3EE319}`（“TigerType 虎码”，默认启用，暂无图标），以 `ITfCategoryMgr` 登记键盘 TIP、沉浸式、托盘三类别；任一步失败执行注销回滚。注销依次撤销配置、类别并删除 CLSID 子树（不存在视为已删除），单项失败不阻止其余步骤，返回第一个错误。套间若已由调用方以其他模式初始化则不重复反初始化；导出截住 panic。
- 暂存与位数：宿主只连接与 DLL 同目录的后台且核对映像路径，因此 x64/x86 DLL 放同一暂存目录（x86 改名 `tigertype_tsf_x86.dll`）共用一个后台，分别由 System32/SysWOW64 的 regsvr32 注册；README 记录构建、复制、对 `S-1-15-2-1` 授读/执行、注册、启动后台、注销与删除步骤，全部须用户逐步授权后手动执行。
- 测试（tsf 共 31 项，新增 3 项）：注册内容（键路径、默认值、`InprocServer32`、线程模型、语言、类别、配置 GUID 不同于 CLSID）；校验拒绝相对路径、缺 DLL、缺后台；注册表写入/删除在 `RegLoadAppKeyW` 加载的临时私有配置单元中往返非 ASCII 路径，删除可重复。确认测试后临时目录无残留。TSF 配置/类别调用会写 HKLM，未在测试中执行。
- DLL 现导出四个函数，x64/x86 均为未修饰导出名：`artifacts/tsf-c-register-dll-exports-20261022.log`、`artifacts/tsf-c-register-dll-exports-x86-20261022.log`。
- 统一入口 `cargo run --locked --offline -p tigertype-app --example check` 于 2026-10-22 实际通过架构/文档、fmt、Clippy、x64/x86 单元/集成/文档测试，每个位数 **110 passed、6 ignored**（同前口径）；日志 `artifacts/tsf-c-register-check-final-20261022.log`。前一次运行期间仍在改代码，日志 `artifacts/tsf-c-register-check-20261022.log` 保留但不作结论。
- 实现批次内注册入口未在本机运行，HKLM、TSF 配置与系统设置中的键盘列表均未改动。

## B2：宿主内候选窗、UIA 与点击选字

- 候选窗（`crates/tsf/src/candidate.rs`）：TSF 线程惰性创建、停用时销毁；窗口类以 TIP 模块注册（重复注册视为已存在，模块驻留不注销）；`WS_EX_NOACTIVATE|WS_EX_TOOLWINDOW`、`WM_MOUSEACTIVATE` 返回 `MA_NOACTIVATE`；owner 取 `ITfContextView::GetWnd`，空则 `GetFocus`，x86 用 `SetWindowLongW` 改 owner。自绘九行内候选（序号+候选字+灰色注释，省略号截断），无提示行，系统颜色兼顾高对比度；未声明 DPI 感知，字号按 `GetDpiForWindow` 缩放；按组合矩形放在工作区内，下方不足翻到上方。窗口过程经本线程登记表以弱引用找回候选窗，窗口数据不存指针，panic 不穿过回调。
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
- 下一步：按 README 迁出本机 C 批暂存注册后，由用户运行 `TigerType-Setup-0.1.0.exe` 安装，验收安装/自启/开始菜单唤起/热重载/升级/卸载，并在真实宿主（含 x86、管理员、UWP 开始菜单/设置搜索）复测；再到另一台电脑（含 Win10、无 VC++ 运行库）验证。
- 安装包风险：提升权限的卸载 `stop` 可能被后台拒绝，此时后台运行到注销；升级时其他已登录用户的旧后台继续运行旧程序，与新 DLL 协议不一致时其宿主按离线放行原键，直到其注销；安装中途失败时旧程序已被改名，须重新运行安装包。从已提权的进程启动安装包时，安装结束拉起的后台也是提权的，所有宿主拒绝连接、普通 `stop` 也停不掉，须提权结束后以普通身份重启后台。任务栏图标让任一已连接宿主都能唤起查字/设置窗（同会话进程本可做到，影响仅限弹窗）。查字窗与后台同进程，查字窗故障（窗口创建失败）不影响输入，但后续唤起报错，须重启后台。
- 注册入口风险：从未执行，HKLM 写入、`RegisterProfile`/`RegisterCategory` 的实际效果、回滚与注销的完整性、默认启用后系统设置中键盘列表的表现均未验证；语言配置无图标；重复注销时系统对未登记项可能报错。AppContainer 用例每次重跑仍须用户授权。独立窗口与候选窗的可见外观、字体/DPI、读屏器仍须验收；组件测试不是端到端输入法。
- B2 已知限制：候选窗用系统颜色与 Microsoft YaHei UI 15pt，未按 Win11Light 锁定配色、字体优先顺序、圆角或 PUA 字根字体；长注释以省略号截断。宿主无布局且无系统光标时不显示候选窗。点击排队会话若宿主始终不执行，同一展示的后续点击被挡住，直到下一次展示更新。UIA 未实现选择容器模式，`Select` 被拒；读屏器效果未实测。候选窗 owner 取自上下文视图或焦点窗口，跨线程 owner 的输入队列影响未验证。
- B1 已知限制：后台未运行时 `OnTestKeyDown` 仍按默认状态吃字母/标点，`OnKeyDown` 连接失败后放行；宿主是否在此情形把原键交还应用须在 C 批实测，若不交还需改判定策略。单击 Shift 判定不感知鼠标，Shift+点击选择后松开会切换中英。后台回收或重启后新会话恢复中文模式，不保留宿主侧英文模式。组合中按 Home/End/Delete 等未映射键会交给应用，下一键按外部改写处理。
- AppContainer 子进程测试已确认须创建并删除临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`）；C 批须对暂存目录授予 ALL APPLICATION PACKAGES 读取/执行，并由用户手动注册。两者都须在执行前单独授权。
- 放开低完整性后，同会话沙箱进程都能连接并占槽；回收只能缓解，同会话恶意代码造成的拒绝服务不在防护范围。后台须以非提升权限运行：提升启动时管道所有者为 Administrators，宿主会拒绝连接。微软提示绕过 AppContainer 规则的输入法可能被拦截；本方案只用显式 ACL 授权，该风险未完全排除。
- 独立工具退出等待当前工作真正结束；磁盘操作异常阻塞可能延迟进程退出，未实现强杀或假定超时即终止。设置冲突会保留外部文件与已确认选项，需关闭工具后重新打开读取外部版本。
- 默认 IPC 仍仅限 EXE；TIP 激活时显式选择进程期驻留。跨进程测试与合成 COM 宿主/线程管理器测试不能替代真实应用、管理员宿主、焦点切换和混合位数端到端故障验收；不以空壳 COM 导出或模拟键盘注入声称完成。
- DLL 首版驻留选择不可逆系统 PIN；必须等宿主退出才释放模块映像，不支持热更新/热卸载。管道关闭超时仍可能留有活跃引用，不能据此结束生命周期或释放关联资源。
- 字体覆盖、无障碍、真实应用、50ms 总预算和长期性能未验证。输入核心测试不代表 Windows 输入法验收。
