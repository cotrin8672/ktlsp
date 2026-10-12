-- Shared editor boundary for the mixed fixture and real-project corpus scenarios.
local M = {}

function M.sanitize_environment()
  local removed = {}
  local suffixes = { 'TOKEN', 'API_KEY', 'SECRET', 'PASSWORD', 'PASSWD', 'ACCESS_KEY', 'PRIVATE_KEY' }
  for name in pairs(vim.fn.environ()) do
    local upper = name:upper()
    for _, suffix in ipairs(suffixes) do
      if upper == suffix or upper:sub(-#suffix - 1) == '_' .. suffix then
        -- Clear this disposable editor process too: removing a cmd_env override
        -- alone would still let its child inherit the original credential.
        vim.env[name] = nil
        removed[#removed + 1] = name
        break
      end
    end
  end
  table.sort(removed)
  return removed
end

function M.path_key(path)
  path = vim.fs.normalize(vim.uv.fs_realpath(path) or path)
  return vim.fn.has('win32') == 1 and path:lower() or path
end

function M.location_key(uri, range)
  if not uri or not range then return '?' end
  return ('%s:%d:%d-%d:%d'):format(M.path_key(vim.uri_to_fname(uri)),
    range.start.line, range.start.character, range['end'].line, range['end'].character)
end

function M.locations(result)
  if type(result) ~= 'table' then return {} end
  return (result.uri or result.targetUri) and { result } or result
end

function M.range_in_source(range, lines)
  local function inside(position)
    if type(position) ~= 'table' then return false end
    for _, key in ipairs({ 'line', 'character' }) do
      local value = position[key]
      if type(value) ~= 'number' or value < 0 or value ~= math.floor(value) then return false end
    end
    local line = lines[position.line + 1]
    return line ~= nil and position.character <= vim.str_utfindex(line, 'utf-16')
  end
  if type(range) ~= 'table' or not inside(range.start) or not inside(range['end']) then return false end
  return range.start.line < range['end'].line
    or (range.start.line == range['end'].line and range.start.character < range['end'].character)
end

function M.exact_definition(result, target, ranges, uri_suffix)
  local found = M.locations(result)
  if #found ~= 1 or type(found[1]) ~= 'table' then return false end
  local location = found[1]
  local uri = location.uri or location.targetUri
  local range = location.targetSelectionRange or location.range
  if type(uri) ~= 'string' or type(range) ~= 'table' then return false end
  if uri_suffix then
    -- Archive and extracted URIs must name the same qualified source file.
    local decoded = uri:gsub('%%(%x%x)', function(hex) return string.char(tonumber(hex, 16)) end)
    local suffix = '/' .. uri_suffix:gsub('\\', '/')
    if decoded:gsub('\\', '/'):sub(-#suffix) ~= suffix then return false end
  else
    local ok, path = pcall(vim.uri_to_fname, uri)
    if not ok or M.path_key(path) ~= M.path_key(target) then return false end
  end
  for _, expected in ipairs(ranges) do
    if vim.deep_equal(range, expected) then return true end
  end
  return false
end

-- A deadline bounds observation; only a valid RPC response can measure an expectation.
function M.request_outcome(response, err, accepts, attempted)
  if attempted == false then return 'not-run' end
  if response == nil then
    return (err == nil or err == 'timeout') and 'unanswered' or 'protocol-error'
  end
  if response.err ~= nil or err ~= nil then return 'protocol-error' end
  return accepts(response.result) and 'matched' or 'mismatch'
end

-- Match Neovim 0.12's LSP full-text encoding: fileformat separators and the eol option.
-- See runtime/lua/vim/lsp.lua:_buf_get_full_text; didOpen and FULL didChange both use it.
local line_endings = { unix = '\n', dos = '\r\n', mac = '\r' }
function M.buffer_full_text(buffer)
  local ending = line_endings[vim.bo[buffer].fileformat] or '\n'
  local text = table.concat(vim.api.nvim_buf_get_lines(buffer, 0, -1, true), ending)
  return vim.bo[buffer].eol and text .. ending or text
end

function M.buffer_sha256(buffer)
  return vim.fn.sha256(M.buffer_full_text(buffer))
end

local function nonnegative_integer(value)
  return type(value) == 'number' and value >= 0 and value < math.huge and value == math.floor(value)
end

local function wire_position(text, position, encoding)
  if type(position) ~= 'table' or not nonnegative_integer(position.line)
      or not nonnegative_integer(position.character) then return nil end
  local first = 1
  for _ = 1, position.line do
    local ending = text:find('[\r\n]', first)
    if not ending then return nil end
    first = ending + (text:sub(ending, ending + 1) == '\r\n' and 2 or 1)
  end
  local ending = text:find('[\r\n]', first)
  local line = text:sub(first, ending and ending - 1 or #text)
  local ok, byte = pcall(vim.str_byteindex, line, encoding, position.character, true)
  if not ok then return nil end
  -- Neovim rounds a UTF-16 index inside a surrogate pair upwards; it is not an exact wire position.
  local exact, units = pcall(vim.str_utfindex, line, encoding, byte, true)
  local next_byte = line:byte(byte + 1)
  if not exact or units ~= position.character or (next_byte and next_byte >= 128 and next_byte < 192) then
    return nil
  end
  return first - 1 + byte
end

-- contentChanges are ordered operations: every range refers to the text after its predecessors.
-- Reconstruct wire bytes from the armed old snapshot, never from the desired post-edit source.
function M.apply_content_changes(text, changes, encoding)
  encoding = encoding or 'utf-16'
  if type(text) ~= 'string' or type(changes) ~= 'table' or #changes == 0 then
    return nil, 'invalid-content-changes'
  end
  if encoding ~= 'utf-8' and encoding ~= 'utf-16' and encoding ~= 'utf-32' then
    return nil, 'unsupported-position-encoding'
  end
  local count = #changes
  for key in pairs(changes) do
    if not nonnegative_integer(key) or key < 1 or key > count then return nil, 'invalid-content-changes' end
  end
  for index = 1, count do
    local change = changes[index]
    if type(change) ~= 'table' or type(change.text) ~= 'string' then return nil, 'invalid-content-change' end
    if change.range == nil then
      text = change.text
    else
      if type(change.range) ~= 'table' then return nil, 'invalid-range' end
      local first = wire_position(text, change.range.start, encoding)
      local last = wire_position(text, change.range['end'], encoding)
      if first == nil or last == nil or last < first then return nil, 'invalid-range' end
      if change.rangeLength ~= nil then
        local ok, length = pcall(vim.str_utfindex, text:sub(first + 1, last), encoding)
        if not nonnegative_integer(change.rangeLength) or not ok or change.rangeLength ~= length then
          return nil, 'invalid-range-length'
        end
      end
      text = text:sub(1, first) .. change.text .. text:sub(last + 1)
    end
  end
  return text
end

-- Arm before one edit; callers flush didChange before awaiting its first target notification.
function M.observe_diagnostics(client, buffer)
  local uri, tick = vim.uri_from_bufnr(buffer), vim.api.nvim_buf_get_changedtick(buffer)
  local key = M.path_key(vim.api.nvim_buf_get_name(buffer))
  local wire_text, encoding = M.buffer_full_text(buffer), client.offset_encoding or 'utf-16'
  local row = { method = 'textDocument/publishDiagnostics', uri = uri, buffer = buffer,
    before_changedtick = tick, before_source_sha256 = vim.fn.sha256(wire_text), offset_encoding = encoding,
    attempted = false, outbound = {}, notifications = {} }
  local function source_hash() return M.buffer_sha256(buffer) end
  local function is_target(value)
    if type(value) ~= 'string' then return false end
    local ok, path = pcall(vim.uri_to_fname, value)
    return ok and M.path_key(path) == key
  end
  local previous = client.handlers['textDocument/publishDiagnostics']
  local handler = previous or vim.lsp.handlers['textDocument/publishDiagnostics']
  local notify, active = client.rpc.notify, true
  local function on_notify(method, params)
    local sent = notify(method, params)
    if active and method == 'textDocument/didChange' and is_target(params.textDocument.uri) then
      local document = params.textDocument
      local item = { sent = not not sent, version = document.version,
        buffer_version = vim.lsp.util.buf_versions[buffer], changedtick = vim.api.nvim_buf_get_changedtick(buffer),
        source_sha256 = source_hash(), sent_ns = vim.uv.hrtime(),
        fileformat = vim.bo[buffer].fileformat, eol = vim.bo[buffer].eol }
      local changes = params.contentChanges
      local change = type(changes) == 'table' and #changes == 1 and changes[1] or nil
      local reconstructed, change_error = M.apply_content_changes(wire_text, changes, encoding)
      item.content_change_count = type(changes) == 'table' and #changes or nil
      if reconstructed then
        item.wire_source_sha256 = vim.fn.sha256(reconstructed)
        item.wire_matches_snapshot = item.wire_source_sha256 == item.source_sha256
        if not item.wire_matches_snapshot then
          item.wire_error = type(change) == 'table' and change.range == nil
            and 'outbound-full-text-mismatch' or 'outbound-incremental-text-mismatch'
        end
        if sent then wire_text = reconstructed end
      else
        item.wire_matches_snapshot = false
        item.wire_error = 'outbound-' .. change_error
      end
      row.wire_error = row.wire_error or item.wire_error
      row.outbound[#row.outbound + 1] = item
      if sent and item.changedtick > tick and item.version == item.buffer_version then
        row.attempted, row.expected_version, row.source_sha256 = true, item.version, item.source_sha256
        row.sent_ns, row.changedtick = item.sent_ns, item.changedtick
        row.wire_source_sha256, row.wire_matches_snapshot = item.wire_source_sha256, item.wire_matches_snapshot
      end
    end
    return sent
  end
  local function on_diagnostics(err, result, ctx, config)
    if active and type(result) == 'table' and is_target(result.uri) then
      local item = { response = { result = vim.deepcopy(result) }, error = err,
        received_ns = vim.uv.hrtime(), version_present = result.version ~= nil and result.version ~= vim.NIL }
      row.notifications[#row.notifications + 1] = item
      if not row.attempted then item.rejected = 'before-edited-source-outbound'
      elseif row.wire_error or #row.outbound ~= 1 or source_hash() ~= row.source_sha256
          or vim.api.nvim_buf_get_changedtick(buffer) ~= row.changedtick
          or vim.lsp.util.buf_versions[buffer] ~= row.expected_version then
        item.rejected = 'source-changed-after-outbound'
      elseif item.version_present and result.version ~= row.expected_version then
        item.rejected = 'different-document-version'
      elseif row.response == nil then
        row.response, row.error, row.received_ns = item.response, err, item.received_ns
        if type(result.diagnostics) ~= 'table' then row.error = 'invalid-diagnostics-notification' end
        row.correlation = item.version_present and 'exact-document-version'
          or 'first-target-after-outbound-with-unchanged-buffer'
        row.generation_certified = item.version_present
        if not item.version_present then
          row.generation_limit = 'server omitted version; assumes the first target notification after didChange belongs to the unchanged buffer snapshot'
        end
      end
    end
    if handler then handler(err, result, ctx, config) end
  end
  client.rpc.notify, client.handlers['textDocument/publishDiagnostics'] = on_notify, on_diagnostics
  return row, function(timeout)
    row.observation_timeout_ms = timeout
    if row.attempted then
      vim.wait(timeout, function() return row.response ~= nil end, 20)
    end
    if row.wire_error or #row.outbound ~= 1 or source_hash() ~= row.source_sha256
        or vim.api.nvim_buf_get_changedtick(buffer) ~= row.changedtick
        or vim.lsp.util.buf_versions[buffer] ~= row.expected_version then
      row.response, row.error = nil, row.wire_error
        or (row.attempted and 'source-correlation-unavailable' or 'didChange-not-observed')
    elseif row.response == nil then row.error = 'timeout' end
    active = false
    if client.rpc.notify == on_notify then client.rpc.notify = notify end
    if client.handlers['textDocument/publishDiagnostics'] == on_diagnostics then
      client.handlers['textDocument/publishDiagnostics'] = previous
    end
    return row.error == nil and row.response and row.response.result or nil, row
  end
end


-- Arm before opts.mutate, then observe compiler acceptance rather than desired diagnostic content.
-- The push probe records editor delivery; semantic items come from one standard diagnostic pull.
function M.canonical_kotlin_diagnostics(client, buffer, opts)
  local timeout = assert(opts.observation_timeout_ms, 'missing canonical observation bound')
  local request_timeout = assert(opts.request_timeout_ms, 'missing diagnostic RPC bound')
  local uri, key = vim.uri_from_bufnr(buffer), M.path_key(vim.api.nvim_buf_get_name(buffer))
  local function source_hash() return M.buffer_sha256(buffer) end
  local path, offset, pending, skip_partial = opts.trace_path or vim.env.KTLSP_TRACE, 0, '', false
  local initial, baseline_error = path and vim.uv.fs_stat(path), nil
  if initial then
    local file = io.open(path, 'rb')
    if file then
      local size = file:seek('end')
      if not size then baseline_error = 'trace-baseline-unavailable'
      else
        offset = size
        if offset > 0 then file:seek('set', offset - 1); skip_partial = file:read(1) ~= '\n' end
      end
      file:close()
    else baseline_error = 'trace-baseline-unavailable' end
  end
  local push, finish_push = M.observe_diagnostics(client, buffer)
  local row = { method = 'textDocument/diagnostic', uri = uri, buffer = buffer, attempted = false,
    before_source_sha256 = source_hash(), observation_timeout_ms = timeout, request_timeout_ms = request_timeout,
    edit = opts.edit_label, trace = { path = path, after_bytes = offset, preexisting_partial_line = skip_partial,
      accepted = {}, pulls = {} }, producer_closure_complete = false, canonical_certified = false }
  local function epoch(id)
    if type(id) ~= 'string' then return nil end
    local analysis, source, store, revision = id:match('^kotlin:(%d+):(%d+):(%d+):(%d+)$')
    if not revision then return nil end
    return analysis .. ':' .. source .. ':' .. store, tonumber(source), tonumber(store)
  end
  local function target(value)
    return type(value) == 'string' and M.path_key(value) == key
  end
  local function unchanged()
    return push.attempted and #push.outbound == 1 and push.wire_matches_snapshot == true
      and not push.wire_error and source_hash() == push.source_sha256
      and vim.api.nvim_buf_get_changedtick(buffer) == push.changedtick
      and vim.lsp.util.buf_versions[buffer] == push.expected_version
  end
  local function read_trace()
    local stat = path and vim.uv.fs_stat(path)
    if not stat then row.trace.last_read_error = 'trace-unavailable'; return end
    if stat.size < offset or (initial and (stat.ino ~= initial.ino or stat.dev ~= initial.dev)) then
      row.error = 'trace-replaced-or-truncated'; return
    end
    initial = initial or stat
    local file, error = io.open(path, 'rb')
    if not file then row.trace.last_read_error = tostring(error); return end
    local position = file:seek('set', offset)
    local bytes = position and file:read('*a')
    local cursor = file:seek()
    file:close()
    if not bytes or not cursor then row.error = 'trace-read-unavailable'; return end
    offset, row.trace.cursor_bytes = cursor, cursor
    pending = pending .. bytes
    while true do
      local newline = pending:find('\n', 1, true)
      if not newline then break end
      local line = pending:sub(1, newline - 1):gsub('\r$', '')
      pending = pending:sub(newline + 1)
      if skip_partial then skip_partial = false
      elseif line ~= '' then
        local ok, event = pcall(vim.json.decode, line)
        if not ok or type(event) ~= 'table' then row.error = 'invalid-trace-event'; return end
        local args = type(event.args) == 'table' and event.args or {}
        if event.name == 'kotlin_diagnostics_accepted' and event.cat == 'compiler'
          and target(args.path) and args.source_sha256 == push.source_sha256 then
          local identity, generation, store = epoch(args.result_id)
          if not identity or generation ~= args.source_generation or store ~= args.store_epoch
            or type(args.clean) ~= 'boolean' or type(args.count) ~= 'number' or args.count < 0
            or args.count ~= math.floor(args.count) or type(args.compiler_version) ~= 'string'
            or type(args.compiler_inputs_sha256) ~= 'string'
            or not args.compiler_inputs_sha256:match('^%x+$') or #args.compiler_inputs_sha256 ~= 64 then
            row.error = 'invalid-compiler-acceptance-receipt'; return
          end
          if opts.compiler_version and args.compiler_version ~= opts.compiler_version then
            row.error = 'selected-compiler-version-mismatch'; return
          end
          row.trace.accepted[#row.trace.accepted + 1] = args
        elseif row.rpc_started_ns and event.name == 'diagnostics_pull' and event.cat == 'lsp' and target(args.path) then
          row.trace.pulls[#row.trace.pulls + 1] = args
        end
      end
    end
  end
  local function finish()
    local _, delivery = finish_push(0)
    row.push = delivery
    row.push.role = 'editor-delivery observation; first push may be the fast phase'
    row.availability = M.request_outcome(row.response, row.error, function() return true end, row.attempted)
    return row.error == nil and row.response and row.response.result or nil, row
  end
  if type(path) ~= 'string' or path == '' then row.error = 'trace-path-unavailable'; return finish() end
  if baseline_error then row.error = baseline_error; return finish() end
  local edited, edit_error = pcall(function() opts.mutate(); opts.flush() end)
  if not edited then row.error = 'fixture-edit-failed: ' .. tostring(edit_error); return finish() end
  row.source_sha256, row.expected_version, row.changedtick = push.source_sha256, push.expected_version, push.changedtick
  row.wire_source_sha256, row.wire_matches_snapshot = push.wire_source_sha256, push.wire_matches_snapshot
  if not unchanged() then row.error = 'edited-source-outbound-unavailable'; return finish() end
  local observed = vim.wait(timeout, function()
    if not unchanged() then row.error = 'source-changed-during-canonical-observation'; return true end
    read_trace()
    return row.error ~= nil or #row.trace.accepted > 0
  end, 20)
  if row.error then return finish() end
  if not observed then row.error = 'timeout'; return finish() end
  -- Drain pre-RPC events so an earlier pull cannot certify this invocation.
  read_trace()
  if row.error or not unchanged() then
    row.error = row.error or 'source-changed-before-diagnostic-pull'; return finish()
  end
  row.trace.before_rpc_bytes, row.rpc_started_ns, row.attempted = offset, vim.uv.hrtime(), true
  local called, response, error = pcall(client.request_sync, client, 'textDocument/diagnostic',
    { textDocument = { uri = uri } }, request_timeout, buffer)
  if not called then row.error = 'diagnostic-RPC-failed: ' .. tostring(response); return finish() end
  row.response, row.error = response, error
  if not row.response or row.response.err ~= nil or row.error ~= nil then return finish() end
  local result = row.response.result
  if type(result) ~= 'table' or result.kind ~= 'full' or type(result.items) ~= 'table' or not vim.islist(result.items) then
    row.error = 'invalid-full-diagnostic-report'; return finish()
  end
  read_trace()
  if row.error or not unchanged() then
    row.error = row.error or 'source-changed-during-diagnostic-pull'; return finish()
  end
  local current = epoch(result.resultId)
  for _, accepted in ipairs(row.trace.accepted) do
    if current and epoch(accepted.result_id) == current then row.accepted = accepted end
  end
  for _, pull in ipairs(row.trace.pulls) do
    if pull.result_id == result.resultId and pull.count == #result.items then row.pull = pull end
  end
  if not row.accepted or not row.pull then row.error = 'canonical-store-correlation-unavailable'; return finish() end
  row.canonical_epoch = current
  row.diagnostic_pass_complete, row.absence_certified = row.accepted.clean, row.accepted.clean
  if opts.require_complete and not row.accepted.clean then
    row.error = 'partial-compiler-result-cannot-certify-absence'; return finish()
  end
  row.canonical_certified = true
  return finish()
end

function M.completion_has(result, token)
  if type(result) ~= 'table' then return false end
  for _, item in ipairs(result.items or result) do
    if item.label == token or item.label:match('^' .. token .. '%f[^%w_]') then return true end
  end
  return false
end

function M.find(lines, anchor, token)
  local position, range
  for index, line in ipairs(lines) do
    local start = line:find(anchor, 1, true)
    if start then
      assert(not position, 'ambiguous anchor ' .. anchor)
      local column = assert(line:find(token, start, true), 'missing token ' .. token .. ' after ' .. anchor) - 1
      position = { line = index - 1, character = vim.str_utfindex(line, 'utf-16', column) }
      range = { start = position,
        ['end'] = { line = index - 1, character = vim.str_utfindex(line, 'utf-16', column + #token) } }
    end
  end
  assert(position, 'missing anchor ' .. anchor)
  return position, range
end

function M.outside_project(project, artifacts)
  project, artifacts = M.path_key(project), M.path_key(artifacts)
  assert(artifacts ~= project and artifacts:sub(1, #project + 1) ~= project .. '/',
    'artifact directory must be outside the source project')
end

function M.config(project, bin, artifacts, build_tool)
  M.removed_environment = M.sanitize_environment()
  local mode = vim.env.KTLSP_PROBE_SERVER or vim.env.KTLSP_MIXED_SERVER or 'ktlsp'
  assert(mode == 'ktlsp' or mode == 'intellij', 'probe server must be ktlsp or intellij')
  if mode == 'ktlsp' then
    return { name = 'ktlsp', root_dir = project, cmd = { (assert(bin, 'missing ktlsp binary')) } }, mode
  end
  vim.opt.runtimepath:prepend(assert(vim.env.IJLS_PLUGIN, 'set IJLS_PLUGIN to intellij-lsp.nvim checkout'))
  local server_home = artifacts .. '/server-home'
  vim.fn.mkdir(server_home, 'p')
  require('intellij-lsp.progress').configure({ build_output = 'never' })
  return require('intellij-lsp.client').config(project, {
    server_path = assert(vim.env.IJLS_SERVER, 'set IJLS_SERVER to the IntelliJ server executable'),
    accept_eula = true, build_tool = build_tool,
    default_sdk = assert(vim.env.IJLS_JDK or vim.env.JAVA_HOME, 'set IJLS_JDK or JAVA_HOME'),
    jvm_args = { '-Duser.home=' .. server_home },
    autotrigger = false, inlay_hints = false, file_templates = false,
    run = false, references = false,
  }), mode
end

function M.import_ready(client, build_tool, timeout, check)
  local progress = require('intellij-lsp.progress')
  local ready = vim.wait(timeout, function() return progress.is_ready(client.id) end, 200)
  check('IntelliJ index ready', ready)
  local state = progress.import_state(client.id)
  local imported = state and state.folders and #state.folders > 0
  for _, folder in ipairs(state and state.folders or {}) do
    imported = imported and folder.status == 'SUCCESS'
  end
  check('IntelliJ ' .. build_tool:gsub('^%l', string.upper) .. ' import succeeds', imported, vim.inspect(state))
  return ready and imported, state
end


function M.observe_shutdown(config)
  local state = { shutdown_requests = {}, exit_notifications = {}, terminate_calls = 0 }
  local callbacks = {}
  if type(config.on_exit) == 'table' then
    for _, callback in ipairs(config.on_exit) do callbacks[#callbacks + 1] = callback end
  elseif config.on_exit then
    callbacks[1] = config.on_exit
  end
  table.insert(callbacks, 1, function(code, signal, client_id)
    state.exit = { code = code, signal = signal, client_id = client_id, received_ns = vim.uv.hrtime() }
  end)
  config.on_exit = callbacks
  return state
end

function M.shutdown(client, state, timeout)
  if state.finished then return state end
  local rpc = client.rpc
  local request, notify, terminate = rpc.request, rpc.notify, rpc.terminate
  rpc.request = function(method, params, callback, ...)
    if method ~= 'shutdown' then return request(method, params, callback, ...) end
    local observation = { sent_ns = vim.uv.hrtime() }
    state.shutdown_requests[#state.shutdown_requests + 1] = observation
    local sent, id = request(method, params, function(err, result, ...)
      observation.response = { received_ns = vim.uv.hrtime(), error = err,
        result_is_null = result == nil or result == vim.NIL }
      return callback(err, result, ...)
    end, ...)
    observation.sent, observation.id = sent, id
    return sent, id
  end
  rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == 'exit' then
      state.exit_notifications[#state.exit_notifications + 1] = { sent = sent, sent_ns = vim.uv.hrtime() }
    end
    return sent
  end
  rpc.terminate = function(...)
    state.terminate_calls = state.terminate_calls + 1
    return terminate(...)
  end
  state.started_ns = vim.uv.hrtime()
  if not state.exit then client:stop(false) end
  local observed, wait_reason = vim.wait(timeout, function() return state.exit ~= nil end, 50)
  state.wait_reason = wait_reason
  state.timed_out = not observed
  if not state.exit then
    state.force_requested = true
    client:stop(true)
    vim.wait(1000, function() return state.exit ~= nil end, 20)
  end
  state.finished_ns = vim.uv.hrtime()
  local response = #state.shutdown_requests == 1 and state.shutdown_requests[1].response or nil
  state.clean = observed and response ~= nil and response.error == nil and response.result_is_null
    and state.shutdown_requests[1].sent and #state.exit_notifications == 1
    and state.exit_notifications[1].sent and state.exit.code == 0 and state.exit.signal == 0
    and state.terminate_calls == 0 and not state.timed_out
  state.clean = not not state.clean
  state.finished = true
  rpc.request, rpc.notify, rpc.terminate = request, notify, terminate
  return state
end

return M
