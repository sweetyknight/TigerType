# TigerType

Windows Rust 专用虎码单字输入法项目：单字输入、拼音查字学习、简繁候选与原生 Win11Light 外观，不兼容 Rime 生态。

- [实施方案](docs/implementation-plan.md)：批准范围、架构、验收。
- [项目状态](docs/project-status.md)：实际已实现、缺陷和验证；不要把规划当成可用功能。
- [开发约束](AGENTS.md)：代码、数据保护和交付流程。

## 构建和检查
需要 Windows x64、锁定 Rust 1.94.1、MSVC 工具链与 i686-pc-windows-msvc 目标。首次准备缓存可运行 `cargo fetch --locked`；之后离线检查，不自动下载原词表或引擎。两种 MSVC 目标经 `.cargo/config.toml` 静态链接 CRT，产物不依赖 VC++ 运行库。

```powershell
cargo run --locked --offline -p tigertype-app --example check
cargo test --workspace --locked --offline
```

完整资源和真实应用验收另见状态。普通测试使用合成数据，不加载 librime、不安装输入法。

统一入口显式运行 lib/bin/integration 测试并单列 doctest，避免 Windows 工作区特性合并时重链接正在运行的 `check.exe`；所有目标仍经 Clippy，DLL 探针仍由集成测试构建/执行。

普通测试还会为当前位数构建本仓库的 `resident_probe` 测试 DLL，在限时子进程内验证显式驻留和管道关闭；不加载到用户应用，不注册 COM/TSF。仅复跑该项：

```powershell
cargo test --locked --offline -p tigertype-ipc --test dll_residency
```

此探针不是 TSF 输入法 DLL。进程期驻留的 DLL 不能热卸载，停止管道不代表宿主已卸载模块；宿主退出前仍会占用映像。

## 离线核心工具

先将批准的原资源按锁定清单准备在本项目 `vendor/tiger`；工具只读取所需的 9 项文本，不自动探测现用输入法目录。

```powershell
# 输出必须为新文件；已有包不会被覆盖。以下路径按本仓库位置填写。
cargo run --locked --offline -p tigertype-app -- import-resources 'D:\Projects\TigerType\.cache\tiger-v1.json'
cargo run --locked --offline -p tigertype-app -- verify-package 'D:\Projects\TigerType\.cache\tiger-v1.json'
cargo run --locked --offline -p tigertype-app -- lookup 'D:\Projects\TigerType\.cache\tiger-v1.json' 'han'
cargo run --locked --offline -p tigertype-app -- lookup 'D:\Projects\TigerType\.cache\tiger-v1.json' '漢'
# 核心诊断仅输出 JSON 状态，不向其他应用上屏。
cargo run --locked --offline -p tigertype-app -- demo-input 'D:\Projects\TigerType\.cache\tiger-v1.json' 'unid '
# 完整锁定资源单列验收，不包含在普通测试的 passed 中。
cargo test --locked --offline -p tigertype-app --test locked_resources -- --ignored --nocapture
```

CLI 查询输出第一页。生成包的摘要用于损坏检测，不是发布者签名；不从不可信来源取得数据包。

产品名称统一为 TigerType，生成程序为 `tigertype.exe`。改名前创建的受管数据目录不兼容，请为查字或后台指定新的目录；不修改旧目录所有权标记，也不与旧后台混用。

## 查字与设置

查字/设置窗在输入后台进程内的固定 UI 线程，与输入法同时使用、共用同一份词表与数据目录。

- 设置页：**任何时候**单击通知区（任务栏右下角）的琥珀色“虎”图标，或右键它选“设置”，直接打开设置页；右键菜单另有“查字”。这个图标由后台常驻，与前台是什么程序、当前用什么输入法无关。Win11 默认把新图标收在“^”溢出区里，可在“设置 → 个性化 → 任务栏 → 其他系统托盘图标”中打开 TigerType，或把它拖到任务栏上常显。
- 当前程序正在用 TigerType 时，任务栏输入法位置也有同样的“虎”图标，单击打开设置页，右键弹出“设置/查字”菜单；后台未运行时菜单项置灰并提示从开始菜单启动。须安装含此修复的版本并重新打开程序（已载入旧 DLL 的程序右键无菜单）。
- 查字窗：开始菜单“TigerType 查字与设置”（后台未运行时一并启动），查字窗里的“设置”按钮（Alt+S）也能打开设置页。

开发时先按下文运行 `serve`，再：

```powershell
# 请本会话的后台显示查字窗（映像须与后台相同）；后台未运行时以安装版位置启动后台并显示。
cargo run --locked --offline -p tigertype-app -- lookup-window
```

输入拼音或粘贴汉字后点击查询（查询框也支持 Enter），每页最多九字列在左侧，选中一字在右侧查看字形、可用编码、完整编码、读音与拆分（可选中复制）；缺资料明确标注。查字始终使用全字集和原字形，不产生上屏提交。

“复制本页资料”仅在用户触发按钮时复制；也可在只读资料框选择文本后用 Ctrl+C。Tab 和控件助记键用于键盘导航，按钮获得焦点后可用 Enter 执行。查询框的 IME 组合确认优先交给编辑控件，不当作查询命令。

设置页可单独打开（查字窗保持隐藏，设置页有自己的任务栏按钮），提供四码唯一自动上屏、常用字过滤、学习提示、输出字形、候选字体（霞鹜文楷/楷体）与配色主题（琥珀/墨/竹/夜/自定义四基色，须无蓝且文字可读）；点击保存或在设置窗按 Enter 才事务写入 `config/settings.json`，Esc 或“取消”关闭设置窗。保存成功即热重载到输入后台：新打开的输入会话立即使用，正在组合的会话在本次组合结束后换上（避免已显示的候选与编号被改掉），无需重启后台。旧版 `settings.json` 缺少新字段会被拒绝，须删除后重新保存。重复唤起已经打开的设置窗保留草稿，关闭后重开才恢复已确认值。冲突或失败不会宣称保存成功；关闭设置窗放弃尚未发送的修改，已发送的保存不会回滚。

关闭查字窗只是隐藏（连同设置窗与其草稿），后台继续运行；再次唤起即恢复到前台。后台停止时窗口随之关闭，并等查字工作线程真正结束才释放数据目录租约。设置只经设置页修改；在后台运行期间手工改写 `settings.json` 不会被读入，且下次保存会因冲突被拒绝。

## 随包字体

候选窗与查字字形默认用霞鹜文楷屏幕阅读版（虎码融合字根版），编码与序号用 Cascadia Mono，字根用 TumanPUA；三者按 `resources/fonts.lock.json` 锁定大小与 SHA256，文件放在 `vendor/fonts`（不入 Git，仅限个人本地使用）。楷体与微软雅黑 UI 是 Windows 自带的微软字体，不随包分发；缺楷体时回退微软雅黑 UI。

- 霞鹜文楷、TumanPUA：虎码官网盘 <https://huma.ysepan.com/> 的“06 其他资源/虎码必装字体！/虎码方案必装字体 2026.08.15.7z”，取其中 `LXGWWenKaiGBScreen.ttf` 与 `TumanPUA.ttf`。
- Cascadia Mono：<https://github.com/microsoft/cascadia-code/releases/download/v2407.24/CascadiaCode-2407.24.zip> 中的 `ttf/CascadiaMono.ttf`。

安装包把三个字体装到全机字体目录（见下文“安装包”）。不走安装包时，可以普通权限只为当前用户安装（无需管理员）：

```powershell
cargo run --locked --offline -p tigertype-app -- install-fonts 'D:\Projects\TigerType\vendor\fonts'
# 完整资源核对（单列，不计入普通测试）
cargo test --locked --offline -p tigertype-app --lib fonts -- --ignored
```

命令先核对全部字体与清单一致才开始，复制到 `%LOCALAPPDATA%\Microsoft\Windows\Fonts` 并登记到当前用户；已有同名而内容不同的文件不覆盖，已装过的不改动。之后新开的程序即可使用；UWP 宿主能否看到当前用户安装的字体尚未实测。

## 手动运行跨进程后台

```powershell
# 单独终端运行。首次指定不存在的新目录；后续只打开本工具创建、带所有权标记的目录，不接管旧输入法目录。
cargo run --locked --offline -p tigertype-app -- serve 'D:\Projects\TigerType\.cache\tiger-v1.json' 'D:\Projects\TigerType\.cache\backend-data'
# 从同一份 tigertype.exe 发出认证停止请求；无需注册或切换输入法。
cargo run --locked --offline -p tigertype-app -- stop
```

`serve` 省略两个路径时使用安装版位置：程序目录下的 `tiger-v1.json` 与 `%APPDATA%\TigerType`（不存在则新建）。`tigertype.exe` 为 GUI 子系统程序，开机自启与开始菜单唤起都不弹控制台；从终端直接运行时借用父控制台输出（PowerShell 不等待 GUI 程序结束，诊断请用 `cargo run`）。无控制台时 `serve`、`lookup-window` 启动失败会弹出错误框。

后台独占本登录会话，预建 16 个请求管道（协议 v9，旧版程序不得混用），设置在内存中，设置页保存后热重载。用 `stop` 停止；强行结束进程不会经过停止确认。后台异常或退出由用户重新启动（安装版可点开始菜单“查字与设置”一并启动）。

管道只授权本登录 SID 与 ALL APPLICATION PACKAGES，并带低完整性标签：AppContainer/低完整性宿主可连接，Untrusted 级被拒。后台只核对同会话，不打开宿主进程；宿主核对管道所有者、会话和后台映像，因此后台须以普通（非提升）权限运行，提升启动时管道所有者为 Administrators，宿主会拒绝连接。16 槽占满或合计会话达上限时，后台回收最久未用、无焦点且无组合的宿主，该宿主下次发送前发现断开并重连一次。`stop` 成功表示停止确认已送出，后台随后取消并排空资源，不承诺宿主 DLL 已卸载。

`tigertype-tsf` 构建出 `tigertype_tsf.dll`：导出 `DllGetClassObject`/`DllCanUnloadNow`/`DllRegisterServer`/`DllUnregisterServer`，含类工厂、按键与焦点 sink、组合编辑组件、宿主进程内候选窗（自绘、UIA、IME WinEvent，点击经异步编辑会话选字）和任务栏输入法图标（单击/右键菜单唤起设置页与查字窗），按键在同步编辑会话内经管道连接与 DLL 同目录的 `tigertype.exe`。未经真实宿主验收，不能当作可用输入法；快捷键、焦点与候选窗规则见实施方案。普通测试用合成 COM 文本宿主/线程管理器、隐藏候选窗、进程内假后台及临时私有注册表配置单元，不激活系统 TSF，不写系统或用户注册表，不往现用应用上屏；唯一例外是输入法图标右键测试会走真实菜单模态循环，菜单在屏幕左上角一闪而过（预先投递按键选择，不移动鼠标）。

## 安装包

用 Inno Setup 6.7 把程序、两种位数 DLL、数据包和随包字体做成一个 `TigerType-Setup-<版本>.exe`，拷到其他电脑上双击安装。资源整体授权未确认，安装包只用于自己的电脑，不公开分发。

打包前准备（每台打包机一次，均不入 Git）：

- Inno Setup 6.7.3：官方 GitHub <https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe>，SHA256 `9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732`，以 `/CURRENTUSER` 为当前用户安装（装到 `%LOCALAPPDATA%\Programs\Inno Setup 6`，打包驱动只在此处查找）。
- 简体中文语言文件：官方仓库同一标签的 <https://raw.githubusercontent.com/jrsoftware/issrc/is-6_7_3/Files/Languages/Unofficial/ChineseSimplified.isl>，放到 `vendor/inno/ChineseSimplified.isl`，SHA256 `7d544b9bb1d142cfa11f2e5d3cc8abe2e55f8e066c5124e3772675aa236e1278`。
- `vendor/tiger` 原资源与 `vendor/fonts` 三个字体（见上文）。

```powershell
# 构建 release（静态 CRT，目标机无需 VC++ 运行库）、校验字体/语言文件、由锁定资源重新生成数据包后编译；不修改系统。
cargo run --locked --offline -p tigertype-app --example package
```

产物为 `target\package\TigerType-Setup-<版本>.exe`，驱动最后打印其 SHA256。安装脚本为 [installer/tigertype.iss](installer/tigertype.iss)；驱动在打包前核对脚本列出的字体与锁定清单一致。

安装（需管理员，Windows 10/11 x64）做这些事：

- 装到 `C:\Program Files\TigerType`（不提供改目录：该目录自带 ALL APPLICATION PACKAGES 读/执行，非管理员不可写）；放入 `tigertype.exe`、`tigertype_tsf.dll`、`tigertype_tsf_x86.dll` 与 `tiger-v1.json`，并分别以 64/32 位注册两个 DLL。
- 字体装到全机字体目录；已有同名文件（如 Win11 自带的 Cascadia Mono）不覆盖，卸载时不删除。
- 在 `HKLM\...\Run` 登记 `tigertype.exe serve`：每个用户登录时以普通权限启动自己的后台，数据在各自 `%APPDATA%\TigerType`；安装结束时以安装者的普通权限立即启动一次。
- 开始菜单“TigerType 查字与设置”：唤起后台的查字窗；后台未运行时一并启动。
- 不添加或切换键盘：装完后由用户在“设置 → 时间和语言 → 语言和区域 → 中文(简体) → 语言选项 → 键盘”中添加“TigerType 虎码”。

升级：直接运行新版安装包。安装前先以原用户身份 `stop` 后台，正在使用的旧程序与 DLL 改名为 `*.old` 并登记重启后删除（已加载 DLL 的进程无法释放映像），新文件就位后新开的程序即用新版本；安装前已打开的程序须重新打开。卸载：在“设置 → 应用”中卸载，先请求停止后台（卸载程序为提升权限，能否被普通权限后台接受尚未实测），再注销两个 DLL、删除程序与自启项；仍被占用的文件在重启后删除；`%APPDATA%\TigerType` 用户数据保留。

本机已有 C 批暂存注册（`C:\TigerType-Staging`）：安装前先在系统设置中移除旧键盘，`& 'C:\TigerType-Staging\tigertype.exe' stop` 停止暂存后台，以管理员分别用 System32/SysWOW64 的 `regsvr32 /u` 注销暂存的两个 DLL；暂存目录待相关进程退出后以管理员删除。暂存后台用的 `.cache\backend-data` 与安装版的 `%APPDATA%\TigerType` 互不相干，旧设置不会迁移。

## 沙箱宿主前提测试（须单独授权）

AppContainer 宿主前提测试会创建并删除一个临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`），默认忽略，须经用户授权后单独运行：

```powershell
cargo test --locked --offline -p tigertype-ipc --lib -- --ignored --exact pipe::sandbox_tests::app_container_host_connects_with_logon_sid_and_limited_query_but_cannot_squat
```

## 数据边界
原资源只读并锁定来源/hash；不读取个人词库，不修改现用输入法。安装版后台与查字窗只用 `%APPDATA%\TigerType`（带所有权标记，不接管已有的其他目录），卸载时保留。

`vendor/`、`.cache/`、`artifacts/`、`target/` 与数据库不入 Git；资源整体授权未确认，仅限个人本地使用，不公开发布。
