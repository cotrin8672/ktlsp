local function ambiguous_definitions(probe, result, target, ranges)
  if result == nil or result == vim.NIL then return true end
  if type(result) == 'table' and vim.islist(result) and #result == 0 then return true end
  local locations = probe.locations(result)
  if #locations ~= #ranges then return false end
  local seen = {}
  for _, location in ipairs(locations) do
    local matched
    for index, range in ipairs(ranges) do
      if probe.exact_definition(location, target, { range }) then
        if seen[index] then return false end
        seen[index], matched = true, true
        break
      end
    end
    if not matched then return false end
  end
  return true
end

local function run(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local packet = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
  local states = vim.json.decode(table.concat(vim.fn.readfile(packet .. '/states.json'), '\n'))
  local client, check = env.client, env.check
  local files = {
    names = 'src/main/kotlin/probe/Names.kt', use = 'src/main/kotlin/probe/NamesUse.kt',
    overloads = 'src/main/kotlin/probe/NamedRouter.kt', overload_use = 'src/main/kotlin/probe/Use.kt',
    invocation = 'src/main/java/probe/Invocation.java', other = 'src/main/java/probe/Other.java',
  }
  local order = { 'names', 'use', 'overloads', 'overload_use', 'invocation', 'other' }
  local paths, original, buffers, pending, notified = {}, {}, {}, {}, {}
  local requests, deliveries, snapshots, policies, planned = {}, {}, {}, {}, {}
  local cases = {
    { 'original-known', 'textDocument/completion' },
    { 'original-result', 'textDocument/completion' },
    { 'original-return-goto', 'textDocument/definition' },
    { 'original-callee', 'textDocument/definition' },
    { 'declaration-only-known', 'textDocument/completion' },
    { 'declaration-only-result', 'textDocument/completion' },
    { 'matching-caller-known', 'textDocument/completion' },
    { 'matching-caller-result', 'textDocument/completion' },
    { 'matching-caller-return-goto', 'textDocument/definition' },
    { 'matching-caller-callee', 'textDocument/definition' },
    { 'restored-known', 'textDocument/completion' },
    { 'restored-result', 'textDocument/completion' },
    { 'restored-return-goto', 'textDocument/definition' },
    { 'restored-callee', 'textDocument/definition' },
    { 'ambiguous-known', 'textDocument/completion' },
    { 'ambiguous-result', 'textDocument/completion' },
    { 'ambiguous-callee', 'textDocument/definition' },
  }
  for _, case in ipairs(cases) do
    local row = { case = 'named-calls:' .. case[1], method = case[2], attempts = 0, outcome = 'not-run' }
    planned[case[1]] = row
    table.insert(assert(env.request_outcomes, 'corpus hook needs request_outcomes'), row)
  end
  local function operational(name, ok, detail)
    return check('named calls: ' .. name, ok, detail, 'operational')
  end
  local function disk_text(path) return table.concat(vim.fn.readfile(path, 'b'), '\n') end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
  local function replace_once(value, before, after)
    local text = table.concat(value, '\n')
    local first, last = text:find(before, 1, true)
    assert(first and not text:find(before, first + 1, true), 'unique edit anchor required: ' .. before)
    return vim.split(text:sub(1, first - 1) .. after .. text:sub(last + 1), '\n', { plain = true })
  end
  probe.outside_project(env.project, env.artifacts)
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then
      notified[params.textDocument.uri] = params.textDocument.version
    end
    return sent
  end
  local function set(key, value)
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, value)
    pending[buffers[key]] = true
  end
  local function deliver(name)
    local expected = {}
    for buf in pairs(pending) do expected[vim.uri_from_bufnr(buf)] = vim.lsp.util.buf_versions[buf] end
    require('vim.lsp._changetracking').flush(client)
    local delivered = true
    for uri, version in pairs(expected) do delivered = delivered and notified[uri] == version end
    deliveries[#deliveries + 1] = { name = name, expected = expected, outbound = vim.deepcopy(notified),
      delivered = delivered, barrier = 'vim.lsp._changetracking.flush(client)' }
    pending = {}
    if next(expected) then operational(name .. ': changed versions delivered before request', delivered) end
    return delivered
  end
  local function snapshot(name, state, wanted)
    local result = { state = state, files = {} }
    local exact = true
    for _, key in ipairs(order) do
      if buffers[key] then
        local text, current = probe.buffer_full_text(buffers[key]), lines(key)
        local full_hash = vim.fn.sha256(text)
        local matches = vim.deep_equal(current, wanted[key]) and full_hash == states[state][files[key]].sha256
        exact = exact and matches
        operational(name .. ': exact whole ' .. key .. ' buffer', matches)
        operational(name .. ': ' .. key .. ' disk unchanged',
          vim.fn.sha256(disk_text(paths[key])) == states.original[files[key]].sha256)
        result.files[files[key]] = { text = text, sha256 = full_hash, modified = vim.bo[buffers[key]].modified }
      end
    end
    snapshots[name] = result
    return exact
  end
  local function restore(name)
    for _, key in ipairs(order) do
      if buffers[key] then
        if not vim.deep_equal(lines(key), original[key]) then set(key, original[key]) end
        vim.bo[buffers[key]].modified = false
      end
    end
    local delivered = deliver(name)
    for _, key in ipairs(order) do
      if buffers[key] then operational(name .. ': ' .. key .. ' clean', not vim.bo[buffers[key]].modified) end
    end
    return snapshot(name, 'original', original) and delivered
  end
  local function position(key, anchor, token) return probe.find(lines(key), anchor, token) end
  local function request(name, anchor, token, accepts, extra)
    local row = assert(planned[name])
    assert(row.attempts == 0, 'semantic retry forbidden: ' .. name)
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers.use) },
      position = position('use', anchor, token) }, extra or {})
    local delivered = deliver(name)
    local response, err
    if delivered then
      row.attempts = 1
      response, err = client:request_sync(row.method, params, 5000, buffers.use)
    end
    row.outcome = probe.request_outcome(response, err, accepts, row.attempts == 1)
    row.first_outcome, row.error = row.outcome, response and response.err or err
    row.response_received = row.outcome == 'matched' or row.outcome == 'mismatch'
    requests[#requests + 1] = { case = row.case, method = row.method, params = params, response = response,
      error = row.error, attempts = row.attempts, outcome = row.outcome, response_received = row.response_received,
      phase = 'first-response-after-buffer-delivery', outbound = vim.deepcopy(notified) }
    if row.attempts == 1 then operational(name .. ': RPC response received', row.response_received, row.outcome) end
    assert(delivered, 'delivery failed; skip dependent RPCs and restore originals')
    return response and response.result, row
  end
  local function semantic(name, ok, row)
    return check('named calls: ' .. name, row.response_received and ok, nil, 'semantic', row.outcome)
  end
  local function completion(name, anchor, token, offered, excluded)
    local function accepts(value)
      if offered and not probe.completion_has(value, offered) then return false end
      for _, absent in ipairs(excluded or {}) do if probe.completion_has(value, absent) then return false end end
      return true
    end
    -- Position is exactly at the authored member start, immediately after the dot: empty prefix.
    local result, row = request(name, anchor, token, accepts, { context = { triggerKind = 1 } })
    if offered then semantic(name .. ': useful ' .. offered, probe.completion_has(result, offered), row) end
    for _, absent in ipairs(excluded or {}) do semantic(name .. ': excludes ' .. absent, not probe.completion_has(result, absent), row) end
    return result, row
  end
  local function exact_goto(name, anchor, token, target, target_anchor, target_token)
    local _, range = position(target, target_anchor, target_token)
    local result, row = request(name, anchor, token, function(value)
      return probe.exact_definition(value, paths[target], { range })
    end)
    semantic(name .. ': exact declaration', probe.exact_definition(result, paths[target], { range }), row)
  end
  local function useful_phase(name, anchor)
    completion(name .. '-known', ' known.body()', 'body', 'body', { 'service' })
    completion(name .. '-result', anchor, 'service', 'service', { 'body' })
    exact_goto(name .. '-return-goto', anchor, 'service', 'invocation', 'public Class<?> service()', 'service')
    exact_goto(name .. '-callee', anchor, 'pick', 'names', 'fun <A,B> pick(', 'pick')
  end
  local ok, err = xpcall(function()
    for _, key in ipairs(order) do
      local path = vim.fs.abspath(env.project .. '/' .. files[key])
      assert(vim.fn.sha256(disk_text(path)) == states.original[files[key]].sha256, 'pinned source changed: ' .. key)
      local buf = vim.fn.bufadd(path)
      vim.fn.bufload(buf)
      paths[key], buffers[key], original[key] = path, buf, vim.fn.readfile(path)
      vim.bo[buf].filetype = files[key]:match('%.java$') and 'java' or 'kotlin'
      env.sources[path] = env.sources[path] or { buf = buf, disk = original[key],
        sha256 = states.original[files[key]].normalized_sha256 }
      assert(operational(key .. ': initially original and clean', vim.deep_equal(lines(key), original[key]) and not vim.bo[buf].modified))
      vim.lsp.buf_attach_client(buf, client.id)
      assert(operational(key .. ': attached to the same client', vim.lsp.buf_is_attached(buf, client.id)))
    end
    local decl_only, matching, ambiguous = vim.deepcopy(original), vim.deepcopy(original), vim.deepcopy(original)
    decl_only.names = replace_once(original.names, 'pick(first:', 'pick(chosen:')
    matching.names, matching.use = decl_only.names, replace_once(original.use, 'first=left', 'chosen=left')
    ambiguous.use = replace_once(original.use, ' known.body()', ' ambiguous(marker=both,type=left).service()\n known.body()')
    assert(snapshot('original', 'original', original))
    useful_phase('original', ' pick(second=right,first=left).service()')
    set('names', decl_only.names)
    assert(snapshot('declaration-only', 'declaration-only', decl_only))
    completion('declaration-only-known', ' known.body()', 'body', 'body', { 'service' })
    local stale, stale_row = completion('declaration-only-result', ' pick(second=right,first=left).service()', 'service', nil, { 'body' })
    policies.declaration_only = { response_received = stale_row.response_received,
      service_present = probe.completion_has(stale, 'service'), body_present = probe.completion_has(stale, 'body'),
      distinctive_members_withheld = stale_row.response_received and not probe.completion_has(stale, 'service') and not probe.completion_has(stale, 'body'),
      role = 'Invalid old label: record conservative withholding separately; internal Unknown is tested in Rust. Best-effort service completion is not universally forbidden; false Other.body is excluded.' }
    set('use', matching.use)
    assert(snapshot('matching-caller', 'matching-caller', matching))
    useful_phase('matching-caller', ' pick(second=right,chosen=left).service()')
    assert(restore('formal-name-restored'))
    useful_phase('restored', ' pick(second=right,first=left).service()')
    set('use', ambiguous.use)
    assert(snapshot('ambiguous', 'ambiguous', ambiguous))
    completion('ambiguous-known', ' known.body()', 'body', 'body', { 'service' })
    completion('ambiguous-result', ' ambiguous(marker=both,type=left).service()', 'service', nil, { 'service', 'body' })
    local _, first = position('names', 'fun <T> ambiguous(type:', 'ambiguous')
    local _, second = position('names', 'fun <T> ambiguous(marker:', 'ambiguous')
    local ranges, uri = { first, second }, vim.uri_from_bufnr(buffers.names)
    local one, two = { uri = uri, range = first }, { uri = uri, range = second }
    assert(ambiguous_definitions(probe, vim.NIL, paths.names, ranges))
    assert(ambiguous_definitions(probe, { one, two }, paths.names, ranges))
    assert(ambiguous_definitions(probe, { two, one }, paths.names, ranges))
    assert(not ambiguous_definitions(probe, { one }, paths.names, ranges))
    assert(not ambiguous_definitions(probe, { one, one }, paths.names, ranges))
    assert(not ambiguous_definitions(probe, { one, { uri = vim.uri_from_bufnr(buffers.use), range = second } }, paths.names, ranges))
    local overbroad = vim.deepcopy(two)
    overbroad.range['end'].character = overbroad.range['end'].character + 1
    assert(not ambiguous_definitions(probe, { one, overbroad }, paths.names, ranges))
    local result, row = request('ambiguous-callee', ' ambiguous(marker=both,type=left).service()', 'ambiguous', function(value)
      return ambiguous_definitions(probe, value, paths.names, ranges)
    end)
    semantic('ambiguous-callee: no arbitrary single target; empty or exact two-candidate set',
      ambiguous_definitions(probe, result, paths.names, ranges), row)
  end, debug.traceback)
  local restored, restore_error = pcall(restore, 'final-restoration')
  client.rpc.notify = notify
  operational('final restoration completed', restored and restore_error == true, restored and nil or tostring(restore_error))
  local function write(name, value) vim.fn.writefile({ vim.json.encode(value) }, env.artifacts .. '/' .. name) end
  write('named-call-requests.json', requests)
  write('named-call-delivery.json', deliveries)
  write('named-call-snapshots.json', snapshots)
  write('named-call-policy-observations.json', { observations = policies,
    default_omission = 'Same manifest requires Invocation.service completion and exact goto. Empty/Unknown remains a positive coverage gap for both servers; no server-specific oracle.',
    ambiguity = 'Compiler oracle must prove genuine overload ambiguity; no single callee/return type may be fabricated.',
    native_compiler_status_at_authoring = 'unrun; root-owned independent controls required',
    request_policy = 'one request per planned case, after outbound didChange flush; no desired-content readiness/retries' })
  if not ok then error(err, 0) end
end
return run, { ambiguous_definitions = ambiguous_definitions }
