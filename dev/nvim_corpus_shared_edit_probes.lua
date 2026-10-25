-- Additional shared unsaved-edit cases for the existing corpus client.
return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local spec = assert(env.manifest.shared_edit)
  local client, order = env.client, { 'producer', 'consumer' }
  local canonical = spec.consumer:match('%.kt$') and (vim.env.KTLSP_PROBE_SERVER or 'ktlsp') ~= 'intellij'
  local paths, buffers, original, snapshots, pending, notified, requests = {}, {}, {}, {}, {}, {}, {}
  local function hash(lines) return vim.fn.sha256(table.concat(lines, '\n')) end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
  local function split(text) return vim.split(text, '\n', { plain = true }) end
  local function in_class(text, body)
    local first
    for position in text:gmatch('()\n}') do first = position end
    assert(first and not text:sub(first + 2):match('%S'), 'top-level closing brace changed')
    return text:sub(1, first - 1) .. '\n' .. body .. '\n' .. text:sub(first)
  end
  local function operational(name, ok, detail)
    return env.check('shared edit: ' .. name, ok, detail, 'operational')
  end
  for _, key in ipairs(order) do
    paths[key] = vim.fs.abspath(env.project .. '/' .. spec[key])
    original[key] = vim.fn.readfile(paths[key])
    assert(hash(original[key]) == spec.source_sha256[key], 'pinned shared-edit source changed: ' .. key)
    local buf = vim.fn.bufadd(paths[key])
    vim.fn.bufload(buf)
    buffers[key] = buf
    vim.bo[buf].filetype = spec[key]:match('%.java$') and 'java' or 'kotlin'
    assert(vim.deep_equal(lines(key), original[key]) and not vim.bo[buf].modified)
    env.sources[paths[key]] = env.sources[paths[key]] or { buf = buf, disk = original[key], sha256 = hash(original[key]) }
    vim.lsp.buf_attach_client(buf, client.id)
    assert(operational(key .. ' uses the same client', vim.lsp.buf_is_attached(buf, client.id)))
  end
  for _, stage in ipairs({ 'text', 'counter' }) do
    snapshots[stage] = {}
    for _, key in ipairs(order) do
      local text = table.concat(original[key], '\n')
      local body = spec.recipes[stage][key]
      if key == 'producer' or spec.consumer_insertion == 'class' then text = in_class(text, body)
      else text = text .. '\n\n' .. body end
      snapshots[stage][key] = split(text)
    end
  end
  snapshots.renamed, snapshots.retracted = {}, { producer = original.producer, consumer = snapshots.counter.consumer }
  for _, key in ipairs(order) do
    snapshots.renamed[key] = split(table.concat(snapshots.counter[key], '\n'):gsub('ktlspSliceCounter', 'ktlspSliceRenamed'))
  end
  for stage, files in pairs(snapshots) do
    for _, key in ipairs(order) do assert(hash(files[key]) == spec.stages[stage][key].sha256, 'whole-buffer oracle changed') end
  end
  local diagnostic_snapshots
  if spec.diagnostic_cycle then
    assert(canonical, 'diagnostic cycle requires the canonical Kotlin diagnostic route')
    diagnostic_snapshots = {
      invalid = { producer = original.producer, consumer = split(table.concat(original.consumer, '\n') .. '\n\n' .. spec.diagnostic_cycle.consumer) },
      fixed = original,
    }
    for phase, files in pairs(diagnostic_snapshots) do
      assert(hash(files.consumer) == spec.diagnostic_cycle.source_sha256[phase], 'diagnostic whole-buffer oracle changed')
    end
    assert(not vim.deep_equal(diagnostic_snapshots.invalid.consumer, original.consumer), 'invalid diagnostic snapshot must change the consumer')
  end
  local notify, rpc_request, raw_rename = client.rpc.notify, client.rpc.request
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then notified[params.textDocument.uri] = params.textDocument.version end
    return sent
  end
  client.rpc.request = function(method, params, callback, ...)
    if method ~= 'textDocument/rename' then return rpc_request(method, params, callback, ...) end
    return rpc_request(method, params, function(err, result, ...)
      raw_rename = { err = vim.deepcopy(err), result = vim.deepcopy(result) }
      return callback(err, result, ...)
    end, ...)
  end
  local function set(files)
    for _, key in ipairs(order) do
      if not vim.deep_equal(lines(key), files[key]) then
        vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, files[key])
        pending[buffers[key]] = true
      end
    end
  end
  local deliveries = {}
  local function deliver(name)
    local expected = {}
    for buf in pairs(pending) do expected[vim.uri_from_bufnr(buf)] = vim.lsp.util.buf_versions[buf] end
    require('vim.lsp._changetracking').flush(client)
    local sent = true
    for uri, version in pairs(expected) do sent = sent and notified[uri] == version end
    deliveries[#deliveries + 1] = { case = name, expected = expected, outbound = vim.deepcopy(notified), delivered = sent }
    pending = {}
    assert(operational(name .. ' versions delivered before request', sent), 'unsaved delivery failed')
  end
  local planned = {}
  for _, item in ipairs({
    { 'text-member', 'completion' }, { 'text-goto', 'definition' }, { 'text-result', 'completion' },
    { 'counter-diagnostics', canonical and 'diagnostic' or 'publishDiagnostics' },
    { 'counter-member', 'completion' }, { 'counter-goto', 'definition' }, { 'counter-result', 'completion' },
    { 'counter-references', 'references' }, { 'rename', 'rename' },
    { 'renamed-goto', 'definition' }, { 'renamed-references', 'references' },
    { 'rename-restoration-goto', 'definition' },
    { 'retracted-member', 'completion' }, { 'retracted-goto', 'definition' },
  }) do
    local row = { case = 'shared-edit:' .. item[1], method = 'textDocument/' .. item[2], attempts = 0, outcome = 'not-run' }
    planned[item[1]] = row
    table.insert(env.request_outcomes, row)
  end
  if diagnostic_snapshots then
    for _, phase in ipairs({ 'invalid', 'fixed' }) do
      local name = phase .. '-diagnostics'
      local row = { case = 'shared-edit:' .. name, method = 'textDocument/diagnostic', attempts = 0, outcome = 'not-run' }
      planned[name] = row
      table.insert(env.request_outcomes, row)
    end
  end
  local function selection(key, name)
    local anchor = key == 'consumer' and ('provider.' .. name .. '()') or (name .. '()')
    local position, range = probe.find(lines(key), anchor, name)
    return position, range
  end
  local function request(name, position, accepts, extra, allowed)
    local row = assert(planned[name])
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers.consumer) }, position = position }, extra or {})
    local response, err
    if allowed ~= false then
      deliver(name)
      row.attempts = 1
      response, err = client:request_sync(row.method, params, 5000, buffers.consumer)
    end
    row.outcome = probe.request_outcome(response, err, accepts, row.attempts == 1)
    row.first_outcome, row.error = row.outcome, response and response.err or err
    row.response_received = row.outcome == 'matched' or row.outcome == 'mismatch'
    requests[#requests + 1] = { case = row.case, method = row.method, params = params, response = response,
      error = row.error, attempts = row.attempts, outcome = row.outcome, response_received = row.response_received }
    env.check(row.case, row.outcome == 'matched', row.outcome, 'semantic', row.outcome)
    return row.outcome, response and response.result
  end
  local function member_completion(case, member, offered, excluded)
    local start = probe.find(lines('consumer'), 'provider.' .. member .. '()', 'provider')
    return request(case, { line = start.line, character = start.character + 9 }, function(value)
      return probe.completion_has(value, offered) and not probe.completion_has(value, excluded)
    end, { context = { triggerKind = 1 } })
  end
  local function goto_member(case, member, allowed)
    local position = selection('consumer', member)
    local _, range = selection('producer', member)
    return request(case, position, function(value) return probe.exact_definition(value, paths.producer, { range }) end, nil, allowed)
  end
  local function references(case, member, allowed)
    local expected = {}
    for _, key in ipairs(order) do
      local _, range = selection(key, member)
      expected[probe.location_key(vim.uri_from_bufnr(buffers[key]), range)] = true
    end
    return request(case, selection('consumer', member), function(value)
      local found, actual = probe.locations(value), {}
      if #found ~= 2 then return false end
      for _, item in ipairs(found) do
        local key = probe.location_key(item.uri, item.range)
        if actual[key] or not expected[key] then return false end
        actual[key] = true
      end
      return vim.deep_equal(expected, actual)
    end, { context = { includeDeclaration = true } }, allowed)
  end
  local function normalize_rename(edit)
    if type(edit) ~= 'table' then return nil end
    local value, seen, numeric_versions = vim.deepcopy(edit), {}, {}
    local function validate(uri, edits)
      if type(uri) ~= 'string' or type(edits) ~= 'table' or not vim.islist(edits) or #edits == 0 then return nil end
      for _, key in ipairs(order) do
        if probe.path_key(vim.uri_to_fname(uri)) == probe.path_key(paths[key]) then
          local _, site = selection(key, 'ktlspSliceCounter')
          if seen[key] then return nil end
          seen[key] = true
          local prior = {}
          for _, item in ipairs(edits) do
            if type(item.newText) ~= 'string' or type(item.range) ~= 'table' then return nil end
            local a, z = item.range.start, item.range['end']
            if not a or not z or type(a.character) ~= 'number' or type(z.character) ~= 'number'
              or a.character % 1 ~= 0 or z.character % 1 ~= 0 or a.line ~= site.start.line or z.line ~= site.start.line
              or a.character < site.start.character or z.character > site['end'].character or a.character > z.character then return nil end
            for _, old in ipairs(prior) do
              if a.character == old.start.character and z.character == old['end'].character
                or a.character < old['end'].character and old.start.character < z.character then return nil end
            end
            prior[#prior + 1] = item.range
          end
          return vim.uri_from_bufnr(buffers[key])
        end
      end
    end
    if value.changes then
      local changes = {}
      for uri, edits in pairs(value.changes) do local canonical = validate(uri, edits); if not canonical then return nil end; changes[canonical] = edits end
      value.changes = changes
    end
    if value.documentChanges then
      if not vim.islist(value.documentChanges) then return nil end
      for index, change in ipairs(value.documentChanges) do
        if change.kind or type(change.textDocument) ~= 'table' then return nil end
        numeric_versions[index] = type(change.textDocument.version) == 'number'
        local canonical = validate(change.textDocument.uri, change.edits)
        if not canonical then return nil end
        change.textDocument.uri = canonical
      end
      if (vim.env.KTLSP_PROBE_SERVER or 'ktlsp') == 'intellij' then require('intellij-lsp.versions').fix_workspace_edit(client.id, value) end
      for index, change in ipairs(value.documentChanges) do
        local version = change.textDocument.version
        if numeric_versions[index] and type(version) ~= 'number' then return nil end
        if version ~= nil and version ~= vim.NIL and version ~= vim.lsp.util.buf_versions[vim.uri_to_bufnr(change.textDocument.uri)] then return nil end
      end
    end
    return seen.producer and seen.consumer and value or nil
  end
  local diagnostics
  local function java_recipe_scope()
    local scope = { supported = false, tasks = {} }
    local cache = vim.env.KTLSP_CACHE_DIR
    for _, file in ipairs(cache and vim.fn.glob(cache .. '/classpath/*.json', false, true) or {}) do
      local parsed, model = pcall(vim.json.decode, table.concat(vim.fn.readfile(file), '\n'))
      if parsed then
        for _, module in ipairs(model.modules or {}) do
          for _, source_set in ipairs(module.source_sets or {}) do
            for _, task in ipairs(source_set.compiler_tasks or {}) do
              if task.language == 'java' then
                for _, source in ipairs(task.source_files or {}) do
                  if probe.path_key(source) == probe.path_key(paths.consumer) then
                    local options = task.options or {}
                    local supported = task.complete == true and type(task.java_home) == 'string'
                      and type(options.javaCompilerRecipe) == 'table'
                    scope.supported = scope.supported or supported
                    scope.tasks[#scope.tasks + 1] = { model = file, task = task.task, java_home = task.java_home,
                      recipe_problem = options.javaCompilerRecipeProblem, supported = supported }
                  end
                end
              end
            end
          end
        end
      end
    end
    return scope
  end
  local function counter_diagnostics()
    local result, row
    if canonical then
      result, row = probe.canonical_kotlin_diagnostics(client, buffers.consumer, {
        edit_label = 'shared-producer-return-type-and-member-change', require_complete = true,
        observation_timeout_ms = 300000, request_timeout_ms = 5000,
        mutate = function() set(snapshots.counter) end,
        flush = function() deliver('counter-diagnostics') end,
      })
    else
      local finish
      row, finish = probe.observe_diagnostics(client, buffers.consumer)
      set(snapshots.counter)
      deliver('counter-diagnostics')
      result, row = finish(60000)
    end
    diagnostics = row
    local record = planned['counter-diagnostics']
    record.attempts = row.attempted and 1 or 0
    record.outcome = probe.request_outcome(row.response, row.error, function(value)
      local items = canonical and value.items or value.diagnostics
      if type(items) ~= 'table' then return false end
      for _, item in ipairs(items) do if item.severity == nil or item.severity == 1 then return false end end
      return true
    end, row.attempted)
    if (record.outcome == 'matched' or record.outcome == 'mismatch')
      and not (canonical and row.canonical_certified and row.absence_certified or not canonical and row.generation_certified) then
      record.outcome = 'uncertified-generation'
    end
    if record.outcome == 'matched' and spec.consumer:match('%.java$')
      and (vim.env.KTLSP_PROBE_SERVER or 'ktlsp') ~= 'intellij' then
      row.java_recipe_scope = java_recipe_scope()
      if not row.java_recipe_scope.supported then
        record.outcome = 'unsupported-java-compiler-policy'
        row.analysis_limit = 'Empty versioned push proves delivery, not absence under an unknown/unsupported Gradle Java compiler policy.'
      end
    end
    record.first_outcome, record.error = record.outcome, row.error
    record.response_received = row.response ~= nil
    requests[#requests + 1] = { case = record.case, method = record.method, observation = row,
      independent_compiler_oracle = spec.oracle_status, outcome = record.outcome, response_received = record.response_received }
    env.check(record.case, record.outcome == 'matched', record.outcome, 'semantic', record.outcome)
    -- An unavailable trace must not leave later requests on the previous snapshot.
    set(snapshots.counter)
  end
  local function diagnostic_cycle()
    -- Restore the actual baseline first: a producer-only retraction need not change its consumer.
    set(original)
    deliver('diagnostic baseline restoration')
    for _, phase in ipairs({ 'invalid', 'fixed' }) do
      local name = phase .. '-diagnostics'
      local _, row = probe.canonical_kotlin_diagnostics(client, buffers.consumer, {
        edit_label = name, require_complete = phase == 'fixed',
        observation_timeout_ms = 300000, request_timeout_ms = 5000,
        mutate = function() set(diagnostic_snapshots[phase]) end,
        flush = function() deliver(name) end,
      })
      local record = planned[name]
      record.attempts, record.response_received = row.attempted and 1 or 0, row.response ~= nil
      local certified = row.attempted and row.canonical_certified and (phase ~= 'fixed' or row.absence_certified)
      record.outcome = certified and probe.request_outcome(row.response, row.error, function(value)
        if type(value.items) ~= 'table' then return false end
        local errors = {}
        for _, item in ipairs(value.items) do
          if item.severity == nil or item.severity == 1 then errors[#errors + 1] = item end
        end
        if phase == 'fixed' then return #errors == 0 end
        if #errors ~= 1 then return false end
        local token = spec.diagnostic_cycle.missing_member
        local _, expected = probe.find(diagnostic_snapshots.invalid.consumer, 'provider.' .. token .. '()', token)
        local item = errors[1]
        return item.message:find(token, 1, true) ~= nil
          and vim.deep_equal(item.range, expected)
      end, true) or 'unmeasured'
      record.first_outcome, record.error = record.outcome, row.error
      requests[#requests + 1] = { case = record.case, method = record.method, observation = row,
        independent_compiler_oracle = spec.oracle_status, outcome = record.outcome, response_received = record.response_received }
      env.check(record.case, record.outcome == 'matched', record.outcome, 'semantic', record.outcome)
    end
  end
  local ok, err = xpcall(function()
    for _, stage in ipairs({ 'text', 'counter' }) do
      if stage == 'counter' then counter_diagnostics() else set(snapshots[stage]) end
      local member = stage == 'text' and 'ktlspSliceText' or 'ktlspSliceCounter'
      local other = stage == 'text' and 'ktlspSliceCounter' or 'ktlspSliceText'
      member_completion(stage .. '-member', member, member, other)
      goto_member(stage .. '-goto', member)
      local offered, excluded = stage == 'text' and 'substring' or 'incrementAndGet', stage == 'text' and 'incrementAndGet' or 'substring'
      local position = probe.find(lines('consumer'), 'provider.' .. member .. '().' .. offered, offered)
      request(stage .. '-result', position, function(value)
        return probe.completion_has(value, offered) and not probe.completion_has(value, excluded)
      end, { context = { triggerKind = 1 } })
    end
    references('counter-references', 'ktlspSliceCounter')
    local normalized
    local outcome = request('rename', selection('consumer', 'ktlspSliceCounter'), function(value)
      normalized = normalize_rename(raw_rename and raw_rename.result or value)
      return normalized ~= nil
    end, { newName = 'ktlspSliceRenamed' })
    local applied = outcome == 'matched' and pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding)
    local exact = applied
    for _, key in ipairs(order) do
      pending[buffers[key]] = true
      exact = exact and vim.deep_equal(lines(key), snapshots.renamed[key])
    end
    env.check('shared edit: applied rename exact whole buffers', not not exact, nil, 'semantic', applied and 'matched' or 'not-run')
    requests[#requests].pre_adapter_response, requests[#requests].normalized_workspace_edit = raw_rename, normalized
    if exact then
      goto_member('renamed-goto', 'ktlspSliceRenamed')
      references('renamed-references', 'ktlspSliceRenamed')
    else
      request('renamed-goto', { line = 0, character = 0 }, function() return false end, nil, false)
      request('renamed-references', { line = 0, character = 0 }, function() return false end, nil, false)
    end
    set(snapshots.counter)
    goto_member('rename-restoration-goto', 'ktlspSliceCounter')
    set(snapshots.retracted)
    member_completion('retracted-member', 'ktlspSliceCounter', spec.control, 'ktlspSliceCounter')
    request('retracted-goto', selection('consumer', 'ktlspSliceCounter'), function(value) return #probe.locations(value) == 0 end)
    if diagnostic_snapshots then diagnostic_cycle() end
  end, debug.traceback)
  local restored, restore_error = pcall(function()
    set(original)
    deliver('final restoration')
    for _, key in ipairs(order) do
      vim.bo[buffers[key]].modified = false
      assert(vim.deep_equal(lines(key), original[key]) and vim.deep_equal(vim.fn.readfile(paths[key]), original[key]))
    end
  end)
  client.rpc.notify, client.rpc.request = notify, rpc_request
  operational('original clean buffers and disk restored', restored, restored and nil or tostring(restore_error))
  local function write(name, value) vim.fn.writefile({ vim.json.encode(value) }, env.artifacts .. '/' .. name) end
  write('shared-edit-requests.json', requests)
  write('shared-edit-delivery.json', deliveries)
  write('shared-edit-diagnostics.json', diagnostics or {})
  write('shared-edit-snapshots.json', { snapshots = snapshots, original = original, oracle_status = spec.oracle_status })
  if not ok then error(err) end
  if not restored then error(restore_error) end
end
