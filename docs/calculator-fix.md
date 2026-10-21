# LUA-001：计算器错误传播修复

日期：2026-10-21。范围仅为输入中间态、求值和结果格式化的错误隔离，不宣称完成 Lua 安全沙箱。

## 根因

原 `lua/calculator_translator.lua` 直接执行 `load("return "..expe)()`。
逐键输入到 `=1+`、`=sin(` 等中间态时，load 返回 nil 和编译错误，随后调用 nil，导致 `LuaTranslation::Next error(2)`。
合法语法仍可能在求值时出错；此外原序列化函数遇到不可串接的函数值也可能在最后拼接时抛出错误。

## 最小修复

- 单独保存 load 的返回值，编译未成功则不产出计算器候选。
- 用一次 pcall 包围求值、原序列化和最终文本拼接；错误时不产出候选，不向 Rime 回调传播。
- 正常 nil 仍不显示候选，0 和 false 保留，原有数学/列表/lambda 辅助函数不重写。
- Candidate / yield 位于 pcall 外，避免把候选产出跨进受保护调用边界。
- 原有输入过滤保留。没有扩大表达式能力，也没有在 Rust 端忽略原生错误。

共享模块由 tiger、tigress、PY_c 三个方案引用，修复在同一资源入口应用。

## 原件、补丁与所有权

原包及 `vendor/tiger` 均未修改。仅提交小的替换片段，不提交完整第三方脚本或词表：

- `resources/patches/calculator-evaluation.lua`：函数内部的替换区段，不是独立 Lua 模块。
- `resources/patches/calculator.json`：补丁 ID、目标和输入/输出校验值。
- `resources::stage_tiger`：仅向空运行目录复制资源，然后校验原件、唯一替换区段及结果 hash；失败时不会发布工作区标记。
- `verify_runtime_tiger`：在启动引擎前拒绝未打补丁、过时或被改动的运行脚本，不静默覆盖修改。
- 每轮报告包含 `runtime-patches.json`，记录实际应用的补丁。未来正式部署也须复用运行资源准备入口，不能直接把原 vendor 当运行目录。

| 对象 | SHA256 |
|---|---|
| 原包 / vendor | `34632691063927b61b34d29e81122de6e2c7edfe983803bdfc8f69b9b45181ec` |
| 修复后的运行副本 | `5a00734be83120ef5d5be8034ab7f8b6f0eea5e17e3ef65558a2135727230467` |

保留原 CRLF/LF 及区段外所有字节。资源整体分发授权仍未确认，补丁和验证仅用于个人本地项目。

## 测试

`fixtures/engine-smoke.json` 从 20 项扩展到 40 项，仍由 Rust 通过真实 librime 逐键执行：

- 3 个方案正常计算 `=1+2`。
- 未完成运算符/括号、非法语法、缺少参数、未知函数、整数除零、不可串接函数值、nil：无候选、无意外提交。
- 0、false、幂、三角函数、最大值、range、sum、lambda、原脚本示例和字符串结果保持正确。
- 每个方案在同一会话中输入错误表达式、清屏后，再计算 `=1+2`，验证状态可恢复。
- 保留每轮 1,000 次会话循环、一次性提交、英文透传及学习开关检查。

原生测试重命名为 `native_engine_outputs_without_errors`，要求子进程返回 0 且 `native_errors` 为空；不再有计算器错误白名单。
新增/更新 Rust 自检覆盖唯一替换、换行与上下文保持、输入 pin 一致性、不覆盖已有目录、拒绝被改动的运行脚本及负例断言。

两轮验证报告：

- `artifacts/rust-engine-44808-1792588650699385700`：完整原生回归。
- `artifacts/rust-engine-51388-1792589290671780800`：补充错误表达式零提交断言后的最终复验。

每轮 40 项输出、1,000 次会话循环、三方案恢复通过，原生错误均为零；首轮 stderr 复核为空，原包/vendor 哈希复验未变，运行副本哈希匹配补丁清单。
fmt、Clippy、常规 Cargo 测试、x86 基础测试和冷启动资源回归通过。测试没有使用 Python、独立 Lua 解释器、现用词典或实际输入法。

```powershell
cargo test --locked --offline -p tigerrime-app --test native -- --ignored --exact native_engine_outputs_without_errors --nocapture
```

## 未覆盖的安全范围

pcall 只能隔离 Lua 异常，不能阻止文件/程序能力、无限循环或内存膨胀。
原有全局表达式环境和过滤规则不构成完整安全边界，资源预算和命名空间污染也未在本次重写。
这些作为 LUA-002 继续跟踪，生产开放前必须完成；当前零错误回归不能替代任意恶意表达式测试、两小时/八小时稳定性或真实宿主验收。
