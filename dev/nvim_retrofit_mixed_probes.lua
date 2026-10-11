local function reference_matcher(probe, sites)
  local by_path, expected = {}, {}
  for _, site in ipairs(sites) do
    local path = probe.path_key(vim.uri_to_fname(site.uri))
    assert(not by_path[path], 'only one pinned service reference per file')
    local identifier = vim.deepcopy(site.range)
    local member
    if site.member_start then
      member = vim.deepcopy(identifier)
      member.start.character = site.member_start
    end
    local key = probe.location_key(site.uri, identifier)
    by_path[path] = { identifier = identifier, member = member, key = key }
    expected[key] = true
  end
  local function normalizeReference(item)
    if type(item) ~= 'table' or type(item.uri) ~= 'string' or type(item.range) ~= 'table' then return nil end
    local ok, path = pcall(function() return probe.path_key(vim.uri_to_fname(item.uri)) end)
    local site = ok and by_path[path]
    if site and (vim.deep_equal(item.range, site.identifier)
      or (site.member and vim.deep_equal(item.range, site.member))) then return site.key end
  end
  return function(value)
    local found, actual = probe.locations(value), {}
    if #found ~= #sites then return false end
    for _, item in ipairs(found) do
      local key = normalizeReference(item)
      if not key or actual[key] then return false end
      actual[key] = true
    end
    return vim.deep_equal(actual, expected)
  end
end

local function substring_matches(probe, value)
  return probe.exact_definition(value, nil,
    { { start = { line = 1924, character = 18 }, ['end'] = { line = 1924, character = 27 } } }, 'java/lang/String.java')
    or probe.exact_definition(value, nil,
      { { start = { line = 403, character = 32 }, ['end'] = { line = 403, character = 41 } } },
      'kotlin-stdlib-1.9.23-sources.jar!/jvmMain/kotlin/text/StringsJVM.kt')
end

local function run(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local intellij = (vim.env.KTLSP_PROBE_SERVER or vim.env.KTLSP_MIXED_SERVER or 'ktlsp') == 'intellij'
  local order = { 'api', 'kotlin', 'sample', 'test' }
  local files = {
    api = { name = 'retrofit/src/main/java/retrofit2/Invocation.java', line = 82, column = 18,
      anchor = 'public Class<?> service()',
      sha256 = '1702334cef6e49c877a5c3bf2f5011e6857bc4dd62502a0394d0119373e4e1b2',
      renamed = 'ac7bca82073397b7213ea2f7a1ec2c541fd00f2b8a490110d844543b8b09f809' },
    kotlin = { name = 'retrofit/src/main/java/retrofit2/KotlinExtensions.kt', line = 41, column = 37,
      anchor = 'invocation.service()',
      sha256 = '1e58c64c2a4a5ff4037293844a3a5acccd6a8034aa1c41d466612c4247323f26',
      renamed = '3819df4e229d50ed3b36bc42829590e31f826d7b567e74f31c729189c991246d' },
    sample = { name = 'samples/src/main/java/com/example/retrofit/InvocationMetrics.java', line = 57, column = 23, member_start = 12,
      anchor = 'invocation.service().getSimpleName(),',
      sha256 = '41120504649f7776b42e6c10c78307f3129e5a642169493dc2ab095fb60fdacd',
      renamed = 'd4600a743e0107b25aa2407a08baf981f9abdbf4d1ebed48450c2c7f7e881cee' },
    test = { name = 'retrofit/java-test/src/test/java/retrofit2/InvocationTest.java', line = 75, column = 26, member_start = 15,
      anchor = 'assertThat(invocation.service()).isEqualTo(ExampleSub.class);',
      sha256 = '85e620b7db9d7b9722429e849676dd32f90b1a9d1e4bf9ba794d9eec44757753',
      renamed = '67f40521ee6921477d6ca64282715e8956d39b7e65c63fbc32e67a4a9dc48ff6' },
  }
  local cases = {
    { 'original-goto', 'textDocument/definition' },
    { 'original-Class-completion', 'textDocument/completion' },
    { 'dirty-String-completion', 'textDocument/completion' },
    { 'dirty-Java-declaration', 'textDocument/definition' },
    { 'dirty-String-substring', 'textDocument/definition' },
    { 'restored-Class-completion', 'textDocument/completion' },
    { 'restored-Java-declaration', 'textDocument/definition' },
    { 'Kotlin-references', 'textDocument/references' },
    { 'Java-references', 'textDocument/references' },
    { 'prepare-service-rename', 'textDocument/prepareRename' },
    { 'rename-service', 'textDocument/rename' },
    { 'renamed-goto', 'textDocument/definition' },
    { 'renamed-references', 'textDocument/references' },
    { 'rename-restoration-goto', 'textDocument/definition' },
    { 'rename-restoration-references', 'textDocument/references' },
  }
  local planned, requests, deliveries, snapshots = {}, {}, {}, {}
  for _, case in ipairs(cases) do
    local row = { case = 'retrofit-service:' .. case[1], method = case[2], attempts = 0, outcome = 'not-run' }
    planned[case[1]] = row
    table.insert(assert(env.request_outcomes, 'corpus hook needs request_outcomes'), row)
  end
  local function operational(name, ok, detail) return check('retrofit service: ' .. name, ok, detail, 'operational') end
  local function hash(lines) return vim.fn.sha256(table.concat(lines, '\n')) end
  local function selection(key, length)
    local file = files[key]
    return { start = { line = file.line, character = file.column },
      ['end'] = { line = file.line, character = file.column + length } }
  end
  local paths, original, renamed, typed, buffers = {}, {}, {}, {}, {}
  probe.outside_project(env.project, env.artifacts)
  for _, key in ipairs(order) do
    local file = files[key]
    paths[key] = vim.fs.abspath(env.project .. '/' .. file.name)
    original[key] = vim.fn.readfile(paths[key])
    assert(operational('original ' .. key .. ' hash', hash(original[key]) == file.sha256, hash(original[key])),
      'pinned Retrofit source changed')
    local _, found = probe.find(original[key], file.anchor, 'service')
    assert(vim.deep_equal(found, selection(key, 7)), 'pinned service range changed: ' .. key)
    renamed[key] = vim.deepcopy(original[key])
    local line = renamed[key][file.line + 1]
    renamed[key][file.line + 1] = line:sub(1, file.column) .. 'serviceType' .. line:sub(file.column + 8)
    assert(hash(renamed[key]) == file.renamed, 'rename whole-text oracle changed: ' .. key)
    typed[key] = original[key]
  end
  local function changed(key, before, after)
    local text = table.concat(original[key], '\n')
    local first, last = text:find(before, 1, true)
    assert(first and not text:find(before, first + 1, true), 'edit anchor must be unique: ' .. before)
    return vim.split(text:sub(1, first - 1) .. after .. text:sub(last + 1), '\n', { plain = true })
  end
  typed.api = changed('api', 'public Class<?> service() {\n    return service;\n  }',
    'public String service() {\n    return service.getName();\n  }')
  typed.kotlin = changed('kotlin', '${service.name}', '${service.substring(0)}')
  assert(hash(typed.api) == 'b0c8b94086a0a3b98f2b422cbf4a53b755965859260368469cfcccbef23360be')
  assert(hash(typed.kotlin) == '6cadb8623013e91500800d49fc7f6959a78eed33105489e470bb4fa50016c931')

  local notified, pending = {}, {}
  local notify = client.rpc.notify
  local rpc_request, raw_rename_response = client.rpc.request
  -- Capture before the IntelliJ client's request wrapper translates document versions.
  client.rpc.request = function(method, params, callback, ...)
    if method ~= 'textDocument/rename' then return rpc_request(method, params, callback, ...) end
    return rpc_request(method, params, function(err, result, ...)
      raw_rename_response = { err = vim.deepcopy(err), result = vim.deepcopy(result) }
      return callback(err, result, ...)
    end, ...)
  end
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then notified[params.textDocument.uri] = params.textDocument.version end
    return sent
  end
  local function lines(key)
    return buffers[key] and vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) or original[key]
  end
  local function attach(key)
    local buf = vim.fn.bufadd(paths[key])
    vim.fn.bufload(buf)
    buffers[key] = buf
    vim.bo[buf].filetype = files[key].name:match('%.java$') and 'java' or 'kotlin'
    env.sources[paths[key]] = env.sources[paths[key]] or { buf = buf, disk = original[key], sha256 = files[key].sha256 }
    assert(operational('initial ' .. key .. ' buffer is original and clean',
      vim.deep_equal(lines(key), original[key]) and not vim.bo[buf].modified))
    vim.lsp.buf_attach_client(buf, client.id)
    assert(operational(key .. ' attaches to the same client', vim.lsp.buf_is_attached(buf, client.id)))
  end
  local function set(key, value)
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, value)
    pending[buffers[key]] = true
  end
  local function deliver(name)
    local expected = {}
    for buf in pairs(pending) do expected[vim.uri_from_bufnr(buf)] = vim.lsp.util.buf_versions[buf] end
    require('vim.lsp._changetracking').flush(client)
    local sent = true
    for uri, version in pairs(expected) do sent = sent and notified[uri] == version end
    deliveries[#deliveries + 1] = { name = name, expected = expected, outbound = vim.deepcopy(notified), delivered = sent }
    pending = {}
    if next(expected) then operational(name .. ': changed versions delivered before request', sent) end
    return sent
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
      operational(name .. ': ' .. key .. ' original and clean', vim.deep_equal(lines(key), original[key])
        and (not buffers[key] or not vim.bo[buffers[key]].modified))
    end
    return delivered
  end
  local function snapshot(name, wanted, kind, outcome)
    local value = { files = {}, sha256 = {} }
    local exact = true
    for _, key in ipairs(order) do
      local current = lines(key)
      value.files[files[key].name], value.sha256[key] = table.concat(current, '\n') .. '\n', hash(current)
      exact = exact and (not wanted or vim.deep_equal(current, wanted[key]))
      if wanted then check('retrofit service: ' .. name .. ': exact whole ' .. key .. ' buffer',
        vim.deep_equal(current, wanted[key]), nil, kind, outcome) end
      operational(name .. ': ' .. key .. ' disk unchanged', vim.deep_equal(vim.fn.readfile(paths[key]), original[key]))
    end
    snapshots[name] = value
    return exact
  end
  local function request(name, key, position, accepts, extra, allowed)
    local row = assert(planned[name])
    assert(row.attempts == 0, 'no semantic retries: ' .. name)
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers[key]) },
      position = position }, extra or {})
    local response, err
    local delivered = allowed == false or deliver(name)
    if allowed ~= false and delivered then
      row.attempts = 1
      response, err = client:request_sync(row.method, params, 5000, buffers[key])
    end
    row.outcome = probe.request_outcome(response, err, accepts, row.attempts == 1)
    row.first_outcome, row.error = row.outcome, response and response.err or err
    row.response_received = row.outcome == 'matched' or row.outcome == 'mismatch'
    requests[#requests + 1] = { case = row.case, method = row.method, params = params, response = response,
      error = row.error, attempts = row.attempts, outcome = row.outcome, response_received = row.response_received,
      phase = 'first-response-after-buffer-delivery', outbound = vim.deepcopy(notified) }
    if row.attempts == 1 then operational(name .. ': RPC response received', row.response_received, row.outcome) end
    check(row.case, row.outcome == 'matched', row.outcome, 'semantic', row.outcome)
    assert(delivered, 'delivery failed; restore original buffers before further requests')
    return row.outcome, response and response.result
  end
  local kotlin_use, receiver = { line = 41, character = 37 }, { line = 44, character = 39 }
  local function goto_service(name, range, allowed)
    return request(name, 'kotlin', kotlin_use, function(value)
      return probe.exact_definition(value, paths.api, { range })
    end, nil, allowed)
  end
  local function completion(name, offered, excluded)
    return request(name, 'kotlin', receiver, function(value)
      return probe.completion_has(value, offered) and not probe.completion_has(value, excluded)
    end, { context = { triggerKind = 1 } })
  end
  local function references(name, key, include, length, allowed)
    local sites = {}
    for _, file in ipairs(order) do
      if include or file ~= 'api' then sites[#sites + 1] = { uri = vim.uri_from_fname(paths[file]),
        range = selection(file, length), member_start = files[file].member_start } end
    end
    return request(name, key, selection(key, length).start, reference_matcher(probe, sites),
      { context = { includeDeclaration = include } }, allowed)
  end
  local function normalize_rename(edit)
    if type(edit) ~= 'table' then return nil end
    local normalized, seen, count, numeric_versions = vim.deepcopy(edit), {}, 0, {}
    local function edits_for(uri, edits)
      if type(uri) ~= 'string' or type(edits) ~= 'table' or not vim.islist(edits) or #edits == 0 then return nil end
      local ok, path = pcall(vim.uri_to_fname, uri)
      if not ok then return nil end
      for _, key in ipairs(order) do
        if probe.path_key(path) == probe.path_key(paths[key]) then
          if not seen[key] then seen[key], count = {}, count + 1 end
          local site = selection(key, 7)
          for _, item in ipairs(edits) do
            if type(item) ~= 'table' or type(item.newText) ~= 'string' or type(item.range) ~= 'table' then return nil end
            local start, finish = item.range.start, item.range['end']
            if type(start) ~= 'table' or type(finish) ~= 'table' or start.line ~= site.start.line
              or finish.line ~= site.start.line or type(start.character) ~= 'number' or type(finish.character) ~= 'number'
              or start.character % 1 ~= 0 or finish.character % 1 ~= 0 or start.character < site.start.character
              or finish.character > site['end'].character or start.character > finish.character then return nil end
            for _, prior in ipairs(seen[key]) do
              local duplicate = start.character == prior.start.character and finish.character == prior['end'].character
              local overlap = start.character < prior['end'].character and prior.start.character < finish.character
              if duplicate or overlap then return nil end
            end
            seen[key][#seen[key] + 1] = item.range
          end
          return vim.uri_from_bufnr(buffers[key]), key
        end
      end
    end
    if normalized.changes ~= nil then
      if type(normalized.changes) ~= 'table' then return nil end
      local changes = {}
      for uri, edits in pairs(normalized.changes) do
        local canonical = edits_for(uri, edits)
        if not canonical then return nil end
        changes[canonical] = vim.list_extend(changes[canonical] or {}, edits)
      end
      normalized.changes = changes
    end
    if normalized.documentChanges ~= nil then
      if type(normalized.documentChanges) ~= 'table' or not vim.islist(normalized.documentChanges) then return nil end
      for index, change in ipairs(normalized.documentChanges) do
        if type(change) ~= 'table' or change.kind ~= nil or type(change.textDocument) ~= 'table' then return nil end
        local canonical = edits_for(change.textDocument.uri, change.edits)
        if not canonical then return nil end
        numeric_versions[index] = type(change.textDocument.version) == 'number'
        change.textDocument.uri = canonical
      end
    end
    if count ~= 4 then return nil end
    if intellij then
      -- The official ledger keys didOpen URIs; canonicalize before translating versions.
      require('intellij-lsp.versions').fix_workspace_edit(client.id, normalized)
    end
    for index, change in ipairs(normalized.documentChanges or {}) do
      local version = change.textDocument.version
      if numeric_versions[index] and type(version) ~= 'number' then return nil end
      local buf = vim.uri_to_bufnr(change.textDocument.uri)
      if version ~= nil and version ~= vim.NIL and version ~= vim.lsp.util.buf_versions[buf] then return nil end
    end
    return normalized
  end

  local ok, err = xpcall(function()
    attach('kotlin')
    snapshot('original', original, 'operational')
    goto_service('original-goto', selection('api', 7))
    completion('original-Class-completion', 'cast', 'substring')
    attach('api')
    set('api', typed.api)
    completion('dirty-String-completion', 'substring', 'cast')
    goto_service('dirty-Java-declaration', { start = { line = 82, character = 16 }, ['end'] = { line = 82, character = 23 } })
    set('kotlin', typed.kotlin)
    request('dirty-String-substring', 'kotlin', receiver, function(value)
      return substring_matches(probe, value)
    end)
    snapshot('typed', typed, 'operational')
    assert(restore('typed restoration'), 'typed restoration was not delivered')
    completion('restored-Class-completion', 'cast', 'substring')
    goto_service('restored-Java-declaration', selection('api', 7))
    attach('sample')
    attach('test')
    references('Kotlin-references', 'kotlin', true, 7)
    references('Java-references', 'api', false, 7)
    local prepared = request('prepare-service-rename', 'kotlin', kotlin_use, function(value)
      return type(value) == 'table' and vim.deep_equal(value.range or value, selection('kotlin', 7))
        and (value.placeholder == nil or value.placeholder == 'service')
    end)
    local normalized
    local outcome = request('rename-service', 'kotlin', kotlin_use, function(value)
      normalized = normalize_rename(raw_rename_response and raw_rename_response.result or value)
      return normalized ~= nil
    end, { newName = 'serviceType' }, prepared == 'matched')
    requests[#requests].pre_adapter_response = raw_rename_response
    requests[#requests].normalized_workspace_edit = normalized
    local applied, apply_error = false, 'rename was not observed within the four identifiers'
    if outcome == 'matched' then applied, apply_error = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding) end
    if outcome == 'matched' then operational('WorkspaceEdit applies in Neovim', applied, applied and nil or tostring(apply_error)) end
    for _, key in ipairs(order) do if vim.bo[buffers[key]].modified then pending[buffers[key]] = true end end
    local exact = snapshot('renamed', renamed, 'semantic', applied and 'matched' or 'not-run')
    goto_service('renamed-goto', selection('api', 11), applied and exact)
    references('renamed-references', 'kotlin', true, 11, applied and exact)
    assert(restore('rename restoration'), 'rename restoration was not delivered')
    goto_service('rename-restoration-goto', selection('api', 7))
    references('rename-restoration-references', 'kotlin', true, 7)
  end, debug.traceback)
  local restored, restore_error = pcall(restore, 'final restoration')
  client.rpc.notify = notify
  client.rpc.request = rpc_request
  operational('final restoration completed', restored and restore_error == true,
    restored and nil or tostring(restore_error))
  snapshot('restored', original, 'operational')
  local function write(name, value) vim.fn.writefile({ vim.json.encode(value) }, env.artifacts .. '/' .. name) end
  write('retrofit-mixed-requests.json', requests)
  write('retrofit-mixed-delivery.json', deliveries)
  write('retrofit-mixed-snapshots.json', { snapshots = snapshots,
    source_oracle = 'Pinned Retrofit source search and independently compiled original, typed and renamed snapshots; prior service-specific IDEA behavior unmeasured',
    rename_version_adapter = intellij and 'canonical URI then intellij-lsp.versions.fix_workspace_edit; numeric versions must match current buffers' or 'none',
    substring_source = { entry = 'java/lang/String.java', jdk = 8,
      sha256 = 'ce62ee1ea4c053a999f4eae39f07b87310cf83aa89f7bdc663beeab507b7b2d9' },
    substring_wrapper_source = { entry = 'jvmMain/kotlin/text/StringsJVM.kt', kotlin = '1.9.23',
      sha256 = '37406171c360a06065998771cef70c32dd74c8ce2f1626225bde401fce832c3a' } })
  if not ok then error(err, 0) end
end

-- The hook remains the first return value; pure predicates are shared with boundary checks and artifact replay.
return run, { references = reference_matcher, substring = substring_matches }
