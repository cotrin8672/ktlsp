return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local provider, barrier = 'fixture.ipc-shadow', assert(vim.env.KTLSP_IPC_BARRIER)
  local project_inputs = vim.env.KTLSP_IPC_PROJECT_INPUTS == '1' or vim.env.KTLSP_IPC_MODE == 'project-inputs'
  local binary_annotations = vim.env.KTLSP_IPC_MODE == 'binary-annotations'
  local proof = { mode = project_inputs and 'project-inputs' or binary_annotations and 'binary-annotations' or 'provider-commands', requests = {}, ordering = {} }
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
    local path = vim.fs.abspath(env.project .. ((project_inputs or binary_annotations) and '/modules/editing/src/main/java/' or '/') .. 'fixture/' .. file)
    local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = 'java'
    paths[key], buffers[key], original[key] = path, buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    env.sources[path] = env.sources[path] or { buf = buf, disk = vim.fn.readfile(path), sha256 = vim.fn.sha256(table.concat(vim.fn.readfile(path), '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function flush() require('vim.lsp._changetracking').flush(client) end
  local function file_uri_key(uri)
    if type(uri) ~= 'string' then return nil end
    local path = vim.uv.fs_realpath(vim.uri_to_fname(uri))
    return path and probe.path_key(path) or nil
  end
  local function settings(enabled, config)
    local command = { assert(vim.env.KTLSP_IPC_CHILD), '--barrier', barrier, '--provider-commands' }
    if project_inputs then table.insert(command, '--project-inputs') end
    if binary_annotations then
      vim.list_extend(command, { '--project-facts', '--jvm-catalog' })
      if config and config.selectBinaryAnnotations then table.insert(command, '--binary-annotations') end
      config = vim.tbl_extend('force', config or {}, { binaryAnnotationsProbe = true })
    end
    client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { [provider] = {
      command = command,
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
    local response, err = client:request_sync('workspace/executeCommand', { command = 'ktlsp.providerCommand', arguments = { params } }, (params.refreshProject or project_inputs or binary_annotations) and 120000 or 10000, buffers.owner)
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
  local function await_current_provider(label, previous_instance)
    local owner = assert(file_uri_key(vim.uri_from_bufnr(buffers.owner)))
    local digest = vim.fn.sha256(probe.buffer_full_text(buffers.owner))
    local accepted
    local ready = vim.wait(120000, function()
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local decoded, event = pcall(vim.json.decode, line)
        if decoded and event.name == 'extension.ipc.accepted' and event.args and event.args.provider == provider
          and event.args.source_sha256 == digest and type(event.args.owner) == 'string'
          and event.args.instance ~= previous_instance then
          local path = vim.uv.fs_realpath(event.args.owner)
          if path and probe.path_key(path) == owner then accepted = event.args end
        end
      end
      return accepted ~= nil
    end, 25)
    check(label .. ' provider accepts the current owner before the first command', ready, nil, 'operational')
    assert(ready, 'provider source preparation did not reach current accepted publication')
    proof.readiness = proof.readiness or {}; proof.readiness[label] = accepted; save()
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
    if not project_inputs and not binary_annotations then settings(true) end
    vim.api.nvim_buf_set_lines(buffers.owner, 0, 0, false, { '// unsaved command 😀' }); flush()
    if project_inputs or binary_annotations then
      settings(true)
      await_current_provider('initial')
    end
    local params = envelope('fixture.echo', { { CallbackName = 'KeepCase', Unicode = '未保存😀' }, '{"LegacyCase":"MiXeD"}' })
    local first = call('actual enabled provider returns the complete opaque envelope', params, function(value) return echo(value, params) and value.result.facts[1].state == 'found' end)
    assert(first and first.result, 'first routed command unavailable')
    if binary_annotations then
      local current = assert(first.result.binaryAnnotations, 'missing unselected annotation fixture')
      assert(current.class.state == 'found', 'compile-only fixture ABI unavailable')
      local function no_annotations(abi)
        if abi.annotations ~= nil then return false end
        for _, values in ipairs({ abi.fields, abi.methods }) do
          for _, member in ipairs(values) do if member.annotations ~= nil then return false end end
        end
        return true
      end
      check('current consumer retains the unselected ABI shape', no_annotations(current.class.value.abi))
      settings(false); settings(true, { selectBinaryAnnotations = true })
      await_current_provider('binary-selected', proof.readiness.initial.instance)
      params = envelope('fixture.echo')
      local selected = call('selected binary annotation consumer receives physical ABI', params, function(value)
        return echo(value, params) and value.result.binaryAnnotations.class.state == 'found'
      end)
      local captured = assert(selected and selected.result.binaryAnnotations, 'missing selected annotation fixture')
      local value, descriptor = captured.class.value, captured.descriptor
      local abi = value.abi
      check('binary representation selection preserves physical input provenance', descriptor.association == 'compileClasspath'
        and value.entry == descriptor.entry and value.authority == 'compiled-input' and value.sourceCurrent.state == 'unknown'
        and abi.identity.internalName == 'annotationfixture/CustomPoint'
        and abi.identity.originId == current.class.value.abi.identity.originId
        and abi.identity.contentSha256 == current.class.value.abi.identity.contentSha256)
      check('class annotation preserves explicit string elements and visibility', vim.deep_equal(abi.annotations, {
        { runtimeVisible = true, descriptor = 'Lannotationfixture/AtCode;', elements = {
          { name = 'value', value = { kind = 'string', value = 'CUSTOM' } },
          { name = 'namespace', value = { kind = 'string', value = 'example' } },
        } },
        { runtimeVisible = false, descriptor = 'Lannotationfixture/Invisible;', elements = {
          { name = 'value', value = { kind = 'string', value = 'classInvisible' } },
        } },
      }))
      local field, method
      for _, member in ipairs(abi.fields) do if member.name == 'field' then field = member end end
      for _, member in ipairs(abi.methods) do if member.name == 'probe' then method = member end end
      assert(field and method, 'annotation fixture field/method missing')
      check('binary field retains invisible declaration annotation', vim.deep_equal(field.annotations, {
        { runtimeVisible = false, descriptor = 'Lannotationfixture/Invisible;', elements = {
          { name = 'value', value = { kind = 'string', value = 'fieldInvisible' } },
        } },
      }))
      local payload = method.annotations[1]
      check('binary method retains annotation descriptor and order', payload.runtimeVisible == true
        and payload.descriptor == 'Lannotationfixture/Payload;' and #payload.elements == 14 and #method.annotations == 2
        and vim.deep_equal(method.annotations[2], { runtimeVisible = false, descriptor = 'Lannotationfixture/Invisible;', elements = {
          { name = 'value', value = { kind = 'string', value = 'methodInvisible' } },
        } }))
      local expected = {
        { 'b', { kind = 'byte', value = -7 } }, { 'c', { kind = 'char', value = 55296 } },
        { 'd', { kind = 'double', bits = '8000000000000000' } }, { 'f', { kind = 'float', bits = '7f800000' } },
        { 'i', { kind = 'int', value = -17 } }, { 'j', { kind = 'long', value = '9223372036854775807' } },
        { 's', { kind = 'short', value = -123 } }, { 'z', { kind = 'boolean', value = true } },
        { 'text', { kind = 'string', value = 'nul\0' .. vim.fn.nr2char(0x1f603) } },
        { 'utf16', { kind = 'string', utf16 = { 55296, 120, 56320, 55357, 56835, 0 } } },
        { 'choice', { kind = 'enum', descriptor = 'Lannotationfixture/Choice;', name = 'SECOND' } },
        { 'type', { kind = 'class', descriptor = '[[I' } },
        { 'nested', { kind = 'annotation', value = { descriptor = 'Lannotationfixture/Nested;', elements = {
          { name = 'value', value = { kind = 'string', value = 'nestedValue' } },
        } } } },
        { 'array', { kind = 'array', values = { { kind = 'int', value = 1 }, { kind = 'int', value = -2 }, { kind = 'int', value = 3 } } } },
      }
      for i, element in ipairs(expected) do
        check('typed binary explicit element ' .. element[1], vim.deep_equal(payload.elements[i], { name = element[1], value = element[2] }))
      end
      for label, capture in pairs({ unselected = current, selected = captured }) do
        check(label .. ' compile-only metadata grants no executable class identity', capture.runtime.state == 'unknown'
          and vim.iter(capture.runtime.reasons or {}):any(function(reason) return reason.code == 'BYTECODE_UNAVAILABLE' end))
      end
      proof.binaryAnnotations = { unselected = current, selected = captured }; save()
      return
    end
    if project_inputs then
      local captured = assert(first.result.projectInputs, 'missing project input read')
      local project, scopes, owner_seen = nil, {}, false
      for _, record in ipairs(captured.records) do
        if record.kind == 'project' then project = record end
        if record.kind == 'sourceSet' then scopes[record.id] = record.name end
      end
      assert(project, 'missing owner-selected project record')
      check('project input capture and index coverage stay distinct', project.capture.state == 'found'
        and project.index.state == 'found' and type(project.index.value.analysisGeneration) == 'number')
      check('all captured Java source sets are enumerated', captured.coverage.state == 'found')
      local main, test = false, false
      local useful_scopes = {}
      local expected_members = {
        { scope = 'main', file = '/fixture/Target.java', class = 'Target', qualifiedName = 'fixture.Target', method = 'twice' },
        { scope = 'test', file = '/fixture/ProjectInputsTest.java', class = 'ProjectInputsTest', qualifiedName = 'fixture.ProjectInputsTest', method = 'testOnly' },
        { scope = 'test', file = '/fixture/ProjectInputsKotlinTest.kt', class = 'ProjectInputsKotlinTest', qualifiedName = 'fixture.ProjectInputsKotlinTest', method = 'kotlinTestOnly' },
      }
      for _, scope in ipairs(captured.scopes) do
        assert(scopes[scope.scope] == scope.name, 'foreign source set inventory')
        main, test = main or scope.name == 'main', test or scope.name == 'test'
        local known_types, known_members = 0, 0
        check('complete source inventory for ' .. scope.name, scope.inventory.coverage.state == 'found')
        for i, source in ipairs(scope.inventory.sources) do
          local snapshot = scope.snapshots[i]
          check('scoped source snapshot ' .. scope.name .. ':' .. i, snapshot.state == 'found'
            and snapshot.value.id == source.id and vim.fn.sha256(snapshot.value.text) == snapshot.value.sha256
            and vim.deep_equal(snapshot.value.identity, source.identity))
          local normalized = assert(scope.sourceFacts[i], 'missing normalized source facts')
          local source_path = source.documentUri and source.documentUri.state == 'found' and file_uri_key(source.documentUri.value)
          local expected
          for _, candidate in ipairs(expected_members) do
            if scope.name == candidate.scope and source_path and source_path:sub(-#candidate.file) == probe.path_key(candidate.file) then expected = candidate end
          end
          check('scoped normalized source identity ' .. scope.name .. ':' .. i,
            normalized.source.id == source.id and vim.deep_equal(normalized.source.identity, source.identity))
          check('own compiler scope for normalized source ' .. scope.name .. ':' .. i, normalized.semanticScope.state == 'found')
          for position, declaration in ipairs(normalized.declarations) do
            local range = declaration.nameRange
            check('foreign declaration name uses its own snapshot ' .. scope.name .. ':' .. declaration.name,
              snapshot.value.text:sub(range.start + 1, range['end']) == declaration.name)
            local typed = assert(normalized.declarationFacts[position], 'missing normalized declaration facts')
            check('typed snapshot declaration identity ' .. scope.name .. ':' .. declaration.name, typed.declaration == declaration.id)
            if declaration.sourceKind == 'Class' or declaration.sourceKind == 'Interface'
              or declaration.sourceKind == 'EnumClass' or declaration.sourceKind == 'Object' then
              check('normalized source class type is useful ' .. scope.name .. ':' .. declaration.name, typed.type.state == 'found')
            end
            if typed.type.state == 'found' then
              known_types = known_types + 1
              check('snapshot type retains its own source context ' .. scope.name .. ':' .. declaration.name,
                typed.type.value.readOnly == true and vim.deep_equal(typed.type.value.contextSource.identity, source.identity))
              if expected and declaration.name == expected.class then
                expected.typeFound = typed.type.value.qualifiedName == expected.qualifiedName
              end
            end
            if typed.member.state == 'found' then
              known_members = known_members + 1
              local member = typed.member.value
              check('snapshot member descriptor and context ' .. scope.name .. ':' .. declaration.name,
                member.readOnly == true and member.signature.state == 'found' and type(member.signature.value.descriptor) == 'string'
                  and #member.signature.value.descriptor > 0 and vim.deep_equal(member.contextSource.identity, source.identity))
              if expected and declaration.name == expected.method then
                expected.memberFound = member.signature.state == 'found' and member.signature.value.descriptor == '(I)I'
              end
            end
          end
          if #normalized.declarations > 0 then
            local location = normalized.sampleLocation
            local location_path = location and type(location.value) == 'table' and file_uri_key(location.value.documentUri)
            check('foreign snapshot declaration location ' .. scope.name .. ':' .. i, location and location.state == 'found'
              and location_path and source_path and location_path == source_path and location.value.source.sha256 == snapshot.value.sha256)
          end
          local owner_path = file_uri_key(params.context.documentUri)
          if source_path and owner_path and source_path == owner_path then
            owner_seen = owner_seen or snapshot.value.text == params.context.bufferText
          end
        end
        check('complete resource inventory for ' .. scope.name, scope.resources.inventory.coverage.state == 'found')
        useful_scopes[scope.name] = { types = known_types, members = known_members }
        if scope.name == 'main' or scope.name == 'test' then
          check('useful normalized types and member descriptors in ' .. scope.name, known_types > 0 and known_members > 0)
        end
      end
      check('project scopes include main and test', main and test)
      for _, expected in ipairs(expected_members) do
        check('scope-correct type and exact member descriptor ' .. expected.scope .. ':' .. expected.method,
          expected.typeFound == true and expected.memberFound == true)
      end
      check('project inventory preserves current unsaved command source', owner_seen)
      check('one client command obtains hydrated project inputs', #proof.requests == 1 and proof.requests[1].attempts == 1
        and proof.requests[1].outcome == 'matched' and first.result.commandCount == 1)
      proof.projectInputs, proof.usefulScopes = captured, useful_scopes
      proof.ordering[#proof.ordering + 1] = 'one-command-with-host-managed-inputs'; save()
      return
    end
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
  if not project_inputs and not binary_annotations then
    for _, case in ipairs({ 'source', 'cancel', 'disable' }) do vim.fn.writefile({ '1' }, barrier .. '/command-' .. case .. '-release') end
  end
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
