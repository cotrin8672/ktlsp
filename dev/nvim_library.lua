---@diagnostic disable: undefined-global
-- Headless Neovim test for *library* goto-definition.
--
-- Drives the real ktlsp server against a project (arg[1]) whose `gradle/libs.versions.toml`
-- declares kotlin-stdlib, and asserts that goto on `listOf(...)` jumps into the JVM-specific
-- stdlib source set while `java.sql.Connection` jumps into indexed JDK source. Run via
-- dev/smoke_library.sh (which sets up the temp project).
--
--     nvim -l dev/nvim_library.lua <project-dir>

local project = arg[1]
assert(project and project ~= "", "usage: nvim -l dev/nvim_library.lua <project-dir>")

local bin = os.getenv("KTLSP_BIN")
if not bin or bin == "" then
  bin = vim.fn.getcwd() .. "/target/release/ktlsp"
  if vim.fn.filereadable(bin) == 0 then
    bin = vim.fn.getcwd() .. "/target/debug/ktlsp"
  end
end
assert(vim.fn.filereadable(bin) == 1, "ktlsp binary not found — run `cargo build` first")

vim.cmd("edit " .. vim.fn.fnameescape(project .. "/Main.kt"))
local bufnr = vim.api.nvim_get_current_buf()
vim.bo[bufnr].filetype = "kotlin"

local id = vim.lsp.start({ name = "ktlsp", cmd = { bin }, root_dir = project })
assert(id, "vim.lsp.start returned nil")
vim.wait(8000, function()
  local c = vim.lsp.get_client_by_id(id)
  return c ~= nil and (c.initialized == true or (c.server_capabilities and c.server_capabilities.definitionProvider))
end, 50)

local function find_token(token, occurrence)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local seen = 0
  for i, text in ipairs(lines) do
    local start = 1
    while true do
      local s = text:find(token, start, true)
      if not s then
        break
      end
      seen = seen + 1
      if seen == occurrence then
        return i - 1, s - 1
      end
      start = s + #token
    end
  end
  error(("no `%s` occurrence %d found in Main.kt"):format(token, occurrence))
end

local function definition_for(token, occurrence)
  local line, col = find_token(token, occurrence)
  local params = { textDocument = { uri = vim.uri_from_bufnr(bufnr) }, position = { line = line, character = col } }
  local loc
  -- Dependency and JDK indexing run asynchronously after `initialize`, so retry until warm.
  for _ = 1, 250 do
    local r = vim.lsp.buf_request_sync(bufnr, "textDocument/definition", params, 1000) or {}
    for _, v in pairs(r) do
      if v.result and (v.result.uri or v.result[1]) then
        loc = v.result
        if loc[1] then
          loc = loc[1]
        end
        break
      end
    end
    if loc then
      break
    end
    vim.wait(200)
  end
  return loc
end

local function check_definition(desc, token, occurrence, predicate)
  local loc = definition_for(token, occurrence)
  if loc and predicate(loc) then
    local short = loc.uri:gsub("^.*/ktlsp%-harness/", ".../ktlsp-harness/")
    print(("PASS  %s -> %s:%d"):format(desc, short, loc.range.start.line))
    return true
  end
  print(("FAIL  %s -> %s"):format(desc, vim.inspect(loc)))
  return false
end

local ok = true
local function check(desc, condition, detail)
  print((condition and "PASS  " or "FAIL  ") .. desc .. (not condition and (" -> " .. (detail or "missing result")) or ""))
  ok = condition and ok
end

local function request(buf, method, params)
  for _, response in pairs(vim.lsp.buf_request_sync(buf, method, params, 8000) or {}) do
    if response.result ~= nil then
      return response.result
    end
  end
end

ok = check_definition("goto listOf", "listOf", 1, function(loc)
  return loc.uri:match("kotlin%-stdlib")
    and (loc.uri:match("jvmMain") or loc.uri:match("commonMain"))
    and loc.uri:match("%.kt$")
end) and ok
local connection_location
ok = check_definition("goto java.sql.Connection", "Connection", 3, function(loc)
  connection_location = loc
  return loc.uri:match("Connection%.java$")
end) and ok

-- Opening indexed dependency sources must replace the same file's closed facts. Keep every
-- change unsaved: the dependency cache is evidence, not a file the probe should rewrite.
if connection_location then
  local main_line = assert(find_token("connection.close()", 1))
  local original_main_line = vim.api.nvim_buf_get_lines(bufnr, main_line, main_line + 1, false)
  local function completion_labels()
    local text = "    connection.clear"
    vim.api.nvim_buf_set_lines(bufnr, main_line, main_line + 1, false, { text })
    vim.wait(200)
    local result = request(bufnr, "textDocument/completion", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      position = { line = main_line, character = #text },
    })
    local labels = {}
    for _, item in ipairs(result and (result.items or result) or {}) do
      labels[item.label] = true
    end
    return labels
  end

  local labels = completion_labels()
  check("closed JDK source offers clearWarnings", labels.clearWarnings, vim.inspect(labels))

  local source_buf = vim.fn.bufadd(vim.uri_to_fname(connection_location.uri))
  vim.fn.bufload(source_buf)
  vim.bo[source_buf].filetype = "java"
  assert(vim.lsp.buf_attach_client(source_buf, id), "could not attach JDK source buffer")
  vim.wait(200)
  local symbols = request(bufnr, "workspace/symbol", { query = "Connection" })
  local source_symbols = 0
  for _, symbol in ipairs(symbols or {}) do
    local location = symbol.location
    if symbol.name == "Connection" and location and location.uri == connection_location.uri then
      source_symbols = source_symbols + 1
    end
  end
  check("opened JDK source has one Connection symbol", source_symbols == 1, tostring(source_symbols))

  local source_lines = vim.api.nvim_buf_get_lines(source_buf, 0, -1, false)
  local declaration_line
  for line, text in ipairs(source_lines) do
    if text:find("void clearWarnings() throws SQLException;", 1, true) then
      declaration_line = line - 1
      break
    end
  end
  check("found JDK clearWarnings declaration", declaration_line ~= nil)
  if declaration_line then
    local original_source_line = source_lines[declaration_line + 1]
    local edited_source_line = original_source_line:gsub("void clearWarnings%(%)", "void clearWarningsChanged()")
    vim.api.nvim_buf_set_lines(source_buf, declaration_line, declaration_line + 1, false, { edited_source_line })
    vim.wait(200)
    labels = completion_labels()
    check("unsaved JDK method appears in Kotlin completion", labels.clearWarningsChanged, vim.inspect(labels))
    check("Kotlin completion removes the old JDK method", not labels.clearWarnings, vim.inspect(labels))
    check("JDK source edit remains unsaved", vim.bo[source_buf].modified)

    local call = "    connection.clearWarningsChanged()"
    vim.api.nvim_buf_set_lines(bufnr, main_line, main_line + 1, false, { call })
    vim.wait(200)
    local location = request(bufnr, "textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) },
      position = { line = main_line, character = #"    connection." },
    })
    if location and location[1] then
      location = location[1]
    end
    local target = location and location.range
    local target_text = target and vim.api.nvim_buf_get_text(source_buf,
      target.start.line, target.start.character, target["end"].line, target["end"].character, {})
    check("goto unsaved JDK method returns its current declaration",
      location and location.uri == connection_location.uri and table.concat(target_text or {}, "\n") == "clearWarningsChanged",
      vim.inspect(location))

    vim.api.nvim_buf_set_lines(source_buf, declaration_line, declaration_line + 1, false, { "" })
    vim.wait(200)
    labels = completion_labels()
    check("deleted JDK method leaves no stale Kotlin completion",
      not labels.clearWarnings and not labels.clearWarningsChanged, vim.inspect(labels))
    vim.lsp.buf_detach_client(source_buf, id)
    assert(not vim.lsp.buf_is_attached(source_buf, id), "could not close JDK source buffer")
    vim.wait(200)
    labels = completion_labels()
    check("closing dirty JDK source restores the on-disk method",
      labels.clearWarnings and not labels.clearWarningsChanged, vim.inspect(labels))
    check("JDK source file was never rewritten", vim.fn.readfile(vim.uri_to_fname(connection_location.uri))[declaration_line + 1] == original_source_line)

    vim.api.nvim_buf_set_lines(source_buf, declaration_line, declaration_line + 1, false, { original_source_line })
    assert(vim.lsp.buf_attach_client(source_buf, id), "could not reopen JDK source buffer")
    vim.wait(200)
    labels = completion_labels()
    check("reopened JDK source retains only its current method",
      labels.clearWarnings and not labels.clearWarningsChanged, vim.inspect(labels))
  end
  vim.api.nvim_buf_set_lines(bufnr, main_line, main_line + 1, false, original_main_line)
end

if ok then
  os.exit(0)
else
  print("(LSP log under: " .. vim.fn.stdpath("log") .. "/lsp.log)")
  os.exit(1)
end
