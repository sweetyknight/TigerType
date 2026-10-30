# TigerType 开发约束

范围以 [实施方案](docs/implementation-plan.md) 为准，实际状态只维护在 [项目状态](docs/project-status.md)。开发期不向后兼容；旧路线代码和文档直接删除，不保留归档或兼容层。

## 产品与架构
- 源码一级按 shared/windows/macos，二级按职责；input-client 是宿主侧共享逻辑，input-method 是平台系统适配，ui/ui-model 只指查字/设置。产品不得反向依赖 tools/dev（含 dev/build），两平台产品不互相引用；具体允许边与目录归属由方案和架构测试同步维护，不保留旧包转发壳。
- Windows 10/11 x64、x86 宿主；macOS 13+ 本机架构按方案分阶段实施（IMK 输入法进程，管理员安装到 `/Library/Input Methods`，仅个人使用、不做代码签名）；Linux 只预留，暂不开发。专用 Rust 虎码单字引擎，不依赖 librime、Lua、OpenCC 运行库或 Rime 配置生态。
- 正式代码、开发工具和测试使用 Rust；不引入 Python、Electron、WebView、云服务、遥测、自动更新或常驻看门狗。Windows 安装包经用户批准由 Inno Setup 编译，`.iss` 只含声明与停后台、登记/改名/清理占用旧文件的少量 Pascal，打包驱动仍为 Rust。macOS 打包、安装、升级与卸载全由 Rust 完成，不用 pkgbuild/productbuild/codesign/lipo/iconutil，链接后不改写可执行文件。
- 资源占用从低：不引入异步运行时、通用界面框架；空闲零定时唤醒。热点先剖析，确需时依次用 `core::arch`、`asm!`，只放在允许 unsafe 的平台边界，并有与安全实现的对照测试。
- 只建立有实现的 crate。依赖边界由 Rust 架构测试检查；新增成员/依赖须同步方案、调用方与测试，不增加通配豁免。
- contracts/engine/input-client/ui-model 禁止 unsafe 和任何平台依赖；engine 不做文件、进程或窗口操作。backend 共享业务不依赖平台组件，文件系统原语只放内部 storage 系统边界。UI 不调用 engine/backend；input-method 不依赖 engine/backend/ui-model/UI、不加载词表；候选窗归 input-method。分层与禁止边以方案“多平台架构”为准。
- 每登录会话一个后台（macOS 为输入法进程本身），引擎与 UI 各有固定线程。不得全局业务可变状态、裸指针/窗口句柄暗中耦合或逐键/逐连接建线程。
- Windows/macOS FFI unsafe 限于边界，资源配对释放，panic 不穿过系统回调（含 Objective-C 回调）。注释以 `SAFETY:` 开头，正文简体中文。
- 还在开发阶段，无需向后兼容，保持代码、文档干净。

## 输入与数据保护
- TSF 单次回调与 IMK 单次按键处理主动等待总预算 50ms；连接、编码、重试共享绝对截止时间；事件驱动、可取消，不忙轮询或持锁阻塞通信。
- 输入路径不读文件/配置、部署、遍历词表、同步启动程序或逐键日志。队列、查询、缓存有界，按键/提交不得静默丢弃。
- 校验协议版本、长度、会话、序号、代次和快照。故障取消未上屏组合、放行原键；不修改已上屏文字、重放不确定按键或接受迟到提交。
- 单后台故障由用户重新启动；线程超时不等于线程已终止。当前 IPC 的 EXE 限制不得在 DLL 驻留/关闭验证前移除。
- `D:\虎码秃版 小狼毫（Win）`、`D:\Projects\weasel` 只读。不读取个人词库，不自动注册或切换输入法。
- 正式数据独立存放 `%APPDATA%\TigerType`（macOS 为 `~/Library/Application Support/TigerType`）；测试只用新建合成临时目录，不接管旧目录或共享现用数据库。
- 原件锁定只读；导入验证来源、大小/hash、格式、路径。失败不覆盖旧文件；拒绝越界、链接/目录联接。
- Cargo/资源锁定；下载使用官方 HTTPS 并验证 hash。递归删除/移动先验证绝对目标；不得修改防护软件、全局 Git 配置或个人数据权限。
- 锁定码表、字体与安装语言文件以原始字节入库（`resources/`、`installer/`，`.gitattributes` 禁止换行转换），换机克隆即可打包；`vendor/`、`.cache/`、`artifacts/`、`target/`、日志和数据库不入 Git。

## 变更与文档
- Ponytail full：先检查 Git/项目状态、相关代码和调用方，做最小完整修改，不覆盖无关未提交工作。用户已授权删除退役 Rime 路线。未经明确授权不派生子代理。
- 新增/修改代码包含简短简体中文注释，说明边界、用途和原因，不逐行复述。
- 四份主文档：README 管入口/命令；AGENTS 管约束；方案管范围/架构/验收；状态管实际实现/问题/最近验证。不重复维护进度。
- 每批更新状态，重要决策更新方案；原始测试证据保存 artifacts，不每批新建专题或保留旧路线文档。组件退役不是旧故障已修复。
- 交付前审查调用方、测试、样例、配置、导出和文档；依赖检查不能替代输入路径、生命周期、安全和无障碍审查。
- 不自动 commit/push。提交前检查暂存内容、敏感数据、生成产物及失败测试；Git 身份只改本仓库。

## 必跑检查
```powershell
cargo run --locked --offline -p tigertype-dev --example check
```
统一执行架构/文档、fmt、Clippy、x64/x86 常规测试；每批结束必须实际运行，失败不得标记完成。架构/文档检查也纳入普通 cargo test。
- 非平凡逻辑有测试，检查器包含反例。缺缓存按 README 准备，不跳过验证。
- 完整资源、真实应用、预期故障和长时验收单列；ignored 不算 passed，旧 librime 测试不是新引擎证据。
- 统计后台和宿主 TSF 增量；微基准不代表端到端，短循环不替代两小时压力/八小时使用。
- 回复末尾说明未验证项和风险，不以代码量或测试数量代替实际完成。
