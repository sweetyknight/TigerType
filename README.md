# TigerType

Windows Rust 专用虎码输入法项目：默认单字输入，可在设置切换词输入；支持拼音查字/查词、简繁候选与原生自绘外观，不兼容 Rime 生态。macOS 端按实施方案“macOS 端”一节分阶段开发，尚不可用；Linux 只预留接入位置。

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

## 码表与离线核心工具

码表随仓库放在 `resources/tiger`：来源为虎码官网盘 <https://huma.ysepan.com/> 下载的“虎码秃版 小狼毫（Win）”方案包（码表版本 2026.08.14），只取所需 9 项原始文本、按包内原相对路径存放且未作修改——单字表、两张 tigress 词表、常用字集、拼音、拆分、读音与两张简繁映射，按 `resources/tiger.lock.json` 锁定大小与 SHA256，以原始字节入库（`.gitattributes` 禁止换行转换，原文 CRLF/LF 混用）。不另生成数据包：后台、命令行工具与打包驱动都直接读这个目录，核对清单后只解析所需数据段，不解释 YAML、Lua 或 OpenCC 配置；任一文件缺失或不符即拒绝启动。程序不读取现用输入法目录。

```powershell
# 码表目录也可换成安装版的 'C:\Program Files\TigerType\tiger'。
cargo run --locked --offline -p tigertype-app -- lookup 'D:\Projects\TigerType\resources\tiger' 'han'
cargo run --locked --offline -p tigertype-app -- lookup 'D:\Projects\TigerType\resources\tiger' '漢'
# 核心诊断仅输出 JSON 状态，不向其他应用上屏。
cargo run --locked --offline -p tigertype-app -- demo-input 'D:\Projects\TigerType\resources\tiger' 'unid '
# 完整码表单列验收，不包含在普通测试的 passed 中。
cargo test --locked --offline -p tigertype-backend --test locked_resources -- --ignored --nocapture
```

CLI 查询输出第一页。锁定清单编译进程序，SHA256 用于核对来源与损坏，不是发布者签名。更换码表版本须同时替换文件、更新清单与安装脚本，打包驱动会核对三者一致。

产品名称统一为 TigerType，生成程序为 `tigertype.exe`。改名前创建的受管数据目录不兼容，请为查字或后台指定新的目录；不修改旧目录所有权标记，也不与旧后台混用。

## 查字与设置

查字/设置窗在输入后台进程内的固定 UI 线程，与输入法同时使用、共用同一份词表与数据目录。

- 设置页：**任何时候**单击通知区（任务栏右下角）的虎码印章图标，或右键它选“设置”，直接打开设置页；右键菜单另有“查字”。这个图标由后台常驻，与前台是什么程序、当前用什么输入法无关；只是前台程序正在用 TigerType 时（任务栏已显示下面的输入法图标）它会自动隐藏，两处同一时刻只显示一个。Win11 默认把新图标收在“^”溢出区里，可在“设置 → 个性化 → 任务栏 → 其他系统托盘图标”中打开 TigerType，或把它拖到任务栏上常显。
- 当前程序正在用 TigerType 时，任务栏输入法位置像微软拼音那样显示一个“中”或“英”字（单击 Shift 切换后随之更新），旁边是虎码印章（输入法配置图标，不再显示语言名“简体”）。单击“中/英”切换中英（同单击 Shift，组合中会把编码原样上屏）；右键弹出菜单：前三项写当前状态——“中文/英文”“半角/全角”“中文标点/英文标点”，点击即切换（全角时数字、字母、符号与空格上屏为全角字符，组合不受影响），其下是“查字”“设置”。后台未运行时菜单项置灰。升级后须重新打开程序才会用上新菜单与新协议。
- 查字窗：开始菜单“TigerType 查字与设置”（后台未运行时一并启动），查字窗里的“设置”按钮（Alt+S）也能打开设置页。

开发时先按下文运行 `serve`，再：

```powershell
# 请本会话的后台显示查字窗（映像须与后台相同）；后台未运行时以安装版位置启动后台并显示。
cargo run --locked --offline -p tigertype-app -- lookup-window
```

查字窗默认 520×400，可缩放。在查询框输入拼音、虎码或粘贴汉字/词后按 Enter 查询，每页最多九项排成一行字块（‹ › 翻页，左右方向键换字），下方详情卡显示字形、码位、可用编码与完整编码（键帽）、读音与拆分（字块）；缺资料明确标注。查询始终使用全字集和原字形，不产生上屏提交。

“复制”只在用户点击时把当前字的全部资料复制为文本，不读取或监视剪贴板。Tab 和控件助记键用于键盘导航，按钮获得焦点后可用 Enter 执行。查询框的 IME 组合确认优先交给编辑控件，不当作查询命令。

设置页可单独打开（查字窗保持隐藏，设置页有自己的任务栏按钮），左侧分四区：**输入**（单字/词输入模式、仅常用字、四码唯一自动上屏、输出字形）、**外观**（候选窗预览、候选字体霞鹜文楷/楷体、每页候选数 3–9 默认 5、显示拆分与编码，配色主题琥珀/墨/竹/玫红/自定义）、**快捷键**（只读说明）、**高级**（打开或重新载入配置文件、版本）。修改立即保存并热重载到输入后台，没有保存/取消按钮：新打开的输入会话立即使用，正在组合的会话在本次组合结束后换上（避免已显示的候选与编号被改掉）。选“自定义”配色后出现背景、文字、高亮、强调四行：点色块从无蓝色板选，或直接输入 `#RRGGBB`；偏蓝或文字不够清楚时不保存，并提示原因与最低对比度。保存失败时控件回到已保存的值并在左下角说明原因；Esc 关闭设置窗。

设置保存在 `%APPDATA%\TigerType\config\settings.toml`：带中文注释，分 `[input]`、`[candidate]`、`[theme]`、`[theme.custom]` 四节，颜色写作 `"#RRGGBB"`，首次运行自动生成。可在“高级”点“打开”用默认编辑器修改，存盘后点“重新载入”生效；格式或取值有误时保留当前设置并指出出错的行。手改之后未重新载入就在界面上改设置，会因文件已被修改而拒绝保存。界面保存按模板重写整个文件，自己加的注释不会保留。旧版 `settings.json` 不再读取。

关闭查字窗只是隐藏（连同设置窗），后台继续运行；再次唤起即恢复到前台。后台停止时窗口随之关闭，并等查字工作线程真正结束才释放数据目录租约。

## 随包字体

候选窗与查字字形默认用霞鹜文楷屏幕阅读版（虎码融合字根版），编码与序号用 Cascadia Mono，字根用 TumanPUA；三者按 `resources/fonts.lock.json` 锁定大小与 SHA256，文件随仓库放在 `resources/fonts`（仅限个人本地使用），来源如下，更换版本时才需重新取得。楷体与微软雅黑 UI 是 Windows 自带的微软字体，不随包分发；缺楷体时回退微软雅黑 UI。

- 霞鹜文楷、TumanPUA：虎码官网盘 <https://huma.ysepan.com/> 的“06 其他资源/虎码必装字体！/虎码方案必装字体 2026.08.15.7z”，取其中 `LXGWWenKaiGBScreen.ttf` 与 `TumanPUA.ttf`。
- Cascadia Mono：<https://github.com/microsoft/cascadia-code/releases/download/v2407.24/CascadiaCode-2407.24.zip> 中的 `ttf/CascadiaMono.ttf`。

安装包把三个字体装到全机字体目录（见下文“安装包”）。不走安装包时，可以普通权限只为当前用户安装（无需管理员）：

```powershell
cargo run --locked --offline -p tigertype-app -- install-fonts 'D:\Projects\TigerType\resources\fonts'
# 完整资源核对（单列，不计入普通测试）
cargo test --locked --offline -p tigertype-app --lib fonts -- --ignored
```

命令先核对全部字体与清单一致才开始，复制到 `%LOCALAPPDATA%\Microsoft\Windows\Fonts` 并登记到当前用户；已有同名而内容不同的文件不覆盖，已装过的不改动。之后新开的程序即可使用；UWP 宿主能否看到当前用户安装的字体尚未实测。

## 手动运行跨进程后台

```powershell
# 单独终端运行。首次指定不存在的新目录；后续只打开本工具创建、带所有权标记的目录，不接管旧输入法目录。
cargo run --locked --offline -p tigertype-app -- serve 'D:\Projects\TigerType\resources\tiger' 'D:\Projects\TigerType\.cache\backend-data'
# 从同一份 tigertype.exe 发出认证停止请求；无需注册或切换输入法。
cargo run --locked --offline -p tigertype-app -- stop
```

`serve` 省略两个路径时使用安装版位置：程序目录下的 `tiger` 码表目录与 `%APPDATA%\TigerType`（不存在则新建）。`tigertype.exe` 为 GUI 子系统程序，开机自启与开始菜单唤起都不弹控制台；从终端直接运行时借用父控制台输出（PowerShell 不等待 GUI 程序结束，诊断请用 `cargo run`）。无控制台时 `serve`、`lookup-window` 启动失败会弹出错误框。

后台独占本登录会话，预建 16 个请求管道（协议 v13，旧版程序不得混用），设置在内存中，设置页保存后热重载。用 `stop` 停止；强行结束进程不会经过停止确认。后台异常或退出由用户重新启动（安装版可点开始菜单“查字与设置”一并启动）。

管道只授权本登录 SID 与 ALL APPLICATION PACKAGES，并带低完整性标签：AppContainer/低完整性宿主可连接，Untrusted 级被拒。后台只核对同会话，不打开宿主进程；宿主核对管道所有者、会话和后台映像，因此后台须以普通（非提升）权限运行，提升启动时管道所有者为 Administrators，宿主会拒绝连接。16 槽占满或合计会话达上限时，后台回收最久未用、无焦点且无组合的宿主，该宿主下次发送前发现断开并重连一次。`stop` 在停止确认后还等后台进程真正退出（与确认共用 5 秒期限）才返回成功，不承诺宿主 DLL 已卸载。

`tigertype-tsf` 构建出 `tigertype_tsf.dll`：导出 `DllGetClassObject`/`DllCanUnloadNow`/`DllRegisterServer`/`DllUnregisterServer`，含类工厂、按键与焦点 sink、组合编辑组件、宿主进程内候选窗（自绘、UIA、IME WinEvent，点击经异步编辑会话选字）和任务栏输入法图标（显示中英状态，单击切换中英，右键菜单显示并切换中英、全半角与标点状态，或唤起查字窗/设置页），按键在同步编辑会话内经管道连接与 DLL 同目录的 `tigertype.exe`。未经真实宿主验收，不能当作可用输入法；快捷键、焦点与候选窗规则见实施方案。普通测试用合成 COM 文本宿主/线程管理器、隐藏候选窗、进程内假后台及临时私有注册表配置单元，不激活系统 TSF，不写系统或用户注册表，不往现用应用上屏；唯一例外是输入法图标右键测试会走真实菜单模态循环，菜单在屏幕左上角一闪而过（预先投递按键选择，不移动鼠标）。

## 安装包

用 Inno Setup 6.7 把程序、两种位数 DLL、锁定码表、虎码印章图标（`tigertype.ico`，打包时现画，注册输入法时登记为配置图标）和随包字体做成一个 `TigerType-Setup-<版本>.exe`，拷到其他电脑上双击安装。资源整体授权未确认，安装包只用于自己的电脑，不公开分发。

码表、字体与简体中文语言文件都随仓库保存，换电脑后克隆即可打包；每台打包机只需另装一次 Inno Setup 6.7.3：官方 GitHub <https://github.com/jrsoftware/issrc/releases/download/is-6_7_3/innosetup-6.7.3.exe>，SHA256 `9c73c3bae7ed48d44112a0f48e66742c00090bdb5bef71d9d3c056c66e97b732`，以 `/CURRENTUSER` 为当前用户安装（装到 `%LOCALAPPDATA%\Programs\Inno Setup 6`，打包驱动只在此处查找）。语言文件 `installer/ChineseSimplified.isl` 取自官方仓库同一标签的 <https://raw.githubusercontent.com/jrsoftware/issrc/is-6_7_3/Files/Languages/Unofficial/ChineseSimplified.isl>，SHA256 `7d544b9bb1d142cfa11f2e5d3cc8abe2e55f8e066c5124e3772675aa236e1278`。

```powershell
# 构建 release（静态 CRT，目标机无需 VC++ 运行库）、校验码表/字体/语言文件后编译；不修改系统。
cargo run --locked --offline -p tigertype-app --example package
```

产物为 `target\package\TigerType-Setup-<版本>.exe`，驱动最后打印其 SHA256。安装脚本为 [installer/tigertype.iss](installer/tigertype.iss)；驱动在打包前核对脚本逐个列出的字体与码表和锁定清单一致。

安装（需管理员，Windows 10/11 x64）做这些事：

- 装到 `C:\Program Files\TigerType`（不提供改目录：该目录自带 ALL APPLICATION PACKAGES 读/执行，非管理员不可写）；放入 `tigertype.exe`、`tigertype_tsf.dll`、`tigertype_tsf_x86.dll` 与 `tiger\` 码表目录（升级时删除旧版的 `tiger-v1.json`），并分别以 64/32 位注册两个 DLL。
- 字体装到全机字体目录；已有同名文件（如 Win11 自带的 Cascadia Mono）不覆盖，卸载时不删除。
- 在 `HKLM\...\Run` 登记 `tigertype.exe serve`：每个用户登录时以普通权限启动自己的后台，数据在各自 `%APPDATA%\TigerType`；安装结束时以安装者的普通权限立即启动一次。
- 开始菜单“TigerType 查字与设置”：唤起后台的查字窗；后台未运行时一并启动。
- 不添加或切换键盘：装完后由用户在“设置 → 时间和语言 → 语言和区域 → 中文(简体) → 语言选项 → 键盘”中添加“TigerType 虎码”。

升级：直接运行新版安装包。安装前先以原用户身份 `stop` 后台（等到旧后台退出）；已加载 DLL 的程序无法释放映像，且协议严格相等，不重开就只能出英文，所以“准备安装”页会列出仍载着现版或历次 `*.old` 旧版 DLL 的程序（资源管理器、浏览器、编辑器等），建议选“自动关闭应用程序”：支持重启的程序（如资源管理器）装完自动重开，其余须手动重开。选择不关闭或关不掉的，旧文件改名为 `*.old` 并登记重启后删除，新文件照常就位，这些程序重开前仍只出英文；装完删掉已无人使用的 `*.old`。静默安装（`/SILENT`）默认会关闭这些程序，加 `/NOCLOSEAPPLICATIONS` 则不关。卸载：在“设置 → 应用”中卸载，先请求停止后台（卸载程序为提升权限，实测可被普通权限后台接受），再注销两个 DLL、删除程序与自启项；仍被占用的文件在重启后删除；`%APPDATA%\TigerType` 用户数据保留。

本机已有 C 批暂存注册（`C:\TigerType-Staging`）：安装前先在系统设置中移除旧键盘，`& 'C:\TigerType-Staging\tigertype.exe' stop` 停止暂存后台，以管理员分别用 System32/SysWOW64 的 `regsvr32 /u` 注销暂存的两个 DLL；暂存目录待相关进程退出后以管理员删除。暂存后台用的 `.cache\backend-data` 与安装版的 `%APPDATA%\TigerType` 互不相干，旧设置不会迁移。

## 沙箱宿主前提测试（须单独授权）

AppContainer 宿主前提测试会创建并删除一个临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`），默认忽略，须经用户授权后单独运行：

```powershell
cargo test --locked --offline -p tigertype-ipc --lib -- --ignored --exact pipe::sandbox_tests::app_container_host_connects_with_logon_sid_and_limited_query_but_cannot_squat
```

## 数据边界
原资源只读并锁定来源/hash；不读取个人词库，不修改现用输入法。安装版后台与查字窗只用 `%APPDATA%\TigerType`（带所有权标记，不接管已有的其他目录），卸载时保留。

锁定码表、字体与语言文件随仓库保存；`vendor/`（只剩退役路线的本地下载）、`.cache/`、`artifacts/`、`target/` 与数据库不入 Git。资源整体授权未确认，仅限个人本地使用：仓库不得推送到公开远端，安装包不公开发布。
