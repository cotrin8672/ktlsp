return function(env)
  if vim.env.KTLSP_IPC_MODE == 'root-files' then
    local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
    local client, check = env.client, env.check
    local root = vim.fs.abspath(env.project)
    local barrier = assert(vim.env.KTLSP_IPC_BARRIER, 'missing child barrier directory')
    local provider = 'fixture.ipc-shadow'
    local proof = { mode = 'root-files', phases = {}, ordering = {} }
    local owner = root .. '/modules/editing/src/main/java/fixture/CaseMixin.java'
    local client_path = root .. '/modules/editing/src/main/java/fixture/CLIENT-CONFIG.JSON'
    local shared = root .. '/shared/root-config.json'
    local hidden = root .. '/build/.root-files/hidden-config.json'
    local gradle_hidden = root .. '/.gradle/gradle-hidden.json'
    local duplicate_a, duplicate_b = root .. '/shared/a/duplicate.json', root .. '/shared/b/duplicate.json'
    local removed, renamed = root .. '/shared/delete-config.json', root .. '/shared/renamed-config.json'
    local added, unsaved = root .. '/shared/added-config.json', root .. '/shared/unsaved-config.json'
    local large, excluded = root .. '/shared/large-config.json', root .. '/ExClUdEd-RoOt/skip-config.json'
    local extension_only = root .. '/shared/EXTENSION.JSON'
    local pager = root .. '/root-file-pages'
    local filter = { basenames = { 'added-config.json', 'client-config.json', 'delete-config.json',
      'duplicate.json', 'gradle-hidden.json', 'hidden-config.json', 'large-config.json', 'renamed-config.json',
      'root-config.json', 'unsaved-config.json' }, extensions = { 'json', 'rootfixture' },
      excludedDirectories = { '.git', '.kotlin', 'excluded-root' } }
    local function save() vim.fn.writefile({ vim.json.encode(proof) }, env.artifacts .. '/extension-ipc-proof.json') end
    local function must(label, value)
      check(label, value, nil, 'semantic', value and 'matched' or 'mismatch')
      assert(value, label)
    end
    local function read(path)
      local file = assert(io.open(path, 'rb')); local text = assert(file:read('*a')); file:close(); return text
    end
    local function write(path, text)
      local file = assert(io.open(path, 'wb')); assert(file:write(text)); file:close()
    end
    local function path_key(path) return probe.path_key(vim.fs.abspath(path)) end
    local function same_uri(uri, path)
      if type(uri) ~= 'string' then return false end
      local ok, decoded = pcall(vim.uri_to_fname, uri)
      return ok and type(decoded) == 'string' and path_key(decoded) == path_key(path)
    end
    local function event(name, evaluation)
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local ok, item = pcall(vim.json.decode, line)
        if ok and item.name == name and item.args and item.args.provider == provider
          and tostring(item.args.evaluation) == tostring(evaluation) then return item end
      end
    end
    local function capture(phase, except, require_accepted)
      local found, accepted
      local ready = vim.wait(120000, function()
        for _, file in ipairs(vim.fn.glob(barrier .. '/root-files-' .. phase .. '-*.json', false, true)) do
          local item = vim.json.decode(table.concat(vim.fn.readfile(file), '\n'))
          if type(item.owner) == 'string' and path_key(item.owner) == path_key(owner)
            and tostring(item.evaluation) ~= tostring(except) then
            local publication = event('extension.ipc.accepted', item.evaluation)
            if not require_accepted or publication then found, accepted = item, publication; return true end
          end
        end
        return false
      end, 25)
      check(phase .. ' capture reaches host barrier', ready, nil, 'operational')
      assert(ready, phase .. ' capture unavailable')
      return found, accepted
    end
    local function settings(phase, pause, token)
      local configuration = { target = 'fixture.Target', rootFilePhase = phase,
        rootFileFilter = filter, rootFilePause = pause }
      if token then configuration.rootFileProbeToken = token end
      client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = {
        [provider] = { enabled = true, configuration = configuration,
          command = { assert(vim.env.KTLSP_IPC_CHILD), '--barrier', barrier, '--root-files' } },
      } } })
    end
    local function disable()
      client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { [provider] = { enabled = false } } } })
    end
    local function attach(path, filetype)
      local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = filetype
      assert(vim.lsp.buf_attach_client(buf, client.id)); return buf
    end
    local owner_buf = attach(owner, 'java')
    local client_buf = attach(client_path, 'json')
    assert(vim.fn.filereadable(unsaved) == 0 and vim.fn.filereadable(added) == 0 and vim.fn.filereadable(renamed) == 0)
    local new_buf = attach(unsaved, 'json')
    local initial_shared, original_client, original_removed = read(shared), read(client_path), read(removed)
    local original_duplicate = read(duplicate_a)
    local originals = { [shared] = initial_shared, [client_path] = original_client, [removed] = original_removed,
      [duplicate_a] = original_duplicate }
    local created = { large, added, renamed }
    local notifications = {}
    local old_notify = client.rpc.notify
    client.rpc.notify = function(method, params)
      local sent = old_notify(method, params)
      if sent and method == 'textDocument/didChange' then
        notifications[params.textDocument.uri] = params.textDocument.version
      end
      return sent
    end
    local function edit(buf, text)
      vim.bo[buf].eol = true
      vim.api.nvim_buf_set_lines(buf, 0, -1, false, vim.split(text:gsub('\n$', ''), '\n', { plain = true }))
      require('vim.lsp._changetracking').flush(client)
      local uri, version = vim.uri_from_bufnr(buf), vim.lsp.util.buf_versions[buf]
      local actual = probe.buffer_full_text(buf)
      assert(notifications[uri] == version and actual:gsub('\r\n', '\n') == text, 'overlay was not delivered')
      return version, actual
    end
    local function pause_file(phase, suffix) return barrier .. '/root-files-' .. phase .. '-' .. suffix end
    local function wait_file(path, label)
      local ready = vim.wait(120000, function() return vim.fn.filereadable(path) == 1 end, 25)
      check(label, ready, nil, 'operational'); assert(ready, label)
    end
    local function inspect(label, capture_value, expected, versions, pages)
      local inventory = capture_value.inventory
      must(label .. ' exact query filter', vim.deep_equal(inventory.filter, filter)
        and vim.deep_equal(capture_value.filter, filter))
      must(label .. ' uses registered workspace root', inventory.root and inventory.root.rootKind == 'workspace'
        and same_uri(inventory.root.uri, root) and type(inventory.root.identity) == 'string'
        and type(inventory.root.revision) == 'string')
      must(label .. ' terminal inventory is complete', inventory.coverage.state == 'found'
        and inventory.coverage.value == true and #inventory.problems == 0 and inventory.pages >= pages)
      must(label .. ' absent basename is a complete empty domain', capture_value.absent.coverage.state == 'found'
        and capture_value.absent.coverage.value == true and #capture_value.absent.files == 0
        and #capture_value.absent.problems == 0 and same_uri(capture_value.absent.root.uri, root)
        and vim.deep_equal(capture_value.absent.filter, { basenames = { 'ktlsp-root-files-guaranteed-absent.fixture' },
          extensions = {}, excludedDirectories = filter.excludedDirectories }))
      local seen, ids, snapshots = {}, {}, {}
      for _, item in ipairs(inventory.files) do
        local key = path_key(vim.uri_to_fname(item.uri))
        must(label .. ' unique root file ' .. key, not seen[key] and not ids[item.file]
          and expected[key] ~= nil and type(item.file) == 'string'
          and same_uri(item.uri, root .. '/' .. item.relativePath))
        seen[key], ids[item.file] = item, true
      end
      for path in pairs(expected) do must(label .. ' expected inventory path ' .. path, seen[path] ~= nil) end
      must(label .. ' excluded directory omitted', seen[path_key(excluded)] == nil)
      for _, item in ipairs(capture_value.snapshots) do
        local key = path_key(vim.uri_to_fname(item.uri)); local want = expected[key]
        must(label .. ' snapshot has issued identity ' .. key, seen[key] and seen[key].file == item.file
          and snapshots[key] == nil and type(want) == 'string' and item.text == want
          and item.sha256 == vim.fn.sha256(want) and #item.chunks > 0)
        snapshots[key] = item
        for _, chunk in ipairs(item.chunks) do
          local value = chunk.value
          must(label .. ' snapshot current content ' .. key, chunk.state == 'found' and value
            and value.file == item.file and same_uri(value.uri, key) and value.root.identity == inventory.root.identity
            and value.identity.kind == 'rootFile' and value.identity.sha256 == item.sha256
            and type(value.identity.revision) == 'string' and value.size == #want
            and (versions[key] == nil or value.documentVersion == versions[key]))
        end
      end
      for path, text in pairs(expected) do
        if type(text) == 'string' then must(label .. ' expected snapshot ' .. path, snapshots[path] ~= nil) end
      end
      must(label .. ' large UTF-8 snapshot spans chunks', snapshots[path_key(large)] and #snapshots[path_key(large)].chunks > 1)
      return seen
    end
    local ok, failure = xpcall(function()
      assert(vim.fn.filereadable(excluded) == 1 and vim.fn.filereadable(extension_only) == 1, 'filter fixtures missing')
      local owner_source, model_path
      for _, path in ipairs(vim.fn.glob(vim.env.KTLSP_CACHE_DIR .. '/classpath/*.json', false, true)) do
        local model = vim.json.decode(table.concat(vim.fn.readfile(path), '\n'))
        for _, module in ipairs(model.modules or {}) do
          if module.module == ':app' and path_key(module.project_dir) == path_key(root .. '/modules/editing') then
            for _, source in ipairs(module.source_sets or {}) do
              if source.name == 'main' then owner_source, model_path = source, path end
            end
          end
        end
      end
      local resources = owner_source and owner_source.resources
      must('nested owner has complete Gradle resource facts', resources and resources.complete == true)
      local selected = false
      for _, path in ipairs({ shared, client_path, hidden, gradle_hidden, duplicate_a, duplicate_b, removed }) do
        local key = path_key(path)
        for _, base in ipairs(resources.roots or {}) do
          local prefix = path_key(base):gsub('/+$', '') .. '/'
          if key:sub(1, #prefix) == prefix then selected = true end
        end
        for _, file in ipairs(resources.files or {}) do if path_key(file.path) == key then selected = true end end
      end
      must('root configs are outside the nested owner resource selection', not selected)
      proof.owner_model = { path = model_path, resource_roots = resources.roots, resource_files = #resources.files }
      vim.fn.mkdir(pager, 'p')
      local expected = {}
      expected[path_key(extension_only)] = true
      for i = 1, 257 do
        local path = ('%s/page-%03d.rootfixture'):format(pager, i)
        write(path, ('page-%03d\n'):format(i)); created[#created + 1] = path; expected[path_key(path)] = true
      end
      local big = '{"text":"' .. string.rep('漢😀', 20000) .. '"}\n'
      write(large, big)
      local client_text = '{"scope":"java-client","value":"overlay-初"}\n'
      local new_text = '{"scope":"new-unsaved","value":"初"}\n'
      local client_version, new_version
      client_version, client_text = edit(client_buf, client_text)
      new_version, new_text = edit(new_buf, new_text)
      for _, path in ipairs({ shared, hidden, gradle_hidden, duplicate_a, duplicate_b, removed, large }) do expected[path_key(path)] = read(path) end
      expected[path_key(client_path)], expected[path_key(unsaved)] = client_text, new_text
      settings('initial', true)
      wait_file(pause_file('initial', 'paused'), 'initial evaluation paused after capture')
      local initial = capture('initial', nil, false)
      must('initial capture belongs to nested owner', type(initial.owner) == 'string'
        and path_key(initial.owner) == path_key(owner))
      must('paused capture matches barrier evaluation', tostring(initial.evaluation) == table.concat(vim.fn.readfile(pause_file('initial', 'paused')), ''))
      local seen = inspect('initial', initial, expected, { [path_key(client_path)] = client_version, [path_key(unsaved)] = new_version }, 2)
      proof.phases.initial = initial; proof.ordering[#proof.ordering + 1] = 'initial-capture'; save()
      local stale_token = seen[path_key(shared)].file
      local fresh_shared = '{"scope":"workspace","value":"changed"}\n'
      write(shared, fresh_shared); assert(vim.uv.fs_unlink(removed)); assert(vim.uv.fs_rename(duplicate_a, renamed))
      write(added, '{"scope":"added"}\n')
      wait_file(pause_file('initial', 'cancelled'), 'stale evaluation cancelled after native disk changes')
      must('old evaluation not published before release', event('extension.ipc.accepted', initial.evaluation) == nil)
      client_text = '{"scope":"java-client","value":"overlay-更新"}\n'
      client_version, client_text = edit(client_buf, client_text)
      vim.fn.writefile({ 'release' }, pause_file('initial', 'release'))
      local rejected = vim.wait(30000, function() return event('extension.ipc.rejected', initial.evaluation) ~= nil end, 25)
      must('old evaluation rejected after release', rejected and event('extension.ipc.accepted', initial.evaluation) == nil)
      expected[path_key(shared)], expected[path_key(client_path)] = fresh_shared, client_text
      expected[path_key(removed)], expected[path_key(duplicate_a)] = nil, nil
      expected[path_key(renamed)], expected[path_key(added)] = original_duplicate, read(added)
      local fresh, accepted = capture('initial', initial.evaluation, true)
      must('fresh accepted evaluation uses nested owner', accepted and same_uri(vim.uri_from_fname(accepted.args.owner), owner))
      local fresh_seen = inspect('fresh', fresh, expected, { [path_key(client_path)] = client_version, [path_key(unsaved)] = new_version }, 2)
      proof.phases.fresh = fresh; proof.ordering[#proof.ordering + 1] = 'stale-rejected-and-fresh-accepted'; save()
      must('fresh provider session is identified', type(accepted.args.instance) == 'number')
      local old_token = fresh_seen[path_key(shared)].file
      must('fresh session uses a new opaque file token', old_token ~= stale_token)
      for _, test in ipairs({ { 'old', old_token }, { 'foreign', 'root-file-foreign-token' }, { 'escape', '../shared/root-config.json' } }) do
        disable(); settings(test[1], false, test[2])
        local negative = capture(test[1], nil, false)
        local rejected_negative = vim.wait(30000, function() return event('extension.ipc.rejected', negative.evaluation) ~= nil end, 25)
        must(test[1] .. ' token rejected without publication', rejected_negative and negative.negative
          and negative.negative.file == test[2] and negative.negative.error.code == -32602
          and event('extension.ipc.accepted', negative.evaluation) == nil)
        proof.phases[test[1]] = negative; save()
      end
    end, debug.traceback)
    disable(); client.rpc.notify = old_notify
    for path, text in pairs(originals) do write(path, text) end
    for _, path in ipairs(created) do if vim.fn.filereadable(path) == 1 then assert(vim.uv.fs_unlink(path)) end end
    vim.fn.delete(pager, 'd')
    vim.api.nvim_buf_set_lines(client_buf, 0, -1, false, vim.split(original_client:gsub('\n$', ''), '\n', { plain = true }))
    vim.bo[client_buf].modified = false; vim.bo[new_buf].modified = false
    vim.lsp.buf_detach_client(new_buf, client.id); vim.api.nvim_buf_delete(new_buf, { force = true })
    require('vim.lsp._changetracking').flush(client)
    for path, text in pairs(originals) do assert(read(path) == text, 'fixture restoration failed') end
    assert(vim.fn.filereadable(unsaved) == 0, 'unsaved fixture reached disk')
    save(); if not ok then error(failure) end
    return
  end
  if vim.env.KTLSP_IPC_MODE == 'provider-commands' or vim.env.KTLSP_IPC_MODE == 'project-inputs'
    or vim.env.KTLSP_IPC_MODE == 'binary-annotations' then return dofile(env.repo .. '/dev/external-extension/command-probes.lua')(env) end
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local linked = vim.env.KTLSP_IPC_MODE == 'linked'
  local host_inputs = vim.env.KTLSP_IPC_MODE == 'host-inputs'
  local catalog = vim.env.KTLSP_IPC_MODE == 'jvm-catalog'
  local locations = vim.env.KTLSP_IPC_MODE == 'source-locations'
  local source_v2 = vim.env.KTLSP_IPC_MODE == 'source-facts-v2'
  local source_facts = vim.env.KTLSP_IPC_MODE == 'source-snapshots' or host_inputs
  local facts = vim.env.KTLSP_IPC_MODE == 'project-facts' or source_facts or catalog or locations
  local barrier = assert(vim.env.KTLSP_IPC_BARRIER, 'missing child barrier directory')
  local paths, buffers, originals = {}, {}, {}
  local proof = { mode = source_v2 and 'source-facts-v2' or locations and 'source-locations' or source_facts and 'source-snapshots' or facts and 'project-facts' or (linked and 'linked' or 'ipc'), ordering = {}, requests = {}, snapshots = {} }
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
  local catalog_probe = catalog and dofile(env.repo .. '/dev/external-extension/jvm-catalog-probes.lua')(env, proof, save) or nil
  local function record(event) proof.ordering[#proof.ordering + 1] = event; save() end
  local function source_locations(label,evaluation)
    local observed=vim.json.decode(table.concat(vim.fn.readfile(barrier..'/locations-'..evaluation..'.json'),'\n'))
    assert(#observed.facts==2,'exact type/member location pair required')
    local text
    if buffers.target then text=probe.buffer_full_text(buffers.target) else
      local target_buffer=vim.fn.bufnr(paths.target)
      local attached=target_buffer~=-1 and vim.lsp.buf_is_attached(target_buffer,client.id)
      check('initial source-location target is never opened or attached',target_buffer==-1 and not attached)
      assert(target_buffer==-1 and not attached,'source-location target must remain unopened until the initial capture')
      local file=assert(io.open(paths.target,'rb'));text=file:read('*a');file:close()
      observed.unopened_target={path=paths.target,buffer=target_buffer,attached=attached,sha256=vim.fn.sha256(text)}
    end
    local target_lines=vim.split(text,'\n',{plain=true})
    -- Equivalent file URIs may encode the Windows drive colon differently.
    local expected_path = vim.uv.fs_realpath(vim.uri_to_fname(vim.uri_from_fname(paths.target)))
    for i,entry in ipairs({{'public class Target','Target',observed.type.value.id},{'public int twice','twice',observed.member.id}}) do
      local _,range=probe.find(target_lines,entry[1],entry[2])
      local fact=observed.facts[i];local value=fact.value
      local actual_path = type(value)=='table' and type(value.documentUri)=='string' and vim.uv.fs_realpath(vim.uri_to_fname(value.documentUri))
      check(label..' exact cross-owner source location '..entry[2],fact.state=='found' and value.target==entry[3]
        and actual_path and expected_path and probe.path_key(actual_path)==probe.path_key(expected_path) and vim.deep_equal(value.range,range))
      check(label..' current source identity for '..entry[2],fact.state=='found' and value.source.sha256==vim.fn.sha256(text)
        and type(value.source.identity.revision)=='string')
    end
    proof.source_locations=proof.source_locations or {};proof.source_locations[label]=observed;save()
  end
  local function source_facts_v2(label,evaluation)
    local observed=vim.json.decode(table.concat(vim.fn.readfile(barrier..'/source-facts-'..evaluation..'.json'),'\n'))
    local declaration=observed.rich
    local text=probe.buffer_full_text(buffers.mixin)
    local function slice(range) return text:sub(range.start+1,range['end']) end
    check(label..' first evaluation reads negotiated source v2',observed.evaluation==evaluation and observed.ownerEvaluation==1
      and observed.legacy.fullRange==nil and observed.legacy.modifiers==nil)
    check(label..' source v2 has exact declaration ranges',slice(declaration.nameRange)=='sourceFactsProbe'
      and slice(declaration.bodyRange)=='{}' and slice(declaration.headerRange):find('public static final void sourceFactsProbe()',1,true)~=nil)
    local annotation=declaration.annotations[1]
    check(label..' annotation name range is exact',slice(annotation.nameRange)=='SourceFactsExample')
    local arguments=annotation.arguments
    local mapped=arguments[1].value.value
    check(label..' decoded literal concatenation has no false content range',mapped.kind=='mappedString'
      and mapped.value=='tick()V' and mapped.contentRange==nil and #mapped.segments==7)
    check(label..' decoded escape preserves whole source spelling',slice(mapped.segments[3].source)==[=[\u0063]=]
      and mapped.segments[3].decoded.start==2 and mapped.segments[3].decoded['end']==3)
    local nested=arguments[2].value.value.annotation
    check(label..' nested annotation keeps exact name and full range',slice(nested.nameRange)=='SourceFactsAt'
      and slice(nested.range):find('@SourceFactsAt(',1,true)==1)
    check(label..' nested annotation contains useful target strings',nested.arguments[1].value.state=='found'
      and nested.arguments[1].value.value.kind=='string' and nested.arguments[1].value.value.value=='INVOKE'
      and nested.arguments[2].value.state=='found' and nested.arguments[2].value.value.kind=='string'
      and nested.arguments[2].value.value.value=='Lpkg/C;m()V')
    for i,name in ipairs({'fixture.SourceFactsExample','fixture.SourceFactsAt'}) do
      check(label..' annotation type resolves through host facts '..name,observed.types[i].state=='found'
        and observed.types[i].value.qualifiedName==name)
    end
    check(label..' nested ordinal is typed Int',vim.deep_equal(nested.arguments[3].value.value,{kind='integer',type='int',value=1}))
    check(label..' long literal is typed Long',vim.deep_equal(arguments[3].value.value,{kind='integer',type='long',value=2}))
    check(label..' ordinary zero-arity callable has known empty parameters',declaration.parameters.state=='found' and #declaration.parameters.value==0)
    local callable=observed.parameterDeclaration
    check(label..' callable exposes ordered authored parameters',callable.parameters.state=='found'
      and vim.deep_equal(vim.tbl_map(function(parameter) return parameter.name end,callable.parameters.value),{'text','slot','ci'}))
    local parameters=callable.parameters.value
    for _,parameter in ipairs(parameters) do
      check(label..' exact parameter and annotation ranges '..parameter.name,slice(parameter.nameRange)==parameter.name
        and slice(parameter.range):sub(1,1)=='@' and parameter.range.start<=parameter.annotations[1].range.start
        and parameter.annotations[1].range['end']<=parameter.nameRange.start
        and parameter.nameRange['end']<=parameter.range['end'] and type(parameter.typeRef)=='string')
    end
    local local_args=parameters[1].annotations[1].arguments
    check(label..' parameter annotation has explicit boolean and integer values',vim.deep_equal(local_args[1].value.value,{kind='boolean',value=true})
      and vim.deep_equal(local_args[2].value.value,{kind='integer',type='int',value=0}))
    local names=local_args[3].value.value
    check(label..' parameter annotation preserves string array',names.kind=='array' and #names.values==1
      and names.values[1].value.value.kind=='string' and names.values[1].value.value.value=='text')
    local at=local_args[4].value.value
    check(label..' parameter annotation preserves nested explicit values',at.kind=='annotation'
      and at.annotation.arguments[1].value.value.value=='INVOKE'
      and at.annotation.arguments[2].value.value.value=='Lpkg/C;m()V'
      and at.annotation.arguments[3].value.value.value==1)
    check(label..' share and marker parameter annotations stay distinct',parameters[2].annotations[1].arguments[1].value.value.value=='slot'
      and #parameters[3].annotations[1].arguments==0)
    for i,name in ipairs({'refs.LocalRef','fixture.CallbackInfo','fixture.Local','fixture.Share','fixture.Cancellable','fixture.SourceFactsAt'}) do
      check(label..' parameter type resolves through host facts '..name,observed.parameterTypes[i].state=='found'
        and observed.parameterTypes[i].value.qualifiedName==name)
    end
    proof.source_facts_v2=proof.source_facts_v2 or {};proof.source_facts_v2[label]=observed;save()
  end

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
      selected = { module = module.module, source_set = source.name, entries = source.entries, java_home = java.java_home } }
    save()
    return source.entries
  end
  local function attach(key)
    local path=paths[key]
    local buf = vim.fn.bufadd(path); vim.fn.bufload(buf); vim.bo[buf].filetype = 'java'
    buffers[key] = buf
    env.sources[path] = env.sources[path] or { buf = buf, disk = originals[key], sha256 = vim.fn.sha256(table.concat(originals[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  for key, file in pairs({ target = 'fixture/Target.java', mixin = 'fixture/CaseMixin.java', use = 'fixture/IpcUse.java' }) do
    local path = vim.fs.abspath(env.project .. (facts and '/modules/editing/src/main/java/' or '/') .. file)
    paths[key],originals[key] = path,vim.fn.readfile(path)
    if not (locations and key=='target') then attach(key) end
  end
  if source_v2 then
    for key,file in pairs({ parameter_ref='refs/LocalRef.java',parameter_decoy='decoy/LocalRef.java' }) do
      local path=vim.fs.abspath(env.project .. '/' .. file)
      paths[key],originals[key]=path,vim.fn.readfile(path)
      attach(key)
    end
    vim.api.nvim_buf_set_lines(buffers.mixin,1,1,false,{'import refs.LocalRef;'})
    vim.api.nvim_buf_set_lines(buffers.mixin, -1, -1, false, {
      '@interface SourceFactsAt { String value(); String target(); int ordinal(); }',
      '@interface SourceFactsExample { String method(); SourceFactsAt at(); long index(); }',
      '@interface Local { boolean argsOnly(); int ordinal(); String[] name(); SourceFactsAt at(); }',
      '@interface Share { String value(); }',
      '@interface Cancellable {}',
      'class CallbackInfo {}',
      'class SourceFactsProbe {',
      [=[  @SourceFactsExample(method="ti\u0063k" + "()V", at=@SourceFactsAt(value="INVOKE", target="Lpkg/C;m()V", ordinal=1), index=2L)]=],
      '  public static final void sourceFactsProbe() {}',
      [=[  public void parameterFactsProbe(@Local(argsOnly=true,ordinal=0,name={"text"},at=@SourceFactsAt(value="INVOKE",target="Lpkg/C;m()V",ordinal=1)) String text,@Share("slot") LocalRef<String> slot,@Cancellable CallbackInfo ci) {}]=],
      '}',
    })
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
      if locations then value.command[#value.command + 1] = '--source-locations' end
      if source_v2 then value.command[#value.command + 1] = '--source-facts-v2' end
      if host_inputs then value.command[#value.command + 1] = '--host-inputs'; value.configuration.runtimeClasses={'java/util/List','vendor/Versioned','vendor/Added','app/Main'} end
      if catalog then value.command[#value.command + 1] = '--jvm-catalog' end
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
  if source_v2 then
    local ok, failure = xpcall(function()
      local evaluation
      local ready = vim.wait(30000, function()
        local files = vim.fn.glob(barrier .. '/source-facts-*.json', false, true)
        if #files == 0 then return false end
        assert(#files == 1, 'source facts must succeed on the first owner evaluation')
        evaluation = vim.json.decode(table.concat(vim.fn.readfile(files[1]), '\n')).evaluation
        for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
          local decoded, event = pcall(vim.json.decode, line)
          if decoded and event.name == 'extension.ipc.accepted' and event.args
            and event.args.provider == provider and tostring(event.args.evaluation) == tostring(evaluation) then
            proof.accepted = event.args
            return true
          end
        end
        return false
      end, 25)
      check('first negotiated source-facts evaluation is accepted by the host', ready)
      assert(ready, 'source facts were not accepted on the first evaluation')
      check('accepted source-facts evaluation matches the unsaved source bytes', proof.accepted.source_sha256 == vim.fn.sha256(probe.buffer_full_text(buffers.mixin)))
      source_facts_v2('initial', evaluation)
    end, debug.traceback)
    settings(false)
    for key, original in pairs(originals) do
      vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, original)
      vim.bo[buffers[key]].modified = false
    end
    deliver(); client.handlers['textDocument/publishDiagnostics'] = previous_diagnostics; save()
    if not ok then error(failure) end
    return
  end
  if catalog then
    local denial, rejected, exited, published
    local ready = vim.wait(15000, function()
      local path = barrier .. '/catalog-denial.json'
      if vim.fn.filereadable(path) == 0 then return false end
      denial = vim.json.decode(table.concat(vim.fn.readfile(path), '\n'))
      for _, line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE) == 1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        local ok, event = pcall(vim.json.decode, line)
        if ok and event.args and event.args.provider == provider and event.name == 'extension.ipc.exited' then exited = event end
        if ok and event.args and tostring(event.args.evaluation) == tostring(denial.evaluation) then
          if event.name == 'extension.ipc.rejected' then rejected = event end
          if event.name == 'extension.ipc.accepted' then published = true end
        end
      end
      return rejected ~= nil and exited ~= nil
    end, 25)
    local before_path = barrier .. '/catalog-denial-before.json'
    local before = vim.fn.filereadable(before_path) == 1 and vim.json.decode(table.concat(vim.fn.readfile(before_path), '\n')) or nil
    local valid = ready and not published and not fresh_diagnostic and denial.error.code == -32602
      and denial.error.message == 'foreign runtime class identity' and type(exited.args.instance) == 'number'
      and before and tostring(before.evaluation) == tostring(denial.evaluation)
      and type(before.descriptor.entry) == 'string' and before.descriptor.internalName == 'vendor/Versioned'
    check('catalog handle denial rejects its evaluation without publication', valid)
    assert(valid, 'catalog denial evaluation did not terminate correctly')
    proof.jvm_catalog.denial = denial; proof.jvm_catalog.denial_before = before; proof.jvm_catalog.denial_rejection = rejected
    proof.jvm_catalog.denial_exited = exited; save()
    settings(false); settings(true)
  end
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
      if catalog_probe then catalog_probe.capture('initial', proof.paused_evaluation) end
      if source_facts then source_snapshots('initial', proof.paused_evaluation) end
      if locations then source_locations('initial',proof.paused_evaluation) end
      if inputs then inputs.capture('initial', proof.paused_evaluation); inputs.change_wire() end
      proof.initial_model_generation = model_state()
      assert(proof.initial_model_generation, 'initial accepted model generation is missing')
      if locations then
        attach('target')
        local changed=vim.api.nvim_buf_get_lines(buffers.target,0,-1,false)
        changed[2]='/* 😀 */ '..changed[2];changed[3]='/* 😀 */ '..changed[3]
        vim.api.nvim_buf_set_lines(buffers.target,0,-1,false,changed);deliver()
      end
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
    if catalog then
      check('fresh catalog contribution uses a new provider instance', type(accepted.args.instance) == 'number'
        and accepted.args.instance ~= proof.jvm_catalog.denial_exited.args.instance)
    end
    check('fresh contribution matches the current owner bytes', accepted.args.source_sha256 == probe.buffer_sha256(buffers.mixin))
    proof.facts_publication = accepted
    local entries = project_facts('fresh', tostring(accepted.args.evaluation))
    if catalog_probe then catalog_probe.capture('fresh', tostring(accepted.args.evaluation)) end
    if source_facts then source_snapshots('fresh', tostring(accepted.args.evaluation)) end
    if locations then source_locations('fresh',tostring(accepted.args.evaluation)) end
    if inputs then inputs.capture('fresh',tostring(accepted.args.evaluation)); inputs.native_lifecycle() end
    local initial = proof.project_facts.initial.selected.entries
    local added = env.project .. '/home/.m2/repository/fixture/reload/added/1.0/added-1.0.jar'
    local found = false
    for _, entry in ipairs(entries) do if probe.path_key(entry) == probe.path_key(added) then found = true end end
    check('fresh projectContext observes the real added classpath entry', found and #entries > #initial)
    record('two-project-fact-operations-and-fresh-contribution-verified')
  end
  if catalog then save(); return end
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
