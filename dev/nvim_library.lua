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

  -- Both languages share these three disposable, unsaved source buffers. Capture both changed
  -- action responses before applying either returned edit; never inject a generated method body.
  local action_report = { requests = {}, outbound = {}, applied = {}, compiler_snapshots = {} }
  local order = { "kotlin", "java_owner", "java_use" }
  local buffers = { kotlin = { path = main, buf = bufnr }, java_owner = files.owner, java_use = files.use }
  for _, name in ipairs(order) do
    local file = buffers[name]
    file.original = vim.api.nvim_buf_get_lines(file.buf, 0, -1, false)
    file.hash, file.disk = probe.buffer_sha256(file.buf), disk_hash(file)
    file.modified, file.uri = vim.bo[file.buf].modified, vim.uri_from_bufnr(file.buf)
    file.wire, file.latest = probe.buffer_full_text(file.buf), nil
  end
  local previous_notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = previous_notify(method, params)
    for _, name in ipairs(order) do
      local file = buffers[name]
      if params and params.textDocument and params.textDocument.uri == file.uri
        and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
        local wire, wire_error
        if method == "textDocument/didOpen" then wire = params.textDocument.text
        else wire, wire_error = probe.apply_content_changes(file.wire, params.contentChanges, client.offset_encoding) end
        if sent and type(wire) == "string" then file.wire = wire end
        file.latest = { sent = not not sent, version = params.textDocument.version,
          source_sha256 = probe.buffer_sha256(file.buf), wire_source_sha256 = wire and vim.fn.sha256(wire),
          sent_ns = vim.uv.hrtime(), error = wire_error }
        action_report.outbound[#action_report.outbound + 1] = { file = name, delivery = vim.deepcopy(file.latest) }
      end
    end
    return sent
  end
  local function snapshot()
    local state = {}
    for _, name in ipairs(order) do
      local file = buffers[name]
      local source = probe.buffer_full_text(file.buf)
      state[name] = { path = file.path, uri = file.uri, source = source, source_sha256 = vim.fn.sha256(source),
        wire_source_sha256 = vim.fn.sha256(file.wire), delivery = vim.deepcopy(file.latest),
        version = vim.lsp.util.buf_versions[file.buf], changedtick = vim.api.nvim_buf_get_changedtick(file.buf) }
    end
    return state
  end
  local function emit(desc, row)
    if row.measured then check("interface methods: " .. desc, row.outcome == "matched", vim.inspect(row))
    else print("UNMEASURED  interface methods: " .. desc .. " -> " .. vim.inspect(row)); ok = false end
  end
  local function ask(desc, name, method, params, accepts)
    flush()
    local row = { phase = desc, file = name, method = method, params = params, attempts = 0, attempted = false,
      timeout_ms = 8000, before = snapshot() }
    for _, role in ipairs(order) do
      local state, delivery = row.before[role], row.before[role].delivery
      if not delivery or not delivery.sent or delivery.error or type(state.version) ~= "number"
        or delivery.version ~= state.version or delivery.source_sha256 ~= state.source_sha256
        or delivery.wire_source_sha256 ~= state.source_sha256 or state.wire_source_sha256 ~= state.source_sha256 then
        row.error = "source-delivery-unavailable: " .. role
      end
    end
    if not row.error then
      row.attempted, row.attempts, row.started_ns = true, 1, vim.uv.hrtime()
      local called, response, request_error = pcall(client.request_sync, client, method, params, row.timeout_ms, buffers[name].buf)
      if called then row.response, row.error = response, request_error else row.error = tostring(response) end
      row.ended_ns = vim.uv.hrtime()
    end
    row.after = snapshot()
    row.source_unchanged = vim.deep_equal(row.before, row.after)
    if not row.source_unchanged then row.error = "source-changed-during-RPC" end
    if method == "textDocument/codeAction" and row.response and row.response.err == nil and not row.error
      and row.response.result ~= nil and row.response.result ~= vim.NIL
      and (type(row.response.result) ~= "table" or not vim.islist(row.response.result)) then row.error = "invalid-code-action-list" end
    row.outcome = probe.request_outcome(row.response, row.error, function(value) return accepts(value, row) end, row.attempted)
    row.measured = row.outcome == "matched" or row.outcome == "mismatch"
    action_report.requests[#action_report.requests + 1] = row
    emit(desc, row)
    return row.response and row.response.result, row
  end
  local function selected(result)
    local actions = {}
    for _, action in ipairs(type(result) == "table" and result or {}) do
      if type(action) == "table" and action.title == "Implement missing methods" then actions[#actions + 1] = action end
    end
    return actions
  end
  local function returned_edit(name, action, state)
    local file, edits = buffers[name], {}
    local safe = type(action) == "table" and action.kind == "quickfix" and type(action.edit) == "table"
      and not action.disabled and not action.command and type(state[name].version) == "number"
    local function collect(uri, changes, version)
      local decoded, path = pcall(vim.uri_to_fname, uri)
      safe = safe and decoded and probe.path_key(path) == probe.path_key(file.path)
        and type(changes) == "table" and vim.islist(changes)
        and (version == nil or version == vim.NIL or version == state[name].version)
      for _, edit in ipairs(type(changes) == "table" and changes or {}) do edits[#edits + 1] = edit end
    end
    if safe then
      if action.edit.changes ~= nil then
        if type(action.edit.changes) ~= "table" then safe = false
        else for uri, changes in pairs(action.edit.changes) do collect(uri, changes) end end
      end
      if action.edit.documentChanges ~= nil then
        if type(action.edit.documentChanges) ~= "table" or not vim.islist(action.edit.documentChanges) then safe = false
        else
          for _, change in ipairs(action.edit.documentChanges) do
            if type(change) == "table" and type(change.textDocument) == "table" and change.kind == nil then
              collect(change.textDocument.uri, change.edits, change.textDocument.version)
            else safe = false end
          end
        end
      end
    end
    local position = probe.find(vim.api.nvim_buf_get_lines(file.buf, 0, -1, false), "// HERE_", "// HERE_")
    local insertion = { line = position.line + 1, character = 0 }
    safe = safe and #edits == 1 and type(edits[1]) == "table" and type(edits[1].newText) == "string"
      and edits[1].newText ~= "" and vim.deep_equal(edits[1].range, { start = insertion, ["end"] = insertion })
      and probe.buffer_sha256(file.buf) == state[name].source_sha256
      and vim.lsp.util.buf_versions[file.buf] == state[name].version
    if not safe then return nil, "missing or unsafe returned workspace edit" end
    return { changes = { [file.uri] = edits } }
  end
  local function actions(desc, name, marker, count, signature)
    local position = probe.find(vim.api.nvim_buf_get_lines(buffers[name].buf, 0, -1, false), marker, marker)
    local result, row = ask(desc, name, "textDocument/codeAction", {
      textDocument = { uri = buffers[name].uri }, range = { start = position, ["end"] = position },
      context = { diagnostics = {} } }, function(value, row)
        local found = selected(value)
        if #found ~= count then return false end
        if count == 0 then return true end
        local edit, error = returned_edit(name, found[1], row.after)
        row.edit_valid, row.edit_validation_error = edit ~= nil, error
        return edit ~= nil and edit.changes[buffers[name].uri][1].newText:find(signature, 1, true) ~= nil
      end)
    return selected(result)[1], row
  end
  local function install(sources)
    for _, name in ipairs(order) do vim.api.nvim_buf_set_lines(buffers[name].buf, 0, -1, false, sources[name]) end
    flush()
  end
  local function record_compiler(phase, expected_valid, prerequisites)
    action_report.compiler_snapshots[#action_report.compiler_snapshots + 1] = {
      phase = phase, expected_valid = expected_valid, prerequisites = prerequisites, sources = snapshot() }
  end
  local function model_snapshot()
    local found = {}
    for _, path in ipairs(vim.fn.globpath((vim.env.KTLSP_CACHE_DIR or "") .. "/classpath", "*.json", false, true)) do
      local handle = assert(io.open(path, "rb")); local raw = handle:read("*a"); handle:close()
      local decoded, value = pcall(vim.json.decode, raw)
      if decoded and type(value) == "table" then
        for _, module in ipairs(value.modules or {}) do
          if type(module.project_dir) == "string" and probe.path_key(module.project_dir) == probe.path_key(project) then
            found[#found + 1] = { path = path, source_sha256 = vim.fn.sha256(raw), raw_text = raw,
              model_version = value.model_version, module = module, stamp = vim.uv.fs_stat(path) }
          end
        end
      end
    end
    if #found ~= 1 then return { error = "unique-current-project-cache-unavailable", matches = #found } end
    local observed = found[1]
    observed.role = "raw project-cache observation; accepted model generation requires the root trace receipt"
    for _, source in ipairs(observed.module.source_sets or {}) do
      if source.name == "main" then
        for _, task in ipairs(source.compiler_tasks or {}) do
          if task.language == "kotlin" and task.task == source.kotlin_compile_task then
            observed.kotlin_task, observed.kotlin_has_plugin = task.task, false
            for _, argument in ipairs(task.arguments or {}) do
              if argument:match("^-Xplugin") then observed.kotlin_has_plugin = true end
            end
          elseif task.language == "java" then
            observed.java_task = task.task
            observed.java_bootstrap_null = task.options and task.options.bootstrapClasspath == vim.NIL
          end
        end
      end
    end
    observed.fixture_minimum_prerequisites = observed.kotlin_task ~= nil and observed.kotlin_has_plugin == false
      and observed.java_task ~= nil and observed.java_bootstrap_null == true
    return observed
  end
  local initial = {
    kotlin = { "package app", "interface Contract { fun required(): Int }", "class Impl : PlainOwner {",
      "    // HERE_KOTLIN", "}", "class OtherKotlin {", "    // OTHER_KOTLIN", "}",
      "fun useRequired(value: Impl) = value.required()" },
    java_owner = { "package app;", "public interface PlainOwner { int required(); }" },
    java_use = { "package app;", "public class PlainUse implements Contract {", "    // HERE_JAVA", "}",
      "class OtherJava {", "    // OTHER_JAVA", "}",
      "class Calls { static long use(PlainUse value) { return value.required(); } }" },
  }
  local function variants(kotlin_return, java_return)
    local sources = vim.deepcopy(initial)
    sources.kotlin[2] = "interface Contract { fun required(): " .. kotlin_return .. " }"
    sources.java_owner[2] = "public interface PlainOwner { " .. java_return .. " required(); }"
    return sources
  end
  local function apply_returned(name, action, row)
    local applied = { request = row.phase, before = snapshot(), success = false }
    local file = buffers[name]
    local normalized, validation_error = returned_edit(name, action, applied.before)
    local owner = name == "java_use" and "kotlin" or "java_owner"
    local safe = row.outcome == "matched" and normalized ~= nil
      and applied.before[name].source_sha256 == row.before[name].source_sha256
      and applied.before[name].version == row.before[name].version
      and applied.before[owner].source_sha256 == row.before[owner].source_sha256
      and applied.before[owner].version == row.before[owner].version
    if safe then
      local edit = normalized.changes[file.uri][1]
      applied.expected_source, applied.error = probe.apply_content_changes(applied.before[name].source,
        { { range = edit.range, text = edit.newText } }, client.offset_encoding)
      if applied.expected_source then
        applied.edit = normalized
        applied.success, applied.error = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding)
      end
    else applied.error = validation_error or "caller or owner changed after returned action" end
    flush()
    applied.after = snapshot()
    applied.whole_buffers_match = applied.success
    for _, role in ipairs(order) do
      local expected = role == name and applied.expected_source or applied.before[role].source
      applied.whole_buffers_match = applied.whole_buffers_match and applied.after[role].source == expected
    end
    check("interface methods: actual " .. name .. " edit applies once and preserves exact whole buffers",
      applied.whole_buffers_match, vim.inspect(applied))
    action_report.applied[name] = applied
    return applied.whole_buffers_match
  end
  local executed, action_error = xpcall(function()
    for _, name in ipairs(order) do assert(vim.lsp.buf_attach_client(buffers[name].buf, id)) end
    install(initial)
    action_report.model = model_snapshot()
    if not action_report.model.fixture_minimum_prerequisites then
      print("UNMEASURED  interface methods: selected ordinary prerequisites unavailable -> " .. vim.inspect(action_report.model))
      ok = false
      return
    end
    actions("unrelated Java class", "java_use", "// OTHER_JAVA", 0)
    actions("unrelated Kotlin class", "kotlin", "// OTHER_KOTLIN", 0)
    actions("initial Java implements Kotlin Int", "java_use", "// HERE_JAVA", 1, "public int required()")
    actions("initial Kotlin implements Java int", "kotlin", "// HERE_KOTLIN", 1, "override fun required(): kotlin.Int")
    record_compiler("initial missing implementations", false, true)
    install(variants("Long", "long"))
    local java_action, java_row = actions("unsaved Kotlin Long changes Java action", "java_use", "// HERE_JAVA", 1, "public long required()")
    local kotlin_action, kotlin_row = actions("unsaved Java long changes Kotlin action", "kotlin", "// HERE_KOTLIN", 1, "override fun required(): kotlin.Long")
    record_compiler("changed missing implementations", false, true)
    local java_applied = apply_returned("java_use", java_action, java_row)
    local kotlin_applied = apply_returned("kotlin", kotlin_action, kotlin_row)
    record_compiler("both actual changed edits applied", true, java_applied and kotlin_applied)
    actions("Java applied implementation is not duplicated", "java_use", "// HERE_JAVA", 0)
    actions("Kotlin applied implementation is not duplicated", "kotlin", "// HERE_KOTLIN", 0)
    for _, name in ipairs({ "java_use", "kotlin" }) do
      if action_report.applied[name].whole_buffers_match then
        local lines = vim.api.nvim_buf_get_lines(buffers[name].buf, 0, -1, false)
        local anchor = name == "java_use" and "public long required()" or "override fun required(): kotlin.Long"
        local _, target = probe.find(lines, anchor, "required")
        local position = probe.find(lines, "value.required()", "required")
        ask(name .. " goto selects actual applied method", name, "textDocument/definition", {
          textDocument = { uri = buffers[name].uri }, position = position }, function(result)
            return probe.exact_definition(result, buffers[name].path, { target })
          end)
      else
        local row = { phase = name .. " goto selects actual applied method", attempted = false, attempts = 0,
          outcome = "not-run", measured = false, error = "returned-edit-not-applied" }
        action_report.requests[#action_report.requests + 1] = row; emit(row.phase, row)
      end
    end
    install(variants("Missing", "Missing"))
    actions("unknown Kotlin return withholds Java action", "java_use", "// HERE_JAVA", 0)
    actions("unknown Java return withholds Kotlin action", "kotlin", "// HERE_KOTLIN", 0)
    local conflict = variants("Long", "long")
    table.insert(conflict.kotlin, "interface Clash { fun required(): Int }")
    conflict.kotlin[3] = "class Impl : PlainOwner, Clash {"
    conflict.java_use[2] = "public class PlainUse implements Contract, Clash {"
    install(conflict)
    actions("incompatible inherited returns withhold Java action", "java_use", "// HERE_JAVA", 0)
    actions("incompatible inherited returns withhold Kotlin action", "kotlin", "// HERE_KOTLIN", 0)
    local removed = vim.deepcopy(initial)
    removed.kotlin[2], removed.java_owner[2] = "interface Contract {}", "public interface PlainOwner {}"
    install(removed)
    actions("removed Kotlin requirement leaves no Java action", "java_use", "// HERE_JAVA", 0)
    actions("removed Java requirement leaves no Kotlin action", "kotlin", "// HERE_KOTLIN", 0)
    install(initial)
    actions("restored Kotlin Int restores Java action", "java_use", "// HERE_JAVA", 1, "public int required()")
    actions("restored Java int restores Kotlin action", "kotlin", "// HERE_KOTLIN", 1, "override fun required(): kotlin.Int")
    action_report.model_after = model_snapshot()
  end, debug.traceback)
  action_report.restored, action_report.disk_unchanged, action_report.restore_errors = true, true, {}
  action_report.restoration = {}
  for _, name in ipairs(order) do
    local file = buffers[name]
    local restored, source_ok, disk_ok = pcall(function()
      vim.api.nvim_buf_set_lines(file.buf, 0, -1, false, file.original)
      vim.bo[file.buf].modified = file.modified
      return probe.buffer_sha256(file.buf) == file.hash, disk_hash(file) == file.disk
    end)
    action_report.restored = action_report.restored and restored and source_ok
    action_report.disk_unchanged = action_report.disk_unchanged and restored and disk_ok
    action_report.restoration[name] = { source_restored = restored and source_ok or false,
      disk_unchanged = restored and disk_ok or false, expected_modified = file.modified }
    if not restored then action_report.restore_errors[name] = tostring(source_ok) end
  end
  local action_flushed, action_flush_error = pcall(flush)
  client.rpc.notify = previous_notify
  action_report.notify_hook_restored = client.rpc.notify == previous_notify
  for _, name in ipairs(order) do
    local state = action_report.restoration[name]
    local read, modified = pcall(function() return vim.bo[buffers[name].buf].modified end)
    if read then state.actual_modified = modified else state.readback_error = tostring(modified) end
    state.modified_restored = read and modified == state.expected_modified
    action_report.restored = action_report.restored and state.modified_restored
  end
  if not action_flushed then action_report.restore_errors.flush = tostring(action_flush_error) end
  action_report.error = not executed and action_error or nil
  check("interface method buffers restore without disk edits", action_report.restored and action_report.disk_unchanged
    and action_flushed and action_report.notify_hook_restored, vim.inspect(action_report))
  if not executed then check("interface method probes execute", false, action_error) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(action_report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/library-interface-implementations.json")
  end
end

if ok then
  os.exit(0)
else
  print("(LSP log under: " .. vim.fn.stdpath("log") .. "/lsp.log)")
  os.exit(1)
end
