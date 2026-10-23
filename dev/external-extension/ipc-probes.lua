return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local linked = vim.env.KTLSP_IPC_MODE == 'linked'
  local barrier = assert(vim.env.KTLSP_IPC_BARRIER, 'missing child barrier directory')
  local paths, buffers, originals = {}, {}, {}
  local proof = { mode = linked and 'linked' or 'ipc', ordering = {}, requests = {}, snapshots = {} }
  local provider = linked and 'fixture.external-shadow' or 'fixture.ipc-shadow'
  local fresh_diagnostic = false
  local previous_diagnostics = client.handlers['textDocument/publishDiagnostics'] or vim.lsp.handlers['textDocument/publishDiagnostics']
  client.handlers['textDocument/publishDiagnostics'] = function(err, result, ctx, config)
    if result then
      for _, diagnostic in ipairs(result.diagnostics or {}) do
        if diagnostic.message == 'ipc-fixture:bound' then fresh_diagnostic = true end
      end
    end
    return previous_diagnostics(err, result, ctx, config)
  end
  local function save()
    vim.fn.writefile({ vim.json.encode(proof) }, env.artifacts .. '/extension-ipc-proof.json')
  end
  local function record(event) proof.ordering[#proof.ordering + 1] = event; save() end
  for key, file in pairs({ target = 'fixture/Target.java', mixin = 'fixture/CaseMixin.java', use = 'fixture/IpcUse.java' }) do
    local path = vim.fs.abspath(env.project .. '/' .. file)
    local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = 'java'
    paths[key], buffers[key], originals[key] = path, buf, vim.fn.readfile(path)
    env.sources[path] = env.sources[path] or { buf = buf, disk = originals[key], sha256 = vim.fn.sha256(table.concat(originals[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
  local function deliver()
    require('vim.lsp._changetracking').flush(client)
  end
  local function request(name, key, anchor, token, method, extra, accepts)
    deliver()
    local position = probe.find(lines(key), anchor, token)
    if method == 'textDocument/completion' then position.character = position.character + #token end
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers[key]) }, position = position }, extra or {})
    local response, err = client:request_sync(method, params, 10000, buffers[key])
    local outcome = probe.request_outcome(response, err, accepts)
    local item = { case = name, method = method, attempts = 1, outcome = outcome }
    env.request_outcomes[#env.request_outcomes + 1] = item
    proof.requests[#proof.requests + 1] = { name = name, params = params, response = response, error = err, outcome = outcome }
    check(name, outcome == 'matched', outcome == 'matched' and nil or vim.inspect(response), 'semantic', outcome)
    save()
    return response and response.result
  end
  local function settings(enabled)
    local value = { enabled = enabled, configuration = linked and { mixinPackage = 'fixture' } or { target = 'fixture.Target' } }
    if not linked then value.command = { assert(vim.env.KTLSP_IPC_CHILD), '--barrier', barrier } end
    client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { [provider] = value } } })
  end
  local function exact(name)
    local _, target_range = probe.find(lines('target'), 'public int twice', 'twice')
    request(name, 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/definition', nil,
      function(result) return probe.exact_definition(result, paths.target, { target_range }) end)
  end
  settings(true)
  if linked then
    local changed = vim.split(table.concat(lines('target'), '\n'):gsub('value %* 2', 'value * 3'), '\n', { plain = true })
    vim.api.nvim_buf_set_lines(buffers.target, 0, -1, false, changed); deliver()
    request('linked provider sees the same unsaved target input', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result) for _, item in ipairs(result and (result.items or result) or {}) do if item.label:match('^twice') then return true end end return false end)
  end
  if not linked then
    local paused = vim.wait(15000, function() return vim.fn.filereadable(barrier .. '/paused') == 1 end, 10)
    check('real extension child reached pause barrier', paused, nil, 'operational')
    assert(paused, 'child did not reach barrier')
    proof.paused_evaluation = table.concat(vim.fn.readfile(barrier .. '/paused'), '')
    record('child-paused')
    request('ordinary useful completion while child remains paused', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result)
        local items = result and (result.items or result) or {}
        for _, item in ipairs(items) do if item.label:match('^twice') then return true end end
        return false
      end)
    check('normal completion returned before release exists', vim.fn.filereadable(barrier .. '/release') == 0)
    check('paused evaluation is not cancelled before the target edit', vim.fn.filereadable(barrier .. '/cancelled') == 0)
    record('normal-completion-returned-before-release')
    local changed = vim.split(table.concat(lines('target'), '\n'):gsub('value %* 2', 'value * 3'), '\n', { plain = true })
    vim.api.nvim_buf_set_lines(buffers.target, 0, -1, false, changed); deliver()
    -- This request orders the delivered target edit before releasing the old child response.
    request('edited target is accepted while child remains paused', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result) for _, item in ipairs(result and (result.items or result) or {}) do if item.label:match('^twice') then return true end end return false end)
    record('unsaved-target-edit-accepted')
    local cancelled = vim.wait(15000, function() return vim.fn.filereadable(barrier .. '/cancelled') == 1 end, 10)
    check('paused child received cancellation before release', cancelled)
    assert(cancelled, 'silent stale child was not cancelled')
    record('child-cancellation-received-before-release')
    vim.fn.writefile({ 'release' }, barrier .. '/release'); record('old-child-response-released')
    local rejected = vim.wait(15000, function()
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local ok, event = pcall(vim.json.decode, line)
        if ok and event.name == 'extension.ipc.rejected' and tostring(event.args.evaluation) == proof.paused_evaluation and event.args.reason == 'Stale' then proof.stale_event = event; return true end
      end
      return false
    end, 10)
    check('old child evaluation is rejected as stale', rejected); record('stale-result-rejected')
  end
  local ready = linked or vim.wait(15000, function() return fresh_diagnostic end, 25)
  check('fresh child result reaches shared semantic binding', ready, nil, 'operational'); assert(ready, 'fresh binding did not arrive')
  record('fresh-binding-visible')
  request('fresh extension completion has shared descriptor and exact editable range', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/completion', { context = { triggerKind = 1 } },
    function(result)
      for _, item in ipairs(result and (result.items or result) or {}) do
        if item.label == 'shadow$twice' and item.detail == 'fixture.Target (I)I' then
          local _, range = probe.find(lines('mixin'), 'public abstract int shadow$twice', 'twice')
          return item.textEdit and item.textEdit.newText == 'twice' and vim.deep_equal(item.textEdit.range, range)
        end
      end
      return false
    end)
  exact('fresh shared goto matches linked target')
  request('shared references include shadow and ordinary call', 'target', 'public int twice', 'twice', 'textDocument/references', { context = { includeDeclaration = true } },
    function(result)
      local found = {}
      for _, location in ipairs(probe.locations(result)) do found[probe.path_key(vim.uri_to_fname(location.uri or location.targetUri))] = true end
      return found[probe.path_key(paths.mixin)] and found[probe.path_key(paths.use)]
    end)
  if not linked then
    request('fresh child diagnostic uses normal LSP path', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/diagnostic', nil,
      function(result) for _, diagnostic in ipairs(result and result.items or {}) do if diagnostic.message == 'ipc-fixture:bound' then return true end end return false end)
    vim.fn.writefile({ 'crash' }, barrier .. '/crash')
    local exited = vim.wait(15000, function()
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local ok, event = pcall(vim.json.decode, line)
        if ok and event.name == 'extension.ipc.exited' and event.args.provider == provider then proof.exit_event = event; return true end
      end
      return false
    end, 10)
    check('idle child crash retracts its published contribution', exited)
    assert(exited, 'idle child crash did not reach the host')
    request('crashed child target binding is withdrawn', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/definition', nil,
      function(result)
        for _, location in ipairs(probe.locations(result)) do
          if probe.path_key(vim.uri_to_fname(location.uri or location.targetUri)) == probe.path_key(paths.target) then return false end
        end
        return true
      end)
    request('normal completion continues after child crash', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result) for _, item in ipairs(result and (result.items or result) or {}) do if item.label:match('^twice') then return true end end return false end)
    request('crashed child diagnostic is withdrawn', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/diagnostic', nil,
      function(result) for _, diagnostic in ipairs(result and result.items or {}) do if diagnostic.message == 'ipc-fixture:bound' then return false end end return true end)
    record('crash-withdrawal-and-normal-completion')
    vim.fn.delete(barrier .. '/crash'); fresh_diagnostic = false; settings(true)
    local restarted = vim.wait(15000, function() return fresh_diagnostic end, 25)
    check('explicit restart publishes a fresh child contribution', restarted)
    assert(restarted, 'child did not recover after explicit restart')
    exact('restarted child restores shared binding')
    record('explicit-restart-restored-binding')
  end
  local before = {}; for key in pairs(buffers) do before[key] = table.concat(lines(key), '\n') end
  local edits = request('shared rename returns applicable edits', 'target', 'public int twice', 'twice', 'textDocument/rename', { newName = 'doubled' }, function(result) return result and (result.changes or result.documentChanges) end)
  assert(edits, 'rename edit missing'); vim.lsp.util.apply_workspace_edit(edits, client.offset_encoding)
  for key in pairs(buffers) do
    local expected = before[key]:gsub('twice', 'doubled')
    local actual = table.concat(lines(key), '\n')
    proof.snapshots[key] = { before = before[key], expected = expected, actual = actual }
    check('applied rename exact whole buffer ' .. key, actual == expected)
  end
  record('rename-applied-and-inspected')
  settings(false); deliver()
  request('disabled extension withdraws the target relation', 'mixin', 'public abstract int shadow$doubled', 'shadow$doubled', 'textDocument/definition', nil,
    function(result)
      for _, location in ipairs(probe.locations(result)) do if probe.path_key(vim.uri_to_fname(location.uri or location.targetUri)) == probe.path_key(paths.target) then return false end end
      return true
    end)
  for key, original in pairs(originals) do
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, original)
    check('restore exact original buffer ' .. key, vim.deep_equal(lines(key), original))
    vim.bo[buffers[key]].modified = false
  end
  deliver(); client.handlers['textDocument/publishDiagnostics'] = previous_diagnostics; save()
end
