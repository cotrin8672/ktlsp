return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local provider, barrier = 'fixture.ipc-shadow', assert(vim.env.KTLSP_IPC_BARRIER)
  local proof = { mode = 'provider-commands', requests = {}, ordering = {} }
  -- Neovim rpc.lua intentionally suppresses -32800 callbacks. Observe the unchanged
  -- first wire response instead, before its native cancellation handler consumes it.
  local native_debug, cancellation_id, cancellation_wire = vim.lsp.log.debug, nil, nil
  vim.lsp.log.debug = function(label, message, ...)
    if label == 'rpc.receive' and cancellation_id ~= nil and type(message) == 'table'
      and message.id == cancellation_id and cancellation_wire == nil then
      cancellation_wire = vim.deepcopy(message)
    end
    return native_debug(label, message, ...)
  end
  local paths, buffers, original = {}, {}, {}
  local function save() vim.fn.writefile({ vim.json.encode(proof) }, env.artifacts .. '/extension-command-proof.json') end
  for key, file in pairs({ owner = 'CaseMixin.java', target = 'Target.java', use = 'IpcUse.java' }) do
    local path = vim.fs.abspath(env.project .. '/fixture/' .. file)
    local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = 'java'
    paths[key], buffers[key], original[key] = path, buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    env.sources[path] = env.sources[path] or { buf = buf, disk = vim.fn.readfile(path), sha256 = vim.fn.sha256(table.concat(vim.fn.readfile(path), '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function flush() require('vim.lsp._changetracking').flush(client) end
  local function settings(enabled, config)
    client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { [provider] = {
      command = { assert(vim.env.KTLSP_IPC_CHILD), '--barrier', barrier, '--provider-commands' },
      executeCommands = { 'fixture.echo', 'fixture.pause', 'fixture.refresh', 'fixture.notAcknowledged' },
      enabled = enabled, configuration = config or {},
    } } } })
  end
  local function envelope(command, args)
    flush()
    return { provider = provider, command = command, arguments = args or {}, context = {
      workspaceRoot = env.project, documentUri = vim.uri_from_bufnr(buffers.owner),
      bufferText = probe.buffer_full_text(buffers.owner), documentVersion = vim.lsp.util.buf_versions[buffers.owner],
    } }
  end
  local function observe(name, params, response, err, accepts, expected_error)
    local problem = response and response.err or err
    local measured = response ~= nil and err == nil
    local passed = measured and (expected_error and problem and problem.code == expected_error
      or not expected_error and not problem and accepts(response.result))
    local outcome = not measured and 'unmeasured' or passed and 'matched' or 'mismatch'
    env.request_outcomes[#env.request_outcomes + 1] = { case = name, method = 'workspace/executeCommand', attempts = 1, outcome = outcome }
    proof.requests[#proof.requests + 1] = { name = name, params = params, response = response, error = err, outcome = outcome, attempts = 1 }
    check(name, passed == true, passed and nil or vim.inspect(problem or response), 'semantic', outcome); save()
    return response and response.result
  end
  local function call(name, params, accepts, expected_error)
    local response, err = client:request_sync('workspace/executeCommand', { command = 'ktlsp.providerCommand', arguments = { params } }, params.refreshProject and 120000 or 10000, buffers.owner)
    return observe(name, params, response, err, accepts, expected_error)
  end
  local function echo(value, params)
    return value and value.protocolVersion == 1 and value.serverVersion == 'fixture-command-v1'
      and vim.deep_equal(value.capabilities, { params.command }) and vim.deep_equal(value.result.arguments, params.arguments)
      and value.result.source == params.context.bufferText
      and value.result.context.documentUri == params.context.documentUri
      and value.result.context.documentVersion == params.context.documentVersion
      and value.result.context.sourceSha256 == vim.fn.sha256(params.context.bufferText)
  end
  local function paused(case, mutate, expected_error)
    local params = envelope('fixture.pause', { { pauseCase = case, lookup = 'fixture.Future' } })
    local done, response, callbacks = false, nil, 0
    local sent, id = client:request('workspace/executeCommand', { command = 'ktlsp.providerCommand', arguments = { params } }, function(err, result)
      callbacks = callbacks + 1; response = { err = err, result = result }; done = true
    end, buffers.owner)
    assert(sent and id, 'command request was not sent')
    if case == 'cancel' then cancellation_id, cancellation_wire = id, nil end
    assert(vim.wait(15000, function() return vim.fn.filereadable(barrier .. '/command-' .. case .. '-paused') == 1 end, 10), 'command did not reach owned child barrier')
    local position = probe.find(vim.api.nvim_buf_get_lines(buffers.use, 0, -1, false), 'return target.twice(2)', 'tw')
    position.character = position.character + 2
    local ordinary = client:request_sync('textDocument/completion', { textDocument = { uri = vim.uri_from_bufnr(buffers.use) }, position = position }, 10000, buffers.use)
    check('ordinary completion proceeds while ' .. case .. ' command child is paused', ordinary and not ordinary.err and ordinary.result and vim.iter(ordinary.result.items or ordinary.result):any(function(item) return item.label:match('^twice') end))
    mutate(id)
    if case ~= 'disable' then
      assert(vim.wait(15000, function() return vim.fn.filereadable(barrier .. '/command-' .. case .. '-cancelled') == 1 end, 10), 'command cancellation was not delivered')
      vim.fn.writefile({ '1' }, barrier .. '/command-' .. case .. '-release')
    end
    assert(vim.wait(15000, function() return case == 'cancel' and cancellation_wire ~= nil or done end, 10), 'command did not complete cancellation')
    if case == 'cancel' then
      check('Neovim consumes the first exact cancellation acknowledgement without a callback', callbacks == 0
        and cancellation_wire.id == id and cancellation_wire.jsonrpc == '2.0'
        and cancellation_wire.error and cancellation_wire.error.code == -32800)
      proof.cancellationWire = cancellation_wire
      response = { err = cancellation_wire.error, result = cancellation_wire.result }
      cancellation_id = nil
    else
      check(case .. ' command has exactly one callback', callbacks == 1)
    end
    observe(case .. ' obsolete command is rejected', params, response, nil, function() return false end, expected_error)
  end
  local ok, failure = xpcall(function()
    local commands = client.server_capabilities.executeCommandProvider.commands
    check('generic provider command is advertised without fixture command IDs', vim.list_contains(commands, 'ktlsp.providerCommand') and not vim.list_contains(commands, 'fixture.echo'))
    settings(true)
    vim.api.nvim_buf_set_lines(buffers.owner, 0, 0, false, { '// unsaved command 😀' }); flush()
    local params = envelope('fixture.echo', { { CallbackName = 'KeepCase', Unicode = '未保存😀' }, '{"LegacyCase":"MiXeD"}' })
    local first = call('actual enabled provider returns the complete opaque envelope', params, function(value) return echo(value, params) and value.result.facts[1].state == 'found' end)
    assert(first and first.result, 'first routed command unavailable')
    local count = first.result.commandCount
    params = envelope('fixture.echo', { { requestEdit = true, CallbackName = 'ApplyExactEdit' } })
    local action = call('complete routed edit preserves host URI and document version', params, function(value)
      return echo(value, params) and value.result.commandCount == count + 1
        and value.result.edit.documentChanges[1].textDocument.uri == params.context.documentUri
        and value.result.edit.documentChanges[1].textDocument.version == params.context.documentVersion
    end)
    assert(action and action.result); count = action.result.commandCount
    local expected_lines = { '// provider action' }
    vim.list_extend(expected_lines, vim.api.nvim_buf_get_lines(buffers.owner, 0, -1, false))
    vim.lsp.util.apply_workspace_edit(action.result.edit, 'utf-16'); flush()
    check('routed edit is applied and inspected in the real buffer', vim.deep_equal(vim.api.nvim_buf_get_lines(buffers.owner, 0, -1, false), expected_lines))
    for _, command in ipairs({ 'fixture.unapproved', 'fixture.notAcknowledged' }) do
      local denied = envelope(command); denied.refreshProject = true
      call('configured and negotiated allowlists reject ' .. command, denied, function() return false end, -32002)
    end
    local invalid = envelope('fixture.echo'); invalid.context.workspaceRoot = env.project .. '/another-workspace'
    call('foreign workspace hint is rejected', invalid, function() return false end, -32602)
    invalid = envelope('fixture.echo'); invalid.context.documentVersion = invalid.context.documentVersion - 1
    call('old document version is rejected', invalid, function() return false end, -32801)
    params = envelope('fixture.echo', { { marker = 'after-denied' } })
    local after = call('denied commands never dispatch to child', params, function(value) return echo(value, params) and value.result.commandCount == count + 1 end)
    assert(after and after.result); count = after.result.commandCount
    paused('source', function()
      vim.api.nvim_buf_set_lines(buffers.target, -1, -1, false, { 'class Future {}' }); flush()
    end, -32801)
    params = envelope('fixture.echo', { { lookup = 'fixture.Future' } })
    after = call('fresh command sees added source after rejecting its negative read', params, function(value) return echo(value, params) and value.result.facts[1].state == 'found' and value.result.commandCount == count + 2 end)
    assert(after and after.result); count = after.result.commandCount
    paused('cancel', function(id) assert(client:cancel_request(id)) end, -32800)
    params = envelope('fixture.echo')
    after = call('cancelled command drains once and leaves provider usable', params, function(value) return echo(value, params) and value.result.commandCount == count + 2 end)
    assert(after and after.result)
    paused('disable', function() settings(false) end, -32801)
    call('disabled provider cannot execute a command', envelope('fixture.echo'), function() return false end, -32002)
    settings(true, { NewConfiguration = 'Preserved' })
    params = envelope('fixture.echo')
    after = call('restart uses a fresh child and current provider configuration', params, function(value) return echo(value, params) and value.result.commandCount == 1 and value.result.configuration.NewConfiguration == 'Preserved' end)
    assert(after and after.result); count = after.result.commandCount
    params = envelope('fixture.refresh', { { lookup = 'fixture.Future' } }); params.refreshProject = true
    call('host refresh completes before one fresh child command', params, function(value)
      return echo(value, params) and value.result.commandCount == count + 1
        and type(value.result.context.refreshGeneration) == 'string'
        and value.result.facts[1].state == 'found'
    end)
    proof.ordering[#proof.ordering + 1] = 'single-host-refresh-then-single-child-dispatch'; save()
  end, debug.traceback)
  for _, case in ipairs({ 'source', 'cancel', 'disable' }) do vim.fn.writefile({ '1' }, barrier .. '/command-' .. case .. '-release') end
  settings(false)
  for key, buf in pairs(buffers) do
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, original[key])
    check('restore exact original buffer ' .. key, vim.deep_equal(vim.api.nvim_buf_get_lines(buf, 0, -1, false), original[key]))
    vim.bo[buf].modified = false
  end
  flush()
  vim.lsp.log.debug = native_debug
  if not ok then error(failure) end
end
