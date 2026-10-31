# 项目状态

更新：2026-10-31。范围与目录职责见 [实施方案](implementation-plan.md)，命令见 [README](../README.md)。本文只维护当前实现、未解决问题和验证；旧批次过程在 Git 历史，原始证据在 artifacts（不入库）。

## 当前状态

- Windows 输入链路、查字/设置与安装器已有实现；源码协议 v14，直接拒绝旧版本。目录重组不改变输入行为，不代表候选残留或高负载真机问题已修复。
- macOS 本机为 macOS 26.6（25G72）/ARM64、Command Line Tools 与 Rust 1.99.1。系统发现、IMKServer 类簇启动崩溃、激活文本对象绑定与菜单 action 已修复并部署。Codex 每键重置组合及上屏缺字修复均已安装，用户确认四码有效、“𤛭、𨰻、𪚥”已能显示；安装器今后同时安装供宿主使用的文楷。其他真实应用与完整生命周期未验收，M5 未完成。
- Linux 仅有方案位置，不建立空目录或空 crate。
- **10-30 本轮只读复核：**本机已重新安装，x64/x86 COM 注册和自启项存在，后台 PID 15848 自 10-29 21:43 运行；系统启动时间约为 10-29 21:42，重启删除队列中未发现 TigerType 项，`C:\TigerType-Staging` 不存在。现用 EXE/两个 DLL 的 SHA256 均与当前重构的 Release 产物不同，不能拿现用构建替当前提交验收。未读取个人词库或配置；本轮尚未升级、注册或切换输入法。证据见 `artifacts/windows-closure-20261030/deployment-before.json`、`build-hashes-before.json`。

| 组件 | 现状 |
|---|---|
| 数据/引擎 | 锁定码表原文随仓库入库（`resources/tiger`），启动时按清单核对后直接解析单字与 tigress 词码，不再生成数据包；单字/词模式（默认单字）、常用字过滤、简繁、拼音反查、查词、全角会话开关、满码顶屏 |
| 多宿主后台 | 16 槽请求管道、AC/低完整性 DACL、双侧认证、懒连接重连一次、LRU 回收（策略在 backend 调度器，反应器只补传输条件）、serve/stop；查字/设置 UI 与工作线程在同进程，设置保存即热重载 |
| TSF TIP | 两种位数；按键同步编辑会话、宿主内自绘候选窗与 UIA（宿主布局通知补位/移动）、点击异步选字、任务栏输入法图标（中/英，单击切换；右键菜单为显示当前状态的中文/英文、半角/全角、中文/英文标点，加查字、设置）；注册时登记同目录 `tigertype.ico` 为配置图标；激活时打开并监听键盘开闭状态 |
| 通知区图标 | 后台常驻，虎码印章图标；单击开设置、右键菜单为查字/设置；任务栏显示本输入法图标时隐藏（两处同一时刻只显示一个） |
| 查字/设置 | 主题自绘（琥珀/墨/竹/玫红/自定义，全部无蓝）。设置窗 720×520 分输入/外观/快捷键/高级四区，修改即时保存、超出时滚动，自定义四色可点色板或输入色值并即时校验；配置为带注释的 `config/settings.toml`，可打开并重新载入。查字窗 520×400：查询框、横排字块、详情卡（编码键帽、拆分字块、复制当前字） |
| 安装包 | Inno Setup 6.7.3；全机安装、两种位数注册（含配置图标 `tigertype.ico`，打包时现画）、随包字体、HKLM Run 自启；升级先停后台并等其退出，准备页经重启管理器列出仍载现版或历次 `*.old` 旧版的程序供关闭/重启，不关的改名后照常安装，装完清掉无人使用的 `*.old`；静默安装/升级及不关闭应用的卸载已在本机实测，交互准备页、自动关闭真实程序、卸载后的重启清理与其他电脑（含 Win10）未实测 |
| 长期使用 | 未开始 |
| macOS 端 | 系统发现、启动崩溃修复及本机更新部署已有证据；真实输入、完整安装生命周期及 M5 仍待验收 |
| Linux 端 | 只预留接入位置，未定框架（IBus 或 Fcitx5） |

## 本批：Mac 菜单栏图标对齐

- 用户截图中虎码印章明显低于其他菜单栏图标。已安装 PNG 是 36×36 像素、72 DPI，AppKit 实际加载为 36×36 点；系统自带输入源图标为 16×16 点。打包绘制现改为 32×32 像素，并在绘制结束后设置 16×16 点逻辑尺寸，以 144 DPI 保存。
- 原生回归实际重新解码输出 PNG，检查像素/逻辑尺寸、墨迹居中且四边不裁切；旧实现因 36 点而失败，修复后通过。证据在 `artifacts/macos-menu-icon-20261031/native-before.log`、`native-after.log`。
- 统一门禁已通过（`unified-check.log`）：架构/文档、fmt、本机工作区 Clippy、共享/Mac 常规与原生测试，以及 Windows x64/x86 类型检查。ARM64 Release 包已重新生成并校验（`package.log`），其中菜单 PNG 与回归测试输出逐字节相同，SHA256 为 `2ed9f5aa11a07b83ade874fa81faa293412488024a4262cfb32b36a30c4d0630`；应用 `.icns` 与现用版相同。现用系统图标尚未替换，实际菜单栏对齐仍待安装后复核；系统缓存旧图时可能须注销重登。

## 本批：Mac 输入法启动崩溃

- 用户报告已安装虎码仍不能打字、没有候选框。只读现场检查确认当前输入源为 `local.inputmethod.tigertype`，系统于 17:26:37、17:26:43（+0800）启动已安装程序后均以 SIGABRT 退出；崩溃栈为 Rust 捕获外部异常失败，词表线程仍在加载，故障在输入服务初始化。
- 用独立包标识、连接名且不读个人数据的 Rust 探针复现：继承 IMKServer 并向父类发送 `initWithName:bundleIdentifier:` 会报 method not found。IMKServer 是系统类簇，正确创建系统实例后同一初始化成功。产品改为主线程持有系统实例，依赖以有类型的 NSObject 强关联传给控制器；退出时配对解除关联，避免系统继续持有服务时泄漏依赖。
- 原生测试原先只注册类，没有创建 IMKServer。现新增隔离包/连接，实际初始化服务与控制器；合成客户端经本地 NSDistantObject 代理接入，验证激活、菜单、事件掩码、离线按键放行及释放。原有标记文本、提交保护、迟到结果与候选面板测试继续执行，未注册、启用或切换系统输入源。证据在 `artifacts/macos-input-fix-20261031/`。
- 统一门禁已通过（`unified-check.log`）：架构/文档、fmt、本机工作区 Clippy、共享/Mac 常规与原生测试，以及 Windows x64/x86 类型检查。ARM64 Release 修复包已生成并核对 Info.plist（`package.log`），可执行文件 SHA256 `0300c20119ea359537b9947ec57c6aa89a4c1569a6976d58b55be41283f954be`，清单 SHA256 `5329e642254b891aee6ffe46013263d8a91d51d944f894fdf5e01d6d67195598`。
- sudo 要求密码后已提供安装命令；随后只读核对确认系统目录的新程序/清单摘要与修复包相符，属主 root:wheel、目录/可执行文件 755。17:51:55（+0800）启动安装版，PID 3853；52 秒后仍运行，未新增 TigerType 崩溃报告。栈采样确认主线程进入 NSApplication 事件循环，引擎已完成加载并在请求队列等待，工作线程存活。证据为 `installed-after.sha256`、`installed-process.txt`、`installed-startup.sample.txt`；这些只证明启动恢复，真实候选与汉字上屏待用户复测。
- 用户确认安装完成，但仍有普通英文字母或带下划线编码、无候选且不能上屏。LLDB 确认初始化 client 是 `_IMKXPCCompatibilityDOProxyInterposerLegacy`，激活 sender/控制器 client 是 `_IPMDServerClientWrapperLegacy`；旧逻辑一直保存前者并按指针拒绝后者。现改为激活时校验并绑定本次文本对象，保留同会话的对象身份检查；清理/提交在系统重入期间持有原对象。新增不同代理/文本对象、旧对象拒绝、换客户端只撤回旧组合、缺接口拒绝及新客户端独占提交的反例。统一门禁再次通过（`unified-client-binding-check.log`）。
- 第二版已安装并核对系统包摘要：可执行文件 SHA256 `8bff4e24788133a1c4ab1d8042125ac2319254a409357317c020bc8841475a54`，清单 SHA256 `29e7108b1ab55f517ee7b8c67e7ec394dde6d5e2815c40603b4442da0d516d2f`。真实按键进入控制器，sender 与绑定文本对象一致、会话激活；曾捕获到 Codex 中组合长度 1、候选窗可见且位于当前屏幕。调试暂停产生 IMK Stall，会干扰宿主连接，因此断点期间的输入/菜单失败不能作为最终验收。
- 完全解除调试并重启安装版后，独立原生空白宿主经用户手动选择虎码、物理输入，记录到 a/b/c/d 的组合选区依次 1/2/3/4，以及 a、sss、ds、nks 的空格提交回调；用户确认“四码能显示，候选也出现”。未出现 Rust panic。证据 `physical-host.log`、`continuous-input-stderr.log`。早期探针给 CGEvent 设置 Unicode 字符串导致绕过 IMK；改为只向探针自身进程投递物理键码后，a/空格两轮进入 IMK 并上屏“来来”（`native-key-post.log`）。这些结果仍不覆盖 Codex 等其他宿主。
- 系统菜单故障另已定位：本机 IMK 先核对控制器能响应菜单 action，再直接把命令字典发送给该 action；原代码只覆盖 `doCommandBySelector:commandDictionary:`，控制器不响应 `invoke:`，因此被系统直接拒绝。现实现控制器 `invoke:`，保留菜单身份与禁用检查；原生回归验证响应 action、打开设置动作以及旧菜单拒绝（`menu-action-check.log`）。统一门禁通过（`unified-menu-action-check.log`），Release 包已生成（`package-menu-action.log`）；用户已安装并核对摘要：可执行文件 SHA256 `6734a2734007bc17d885e48ac68de017ffaa93bf47a07e850ccc425f8aee63c0`、清单 SHA256 `65dcfa1b7a312d91407a7867c3a72b076b36918a075aa6e570ae02b1d1117c62`。用户随后明确 Codex 仍只能输入第一个码，不能将菜单修复视为该故障已恢复。
- Codex 取证确认后续按键均进入控制器且返回 handled，但每次组合仍为长度 1。限定用户测试 a/b 的一次性调试捕获 `matches_marked`：`markedRange=(6,1)` 有效，预期为 a（0x61），`attributedSubstringFromRange` 却返回换行（0x0a）；内容比较失败触发 `Host::discard`，导致下一码重开组合。Chromium 源码的标记范围同步更新、文档缓存异步更新解释了这一区别，证据摘要见 `codex-root-cause.txt`。未证实的重复激活假设未纳入修改，调试已全部解除。
- 组合校验已移除文档子串比较，只按绑定客户端当前标记范围、UTF-16 长度与既有代次保护判断；所有写入继续使用 NSNotFound，不指定已提交文字范围。新增滞后换行缓存下四码及提交回归，旧实现实际失败（`stale-snapshot-before.log`），新实现通过（`stale-snapshot-after.log`）；组合已结束但缓存仍留旧编码时拒绝提交的反例也通过。完整门禁通过（`unified-stale-snapshot-check.log`，含本机原生测试和 Windows x64/x86 类型检查），Release 包已生成（`package-stale-snapshot.log`），可执行文件 SHA256 `17589a7ee973358ddc3a6d730dc48ba6a4b4cac9d78c5bd9a04119148647af7b`、清单 SHA256 `351e66e566ece6a1c2d248de90215d8a9542fbeafe5a48b26aafaf324045183a`。系统目录摘要已与新包核对一致（`stale-snapshot-installed.log`）；用户安装后确认四码有效，发来的“𤛭、𨰻、𪚥”编码完整，但截图为缺字占位符，继续按字体问题处理。

## 本批：Mac 上屏扩展汉字缺字

- 用户确认 Codex 中四码恢复，但 qqqq、zzzz、tttt 上屏显示条纹方框。锁定码表首选分别确为“𤛭、𨰻、𪚥”，字符内容未损坏。只读探针发现系统可见字体没有“𤛭”，CoreText 默认回退为 LastResort；“𨰻”有系统覆盖，仍不能据此认为 Codex 的字体回退正常。随包文楷覆盖全部三个样例（`artifacts/macos-input-fix-20261031/font-qqqq.log`、`shared-font-before.log`）。
- 根因是 Mac 只把文楷注册到虎码进程，输入法上屏后由 Codex 自行绘制，无法访问这份私有字体。安装器已增加将锁定文楷发布到 `/Library/Fonts/LXGWWenKaiGBScreen.ttf`；同字节复用，冲突、链接及不安全权限拒绝，完整落盘后原子发布，卸载保留供其他应用使用的字体。未增加字体依赖或改动原始字体。
- 合成临时目录的发布、重复安装、内容冲突、符号/硬链接、目录权限和中断残留反例通过（`shared-font-install-tests.log`）；原生文楷覆盖三个样例断言通过（`shared-font-native.log`）。隔离探针确认同字节 URL 已注册时产品的私有注册仍成功（`font-registration.log`）。完整门禁通过（`shared-font-unified-check.log`），Release 包已生成并由用户安装（`shared-font-package.log`）。
- 安装后可执行文件 SHA256 `0941ccaf3c656474316257b139fbc81fd2f5bef76d0fb13869869a5db313310d`、清单 SHA256 `af52ea9cd61b1be7d6cf6205221bc72f4cce9b9f38e1a49b049e07c0b5f530c0` 均与新包相同；系统文楷为 root:admin/644，SHA256 与锁定原件一致（`shared-font-installed.log`）。新进程中系统可见文楷覆盖三个字，“𤛭”的默认回退由 LastResort 变为文楷（`shared-font-after.log`）；全机安装后的私有字体原生回归通过（`shared-font-installed-native.log`）。用户随后明确确认“已经可以显示了”，本次 Codex 输入与三个缺字样例已获得现场验证；不推广为所有宿主、全部 Unicode 字形或长期验收通过。

## 本批：Mac 原生验证与修复

- 统一入口实际执行架构/文档、fmt、本机 Clippy、共享/Mac 常规与文档测试，以及 Windows x64/x86 类型检查。IMK 子类注册、合成文本客户端、UTF-16、菜单字典转发、候选点击失效、隐藏窗口、设置异步回复、固定工作线程/热重载、Mac 文件租约/链接拒绝/事务备份与 kqueue 子进程等待已在 Mac 执行；不把这些合成测试当作真实宿主上屏。
- 修复原生输入测试同时链接库和重复包含源文件产生的 Objective-C 类重复符号；主线程 harness 现在只编译一份类定义。修复查询框提示文字依赖系统外观导致对比度不足，补齐连续换主题的原生反例；按 NSButton 实际坐标方向修正开关与色块的标题/说明顺序，并检查无障碍按钮动作与禁用拒绝。
- 图标绘制复用产品的已校验字节私有字体注册，删除忽略错误且不注销的 URL 注册路径。连续两次图标绘制/注册通过；原生字体检查确认私有文楷覆盖码表全部 353 个 BMP 私用区字根。本机没有 Kaiti SC；文楷与已检查的系统字体均不含 U+20000，候选/查字离屏图显示缺字框，U+20BB7 则有文楷字形。这是字形覆盖限制，不能宣称所有非 BMP 字形验收通过。
- 完整锁定码表解析/索引与完整资源合成组包、损坏拒绝已单列执行通过。真实 ARM64 Release 包也已生成，Info.plist 合法、Mach-O 最低系统为 13.0，保留链接器生成的 LC_CODE_SIGNATURE；未运行 codesign 或修改链接后的可执行文件。修正标识后的生成物位于 `target/package/TigerType.app`，可执行文件 SHA256 为 `e37d9894862aeb157b2a080ded02aa3b32a1c3d99cc2bf03e575cbe354cb5890`，清单 SHA256 为 `21304436c27423fd099874527f48727b7303c6ca76d8818274e6be0f1c55a3cb`。
- 用户已于 10-31 13:29（本机 +0800）安装上一版并注销重登；系统包可执行文件 SHA256 为 `afa90ad5525917d3cf7b0281d861fddd44d80d1a3bbcac1df1bd8fffaa381bf8`，root:wheel/755。重登后只读 TIS 探针仍返回 0 个 TigerType 输入源，用户截图也显示搜索为空。旧包标识 `local.tigertype.inputmethod` 以 inputmethod 结尾；源码现改为 `local.inputmethod.tigertype`，同步 Info.plist、TISInputSourceID 与 IMKServer，并增加一致性检查。这一格式问题有外部同类 A/B 实测佐证，本机修复后的系统发现尚待证实。
- 同时修复声明变更后旧包无法升级的问题：旧目标核对自身摘要、固定文件集合及锁定资源，新来源和 staging 仍严格核对当前声明；摘要不一致与资源损坏反例继续拒绝。完整资源测试及统一门禁已通过。自动安装遇到 sudo 密码要求后，用户于 13:47:42（+0800）完成管理员安装；系统目录的标识、可执行文件与清单摘要均与修复包相符，属主/权限正确。安装后当前会话 TIS 仍为 0，未启动 TigerType 进程；已请用户保存工作后重新登录，再核对系统发现，不自动注册或切换。
- 已生成五种主题的候选/四区设置、查字与图标离屏图；抽查了全部主题及四类页面，发现并修正上述文字问题。滚动后的全部内容、实际交互、Windows 逐图对照和 VoiceOver 仍未完成。证据集中在 `artifacts/macos-validation-20261031/`，完整资源测试另在本次 `artifacts/rust-tiger-*` 目录保存 JSON；未读取现用用户数据，未自动注册、启用或切换输入法。

### 仍须完成的 Mac 验收

1. 本批启动崩溃修复已安装，系统目录摘要、实际进程身份与词表加载完成已核对；继续验证真实输入。
2. TextEdit 验证打字、选重/翻页/退格、单击 Shift、⌃.、选区替换、非 BMP、焦点切换、故障撤回、不重放和迟到点击拒绝。
3. 真实查字/设置事务与热重载、全键盘导航、VoiceOver、增强对比度、多屏/全屏 Space、菜单/拖动跟踪循环；核对缺失字体及罕见字覆盖，完成全部离屏图与 Windows 的对照。
4. 提权安装/升级/卸载、全部用户旧进程退出、失败恢复与异常中断。临时目录的冲突副本、链接拒绝和包替换恢复已原生测试，但不能代替系统安装生命周期验收。
5. 方案真实应用矩阵、完整回调 50ms 预算、P95/P99、空闲与长期资源占用、两小时压力和八小时使用；macOS 13 最低版本也没有真机运行证据。

风险：系统加载、独立原生宿主、Codex 四码输入及本次三个缺字样例已有通过证据；其他宿主、全部字形与长期使用未验收。主动等待有绝对期限，但不能中断已进入的同步系统调用。设置写盘成功而引擎确认失败会提示重启，磁盘不假称回滚。安装失败保留 staging/旧包证据；若失败在新包就位后的停止/清理阶段，新包可能已安装，须核对状态。当前不自动清理历史中断目录。

## 本批：Mac 编辑器工作区报错

- 已复现 `cargo check --workspace --all-targets` 的两处遗漏：Windows 入口无条件调用被 cfg 排除的库入口，TSF 的正式/测试依赖未限定 Windows，导致 Mac 编译 COM 绑定时报错。Windows 入口现按平台选择，非 Windows 误运行明确失败；TSF 依赖与已有源码使用相同平台条件。
- 本机统一 Clippy 扩为整个工作区，补齐命令矩阵断言，防止仅检查共享/Mac 包漏掉编辑器所见错误。修复前后证据在 `artifacts/macos-validation-20261031/editor-check-before.log`、`editor-check-after.log`；后者已无错误通过。完整统一门禁通过（`editor-unified-check.log`），Mac 误运行 Windows 入口按预期输出平台错误并以状态 1 退出（`editor-windows-entry-on-mac.log`）。此批不改变已安装 Mac 输入法，系统发现问题仍按上一节单独验收。

## Windows 现场验证（待继续）

- 修正 README 的 DLL 构建名/部署名混淆，移除“本机仍有暂存注册”的过期断言；部署状态仅在本文维护。
- 将候选窗与隐藏启动的历史“已安装”改为历史记录，现场验收须确认本次构建、目标宿主实际加载的 DLL，不能只重开应用后推定已验证。
- 按用户要求，目录与边界重组和本轮审阅文档修正合并为同一提交；本轮未改产品逻辑。现场闭环目标为微信显示/残留、高负载候选首帧和 x86 真实宿主，部署身份核对尚未完成，均不标通过。
- 本轮证据集中于 `artifacts/windows-closure-20261030/`；安装、关闭现有应用或切换输入法前另行确认，不接管现用数据库作为测试数据。
- 当前安装包已重新离线构建，尚未安装；现场等待用户确认升级及临时停后台、改用新建合成数据目录的测试安排。只读枚举发现 13 个进程载有现用 DLL，未关闭它们；微信未运行。升级后须逐个核对测试宿主的实际加载身份，不把旧宿主或磁盘文件核对当作当前构建的输入证据。

## 仍未闭环的运行验证

- 10-29 高负载修复已合并到源码：定位请求合并、有效位置不做额外查询、Prepare 合并往返、正常 I/O 直接等待内核事件。单 CPU 合成竞争通过，但握手不计入首键，数据不是 TSF/DWM 首帧；x86 尾部波动原因未定位。源码修复不能代替真实 CPU/GPU 压力验收。
- 微信不显示/残留：10-27 通用生命周期修复曾安装，之后曾卸载；本轮发现已重新安装，但现用构建与当前产物不同，现场复测仍未闭环。磁盘 DLL 哈希与已映射模块不是同一证据。
- Mac 原生隐藏窗口/离屏 PNG、工作线程及隔离设置热重载已执行；真实宿主、完整键盘导航、VoiceOver、多屏/全屏 Space 与跟踪循环仍待验收。

## 未解决问题与已知限制

- 拆分显示：源码已改为私有加载锁定文楷，不再依赖系统安装或其他字体替代。Windows 在 COM 构造/后台启动时校验大小与 SHA256、固定只读文件后私有注册；安装包新增按摘要命名的副本。Mac 在 IMK 服务前从校验后的字节私有注册。保留原有三个字体、楷体选项、配置和 v14 协议。Windows 现用宿主尚未部署；Mac 原生注册与 353 个 BMP 私用区字根检查通过，真实输入宿主仍未通过，U+20000 缺字见本批记录。原始 Windows 证据在 `artifacts/private-font-20261031/`，Mac 证据在 `artifacts/macos-validation-20261031/`。
- 高负载候选延迟：10-29 已修复定位与传输中转瓶颈，单 CPU 合成竞争反例通过；当前源码产物尚未部署，现用构建的来源未确认，真实候选首帧验收仍待执行。
- 微信候选不显示/空框残留：通用窗口生命周期、显隐恢复、层级及焦点竞态修复曾于 10-27 安装，随后卸载；当时旧宿主未重开，不能视为修复后的现场验证。本轮须确认重新部署的构建及宿主实际加载模块后复测，两类问题不能合并为一个未经证实的根因。
- 特殊隐藏启动的首次查字不显示：源码与回归测试已修复，历史修复包曾安装后卸载；正式安装器与 HKLM Run 的 `serve` 不使用该标志。候选隐藏时的零面积防护仍缺目标宿主现场证据，不能标记空白边框已确认消失。
- 单击 Shift 判定不感知鼠标：Shift+鼠标选择后松开会切换中英。
- 组合中经菜单切换全角或标点后，候选窗仍是切换前的快照，鼠标点选因编号过期被后台拒绝（不上屏、不出错），继续打字即恢复。
- 设置窗内容区滚动后未重排的子控件靠整体重画跟上，快速滚轮时可能短暂闪烁；滚动条为系统经典样式（灰色）。
- `settings.toml` 经界面保存会按模板重写，用户自加的注释不保留；记事本以外的编辑器若存成 UTF-16 会被拒绝（只收 UTF-8，带 BOM 可）。
- 印章图标与通知区图标依赖本机已装霞鹜文楷（安装包随附），缺失时字形退到楷体。
- 宿主若始终不执行排队的点击会话，同一展示的后续点击被挡住，直到下一次展示更新（过期与失效已能解锁）。
- 后台回收或重启后新会话恢复中文与半角，不保留宿主侧状态；组合中按 Home/End/Delete 等未映射键交给应用，下一键按外部改写处理。
- 任务栏图标字色只在激活时读一次深浅色（`shortcut:` 已注明）；UIA 未实现选择容器，`Select` 被拒。
- DLL 进程期驻留且协议严格相等：升级时未关闭的程序仍载旧 DLL，与新后台不通，只能放行原键（只出英文）直到重开；安装准备页可代为关闭，但本机实测 12 个占用程序中只有资源管理器与 Codex 支持自动重开，其余须手动重开。连接失败时静默放行、无提示。
- 通知区图标：10-24 01:40 一次“溢出区→桌面”后未恢复显示，之后未复现，原因未查明。
- 候选窗位置依赖宿主：Chromium 系宿主在布局通知到达前（实测 2–6ms）候选窗可能先在旧位置出现再移动；从不给位置、不发布局通知又无系统光标的宿主，候选窗退到上次组合位置或输入区域外沿，只是近似位置；连窗口都取不到时仍不显示。
- 微软输入法候选框消失：两次取证均为 TextInputHost 主线程卡在 EdgeHTML 的 `PeekMessage(WM_PAINT)` 忙等，重启 TextInputHost 即恢复；两次发作都紧跟电脑操控会话开始，TigerType DLL 不在 TextInputHost 中，未发现两者通道（`artifacts/textinputhost-*-20261024.log`）。
- 安装包：从已提权进程启动安装包会拉起提权后台，所有宿主拒连，须以普通身份重启后台；静默安装默认会关闭占用旧版的程序（不关须加 `/NOCLOSEAPPLICATIONS`）；`*.old` 在重启前一直留在重启删除队列，Inno 因此不能用 `[InstallDelete]` 清理它们；升级时其他已登录用户的旧后台继续运行；安装中途失败时旧程序已改名，须重跑安装包。
- 本机 `C:\Windows\Fonts` 自带另一版 Cascadia Mono，与随包版本并存，由系统择一。
- 放开低完整性后同会话沙箱进程都能连接占槽，回收只能缓解拒绝服务；后台须以非提升权限运行。DLL 驻留为不可逆系统 PIN，不支持热卸载；管道关闭超时不等于资源已释放。
- 仓库含授权未确认的码表与字体，不得推送到公开远端；更换资源版本会使 Git 历史继续增大。后台每次启动全量核对并解析码表（release 约 0.45s），登录后首次输入前须完成；未测机械硬盘或低配机。

## 未验证

- P0 重构后的其余真机回归：其他宿主、x86 真实应用、完整回调预算、组合中顶屏热重载、旧 DLL 混用；记事本/Typora 的核心输入与隔离热重载已测，交互安装未测。
- UI 重设计批次剩余真机交互：设置窗快捷键区、色板菜单、自定义颜色、完整键盘导航；查字窗查询/翻页/复制；任务栏是否显示虎码印章而不再显示“简体”；任务栏输入法图标右键菜单三个状态项的切换；读屏器朗读新控件。输入/外观/高级分区、滚动、即时保存与合法/非法重新载入，以及两宿主每页 3/5/7 项仅有历史部分覆盖记录，不是本轮现场证据。
- 候选窗布局通知在 Chrome/Edge、VS Code、微信、WebView2 与 UWP 宿主中的表现（只实测了 Typora 与 Notepad++）；读屏器对布局移动时新增 `EVENT_OBJECT_IME_CHANGE` 的反应。
- 10-24 外部审阅批次修复的真机表现（选中替换、组合中停后台与单击 Shift、点击过期、托盘/系统菜单展开时 `stop`、顶屏热重载）。
- 移动/缩放窗口期间 `stop`：与菜单共用登记与唤醒，但隐藏窗口进不了移动/缩放循环，无自动测试；`WM_CANCELMODE` 能否结束键盘发起的移动未实测。列表框滚动条拖动属子控件内部循环，未覆盖。
- 管理员宿主、UWP（开始菜单/设置搜索）、读屏器、跨屏 DPI、深色任务栏与多显示器副任务栏、Win10、无 VC++ 运行库的其他电脑、卸载后的重启清理。
- 安装准备页的交互显示，以及选“自动关闭应用程序”时真实程序（资源管理器、浏览器、编辑器等）的关闭、重开与未保存内容提示；关闭后这些程序是否立即能打中文。
- 50ms 端到端预算、长期 CPU/内存/句柄/GDI 增量、两小时压力与八小时使用。


## 最近验证

- 10-31 工作区提交前复核：完整统一门禁通过，日志 `artifacts/workspace-commit-20261031/check.log`；包括架构/文档、fmt、本机工作区 Clippy、共享/Mac 常规与原生测试及 Windows x64/x86 类型检查。已检查全部改动与生成文件边界；本轮未重新打包、安装或执行真实应用及长期验收，异平台原生测试和 ignored 项不计通过。
- 10-31 Mac 关于文案：按截图去掉简介前的“Rust 专用”，只显示“虎码输入法”；统一门禁通过（架构/文档、fmt、本机 Clippy、共享/Mac 常规与原生测试、Windows x64/x86 类型检查），证据见 `artifacts/macos-about-copy-20261031/check.log`。本批未重新打包或安装，未验收已安装窗口。
- 10-31 Git 忽略规则：补齐各层目录的 `.DS_Store`、`._*`、`Thumbs.db` 与 `desktop.ini`（兼容首字母大写）；当前无此类已跟踪文件。根目录/子目录命中与源码、配置、资源不误忽略检查通过，统一门禁通过；证据见 `artifacts/gitignore-20261031/ignore-rules.log`、`check.log`。本批只改仓库规则，未执行真实应用或长期验收。
- 10-31 Mac 系统发现修复：用户安装并重登后 TIS 数量仍为 0（`artifacts/macos-validation-20261031/input-sources-after-install.log`、`input-sources-before-discovery-fix.log`）。修正包标识后单元与完整资源反例测试通过（`discovery-unit.log`、`discovery-package-test.log`），统一门禁通过（`discovery-check.log`、`discovery-final-check.log`），真实 Release 组包通过（`discovery-package.log`）；自动安装要求交互密码（`discovery-install-attempt.log`），随后用户完成安装，系统目录摘要核对通过，但当前会话 TIS 仍为 0（`discovery-installed.log`）。不能把已通过的包契约检查和磁盘安装核对当作系统发现、加载或真实输入的证据。
- 10-31 Mac 本机批次：统一入口初跑通过但带 Objective-C 重复类符号警告（`artifacts/macos-validation-20261031/check-initial.log`），修复后统一入口无该警告通过（`check-fixed.log`）；查询框连续换主题反例曾失败并修复，原生窗口复测通过（`appkit-fixed.log`）。完整码表与完整资源组包分别通过（`locked-resources.log`、`locked-package.log`），真实 Release 组包通过（`package.log`）；原生私有字体及 353 字根结果见 `native-fonts.log`。图像保存在同目录 `candidate/`、`ui/`、`icons/`；只读系统发现结果见 `input-sources-unsandboxed.log`。真实输入、管理员安装生命周期和长期指标未通过，沙箱内 TIS 服务失败的日志不作为系统发现证据。
- 10-31 本批提交前复核：完整统一门禁再次通过，日志 `artifacts/private-font-20261031/commit-check.log`；复核源码、调用方、安装映射、资源清单及文档，字体原件未修改，生成产物与测试日志不入 Git。私有字体的多宿主累计内存/GDI/句柄增量尚未测量，同进程多个输入法对象仍可能重复校验和注册；约 32ms 仅是先前单次初始化记录，不代表资源占用验收。未打包、安装或推送，Mac 原生与真实宿主结论不变。
- 10-31 私有字体加载：新增有实现的 windows-fonts 边界，COM 构造与后台启动使用同一校验/私有注册路径；字体副本、路径固定、配对注销及安装映射已接通。完整统一门禁通过（含 x64/x86 测试与两种 Apple 类型检查），日志 `artifacts/private-font-20261031/final-check.log`；安装映射反例测试通过（`package-mapping.log`），未调用 Inno Setup 或部署。测试用唯一合成字体族证明系统原本无该字体时能私有加载、卸载后不可选；实际 GDI 字体数据 SHA256 与锁定原件一致，码表内 353 个 BMP 私用区字根均有字形；缺文件、错大小/摘要、硬链接和占用期间改写/目录改名均拒绝，其他文件仍可替换。Release 私有加载本机单次约 32ms（`loader-release-final.log`），在初始化阶段，不能作为冷盘或端到端输入预算证据。Mac 私有注册只通过类型检查，CoreText 同名字体冲突会明确失败，原生运行待验证；本机未安装、切换输入法、改动系统字体或现用数据。现有旧安装包不含本轮私有副本，须重新打包后再做真实宿主、UWP/管理员及升级占用验收。
- 10-31 拆分显示修复：完整统一门禁通过（架构/文档、fmt、Clippy、Windows x64/x86 常规与文档测试、两种 Apple 目标类型检查），日志 `artifacts/learning-font-fix-20261031/check.log`。新增私有加载字体的回归覆盖“你/这/题/缺”、文楷/楷体选项、96/144/192 DPI、TumanPUA 字根回退、查字/预览字体及同源宽度测量；旧等宽字体反例实际失败后恢复正确实现（`negative-old-mono.log`），真实 Popup 绘制的合成离屏图 `candidate-fixed.bmp` 已目视核对。恢复三个字体、选项、配置和 v14 协议，不保留上一轮扩大范围的修改；原始诊断记录保留。未安装、切换输入法或修改现用数据；Mac 原生字形、真实宿主、全字根与长期验收未执行，ignored 不计 passed。
- 10-30 提交前复核：补齐 IMK 系统菜单转发和 Cargo 构建产物读取后，完整统一门禁与完整锁定资源合成组包再次通过，日志 `artifacts/macos-adaptation-20261030/commit-check.log`、`commit-locked-package.log`。菜单身份/非法动作反例在 Windows 执行，命令字典桥接和编译产物硬链接测试只做 Apple 类型检查；Mac 原生验收结论不变。
- 10-30 Mac 全适配批次：首轮及复核后完整统一门禁通过，日志 `artifacts/macos-adaptation-20261030/check.log`、`final-check.log`（架构/文档、fmt、Clippy、Windows x64/x86 常规与文档测试，以及全部共享/Mac 产品、开发工具与打包器的两种 Apple 目标类型检查）。控制器释放/焦点代次、提权 staging 权限、工作队列和限时子进程均已纳入检查；先前并行诊断遇到的 `unused_assignments` 已修正。完整锁定码表/字体的包复制、摘要回读、旧目标拒绝与损坏拒绝已在 Windows 显式通过，日志 `locked-package.log`；该测试只产生不可安装的合成包，不是实际 Mac 包。未运行 Mac 原生程序、安装或真实输入，ignored 与原生未运行项不计通过。
- 10-30 拆分诊断：一次性 Rust 离屏探针断言与已安装程序四个只读 `lookup` 通过，保存字体覆盖日志、对照 BMP、资源哈希及原包字体配置；不据此声称宿主端到端通过。完整锁定码表测试显式执行通过（`locked-resources-final.log`，原始结果 `artifacts/rust-tiger-34520-1793338464000174900/result.json`）。统一门禁复跑完成架构/文档、fmt、Windows Clippy、x64/x86 常规及文档测试，但在 Apple ARM64 的开发工具类型检查被现有改动 `tools/dev/src/process.rs:104` 的 `unused_assignments` 阻断，整体未通过（`check-final.log`）。排查期间工作区另有持续中的 Mac 改动；首轮 Clippy 与锁文件阻断记录仍保存在 `check.log`、`locked-resources.log`，本次未修无关内容或移除锁定参数。
- 10-30 Mac 输入边界批次：实际执行完整统一门禁通过（架构/文档、fmt、Clippy、x64/x86 常规与文档测试，以及 ARM64/Intel Apple 目标组件/预览类型检查），日志为 `artifacts/macos-input-foundation-20261030/check.log`；按键/Shift/非法字符与 UTF-16 位置反例在两种 Windows 位数实际执行。AppKit 合成事件、下划线属性和标记选区断言仅通过交叉类型检查，尚未在 Mac 执行；不计为原生测试通过，未安装或切换输入法。
- 10-30 Windows 文档/现场批次：修正文档后实际执行完整统一门禁通过（架构/文档、fmt、Clippy、x64/x86 常规与文档测试，以及 Apple 两目标组件/预览类型检查），日志为 `artifacts/windows-closure-20261030/check.log`；离线打包核对全部码表/字体/语言文件并构建两个位数通过，日志为同目录 `package.log`，安装包 SHA256 `9ad7e99abd444003c28239dbb285c5e116649a3a95e63ccc2b18afb336ab8552`。该批仅枚举应用及只读探测部署，未执行真实输入、安装、切换或关闭应用。
- 本轮部署与占用证据：`artifacts/windows-closure-20261030/deployment-before.json`、`build-hashes-before.json`、`loaded-before.json`；模块路径、基址和映像大小仅说明存在占用，不证明内存中 DLL 就是磁盘当前字节。
- 重组批次证据：`artifacts/layout-refactor-20261029/final-check.log`；安装映射测试 `package-mapping-tests.log`，锁定码表/字体 `locked-tables.log`、`locked-fonts.log`，历史离线安装包编译 `package.log`（SHA256 `e850b6e03b0454191d77bc87c751f50e7a4af5a502ad3e2c0b27be37a6bf8499`，产物已由本轮重建替换）。
- 重组前基线：`artifacts/architecture-plan-baseline-approved-20261029-233024.log`，架构/文档、fmt、Clippy、x64/x86 常规及文档测试、两种 Apple 架构组件/预览类型检查通过。
- 最近 Mac 展示提交前门禁：`artifacts/macos-views-20261029/commit-check.log`；草稿覆盖、非法设置回复和候选坐标三处回归的反例已失败再修复通过。未运行 Mac 原生测试。
- 高负载修复：`artifacts/candidate-fix-20261029/review-check.log`；合成单 CPU Release 为 `review-cpu-x64.log` 与 `review-cpu-x86-1.log` 至 `-3.log`。不是端到端或长期结论。
- 最近核心真机回归：`artifacts/p0-real-20261027/`，新开记事本/Typora 输入、点选、隔离热重载、停机/重连与短时采样；不能覆盖全部宿主。
- 卸载与重启删除证据：`artifacts/reinstall-20261029/uninstall-result.log`、`uninstall-verification.json`。卸载前准备包 SHA256 为 `ef73f76b8905f8607768b92b59dc50e27719e44702a6e957d28041ed3d19d43c`，不是本轮重组源码的产物。
- 候选生命周期/残留取证分别在 `artifacts/wechat-candidate-20261027/`、`artifacts/popup-residue-20261027/`；安装成功不等于用户截图中的问题已确认消失。

本轮不把 ignored 当 passed，不以目录整理、类型检查、微基准或历史通过替代真实应用、无障碍和两小时压力/八小时使用。
