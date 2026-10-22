# TigerType

Windows Rust 专用虎码单字输入法项目：单字输入、拼音查字学习、简繁候选与原生 Win11Light 外观，不兼容 Rime 生态。

- [实施方案](docs/implementation-plan.md)：批准范围、架构、验收。
- [项目状态](docs/project-status.md)：实际已实现、缺陷和验证；不要把规划当成可用功能。
- [开发约束](AGENTS.md)：代码、数据保护和交付流程。

## 构建和检查
需要 Windows x64、锁定 Rust 1.94.1、MSVC 工具链与 i686-pc-windows-msvc 目标。首次准备缓存可运行 `cargo fetch --locked`；之后离线检查，不自动下载原词表或引擎。

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

## 独立查字与设置

```powershell
# 首次指定不存在的新目录；后续只打开本工具创建、带所有权标记的目录。
cargo run --locked --offline -p tigertype-app -- lookup-window 'D:\Projects\TigerType\.cache\tiger-v1.json' 'D:\Projects\TigerType\.cache\lookup-data'
```

输入拼音或粘贴汉字后点击查询（查询框也支持 Enter），每页最多九字，可翻页查看可用编码、完整编码、读音与拆分；缺资料明确标注。查字始终使用全字集和原字形，不产生上屏提交。

“复制本页资料”仅在用户触发按钮时复制；也可在只读资料框选择文本后用 Ctrl+C。Tab 和控件助记键用于键盘导航，按钮获得焦点后可用 Enter 执行。查询框的 IME 组合确认优先交给编辑控件，不当作查询命令。

设置从查字窗打开，仅提供四码唯一自动上屏、常用字过滤、学习提示、输出字形四项；点击保存或在设置窗按 Enter 才事务写入 `config/settings.json`。字形下拉框展开时 Enter/Esc 先确认/取消下拉选择；收起后 Esc 关闭设置窗。重复唤起已经打开的设置窗保留草稿，关闭后重开才恢复已确认值。冲突或失败不会宣称保存成功；关闭设置窗放弃尚未发送的修改，已发送的保存不会回滚。

这是独立工具，不是单后台或输入法。关闭主窗口会断开请求通道并等待当前工作结束，不强行终止保存线程。正式 APPDATA、TSF 和视觉/无障碍验收情况见项目状态；不安装、不注册、不切换现用输入法。

## 手动运行跨进程后台

```powershell
# 单独终端运行；目录规则与 lookup-window 相同，不接管 APPDATA 或旧输入法目录。
cargo run --locked --offline -p tigertype-app -- serve 'D:\Projects\TigerType\.cache\tiger-v1.json' 'D:\Projects\TigerType\.cache\backend-data'
# 从同一份 tigertype.exe 发出认证停止请求；无需注册或切换输入法。
cargo run --locked --offline -p tigertype-app -- stop
```

后台独占本登录会话，预建 16 个请求管道（协议 v6，旧版程序不得混用），读取设置后固定在内存中；修改设置须先停止后台，之后重新启动。后台不显示窗口，用 `stop` 停止；在终端强行结束进程不会经过停止确认。后台异常或退出由用户重新启动。

管道只授权本登录 SID 与 ALL APPLICATION PACKAGES，并带低完整性标签：AppContainer/低完整性宿主可连接，Untrusted 级被拒。后台只核对同会话，不打开宿主进程；宿主核对管道所有者、会话和后台映像，因此后台须以普通（非提升）权限运行，提升启动时管道所有者为 Administrators，宿主会拒绝连接。16 槽占满或合计会话达上限时，后台回收最久未用、无焦点且无组合的宿主，该宿主下次发送前发现断开并重连一次。`stop` 成功表示停止确认已送出，后台随后取消并排空资源，不承诺宿主 DLL 已卸载。

`tigertype-tsf` 构建出 `tigertype_tsf.dll`：导出 `DllGetClassObject`/`DllCanUnloadNow`/`DllRegisterServer`/`DllUnregisterServer`，含类工厂、按键与焦点 sink、组合编辑组件和宿主进程内候选窗（自绘、UIA、IME WinEvent，点击经异步编辑会话选字），按键在同步编辑会话内经管道连接与 DLL 同目录的 `tigertype.exe`。未经真实宿主验收，不能当作可用输入法；快捷键、焦点与候选窗规则见实施方案。普通测试用合成 COM 文本宿主/线程管理器、隐藏候选窗、进程内假后台及临时私有注册表配置单元，不激活系统 TSF，不在桌面显示窗口，不写系统或用户注册表，不往现用应用上屏。

## 真实宿主验收（C 批，须单独授权）

以下步骤会修改系统（文件权限、HKLM 注册表与 TSF 配置），只在用户对每一步明确授权后由用户手动执行；开发流程不会自动执行。宿主只连接与 DLL 同目录的后台，因此 x64 与 x86 DLL 放在同一暂存目录，x86 DLL 改名，二者共用一个 `tigertype.exe`。

```powershell
# 1. 构建（不修改系统）。
cargo build --locked --offline --release -p tigertype-app -p tigertype-tsf
cargo build --locked --offline --release -p tigertype-tsf --target i686-pc-windows-msvc
# 2. 新建独立暂存目录并复制产物；对 ALL APPLICATION PACKAGES（S-1-15-2-1）授予读取/执行，供 AppContainer 宿主加载。
New-Item -ItemType Directory 'C:\TigerType-Staging'
Copy-Item target\release\tigertype.exe, target\release\tigertype_tsf.dll 'C:\TigerType-Staging\'
Copy-Item target\i686-pc-windows-msvc\release\tigertype_tsf.dll 'C:\TigerType-Staging\tigertype_tsf_x86.dll'
icacls 'C:\TigerType-Staging' /grant '*S-1-15-2-1:(OI)(CI)(RX)'
# 注册前加固：C:\ 下新建目录继承 Authenticated Users 修改权，而注册后 DLL 会进入管理员进程；
# 去掉继承，只留 Administrators/SYSTEM 完全控制与 Users、ALL APPLICATION PACKAGES 读/执行（之后删除需管理员）。
icacls 'C:\TigerType-Staging' /inheritance:r /grant:r '*S-1-5-32-544:(OI)(CI)(F)' '*S-1-5-18:(OI)(CI)(F)' '*S-1-5-32-545:(OI)(CI)(RX)' '*S-1-15-2-1:(OI)(CI)(RX)'
# 3. 以管理员身份注册两种位数（写 HKLM COM 类、简体中文语言配置与 TSF 类别）；用双引号，cmd 与 PowerShell 通用（cmd 不认单引号）。
C:\Windows\System32\regsvr32.exe "C:\TigerType-Staging\tigertype_tsf.dll"
C:\Windows\SysWOW64\regsvr32.exe "C:\TigerType-Staging\tigertype_tsf_x86.dll"
# 4. 以普通权限从暂存目录启动后台，再在系统设置中为简体中文添加 TigerType 键盘后试用。
& 'C:\TigerType-Staging\tigertype.exe' serve 'D:\Projects\TigerType\.cache\tiger-v1.json' 'D:\Projects\TigerType\.cache\backend-data'
```

更新已部署的 DLL：DLL 常驻在已加载它的进程中，无法覆盖或删除，只能以管理员先改名旧文件、再复制新文件（注册路径不变，无需重新注册）；已打开的程序重启后才用到新版本，旧文件在重启或注销后再删除。后台程序有改动时先以普通权限 `stop`，再以管理员 `copy "D:\Projects\TigerType\target\release\tigertype.exe" C:\TigerType-Staging\tigertype.exe`，然后以普通权限重新 `serve`。

```bat
cd /d C:\TigerType-Staging
ren tigertype_tsf.dll tigertype_tsf.old1.dll
ren tigertype_tsf_x86.dll tigertype_tsf_x86.old1.dll
copy "D:\Projects\TigerType\target\release\tigertype_tsf.dll" tigertype_tsf.dll
copy "D:\Projects\TigerType\target\i686-pc-windows-msvc\release\tigertype_tsf.dll" tigertype_tsf_x86.dll
```

验收后注销：先在系统设置中移除该键盘，`& 'C:\TigerType-Staging\tigertype.exe' stop` 停止后台，再以管理员身份分别运行 `regsvr32 /u`（System32 对应 x64 DLL、SysWOW64 对应 x86 DLL）。DLL 在已加载它的进程退出前无法删除，退出后再删除暂存目录。注册时若同目录缺少 `tigertype.exe` 或路径不是绝对路径即拒绝；任一步失败回滚已写内容。语言配置暂无图标。

AppContainer 宿主前提测试会创建并删除一个临时 AppContainer 配置（当前用户注册表与 `%LOCALAPPDATA%\Packages`），默认忽略，须经用户授权后单独运行：

```powershell
cargo test --locked --offline -p tigertype-ipc --lib -- --ignored --exact pipe::sandbox_tests::app_container_host_connects_with_logon_sid_and_limited_query_but_cannot_squat
```

## 数据边界
原资源只读并锁定来源/hash；不读取个人词库，不修改现用输入法。正式数据独立使用 `%APPDATA%\TigerType`，开放情况见状态。

`vendor/`、`.cache/`、`artifacts/`、`target/` 与数据库不入 Git；资源整体授权未确认，仅限个人本地使用，不公开发布。
