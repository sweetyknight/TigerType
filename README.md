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

这是独立工具，不是单后台或输入法。关闭主窗口会断开请求通道并等待当前工作结束，不强行终止保存线程。正式 APPDATA、候选窗、TSF 和视觉/无障碍验收情况见项目状态；不安装、不注册、不切换现用输入法。

## 候选显示与点击回送联调

```powershell
cargo run --locked --offline -p tigertype-app -- preview-candidates 'D:\Projects\TigerType\.cache\tiger-v1.json' 'u'
```

使用真实数据包和引擎生成一页竖排候选，窗口以非激活方式显示。点击候选后，带宿主/会话/焦点/快照的意图回到调度器，由预览宿主领取一次，最终选择只打印到控制台；不会向其他应用输入文字。点击“关闭预览”也可退出。编码限 1–4 位小写虎码，无候选明确报错。

这是联调工具，不是 TSF 宿主。协议 v4 与旧版不兼容，旧程序不得混用。候选目前使用标准系统控件，最终 Win11Light 外观、完整字体回退和真实无障碍验收见项目状态。

## 手动运行跨进程后台

```powershell
# 单独终端运行；目录规则与 lookup-window 相同，不接管 APPDATA 或旧输入法目录。
cargo run --locked --offline -p tigertype-app -- serve 'D:\Projects\TigerType\.cache\tiger-v1.json' 'D:\Projects\TigerType\.cache\backend-data'
# 从同一份 tigertype.exe 发出认证停止请求；无需注册或切换输入法。
cargo run --locked --offline -p tigertype-app -- stop
```

后台独占本登录会话，预建 16 对请求/通知管道，读取设置后固定在内存中；修改设置须先停止后台，之后重新启动。空闲时候选窗隐藏，可用 `stop` 停止；候选显示时也可点击“停止后台”。后台异常或退出由用户重新启动。`stop` 成功表示停止确认已送出，后台随后取消并排空资源，不承诺宿主 DLL 已卸载。

`tigertype-tsf` 当前提供 COM 组合编辑组件，不是可注册的完整 TIP DLL；尚未提供类工厂和系统按键/焦点 sink 的组装，不能把启动后台当成系统输入法已经可用。普通测试用合成 COM 文本宿主及限时后台子进程，不往现用应用上屏。

## 数据边界
原资源只读并锁定来源/hash；不读取个人词库，不修改现用输入法。正式数据独立使用 `%APPDATA%\TigerType`，开放情况见状态。

`vendor/`、`.cache/`、`artifacts/`、`target/` 与数据库不入 Git；资源整体授权未确认，仅限个人本地使用，不公开发布。
