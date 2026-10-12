---@diagnostic disable: undefined-global
-- Headless Neovim test for *library* goto-definition.
--
-- Drives the real ktlsp server against a compiled Gradle Kotlin project (arg[1]), and asserts
-- that goto on `listOf(...)` jumps into stdlib source while `java.sql.Connection` jumps into
-- indexed JDK source. Run via dev/ktlsp-harness.sh library or dev/smoke_library.sh.
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

local main = project .. "/src/main/kotlin/Main.kt"
if vim.fn.filereadable(main) == 0 then main = project .. "/Main.kt" end
vim.cmd("edit " .. vim.fn.fnameescape(main))
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

local plain_use = project .. "/src/main/java/app/PlainUse.java"
if vim.fn.filereadable(plain_use) == 1 then
  local script_dir = vim.fs.dirname(debug.getinfo(1, "S").source:gsub("^@", ""))
  local probe = dofile(script_dir .. "/nvim_lsp_probe.lua")
  local client = assert(vim.lsp.get_client_by_id(id))
  local files = { use = { path = plain_use }, owner = { path = project .. "/src/main/java/app/PlainOwner.java" } }
  local report = { requests = {}, outbound = {} }
  local function disk_text(file) return table.concat(vim.fn.readfile(file.path, "b"), "\n") end
  local function disk_hash(file) return vim.fn.sha256(disk_text(file)) end
  for _, file in pairs(files) do
    file.buf = vim.fn.bufadd(file.path)
    vim.fn.bufload(file.buf)
    file.original = vim.api.nvim_buf_get_lines(file.buf, 0, -1, false)
    file.hash, file.disk = probe.buffer_sha256(file.buf), disk_hash(file)
    file.modified, file.uri = vim.bo[file.buf].modified, vim.uri_from_bufnr(file.buf)
    file.wire = probe.buffer_full_text(file.buf)
    vim.bo[file.buf].filetype = "java"
  end
  local function source_state(file)
    local source, disk = probe.buffer_full_text(file.buf), disk_text(file)
    return { source_sha256 = vim.fn.sha256(source), disk_sha256 = vim.fn.sha256(disk),
      version = vim.lsp.util.buf_versions[file.buf], changedtick = vim.api.nvim_buf_get_changedtick(file.buf),
      attached = vim.lsp.buf_is_attached(file.buf, id) }, source, disk
  end
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    for name, file in pairs(files) do
      if params and params.textDocument and params.textDocument.uri == file.uri
        and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
        local wire, err
        if method == "textDocument/didOpen" then wire = params.textDocument.text
        else wire, err = probe.apply_content_changes(file.wire, params.contentChanges, client.offset_encoding or "utf-16") end
        if sent and wire then file.wire = wire end
        file.latest = { sent = not not sent, version = params.textDocument.version,
          source_sha256 = probe.buffer_sha256(file.buf), wire_source_sha256 = wire and vim.fn.sha256(wire), error = err }
        report.outbound[#report.outbound + 1] = { file = name, delivery = vim.deepcopy(file.latest) }
      end
    end
    return sent
  end
  local function flush() require("vim.lsp._changetracking").flush(client) end
  local function query(phase, anchors)
    flush()
    local row = { phase = phase, method = "textDocument/diagnostic", attempts = 0, attempted = false,
      params = { textDocument = { uri = files.use.uri } }, timeout_ms = 8000,
      delivery = {}, sources = {}, before = {}, after = {}, source_unchanged = true, expected = {}, actual = {} }
    for name, file in pairs(files) do
      local state, source, disk = source_state(file)
      row.before[name], row.sources[name], row.delivery[name] = state, source, vim.deepcopy(file.latest)
      local delivered
      if state.attached then
        delivered = file.latest and file.latest.sent and file.latest.source_sha256 == state.source_sha256
          and file.latest.wire_source_sha256 == state.source_sha256 and not file.latest.error
          and file.latest.version == state.version
      else
        row.sources[name] = disk
        row.delivery[name] = { closed = true, source_sha256 = state.disk_sha256, disk_sha256 = state.disk_sha256 }
        delivered = name == "owner" and not file.latest and state.source_sha256 == state.disk_sha256
          and state.disk_sha256 == file.disk
      end
      if not delivered then row.error = row.error or ("source-delivery-unavailable: " .. name) end
    end
    local lines = vim.api.nvim_buf_get_lines(files.use.buf, 0, -1, false)
    local function key(range, code)
      return ("%d:%d:%d:%d:%s"):format(range.start.line, range.start.character,
        range["end"].line, range["end"].character, code)
    end
    for _, anchor in ipairs(anchors) do
      local _, range = probe.find(lines, anchor, anchor)
      row.expected[#row.expected + 1] = key(range, "call_shape_mismatch")
    end
    if not row.error then
      row.attempted, row.attempts, row.started_ns = true, 1, vim.uv.hrtime()
      local called, response, request_error = pcall(client.request_sync, client,
        row.method, row.params, row.timeout_ms, files.use.buf)
      if called then row.response, row.error = response, request_error
      else row.error = "diagnostic-RPC-failed: " .. tostring(response) end
      row.ended_ns = vim.uv.hrtime()
    end
    for name, file in pairs(files) do
      row.after[name] = source_state(file)
      if not vim.deep_equal(row.before[name], row.after[name]) then
        row.source_unchanged = false
        row.error = row.error or ("source-changed-during-diagnostic-pull: " .. name)
      end
    end
    local result = row.response and row.response.result
    row.valid_report = type(result) == "table" and result.kind == "full"
      and type(result.items) == "table" and vim.islist(result.items)
    if row.response and row.response.err == nil and row.error == nil then
      if not row.valid_report then row.error = "invalid-full-diagnostic-report"
      else
        for _, item in ipairs(result.items) do
          if item.severity == vim.diagnostic.severity.ERROR then row.actual[#row.actual + 1] = key(item.range, item.code or "") end
        end
      end
    end
    table.sort(row.expected)
    table.sort(row.actual)
    row.availability = probe.request_outcome(row.response, row.error, function() return true end, row.attempted)
    if row.availability == "matched" then row.availability = "responded" end
    row.outcome = probe.request_outcome(row.response, row.error, function()
      return row.valid_report and vim.deep_equal(row.actual, row.expected)
    end, row.attempted)
    row.measured = row.outcome == "matched" or row.outcome == "mismatch"
    report.requests[#report.requests + 1] = row
    if row.measured then check("selected Java diagnostics: " .. phase, row.outcome == "matched", vim.inspect(row))
    else
      print("UNMEASURED  selected Java diagnostics: " .. phase .. " -> " .. vim.inspect(row))
      ok = false
    end
  end
  local success, err = xpcall(function()
    assert(vim.lsp.buf_attach_client(files.use.buf, id))
    query("original closed owner", {})
    local invalid = vim.deepcopy(files.use.original)
    for i, line in ipairs(invalid) do
      invalid[i] = line:gsub('accept%("Ada"%)', 'accept()'):gsub('accept%(1, 2%)', 'accept(true)')
    end
    vim.api.nvim_buf_set_lines(files.use.buf, 0, -1, false, invalid)
    query("wrong arity and type", { "value.accept()", "value.accept(true)" })
    assert(vim.lsp.buf_attach_client(files.owner.buf, id))
    local changed = vim.deepcopy(files.owner.original)
    for i, line in ipairs(changed) do changed[i] = line:gsub("accept%(String value%)", "accept(boolean value)") end
    vim.api.nvim_buf_set_lines(files.owner.buf, 0, -1, false, changed)
    query("unsaved owner accepts boolean", { "value.accept()" })
    local repaired = vim.deepcopy(files.use.original)
    for i, line in ipairs(repaired) do repaired[i] = line:gsub('accept%("Ada"%)', 'accept(true)') end
    vim.api.nvim_buf_set_lines(files.use.buf, 0, -1, false, repaired)
    query("repaired caller", {})
    vim.api.nvim_buf_set_lines(files.owner.buf, 0, -1, false, files.owner.original)
    query("restored owner rejects boolean", { "value.accept(true)" })
    vim.api.nvim_buf_set_lines(files.use.buf, 0, -1, false, files.use.original)
    query("restored caller", {})
  end, debug.traceback)
  client.rpc.notify = notify
  report.restored, report.disk_unchanged, report.restore_errors = true, true, {}
  for name, file in pairs(files) do
    local restored, source_ok, disk_ok = pcall(function()
      if probe.buffer_sha256(file.buf) ~= file.hash then
        vim.api.nvim_buf_set_lines(file.buf, 0, -1, false, file.original)
      end
      vim.bo[file.buf].modified = file.modified
      return probe.buffer_sha256(file.buf) == file.hash, disk_hash(file) == file.disk
    end)
    report.restored = report.restored and restored and source_ok
    report.disk_unchanged = report.disk_unchanged and restored and disk_ok
    if not restored then report.restore_errors[name] = tostring(source_ok) end
  end
  local flushed, flush_error = pcall(flush)
  if not flushed then report.restore_errors.flush = tostring(flush_error) end
  report.error = not success and err or nil
  check("selected Java diagnostic buffers restore without disk edits",
    report.restored and report.disk_unchanged and flushed, vim.inspect(report.restore_errors))
  if not success then check("selected Java diagnostic probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/library-java-diagnostics.json")
  end
end

if ok then
  os.exit(0)
else
  print("(LSP log under: " .. vim.fn.stdpath("log") .. "/lsp.log)")
  os.exit(1)
end
