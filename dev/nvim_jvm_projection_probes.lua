return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local notified, pending, deliveries = {}, {}, {}
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then notified[params.textDocument.uri] = params.textDocument.version end
    return sent
  end
  local files = { api = 'KotlinExtensions.kt', java = 'JavaUse.java', kotlin = 'KotlinUse.kt', decoy = 'Decoy.java' }
  local buffers, original, paths, snapshots, requests = {}, {}, {}, {}, {}
  for key, name in pairs(files) do
    local path = vim.fs.abspath(env.project .. '/' .. name)
    local buf = vim.fn.bufadd(path)
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = name:match('%.java$') and 'java' or 'kotlin'
    buffers[key], paths[key], original[key] = buf, path, vim.fn.readfile(path)
    env.sources[path] = env.sources[path] or { buf = buf, disk = original[key], sha256 = vim.fn.sha256(table.concat(original[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
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
    if next(expected) then check(name .. ': all changed buffers delivered before request', sent) end
    deliveries[#deliveries + 1] = { name = name, expected = expected, outbound = vim.deepcopy(notified), delivered = sent }
    pending = {}
  end
  local function restore()
    for key in pairs(files) do set(key, original[key]); vim.bo[buffers[key]].modified = false end
    deliver('restore')
  end
  local function changed(key, before, after)
    local text = table.concat(original[key], '\n')
    local start, finish = assert(text:find(before, 1, true))
    return vim.split(text:sub(1, start - 1) .. after .. text:sub(finish + 1), '\n', { plain = true })
  end
  local function position(key, anchor, token) return probe.find(lines(key), anchor, token) end
  local function location(key, anchor, token)
    local _, range = position(key, anchor, token)
    return probe.location_key(vim.uri_from_bufnr(buffers[key]), range)
  end
  local function request(name, key, anchor, token, method, extra)
    local pos = position(key, anchor, token)
    if method == 'textDocument/completion' then pos.character = pos.character + #token end
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers[key]) }, position = pos }, extra or {})
    deliver(name)
    local response, err = client:request_sync(method, params, 5000, buffers[key])
    requests[#requests + 1] = { name = name, method = method, params = params, response = response, error = err,
      attempts = 1, phase = 'first-response-after-buffer-delivery' }
    check(name .. ': RPC response', response ~= nil and response.err == nil and err == nil)
    return response and response.result
  end
  local function completion(name, key, anchor, prefix, offered, excluded)
    local result = request(name, key, anchor, prefix, 'textDocument/completion', { context = { triggerKind = 1 } })
    if offered then check(name .. ': useful ' .. offered, probe.completion_has(result, offered)) end
    for _, token in ipairs(excluded or {}) do check(name .. ': excludes ' .. token, not probe.completion_has(result, token)) end
  end
  local function no_goto(name, key, anchor, token)
    local result = request(name, key, anchor, token, 'textDocument/definition')
    check(name .. ': no arbitrary definition', #probe.locations(result) == 0, vim.inspect(result))
  end
  local function exact_goto(name, key, anchor, token)
    local _, range = position('api', 'Call<T?>.await()', 'await')
    local result = request(name, key, anchor, token, 'textDocument/definition')
    check(name .. ': exact authored declaration', probe.exact_definition(result, paths.api, { range }), vim.inspect(result))
  end
  local java_anchor = 'return KotlinExtensions.awaitNullable'
  -- Request inside the argument list, where signature help is defined.
  do
    local pos = position('java', java_anchor, 'awaitNullable')
    pos.character = pos.character + #'awaitNullable(call, '
    local response = client:request_sync('textDocument/signatureHelp', { textDocument = { uri = vim.uri_from_bufnr(buffers.java) }, position = pos }, 5000, buffers.java)
    local signature = response and response.result
    requests[#requests + 1] = { name = 'lowered JVM signature arguments', method = 'textDocument/signatureHelp', response = response, attempts = 1 }
    check('lowered JVM signature RPC response', response ~= nil and response.err == nil)
    check('lowered JVM signature has two parameters and Object return', type(signature) == 'table'
      and #signature.signatures == 1 and #signature.signatures[1].parameters == 2
      and signature.signatures[1].label:match('^awaitNullable%(') ~= nil
      and signature.signatures[1].label:match(': Object$') ~= nil, vim.inspect(signature))
  end
  local result = request('cross-view references', 'java', java_anchor, 'awaitNullable', 'textDocument/references', { context = { includeDeclaration = true } })
  local actual, expected = {}, {
    location('api', 'Call<T?>.await()', 'await'), location('java', java_anchor, 'awaitNullable'),
    location('kotlin', 'return call.await()', 'await'),
  }
  for _, item in ipairs(probe.locations(result)) do actual[#actual + 1] = probe.location_key(item.uri, item.range) end
  table.sort(actual); table.sort(expected)
  check('cross-view references are exact with no decoy', vim.deep_equal(actual, expected), vim.inspect(result))

  local function rename(name, key, anchor, token, new_name, wanted)
    local edit = request(name, key, anchor, token, 'textDocument/rename', { newName = new_name })
    local normalized, safe = type(edit) == 'table' and vim.deepcopy(edit), type(edit) == 'table'
    local function canonical(uri)
      for file, path in pairs(paths) do
        if probe.path_key(vim.uri_to_fname(uri)) == probe.path_key(path) then return vim.uri_from_bufnr(buffers[file]) end
      end
    end
    if normalized and normalized.changes then
      local changes = {}
      for uri, edits in pairs(normalized.changes) do
        local target = canonical(uri)
        safe = safe and target ~= nil
        if target then changes[target] = vim.list_extend(changes[target] or {}, edits) end
      end
      normalized.changes = changes
    end
    for _, change in ipairs(normalized and normalized.documentChanges or {}) do
      local target = change.textDocument and canonical(change.textDocument.uri)
      safe = safe and target ~= nil
      if target then change.textDocument.uri = target end
    end
    local applied, err = false, 'unsafe edit'
    if safe then applied, err = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding) end
    check(name .. ': edit applies in Neovim', applied, applied and nil or tostring(err))
    for file in pairs(files) do
      if vim.bo[buffers[file]].modified then pending[buffers[file]] = true end
    end
    deliver(name .. ': applied edit')
    local snapshot = {}
    for file in pairs(files) do
      check(name .. ': exact whole ' .. file .. ' buffer', vim.deep_equal(lines(file), wanted[file] or original[file]))
      snapshot[files[file]] = table.concat(lines(file), '\n') .. '\n'
      check(name .. ': disk unchanged ' .. file, vim.deep_equal(vim.fn.readfile(paths[file]), original[file]))
    end
    snapshots[name] = snapshot
    restore()
  end
  rename('Java alias rename', 'java', java_anchor, 'awaitNullable', 'awaitJava', {
    api = changed('api', '@JvmName("awaitNullable")', '@JvmName("awaitJava")'),
    java = changed('java', 'KotlinExtensions.awaitNullable(', 'KotlinExtensions.awaitJava('),
  })
  rename('Kotlin authored rename', 'api', 'Call<T?>.await()', 'await', 'awaitPayload', {
    api = changed('api', 'Call<T?>.await()', 'Call<T?>.awaitPayload()'),
    kotlin = changed('kotlin', 'call.await()', 'call.awaitPayload()'),
  })
  set('api', changed('api', 'awaitNullable', 'awaitUpdated'))
  no_goto('dirty annotation removes old alias', 'java', java_anchor, 'awaitNullable')
  set('java', changed('java', 'KotlinExtensions.awaitNullable(', 'KotlinExtensions.awaitUpdated('))
  completion('dirty annotation new alias', 'java', 'return KotlinExtensions.awaitUpdated', 'awaitUpdated', 'awaitUpdated', { 'awaitNullable' })
  exact_goto('dirty annotation new alias', 'java', 'return KotlinExtensions.awaitUpdated', 'awaitUpdated')
  restore()
  for _, owner in ipairs({ 'KotlinExtensions.await', 'MissingFacade.awaitNullable' }) do
    set('java', changed('java', 'KotlinExtensions.awaitNullable', owner))
    local token = owner:match('%.(.+)$')
    completion(owner, 'java', 'return ' .. owner, token, nil, { token })
    no_goto(owner, 'java', 'return ' .. owner, token)
    completion(owner .. ': known control', 'java', 'call.regular()', 'regular', 'regular')
    restore()
  end
  set('java', changed('java', 'awaitNullable(call, continuation)', 'awaitN'))
  completion('unfinished Java selector', 'java', 'return KotlinExtensions.awaitN', 'awaitN', 'awaitNullable')
  restore()
  set('kotlin', changed('kotlin', 'call.await()', 'call.awaitNullable()'))
  completion('JVM alias is not a Kotlin source name', 'kotlin', 'return call.awaitNullable()', 'awaitNullable', nil, { 'awaitNullable' })
  no_goto('JVM alias is not a Kotlin source name', 'kotlin', 'return call.awaitNullable()', 'awaitNullable')
  restore()
  vim.fn.writefile({ vim.json.encode(requests) }, env.artifacts .. '/jvm-projection-requests.json')
  vim.fn.writefile({ vim.json.encode(deliveries) }, env.artifacts .. '/jvm-projection-delivery.json')
  vim.fn.writefile({ vim.json.encode(snapshots) }, env.artifacts .. '/jvm-renamed-snapshots.json')
end
