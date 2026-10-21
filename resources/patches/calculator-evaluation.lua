  -- 输入中间态可能尚未构成表达式；编译失败不应成为引擎错误。
  local chunk = load("return "..expe)
  if not chunk then return end

  -- 求值、序列化和拼接一起保护；yield 留在 pcall 外，避免跨保护边界产出候选。
  local ok, result = pcall(function()
    local value = chunk()
    if value == nil then return nil end
    return exp.."="..serialize(value)
  end)
  if not ok or result == nil then return end
  yield(Candidate("number", seg.start, seg._end, result, ""))
