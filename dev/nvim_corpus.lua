-- Exact, read-only real-project probes through the existing editor harness.
-- nvim -l dev/nvim_corpus.lua <root> <manifest.json> <ktlsp-bin> <artifacts>
local script_dir = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(script_dir .. '/nvim_lsp_probe.lua')
local project = assert(vim.uv.fs_realpath((assert(arg[1], 'missing project root'))))
local manifest_path = assert(arg[2], 'missing probe manifest')
local artifacts = vim.fs.normalize((assert(arg[4], 'missing artifact directory')))
probe.outside_project(project, artifacts)
local manifest = vim.json.decode(table.concat(vim.fn.readfile(manifest_path), '\n'))
assert(type(manifest.probes) == 'table' and #manifest.probes > 0, 'manifest needs at least one probe')
local wait_index = vim.env.KTLSP_CORPUS_WAIT_INDEX == '1'
vim.fn.mkdir(artifacts, 'p')
vim.opt.hidden = true
vim.cmd('filetype plugin indent on')
vim.lsp.set_log_level('debug')

local checks, failures, sources, prepared, request_outcomes = {}, {}, {}, {}, {}
local function check(name, ok, detail, kind, outcome)
  kind = kind or 'semantic'
  local measured = outcome == nil or outcome == 'matched' or outcome == 'mismatch'
  local status = measured and (ok and 'matched' or 'mismatch') or outcome
  checks[#checks + 1] = { name = name, ok = not not ok, detail = detail, kind = kind,
    measured = measured, outcome = status, request_outcome = outcome }
  local label = kind == 'operational' and (ok and 'OPERATION OK ' or 'OPERATION INCOMPLETE ')
    or (measured and (ok and 'PASS semantic ' or 'FAIL semantic ') or 'UNMEASURED semantic ')
  print(label .. name .. (detail and (' — ' .. detail) or ''))
  if not ok then failures[#failures + 1] = name end
  return ok
end
local function operational(name, ok, detail)
  return check(name, ok, detail, 'operational')
end
local function write(name, value)
  vim.fn.writefile({ vim.json.encode(value) }, artifacts .. '/' .. name)
end
local function full_path(path)
  if not path:match('^[/\\]') and not path:match('^%a:[/\\]') then path = project .. '/' .. path end
  return vim.fs.abspath(path)
end
local function source_file(path)
  path = full_path(path)
  if not sources[path] then
    local disk = vim.fn.readfile(path)
    local buf = vim.fn.bufadd(path)
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = path:match('%.java$') and 'java' or 'kotlin'
    sources[path] = { buf = buf, disk = disk, sha256 = vim.fn.sha256(table.concat(disk, '\n')) }
  end
  return path, sources[path]
end
local function replace_once(lines, anchor, replacement)
  local text = table.concat(lines, '\n')
  local first, last = text:find(anchor, 1, true)
  assert(first and not text:find(anchor, first + 1, true), 'edit anchor must be unique: ' .. anchor)
  return vim.split(text:sub(1, first - 1) .. replacement .. text:sub(last + 1), '\n', { plain = true })
end
local function positions(lines, case)
  local position = probe.find(lines, assert(case.anchor), assert(case.token))
  local prefix = case.completion_prefix or case.token
  assert(case.token:sub(1, #prefix) == prefix, 'completion prefix must match token: ' .. case.id)
  return position, { line = position.line, character = position.character + vim.str_utfindex(prefix, 'utf-16') }
end
-- A target is read as oracle data, not opened/indexed merely to make a result available.
for _, case in ipairs(manifest.probes) do
  assert(case.completion_token ~= false or case.known_control,
    'negative-only completion needs a same-phase known receiver: ' .. case.id)
  local path, source = source_file(assert(case.source, 'probe needs source'))
  local edits = {}
  for _, edit in ipairs(case.buffer_edits or {}) do
    local edit_path, original = source_file(edit.source or case.source)
    assert(not edit.source_sha256 or edit.source_sha256 == original.sha256,
      'edited peer changed since oracle: ' .. case.id)
    edits[edit_path] = replace_once(edits[edit_path] or original.disk,
      assert(edit.anchor), assert(edit.replacement))
  end
  assert(not case.source_sha256 or case.source_sha256 == source.sha256, 'source changed since oracle: ' .. case.id)
  local position, completion_position = positions(edits[path] or source.disk, case)
  local target, expected_ranges
  if case.definition ~= false then
    target = full_path(assert(case.target_file or case.target, 'probe needs target'))
    local target_lines = vim.fn.readfile(target)
    assert(not case.target_sha256 or case.target_sha256 == vim.fn.sha256(table.concat(target_lines, '\n')),
      'target changed since oracle: ' .. case.id)
    local _, selection = probe.find(target_lines, assert(case.target_anchor), case.target_token or case.token)
    expected_ranges = { selection }
    if case.target_ranges then
      assert(case.target_sha256, 'explicit target ranges require a pinned target hash: ' .. case.id)
      for _, range in ipairs(case.target_ranges) do
        assert(probe.range_in_source(range, target_lines), 'target range outside source: ' .. case.id)
        expected_ranges[#expected_ranges + 1] = range
      end
    end
  end
  local prep = { case = case, buf = source.buf, position = position,
    completion_position = completion_position, edits = edits, has_edits = next(edits) ~= nil,
    ranges = expected_ranges, target = target }
  if case.known_control then
    assert(case.known_control.completion_token ~= false and case.known_control.completion ~= false,
      'known receiver control must require a useful completion: ' .. case.id)
    local control = vim.tbl_extend('force', case.known_control, { id = case.id .. ':known-control' })
    local control_position, control_completion = positions(edits[path] or source.disk, control)
    prep.known_control = { case = control, buf = source.buf, position = control_position,
      completion_position = control_completion, has_edits = prep.has_edits }
  end
  prepared[#prepared + 1] = prep
end

-- Keep unstarted requests visible when initialization/import reaches its execution bound.
for _, prep in ipairs(prepared) do
  local function register(item, method)
    item.requests = item.requests or {}
    local record = { case = item.case.id, method = method, attempts = 0, outcome = 'not-run' }
    item.requests[method] = record
    request_outcomes[#request_outcomes + 1] = record
  end
  if prep.known_control then register(prep.known_control, 'textDocument/completion') end
  if prep.case.completion ~= false then register(prep, 'textDocument/completion') end
  if prep.case.definition ~= false then register(prep, 'textDocument/definition') end
end

local function git_snapshot()
  local function run(args)
    local command = vim.list_extend({ 'git', '-C', project }, args)
    local result = vim.system(command, { text = true, timeout = 15000 }):wait()
    return { code = result.code, stdout = result.stdout, stderr = result.stderr }
  end
  return { revision = run({ 'rev-parse', 'HEAD' }), status = run({ 'status', '--porcelain', '--untracked-files=no' }) }
end
local before_git = git_snapshot()
local config, mode = probe.config(project, vim.env.KTLSP_BIN or arg[3], artifacts, manifest.build_tool or 'gradle')
local index_state = { project = false, libraries = false, messages = {} }
local semantic_phase = 'not-measured'
if mode == 'ktlsp' then
  local handler = vim.lsp.handlers['window/logMessage']
  config.handlers = { ['window/logMessage'] = function(err, params, ctx, handler_config)
    local message = params and params.message or ''
    local project_files = message:match('^ktlsp indexed (%d+) project files')
    local library_files = message:match('^ktlsp indexed (%d+) library files')
    if project_files or library_files then index_state.messages[#index_state.messages + 1] = message end
    if project_files then index_state.project, index_state.project_files = true, tonumber(project_files) end
    if library_files then
      index_state.libraries, index_state.library_files = true, tonumber(library_files)
      local failed, missing = message:match('(%d+) failed, (%d+) missing sources')
      index_state.failed, index_state.missing_sources = tonumber(failed), tonumber(missing)
    end
    if handler then return handler(err, params, ctx, handler_config) end
  end }
end
vim.api.nvim_set_current_buf(prepared[1].buf)
local shutdown_state = probe.observe_shutdown(config)
local client_started_ns = vim.uv.hrtime()
local id = vim.lsp.start(config, { bufnr = prepared[1].buf })
local client, import_state
local sent_versions, outbound_changes, delivery_records = {}, {}, {}
local active_case
local function finish()
  if client then
    local stopped = probe.shutdown(client, shutdown_state, 15000)
    operational('client shuts down gracefully', stopped.clean, vim.inspect(stopped))
  end
  for path, source in pairs(sources) do
    operational('source remains unchanged: ' .. path, vim.deep_equal(vim.fn.readfile(path), source.disk)
      and not vim.bo[source.buf].modified)
  end
  local after_git = git_snapshot()
  if before_git.status.code == 0 and after_git.status.code == 0 then
    operational('tracked working-tree status remains unchanged', before_git.status.stdout == after_git.status.stdout)
  end
  local hashes = {}
  for path, source in pairs(sources) do hashes[path] = source.sha256 end
  local semantic = { checks = {}, matched = 0, mismatched = 0, unmeasured = 0 }
  local operations = { checks = {}, failures = {}, request_availability = {
    ['protocol-error'] = 0, unanswered = 0, ['not-run'] = 0 } }
  for _, item in ipairs(checks) do
    if item.kind == 'operational' then
      operations.checks[#operations.checks + 1] = item
      if not item.ok then operations.failures[#operations.failures + 1] = item.name end
    else
      semantic.checks[#semantic.checks + 1] = item
      if not item.measured then semantic.unmeasured = semantic.unmeasured + 1
      elseif item.ok then semantic.matched = semantic.matched + 1
      else semantic.mismatched = semantic.mismatched + 1 end
    end
  end
  local unavailable, not_run = 0, 0
  for _, item in ipairs(request_outcomes) do
    if operations.request_availability[item.outcome] ~= nil then
      operations.request_availability[item.outcome] = operations.request_availability[item.outcome] + 1
      unavailable = unavailable + 1
      if item.outcome == 'not-run' then not_run = not_run + 1 end
    end
  end
  semantic.not_run_requests = not_run
  semantic.status = semantic.mismatched > 0 and 'failed'
    or (#semantic.checks == 0 and 'not-measured')
    or ((semantic.unmeasured > 0 or not_run > 0) and 'incomplete') or 'passed'
  operations.status = (#operations.failures == 0 and unavailable == 0) and 'complete' or 'incomplete'
  local coverage = { observed = index_state.libraries and index_state.failed ~= nil and index_state.missing_sources ~= nil,
    project_files_in_last_scan = index_state.project_files or vim.NIL, library_files = index_state.library_files or vim.NIL,
    failed = index_state.failed or vim.NIL, missing_sources = index_state.missing_sources or vim.NIL }
  local status = semantic.status == 'failed' and 'failed'
    or ((semantic.status ~= 'passed' or operations.status ~= 'complete') and 'incomplete') or 'passed'
  write('corpus-results.json', { mode = mode, project = project, manifest = manifest,
    manifest_path = manifest_path, checks = checks, failures = failures, source_sha256 = hashes,
    status = status, semantic = semantic, operations = operations, coverage = coverage, request_outcomes = request_outcomes,
    server = client and client.server_info, shutdown = shutdown_state, binary = vim.env.KTLSP_BIN or arg[3],
    plugin = vim.env.IJLS_PLUGIN, server_executable = vim.env.IJLS_SERVER,
    removed_environment_names = probe.removed_environment,
    jdk = vim.env.IJLS_JDK or vim.env.JAVA_HOME, gradle_user_home = vim.env.GRADLE_USER_HOME,
    import_state = import_state, git_before = before_git, git_after = after_git, neovim = vim.version(),
    index_state = index_state, wait_index = wait_index, semantic_phase = semantic_phase,
    delivery = delivery_records, outbound_changes = outbound_changes,
    query_positions = 'definition at identifier start; completion after completion_prefix or full identifier',
    performance = 'separate startup observation and post-index first response; not timing acceptance' })
  print(('\nSEMANTIC %s: %d matched, %d mismatched, %d unmeasured, %d not-run requests'):format(
    semantic.status, semantic.matched, semantic.mismatched, semantic.unmeasured, not_run))
  print('OPERATIONS ' .. operations.status .. '; SOURCE INPUTS ' .. vim.inspect(coverage))
  if #failures > 0 then print('RAW FAILURES: ' .. table.concat(failures, ', ')) end
  vim.cmd(status == 'passed' and 'qa!' or 'cq!')
end
local initialized = vim.wait(120000, function()
  client = id and vim.lsp.get_client_by_id(id)
  return client and client.initialized
end, 100)
operational('one client initializes', initialized, mode)
if not initialized then finish() end
for _, source in pairs(sources) do vim.lsp.buf_attach_client(source.buf, id) end
print('SERVER ' .. vim.inspect(client.server_info))
if vim.env.KTLSP_CORPUS_FIRST_USE == '1' then
  local prep = prepared[1]
  assert(mode == 'ktlsp' and wait_index and not prep.has_edits,
    'first-use measurement requires an unchanged ktlsp case and normal indexing readiness')
  assert(prep.case.completion ~= false and prep.case.completion_token ~= false,
    'first-use measurement requires a positive completion')
  local token = prep.case.completion_token or prep.case.token
  local params = { textDocument = { uri = vim.uri_from_bufnr(prep.buf) },
    position = prep.completion_position, context = { triggerKind = 1 } }
  local measurement = { case = prep.case.id, token = token, params = params,
    polling_interval_ms = 500, request_count = 0, attempts = {},
    meaning = 'earliest observed useful completion response since LSP start; excludes completion-menu display' }
  local deadline = vim.uv.hrtime() + (tonumber(vim.env.KTLSP_CORPUS_READY_TIMEOUT_MS) or 600000) * 1000000
  repeat
    local seconds, micros = vim.uv.gettimeofday()
    local started = vim.uv.hrtime()
    local response, err = client:request_sync('textDocument/completion', params, 5000, prep.buf)
    local outcome = probe.request_outcome(response, err, function(value)
      if not probe.completion_has(value, token) then return false end
      for _, excluded in ipairs(prep.case.completion_excludes or {}) do
        if probe.completion_has(value, excluded) then return false end
      end
      return true
    end)
    local observed_ms = (vim.uv.hrtime() - client_started_ns) / 1000000
    measurement.request_count = measurement.request_count + 1
    measurement.attempts[#measurement.attempts + 1] = {
      started_epoch_us = seconds * 1000000 + micros, observed_ms = observed_ms,
      request_elapsed_ms = (vim.uv.hrtime() - started) / 1000000, outcome = outcome,
      project_ready = index_state.project, libraries_ready = index_state.libraries }
    if outcome == 'matched' and not measurement.first_useful_ms then
      measurement.first_useful_ms, measurement.first_useful_result = observed_ms, response.result
    end
    measurement.libraries_ready = index_state.libraries
    write('corpus-first-use.json', measurement)
    if not index_state.libraries then vim.wait(measurement.polling_interval_ms) end
  until index_state.libraries or vim.uv.hrtime() >= deadline
end
if mode == 'intellij' then
  local imported
  imported, import_state = probe.import_ready(client, manifest.build_tool or 'gradle',
    tonumber(vim.env.KTLSP_CORPUS_READY_TIMEOUT_MS) or 600000, operational)
  if not imported then finish() end
elseif wait_index then
  -- Same readiness meaning as dev/bench_initialize.py: both log messages must arrive.
  local ready = vim.wait(tonumber(vim.env.KTLSP_CORPUS_READY_TIMEOUT_MS) or 600000,
    function() return index_state.project and index_state.libraries end, 100)
  operational('ktlsp project and dependency indexing finishes', ready, vim.inspect(index_state))
  if not ready then finish() end
end
local strict_first = wait_index or mode == 'intellij'
semantic_phase = strict_first and 'first-response-after-indexing' or 'startup-with-bounded-retries'
if wait_index and mode == 'ktlsp' and (index_state.failed ~= 0 or index_state.missing_sources ~= 0) then
  semantic_phase = 'first-response-after-indexing-with-incomplete-inputs'
end

local rpc_notify = client.rpc.notify
client.rpc.notify = function(method, params)
  local sent = rpc_notify(method, params)
  if sent and method == 'textDocument/didChange' then
    local document = params.textDocument
    sent_versions[document.uri] = document.version
    outbound_changes[#outbound_changes + 1] = { case = active_case, uri = document.uri, version = document.version }
  end
  return sent
end
local function deliver_edits(prep, restore)
  local expected = {}
  active_case = prep.case.id .. (restore and ':restore' or ':edit')
  for path, lines in pairs(prep.edits) do
    local source = sources[path]
    vim.api.nvim_buf_set_lines(source.buf, 0, -1, false, restore and source.disk or lines)
    expected[vim.uri_from_bufnr(source.buf)] = vim.lsp.util.buf_versions[source.buf]
    if restore then vim.bo[source.buf].modified = false end
  end
  -- Test-side transport barrier; no sleep, index warm-up, or acceptance retry.
  require('vim.lsp._changetracking').flush(client)
  local delivered, unchanged = true, true
  for uri, version in pairs(expected) do delivered = delivered and sent_versions[uri] == version end
  for path in pairs(prep.edits) do
    local source = sources[path]
    unchanged = unchanged and vim.deep_equal(vim.fn.readfile(path), source.disk)
      and (restore and not vim.bo[source.buf].modified or not restore and vim.bo[source.buf].modified)
  end
  delivery_records[#delivery_records + 1] = { case = prep.case.id, restore = restore,
    expected = expected, outbound = vim.deepcopy(sent_versions), delivered = delivered,
    barrier = 'vim.lsp._changetracking.flush(client)' }
  operational(prep.case.id .. (restore and ': restored buffers delivered' or ': unsaved edits delivered'), delivered)
  operational(prep.case.id .. (restore and ': disk golden and clean buffers restored' or ': edits remain buffer-only'), unchanged)
  return delivered and unchanged
end

local function request(prep, method, accepts)
  local position = method == 'textDocument/completion' and prep.completion_position or prep.position
  local params = { textDocument = { uri = vim.uri_from_bufnr(prep.buf) }, position = position }
  if method == 'textDocument/completion' then params.context = { triggerKind = 1 } end
  local deadline = vim.uv.hrtime() + (tonumber(vim.env.KTLSP_CORPUS_REQUEST_TIMEOUT_MS) or 15000) * 1000000
  local attempts, result, error, matched, first, response_received = 0, nil, nil, false, nil, false
  local outcome, attempt_outcomes = 'not-run', {}
  local once = strict_first or prep.has_edits
  local phase = prep.has_edits and 'first-response-after-unsaved-buffer-delivery' or semantic_phase
  local sent_before = vim.deepcopy(sent_versions)
  repeat
    attempts = attempts + 1
    local response, err = client:request_sync(method, params, 5000, prep.buf)
    result, error = response and response.result, response and response.err or err
    outcome = probe.request_outcome(response, err, accepts)
    response_received = outcome == 'matched' or outcome == 'mismatch'
    matched = outcome == 'matched'
    attempt_outcomes[#attempt_outcomes + 1] = { attempt = attempts, outcome = outcome,
      error = error, response_received = response ~= nil }
    if not first then first = { result = result, error = error, matched = matched,
      response_received = response_received, outcome = outcome } end
    if not matched and not once then vim.wait(200) end
  until matched or once or vim.uv.hrtime() >= deadline
  local record = prep.requests[method]
  record.attempts, record.outcome, record.first_outcome, record.error = attempts, outcome, first.outcome, error
  record.response_received = response_received
  vim.fn.writefile({ vim.json.encode({ case = prep.case.id, method = method, params = params,
    attempts = attempts, matched = matched, result = result, error = error, first = first,
    response_received = response_received, outcome = outcome, attempt_outcomes = attempt_outcomes,
    phase = phase, sent_before = sent_before, sent_after = vim.deepcopy(sent_versions) }) },
    artifacts .. '/corpus-requests.jsonl', 'a')
  print(('REQUEST %s %s attempts=%d first_matched=%s matched=%s outcome=%s'):format(
    prep.case.id, method, attempts, tostring(first.matched), tostring(matched), outcome))
  return matched, result, response_received, outcome
end

local function exact_definition(prep, result)
  return probe.exact_definition(result, prep.target, prep.ranges, prep.case.target_uri_suffix)
end
local function completion(prep)
  local token = prep.case.completion_token or prep.case.token
  local positive = prep.case.completion_token ~= false
  local matched, result, response_received, outcome = request(prep, 'textDocument/completion', function(value)
    if positive and not probe.completion_has(value, token) then return false end
    for _, excluded in ipairs(prep.case.completion_excludes or {}) do
      if probe.completion_has(value, excluded) then return false end
    end
    return true
  end)
  if positive then check(prep.case.id .. ': completion offers ' .. token,
    response_received and probe.completion_has(result, token), nil, 'semantic', outcome) end
  for _, excluded in ipairs(prep.case.completion_excludes or {}) do
    check(prep.case.id .. ': completion excludes ' .. excluded,
      response_received and not probe.completion_has(result, excluded), nil, 'semantic', outcome)
  end
  if not positive and #(prep.case.completion_excludes or {}) == 0 then
    check(prep.case.id .. ': completion response available', matched, nil, 'semantic', outcome)
  end
  return matched, outcome
end
for _, prep in ipairs(prepared) do
  local delivered = not prep.has_edits or deliver_edits(prep, false)
  if delivered then
    if prep.known_control then
      local matched, outcome = completion(prep.known_control)
      check(prep.case.id .. ': same-phase known receiver is useful', matched, nil, 'semantic', outcome)
    end
    if prep.case.completion ~= false then completion(prep) end
    if prep.case.definition ~= false then
      local matched, result, _, outcome = request(prep, 'textDocument/definition', function(value)
        return exact_definition(prep, value)
      end)
      check(prep.case.id .. ': goto exact declaration', matched, matched and nil or vim.inspect(result), 'semantic', outcome)
    end
  end
  -- Restore even after a failed edit barrier; later probes need confirmed original inputs.
  if prep.has_edits and not deliver_edits(prep, true) then finish() end
end
if vim.env.KTLSP_CORPUS_EXTRA_PROBES then
  local ok, err = pcall(function()
    dofile(vim.env.KTLSP_CORPUS_EXTRA_PROBES)({ repo = vim.fs.dirname(script_dir), client = client,
      sources = sources, check = check, project = project, artifacts = artifacts, request_outcomes = request_outcomes })
  end)
  operational('external corpus probes complete', ok, ok and nil or tostring(err))
end
finish()
