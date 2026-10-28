return function(env)
  if vim.env.KTLSP_IPC_MODE == 'provider-commands' then return dofile(env.repo .. '/dev/external-extension/command-probes.lua')(env) end
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local linked = vim.env.KTLSP_IPC_MODE == 'linked'
  local host_inputs = vim.env.KTLSP_IPC_MODE == 'host-inputs'
  local source_facts = vim.env.KTLSP_IPC_MODE == 'source-snapshots' or host_inputs
  local facts = vim.env.KTLSP_IPC_MODE == 'project-facts' or source_facts
  local barrier = assert(vim.env.KTLSP_IPC_BARRIER, 'missing child barrier directory')
  local paths, buffers, originals = {}, {}, {}
  local proof = { mode = source_facts and 'source-snapshots' or facts and 'project-facts' or (linked and 'linked' or 'ipc'), ordering = {}, requests = {}, snapshots = {} }
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
  local inputs = host_inputs and dofile(env.repo .. '/dev/external-extension/host-input-probes.lua')(env, proof, save) or nil
  local function record(event) proof.ordering[#proof.ordering + 1] = event; save() end
  local function model_state()
    local requested, accepted
    local file = io.open(vim.env.KTLSP_TRACE, 'r')
    if file then
      for line in file:lines() do
        if line:find('workspace.refresh_requested', 1, true) or line:find('workspace.accepted_finish', 1, true) then
          local ok, event = pcall(vim.json.decode, line)
          if ok and event.args and probe.path_key(event.args.root or '') == probe.path_key(env.project) then
            if event.name == 'workspace.refresh_requested' then requested = event.args.generation end
            if event.name == 'workspace.accepted_finish' then accepted = event.args end
          end
        end
      end
      file:close()
    end
    return requested, accepted
  end
  local function source_snapshots(label, evaluation)
    local file = barrier .. '/sources-' .. evaluation .. '.json'
    assert(vim.fn.filereadable(file) == 1, 'missing child source snapshot capture')
    local observed = vim.json.decode(table.concat(vim.fn.readfile(file), '\n'))
    local sources, snapshots = observed.inventory.sources, observed.snapshots
    check(label .. ' source inventory is explicitly complete', observed.inventory.coverage.state == 'found')
    local owner, target, empty, seen = false, false, false, {}
    for i, source in ipairs(sources) do
      local snapshot = snapshots[i]
      local key = probe.path_key(source.displayPath)
      assert(not seen[key], 'duplicate source identity'); seen[key] = true
      check(label .. ' source snapshot identity ' .. i, snapshot.state == 'found'
        and snapshot.value.id == source.id and vim.deep_equal(snapshot.value.identity, source.identity))
      if snapshot.state == 'found' then
        check(label .. ' source snapshot hash ' .. i, vim.fn.sha256(snapshot.value.text) == snapshot.value.sha256)
        check(label .. ' source revision is opaque host generation ' .. i,
          type(source.identity.revision) == 'string' and source.identity.revision:match('^%d+$') ~= nil)
        local expected
        for k, path in pairs(paths) do
          if key == probe.path_key(path) then
            expected = probe.buffer_full_text(buffers[k])
            if k == 'mixin' then owner = true end
            if k == 'target' then target = true end
          end
        end
        if expected then check(label .. ' snapshot is current wire ' .. i, snapshot.value.text == expected) end
        if source.displayPath:match('/fixture/EmptySource%.java$') then
          empty = snapshot.value.text == '' and snapshot.value.sha256 == vim.fn.sha256('')
        end
      end
    end
    check(label .. ' inventory contains owner and related target', owner and target)
    check(label .. ' declaration-free empty source body is preserved', empty)
    proof.source_snapshots = proof.source_snapshots or {}
    proof.source_snapshots[label] = observed; save()
  end
  local function project_facts(label, evaluation)
    local file = barrier .. '/facts-' .. evaluation .. '.json'
    assert(vim.fn.filereadable(file) == 1, 'missing actual child facts for ' .. evaluation)
    local observed = vim.json.decode(table.concat(vim.fn.readfile(file), '\n'))
    assert(probe.path_key(observed.owner) == probe.path_key(paths.mixin), 'facts owner differs')
    local module, source, model_path
    for _, path in ipairs(vim.fn.glob(vim.env.KTLSP_CACHE_DIR .. '/classpath/*.json', false, true)) do
      local model = vim.json.decode(table.concat(vim.fn.readfile(path), '\n'))
      for _, candidate in ipairs(model.modules or {}) do
        if candidate.module == ':app' and probe.path_key(candidate.project_dir) == probe.path_key(env.project .. '/modules/editing') then
          for _, selected in ipairs(candidate.source_sets or {}) do
            if selected.name == 'main' then module, source, model_path = candidate, selected, path end
          end
        end
      end
    end
    assert(source and model_path, 'actual accepted Gradle main source set is missing')
    assert(not source.classpath_incomplete and #source.unresolved == 0 and #module.unresolved == 0,
      'project facts fixture requires a complete actual classpath')
    local snapshot = env.artifacts .. '/project-facts-' .. label .. '-model.json'
    assert(vim.uv.fs_copyfile(model_path, snapshot))
    local context, jvm = observed.facts[1], observed.facts[2]
    local function found(value) return value and value.state == 'found' end
    local function same_paths(left, right)
      if type(left) ~= 'table' or #left ~= #right then return false end
      for i, path in ipairs(right) do
        if type(left[i]) ~= 'string' or probe.path_key(left[i]) ~= probe.path_key(path) then return false end
      end
      return true
    end
    check(label .. ' projectContext uses the actual Gradle scope', found(context.scope)
      and context.scope.value.module == module.module and context.scope.value.sourceSet == source.name)
    check(label .. ' projectContext source roots match accepted metadata', found(context.sourceRoots)
      and same_paths(context.sourceRoots.value, source.source_roots))
    check(label .. ' projectContext output roots match accepted metadata', found(context.outputRoots)
      and same_paths(context.outputRoots.value, source.output_roots))
    local entries, origins = {}, true
    for _, entry in ipairs(found(context.classpath) and context.classpath.value.entries or {}) do
      entries[#entries + 1] = entry.displayPath
      origins = origins and type(entry.id) == 'string' and entry.id ~= ''
    end
    check(label .. ' projectContext preserves the declared classpath order', found(context.classpath)
      and #entries > 0 and origins and same_paths(entries, source.entries)
      and found(context.classpath.value.coverage))
    local java
    for _, task in ipairs(source.compiler_tasks) do if task.language == 'java' then assert(not java); java = task end end
    assert(java and java.complete and type(java.java_home) == 'string', 'selected Java SDK metadata is missing')
    local identity_prefix = 'jdk-api:' .. java.java_home:gsub('\\', '/') .. '#release=full#'
    check(label .. ' projectContext uses the selected full SDK', found(context.sdk)
      and context.sdk.value.api.kind == 'full' and context.sdk.value.identity:sub(1, #identity_prefix) == identity_prefix
      and context.sdk.value.identity:sub(#identity_prefix + 1):match('^' .. string.rep('%x', 64) .. '$') ~= nil)
    check(label .. ' workspace URI remains explicitly unknown', context.workspaceUri.state == 'unknown')
    check(label .. ' typeJvm has a checked source binary name', found(jvm.binaryName)
      and jvm.binaryName.value.internalName == 'fixture/Target' and jvm.binaryName.value.descriptor == 'Lfixture/Target;')
    local method = false
    for _, member in ipairs(jvm.declaredMembers.members or {}) do
      if member.name == 'twice' and member.kind == 'method' and found(member.signature)
        and member.signature.value.descriptor == '(I)I' and found(member.dispatch) and member.dispatch.value == 'instance'
        and found(member.visibility) and member.visibility.value == 'Public' and found(member.synthetic) and member.synthetic.value == false then method = true end
    end
    check(label .. ' typeJvm retains the typed editing member', method)
    check(label .. ' source-only raw ABI remains unknown', jvm.abi.state == 'unknown'
      and jvm.jvmAbiCoverage.state == 'unknown' and jvm.classFlags.state == 'unknown')
    proof.project_facts = proof.project_facts or {}
    proof.project_facts[label] = { evaluation = evaluation, observed = observed, model = snapshot,
      selected = { module = module.module, source_set = source.name, entries = source.entries } }
    save()
    return source.entries
  end
  for key, file in pairs({ target = 'fixture/Target.java', mixin = 'fixture/CaseMixin.java', use = 'fixture/IpcUse.java' }) do
    local path = vim.fs.abspath(env.project .. (facts and '/modules/editing/src/main/java/' or '/') .. file)
    local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = 'java'
    paths[key], buffers[key], originals[key] = path, buf, vim.fn.readfile(path)
    env.sources[path] = env.sources[path] or { buf = buf, disk = originals[key], sha256 = vim.fn.sha256(table.concat(originals[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local empty_path, empty_buffer
  if source_facts then
    empty_path = env.project .. '/modules/editing/src/main/java/fixture/EmptySource.java'
    assert(vim.fn.filereadable(empty_path) == 0, 'empty source sidecar must be disposable')
    vim.fn.writefile({}, empty_path)
    empty_buffer = vim.fn.bufadd(empty_path); vim.fn.bufload(empty_buffer)
    vim.bo[empty_buffer].filetype = 'java'; vim.bo[empty_buffer].eol = false
    vim.lsp.buf_attach_client(empty_buffer, client.id)
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
    if not linked then
      value.command = { assert(vim.env.KTLSP_IPC_CHILD), '--barrier', barrier }
      if facts then value.command[#value.command + 1] = '--project-facts' end
      if source_facts then value.command[#value.command + 1] = '--source-snapshots' end
      if host_inputs then value.command[#value.command + 1] = '--host-inputs'; value.configuration.runtimeClasses={'java/util/List','vendor/Versioned','vendor/Added','app/Main'} end
    end
    client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { [provider] = value } } })
  end
  local function exact(name)
    local _, target_range = probe.find(lines('target'), 'public int twice', 'twice')
    request(name, 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/definition', nil,
      function(result) return probe.exact_definition(result, paths.target, { target_range }) end)
  end
  if inputs then inputs.setup() end
  settings(true)
  if linked then
    local changed = vim.split(table.concat(lines('target'), '\n'):gsub('value %* 2', 'value * 3'), '\n', { plain = true })
    vim.api.nvim_buf_set_lines(buffers.target, 0, -1, false, changed); deliver()
    request('linked provider sees the same unsaved target input', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result) for _, item in ipairs(result and (result.items or result) or {}) do if item.label:match('^twice') then return true end end return false end)
  end
  if not linked then
    local paused = vim.wait(host_inputs and 120000 or 15000, function() return vim.fn.filereadable(barrier .. '/paused') == 1 end, 10)
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
    if facts then
      project_facts('initial', proof.paused_evaluation)
      if source_facts then source_snapshots('initial', proof.paused_evaluation) end
      if inputs then inputs.capture('initial', proof.paused_evaluation); inputs.change_wire() end
      proof.initial_model_generation = model_state()
      assert(proof.initial_model_generation, 'initial accepted model generation is missing')
      if source_facts then
        local changed = vim.api.nvim_buf_get_lines(buffers.target, 0, -1, false)
        changed[#changed + 1] = '// 未保存 source snapshot 😀'
        vim.api.nvim_buf_set_lines(buffers.target, 0, -1, false, changed); deliver()
      end
      local build = env.project .. '/modules/editing/build.gradle'
      assert(vim.fn.filereadable(env.project .. '/home/.m2/repository/fixture/reload/added/1.0/added-1.0.jar') == 1,
        'the classpath-change fixture JAR must already be built')
      -- The owned controller restores this file after shutdown, avoiding another import.
      assert(vim.uv.fs_copyfile(build, barrier .. '/build.gradle.before'))
      local content = vim.fn.readfile(build)
      content[#content + 1] = "dependencies { implementation files(rootProject.file('home/.m2/repository/fixture/reload/added/1.0/added-1.0.jar')) }"
      vim.fn.writefile(content, build)
      client:notify('workspace/didChangeWatchedFiles', { changes = { { uri = vim.uri_from_fname(build), type = 2 } } })
      record('real-classpath-configuration-change-delivered')
    else
    local changed = vim.split(table.concat(lines('target'), '\n'):gsub('value %* 2', 'value * 3'), '\n', { plain = true })
    vim.api.nvim_buf_set_lines(buffers.target, 0, -1, false, changed); deliver()
    -- This request orders the delivered target edit before releasing the old child response.
    end
    request(facts and 'normal completion remains useful during the model change' or 'edited target is accepted while child remains paused', 'use', 'return target.twice(2)', 'tw', 'textDocument/completion', nil,
      function(result) for _, item in ipairs(result and (result.items or result) or {}) do if item.label:match('^twice') then return true end end return false end)
    record(facts and 'model-change-accepted-before-release' or 'unsaved-target-edit-accepted')
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
    if facts then
      local requested, accepted
      local ready_model = vim.wait(tonumber(vim.env.KTLSP_CORPUS_READY_TIMEOUT_MS) or 600000, function()
        requested, accepted = model_state()
        return requested and requested > proof.initial_model_generation and accepted
          and accepted.generation == requested and accepted.failed == 0
          and accepted.missingSources == 0 and accepted.classpathComplete == true
      end, 250)
      proof.model_refresh = { requested = requested, accepted = accepted, ready = ready_model }
      check('dependency change reaches a new complete accepted model', ready_model, nil, 'operational')
      assert(ready_model, 'dependency change did not reach an accepted model')
      record('changed-model-accepted-before-contribution-observation')
    end
  end
  local ready = linked or vim.wait(15000, function() return fresh_diagnostic end, 25)
  check('fresh child result reaches shared semantic binding', ready, nil, 'operational'); assert(ready, 'fresh binding did not arrive')
  record('fresh-binding-visible')
  if facts then
    local accepted
    local ready_facts = vim.wait(15000, function()
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local ok, event = pcall(vim.json.decode, line)
        if ok and event.name == 'extension.ipc.accepted' and event.args.provider == provider
          and probe.path_key(event.args.owner) == probe.path_key(paths.mixin)
          and tostring(event.args.evaluation) ~= proof.paused_evaluation then accepted = event; return true end
      end
      return false
    end, 25)
    assert(ready_facts, 'fresh facts did not produce an accepted contribution')
    check('fresh contribution matches the current owner bytes', accepted.args.source_sha256 == probe.buffer_sha256(buffers.mixin))
    proof.facts_publication = accepted
    local entries = project_facts('fresh', tostring(accepted.args.evaluation))
    if source_facts then source_snapshots('fresh', tostring(accepted.args.evaluation)) end
    if inputs then inputs.capture('fresh',tostring(accepted.args.evaluation)); inputs.native_lifecycle() end
    local initial = proof.project_facts.initial.selected.entries
    local added = env.project .. '/home/.m2/repository/fixture/reload/added/1.0/added-1.0.jar'
    local found = false
    for _, entry in ipairs(entries) do if probe.path_key(entry) == probe.path_key(added) then found = true end end
    check('fresh projectContext observes the real added classpath entry', found and #entries > #initial)
    record('two-project-fact-operations-and-fresh-contribution-verified')
  end
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
  if inputs then inputs.raw_commands() end
  for key, original in pairs(originals) do
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, original)
    check('restore exact original buffer ' .. key, vim.deep_equal(lines(key), original))
    vim.bo[buffers[key]].modified = false
  end
  if empty_buffer then
    vim.api.nvim_buf_delete(empty_buffer, { force = true })
    assert(vim.fn.delete(empty_path) == 0, 'remove disposable empty source')
    check('empty source sidecar is removed after disable', vim.fn.filereadable(empty_path) == 0)
  end
  if inputs then inputs.cleanup() end
  deliver(); client.handlers['textDocument/publishDiagnostics'] = previous_diagnostics; save()
end
