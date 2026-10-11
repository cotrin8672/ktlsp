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

-- Arm before one edit; callers flush didChange before awaiting its first target notification.
function M.observe_diagnostics(client, buffer)
  local uri, tick = vim.uri_from_bufnr(buffer), vim.api.nvim_buf_get_changedtick(buffer)
  local key = M.path_key(vim.api.nvim_buf_get_name(buffer))
  local row = { method = 'textDocument/publishDiagnostics', uri = uri, buffer = buffer,
    before_changedtick = tick, attempted = false, outbound = {}, notifications = {} }
  local function source_hash()
    return vim.fn.sha256(table.concat(vim.api.nvim_buf_get_lines(buffer, 0, -1, false), '\n')
      .. (vim.bo[buffer].eol and '\n' or ''))
  end
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
        source_sha256 = source_hash(), sent_ns = vim.uv.hrtime() }
      row.outbound[#row.outbound + 1] = item
      if sent and item.changedtick > tick and item.version == item.buffer_version then
        row.attempted, row.expected_version, row.source_sha256 = true, item.version, item.source_sha256
        row.sent_ns, row.changedtick = item.sent_ns, item.changedtick
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
      elseif #row.outbound ~= 1 or source_hash() ~= row.source_sha256
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
    if #row.outbound ~= 1 or source_hash() ~= row.source_sha256
        or vim.api.nvim_buf_get_changedtick(buffer) ~= row.changedtick
        or vim.lsp.util.buf_versions[buffer] ~= row.expected_version then
      row.response, row.error = nil, row.attempted and 'source-correlation-unavailable' or 'didChange-not-observed'
    elseif row.response == nil then row.error = 'timeout' end
    active = false
    if client.rpc.notify == on_notify then client.rpc.notify = notify end
    if client.handlers['textDocument/publishDiagnostics'] == on_diagnostics then
      client.handlers['textDocument/publishDiagnostics'] = previous
    end
    return row.error == nil and row.response and row.response.result or nil, row
  end
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
