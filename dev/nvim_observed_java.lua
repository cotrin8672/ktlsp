-- This is an EXTRA_PROBES callback for the existing corpus client, never a second LSP client.
-- All semantic requests have exactly one attempt. Compiles have distinct lifecycle purposes.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local client, sources = ctx.client, ctx.sources
  local checks, phases, delivery, sent_versions = {}, {}, {}, {}
  local external_build = vim.env.KTLSP_JAVA_EXTERNAL_BUILD == '1'
  local watched_changes, wire_texts = {}, {}
  local external_watch_snapshot
  local initial_getter_queries, config_serial = 0, 0
  local original_notify = client.rpc.notify
  local person = ctx.project .. '/src/main/java/producerfixture/Person.java'
  local consumer = ctx.project .. '/src/main/java/producerfixture/Consumer.java'
  local getter_anchor = 'public String read(Person person) { return person.getName(); }'
  local chain_anchor = 'return person.getName().length();'
  local control_anchor = 'public String control(Person person) { return person.authored(); }'
  local function check(name, ok, detail)
    checks[#checks + 1] = { name = name, ok = not not ok, detail = detail }
    return ctx.check(name, ok, detail)
  end
  local function append(name, value)
    vim.fn.writefile({ vim.json.encode(value) }, ctx.artifacts .. '/' .. name, 'a')
  end
  for _, value in pairs(sources) do
    sent_versions[vim.uri_from_bufnr(value.buf)] = vim.lsp.util.buf_versions[value.buf]
    if external_build then wire_texts[vim.uri_from_bufnr(value.buf)] = probe.buffer_full_text(value.buf) end
  end
  client.rpc.notify = function(method, params)
    local watch_before = external_build and method == 'workspace/didChangeWatchedFiles'
      and external_watch_snapshot and external_watch_snapshot() or nil
    local sent = original_notify(method, params)
    if sent and (method == 'textDocument/didChange' or method == 'textDocument/didOpen') then
      sent_versions[params.textDocument.uri] = params.textDocument.version
      if external_build then
        local uri = params.textDocument.uri
        if method == 'textDocument/didOpen' then wire_texts[uri] = params.textDocument.text
        else wire_texts[uri] = probe.apply_content_changes(wire_texts[uri], params.contentChanges, client.offset_encoding or 'utf-16') end
      end
    end
    if sent and method == 'workspace/didChangeConfiguration' then config_serial = config_serial + 1 end
    if external_build and sent and method == 'workspace/didChangeWatchedFiles' then
      watched_changes[#watched_changes + 1] = { params = vim.deepcopy(params), sent_ns = vim.uv.hrtime(), client = client.id,
        before = watch_before, after = external_watch_snapshot and external_watch_snapshot() or nil }
    end
    return sent
  end
  local function source(path)
    if not sources[path] then
      local disk, buf = vim.fn.readfile(path), vim.fn.bufadd(path)
      vim.fn.bufload(buf)
      vim.bo[buf].filetype = 'java'
      sources[path] = { buf = buf, disk = disk, sha256 = vim.fn.sha256(table.concat(disk, '\n')) }
    end
    vim.lsp.buf_attach_client(sources[path].buf, client.id)
    return sources[path]
  end
  local function lines(path) return vim.api.nvim_buf_get_lines(source(path).buf, 0, -1, false) end
  local function position(path, anchor, token, prefix)
    local value = probe.find(lines(path), anchor, token)
    if prefix then value.character = value.character + vim.str_utfindex(prefix, 'utf-16') end
    return value
  end
  local function rpc(name, method, params, path, timeout)
    assert(not external_build or method ~= 'workspace/executeCommand', 'external build mode must not ask the LSP to generate code')
    local response, err = client:request_sync(method, params, timeout or 10000, source(path or consumer).buf)
    local received = response ~= nil and response.err == nil and err == nil
    check(name .. ': first response received', received, vim.inspect(response and response.err or err))
    append('compiled-java-requests.jsonl', { name = name, method = method, params = params,
      response = response, error = err, attempts = 1, received = received,
      config_serial = config_serial, delivered_versions = vim.deepcopy(sent_versions) })
    return response and response.result, received
  end
  local function completion(name, anchor, token, prefix, expected, expected_token)
    local result, received = rpc(name, 'textDocument/completion', {
      textDocument = { uri = vim.uri_from_bufnr(source(consumer).buf) },
      position = position(consumer, anchor, token, prefix), context = { triggerKind = 1 },
    })
    local matching = {}
    local wanted = expected_token or token
    if type(result) == 'table' then
      for _, item in ipairs(result.items or result) do
        if item.label == wanted or item.label:match('^' .. wanted .. '%f[^%w_]') then matching[#matching + 1] = item end
      end
    end
    check(name .. (expected and ': exactly one useful candidate' or ': stale candidate absent'),
      received and (expected and #matching == 1 or not expected and #matching == 0), vim.inspect(matching))
    return received and #matching == 1
  end
  local function getter(name, expected, anchor)
    if name == 'initial-first-generated-completion' then initial_getter_queries = initial_getter_queries + 1 end
    return completion(name, anchor or getter_anchor, 'getName', 'ge', expected)
  end
  local function authored(name)
    completion(name .. ': authored same-phase control', control_anchor, 'authored', 'au', true)
  end
  local function barrier(name)
    rpc(name .. ': delivered-notification transport barrier', 'textDocument/documentSymbol', {
      textDocument = { uri = vim.uri_from_bufnr(source(consumer).buf) },
    })
  end
  local function flush(name)
    require('vim.lsp._changetracking').flush(client)
    local expected, delivered = {}, true
    for _, path in ipairs({ person, consumer }) do
      local buf = source(path).buf
      local uri, version = vim.uri_from_bufnr(buf), vim.lsp.util.buf_versions[buf]
      expected[uri] = version
      delivered = delivered and sent_versions[uri] == version
    end
    delivery[#delivery + 1] = { name = name, expected = expected,
      outbound = vim.deepcopy(sent_versions), delivered = delivered,
      barrier = 'vim.lsp._changetracking.flush(client)' }
    check(name .. ': exact didChange versions delivered', delivered, vim.inspect(expected))
    barrier(name)
  end
  local function set(path, value, name)
    vim.api.nvim_buf_set_lines(source(path).buf, 0, -1, false, vim.split(value, '\n', { plain = true }))
    flush(name)
    check(name .. ': buffer only', vim.bo[source(path).buf].modified
      and vim.deep_equal(vim.fn.readfile(path), source(path).disk))
  end
  local function restore(path, name)
    vim.api.nvim_buf_set_lines(source(path).buf, 0, -1, false, source(path).disk)
    vim.bo[source(path).buf].modified = false
    flush(name)
  end
  local function configure(name, enabled, revision)
    check(name .. ': configuration delivered', client:notify('workspace/didChangeConfiguration', {
      settings = { analysisProviders = { ['ktlsp.compiled-java'] = {
        enabled = enabled, configuration = { testRevision = revision },
      } } },
    }))
    barrier(name)
  end
  local function compile(name, expected_status)
    local result, received = rpc(name, 'workspace/executeCommand', {
      command = 'ktlsp.compile', arguments = { { uri = vim.uri_from_bufnr(source(consumer).buf) } },
    }, consumer, 240000)
    expected_status = expected_status or 'observed-success'
    local good = received and type(result) == 'table' and result.status == expected_status
    check(name .. ': exact compile outcome ' .. expected_status, good, vim.inspect(result))
    if expected_status == 'observed-success' then
      check(name .. ': selected fixture task really executed', good and result.executed == true and result.task == ':compileJava', vim.inspect(result))
      check(name .. ': producer closure remains unknown', good and result.compilerClosureComplete == false)
    end
    phases[#phases + 1] = { name = name, result = result, received = received, expected = expected_status }
    return good
  end
  local external_restore
  local function run_external()
    local function bytes(path)
      local file = assert(io.open(path, 'rb'))
      local value = file:read('*a'); file:close(); return value
    end
    local function write(path, value)
      local file = assert(io.open(path, 'wb')); assert(file:write(value)); file:close()
    end
    local config = ctx.project .. '/lombok.config'
    local original = { [person] = bytes(person), [consumer] = bytes(consumer), [config] = bytes(config) }
    assert(bytes(ctx.project .. '/settings.gradle.kts'):find('ktlsp-java-lombok-clean-receipt', 1, true),
      'external builds require the disposable --java-only fixture')
    assert(vim.fn.sha256(table.concat(vim.fn.readfile(person), '\n')) == ctx.manifest.probes[1].target_sha256)
    local caller = source(consumer)
    local old_buffer = { lines = lines(consumer), text = probe.buffer_full_text(caller.buf),
      modified = vim.bo[caller.buf].modified, eol = vim.bo[caller.buf].eol, fileformat = vim.bo[caller.buf].fileformat }
    local active_build
    local function join_build(cancel)
      if not active_build then return true end
      -- jobstart/jobstop own a Neovim job, including its process tree; never kill by name/PID.
      if cancel and vim.fn.jobwait({ active_build.id }, 0)[1] == -1 then
        active_build.record.stop_requested = true
        active_build.record.stop_result = vim.fn.jobstop(active_build.id)
      end
      local status = vim.fn.jobwait({ active_build.id }, 10000)[1]
      active_build.record.join_status = status
      local joined = status >= 0 or status == -3 -- Exited jobs may already have been released.
      active_build.record.joined = joined
      if status >= 0 then active_build.record.exit = status end
      if joined then active_build = nil end
      return joined
    end
    external_restore = function()
      assert(join_build(true), 'owned external build did not terminate; outer job cleanup must drain it')
      for path, value in pairs(original) do if bytes(path) ~= value then write(path, value) end end
      vim.api.nvim_buf_set_lines(caller.buf, 0, -1, false, old_buffer.lines)
      vim.bo[caller.buf].eol, vim.bo[caller.buf].fileformat, vim.bo[caller.buf].modified = old_buffer.eol, old_buffer.fileformat, old_buffer.modified
      require('vim.lsp._changetracking').flush(client)
      for path, value in pairs(original) do check('external build disk restored: ' .. path, bytes(path) == value) end
      check('external build original consumer buffer restored', probe.buffer_full_text(caller.buf) == old_buffer.text)
    end
    local reader = { offset = 0, tail = '' }
    local function state()
      local file = assert(io.open(assert(vim.env.KTLSP_TRACE), 'rb'))
      if file:seek('end') < reader.offset then reader = { offset = 0, tail = '' } end
      file:seek('set', reader.offset)
      local chunk = file:read('*a'); reader.offset = file:seek(); file:close()
      local pending = reader.tail .. chunk
      local last = 1
      for line, next_position in pending:gmatch('([^\n]*)\n()') do
        last = next_position
        local decoded, event = pcall(vim.json.decode, line)
        local args = decoded and type(event) == 'table' and event.args
        if type(args) == 'table' and probe.path_key(args.root or '') == probe.path_key(ctx.project) then
          reader.events = (reader.events or 0) + 1
          if event.name == 'workspace.refresh_requested' then reader.requested = args.generation
          elseif event.name == 'workspace.accepted_finish' then reader.accepted, reader.accepted_at = args, reader.events
          elseif event.name == 'workspace.source_install_begin' then reader.source_begin, reader.source_begin_at = args, reader.events
          elseif event.name == 'workspace.source_install' then reader.sources, reader.sources_at = args, reader.events
          elseif event.name == 'deps.index_workspace' then reader.dependencies, reader.dependencies_at = args, reader.events end
        end
      end
      reader.tail = pending:sub(last)
      return reader
    end
    local function available(name, ok, detail)
      checks[#checks + 1] = { name = name, ok = not not ok, detail = detail, kind = 'operational' }
      ctx.check(name, ok, detail, 'operational')
      return ok
    end
    local function edit_caller(api)
      local value, count = original[consumer]:gsub('getName', api)
      assert(count == 2)
      local edited = vim.split(value:gsub('\n$', ''), '\n', { plain = true })
      edited[#edited + 1] = '// unsaved caller survives external Gradle builds'
      vim.api.nvim_buf_set_lines(caller.buf, 0, -1, false, edited)
      require('vim.lsp._changetracking').flush(client)
      local version = vim.lsp.util.buf_versions[caller.buf]
      local uri, source_sha256 = vim.uri_from_bufnr(caller.buf), probe.buffer_sha256(caller.buf)
      local wire_sha256 = type(wire_texts[uri]) == 'string' and vim.fn.sha256(wire_texts[uri]) or nil
      check('external caller edit delivered: ' .. api, sent_versions[uri] == version and wire_sha256 == source_sha256)
      delivery[#delivery + 1] = { mode = 'external-build', api = api, version = version,
        source_sha256 = source_sha256, wire_sha256 = wire_sha256, unsaved = vim.bo[caller.buf].modified }
    end
    local function output_identity()
      local root = ctx.project .. '/build/classes/java/main/producerfixture/'
      return { Person = vim.fn.sha256(bytes(root .. 'Person.class')), Consumer = vim.fn.sha256(bytes(root .. 'Consumer.class')) }
    end
    external_watch_snapshot = function()
      local ordinal = state().events or 0
      local found, outputs = pcall(output_identity)
      return { trace_events = ordinal, outputs = found and outputs or nil }
    end
    local function build(name, mutate)
      local before = state().requested or 0
      local watch_start = #watched_changes
      local buffer_before = probe.buffer_full_text(caller.buf)
      mutate()
      local inputs = {}
      for path in pairs(original) do inputs[path] = vim.fn.sha256(bytes(path)) end
      local command = vim.fn.has('win32') == 1
        and { vim.env.COMSPEC or 'cmd.exe', '/d', '/s', '/c',
          'call "' .. ctx.project .. '/gradlew.bat" :classes --rerun-tasks --no-build-cache --offline --no-daemon --console=plain --max-workers=1' }
        or { 'bash', ctx.project .. '/gradlew', ':classes', '--rerun-tasks', '--no-build-cache', '--offline', '--no-daemon', '--console=plain', '--max-workers=1' }
      local phase = { name = name, command = command, watch_start = watch_start, before_generation = before, client = client.id,
        disk_input_sha256 = inputs, unsaved_caller_sha256 = vim.fn.sha256(buffer_before) }
      phases[#phases + 1] = phase
      local completed, output = nil, {}
      local job = vim.fn.jobstart(command, { cwd = ctx.project, detach = false, stdout_buffered = true, stderr_buffered = true,
        on_stdout = function(_, data) output[#output + 1] = table.concat(data, '\n') end,
        on_stderr = function(_, data) output[#output + 1] = table.concat(data, '\n') end,
        on_exit = function(_, code) completed = code end,
      })
      assert(job > 0, 'external Gradle launcher failed')
      phase.job, phase.pid, phase.operational_timeout_ms = job, vim.fn.jobpid(job), 120000
      phase.process_owner = 'Neovim jobstart/jobstop; detach=false; process-tree stop and bounded join'
      active_build = { id = job, record = phase }
      local finished = vim.wait(120000, function() return completed ~= nil end, 20)
      phase.timed_out = not finished
      local joined = join_build(not finished)
      if not joined then joined = join_build(true) end
      phase.exit, phase.joined = completed, joined
      write(ctx.artifacts .. '/external-java-' .. name .. '-build.out', table.concat(output, '\n'))
      if not available('external ' .. name .. ': actual Gradle classes build succeeds and owned job joins',
        finished and joined and completed == 0, vim.inspect({ exit = completed, timed_out = phase.timed_out, joined = joined })) then return false end
      phase.outputs = output_identity()
      local output_root = probe.path_key(ctx.project .. '/build/classes/java/main')
      local ready = vim.wait(180000, function()
        local current = state()
        local output_event = false
        for index = watch_start + 1, #watched_changes do
          local watch = watched_changes[index]
          for _, change in ipairs(watch.params.changes or {}) do
            local path = probe.path_key(vim.uri_to_fname(change.uri))
            if (path == output_root or path:sub(1, #output_root + 1) == output_root .. '/') and watch.before and watch.after
              and vim.deep_equal(watch.before.outputs, phase.outputs) and vim.deep_equal(watch.after.outputs, phase.outputs)
              and (current.source_begin_at or 0) > watch.before.trace_events
              and (current.sources_at or 0) > watch.before.trace_events
              and (current.accepted_at or 0) > watch.before.trace_events
              and (current.dependencies_at or 0) > watch.before.trace_events then
              output_event, phase.output_identity_watch = true, index
            end
          end
        end
        phase.requested, phase.accepted, phase.sources, phase.dependencies = current.requested, current.accepted, current.sources, current.dependencies
        phase.source_begin, phase.source_begin_at = current.source_begin, current.source_begin_at
        phase.actual_output_watch = output_event
        return output_event and current.requested ~= nil and current.requested > before
          and current.accepted and current.accepted.generation == current.requested
          and current.source_begin and current.source_begin.generation == current.requested
          and current.sources and current.sources.generation == current.requested
          and current.dependencies and current.dependencies.generation == current.requested
      end, 50)
      phase.watch_end, phase.ready = #watched_changes, ready
      if not available('external ' .. name .. ': real output watch and latest accepted model', ready,
        vim.inspect({ requested = phase.requested, accepted = phase.accepted, actual_output_watch = phase.actual_output_watch })) then return false end
      check('external ' .. name .. ': unsaved consumer survives build and refresh', vim.bo[caller.buf].modified and probe.buffer_full_text(caller.buf) == buffer_before)
      check('external ' .. name .. ': actual compiled outputs remain current', vim.deep_equal(output_identity(), phase.outputs))
      return true
    end
    local function useful(name, api, obsolete)
      phases[#phases].expected_api, phases[#phases].obsolete_api = api, obsolete
      edit_caller(api)
      local phase = phases[#phases]
      local current = state()
      phase.request_generation = current.requested
      if not available('external ' .. name .. ': request uses current ingested output generation',
        current.requested == phase.requested and current.accepted and current.accepted.generation == current.requested
          and vim.deep_equal(output_identity(), phase.outputs)) then return false end
      local anchor = getter_anchor:gsub('getName', api)
      local good
      if name == 'initial' then good = getter('initial-first-generated-completion', true, anchor)
      else good = completion('external ' .. name .. ': generated method', anchor, api, api:sub(1, 2), true) end
      authored('external ' .. name)
      if not good then return false end
      if obsolete then completion('external ' .. name .. ': obsolete method absent', anchor, api, '', false, obsolete) end
      local hover = rpc('external ' .. name .. ': generated signature', 'textDocument/hover', {
        textDocument = { uri = vim.uri_from_bufnr(caller.buf) }, position = position(consumer, anchor, api),
      })
      check('external ' .. name .. ': exact String signature', type(hover) == 'table' and type(hover.contents) == 'table'
        and hover.contents.value == '```java\n' .. api .. '(): String\n```', vim.inspect(hover))
      completion('external ' .. name .. ': String result chain', chain_anchor:gsub('getName', api), 'length', 'le', true)
      local after = state()
      check('external ' .. name .. ': output identity and generation preserved through first responses',
        after.requested == phase.request_generation and after.accepted and after.accepted.generation == after.requested
          and vim.deep_equal(output_identity(), phase.outputs))
      return true
    end
    edit_caller('getName')
    if not build('initial', function() end) or not useful('initial', 'getName') then return end
    if not build('config', function()
      write(config, original[config] .. 'lombok.accessors.fluent = true\n')
      write(consumer, (original[consumer]:gsub('getName', 'name')))
    end) or not useful('config', 'name', 'getName') then return end
    if not build('source', function()
      write(config, original[config])
      local changed, fields = original[person]:gsub('String name =', 'String title =')
      changed = changed:gsub('return name;', 'return title;')
      assert(fields == 1)
      write(person, changed); write(consumer, (original[consumer]:gsub('getName', 'getTitle')))
    end) or not useful('source', 'getTitle', 'name') then return end
    if not build('restored', function() for path, value in pairs(original) do write(path, value) end end) then return end
    useful('restored', 'getName', 'getTitle')
  end
  local function run()
    source(consumer); source(person)
    barrier('saved owners opened before compile')
    local commands = client.server_capabilities.executeCommandProvider
    check('standard executeCommand advertises ktlsp.compile', type(commands) == 'table'
      and vim.list_contains(commands.commands or {}, 'ktlsp.compile'))
    local registrations
    local function has_current_root_glob(values)
      for _, registration in ipairs(values or {}) do
        local watches = registration.registerOptions and registration.registerOptions.watchers or {}
        -- This pure main/test Java model has only in-project roots and module dependencies.
        -- Its final registration is the single root glob, unlike the two initial build globs.
        if #watches == 1 then
          local pattern = watches[1].globPattern
          if type(pattern) == 'string' then
            if pattern:gsub('\\', '/'):lower() == (ctx.project .. '/**/*'):gsub('\\', '/'):lower() then return true end
          elseif type(pattern) == 'table' and pattern.pattern == '**/*' then
            local base = type(pattern.baseUri) == 'table' and pattern.baseUri.uri or pattern.baseUri
            if type(base) == 'string' and probe.path_key(vim.uri_to_fname(base)) == probe.path_key(ctx.project) then return true end
          end
        end
      end
      return false
    end
    local ready = vim.wait(30000, function()
      registrations = client.dynamic_capabilities:get('workspace/didChangeWatchedFiles', { bufnr = source(consumer).buf })
      return has_current_root_glob(registrations)
    end, 20)
    check('current pure-Java model root-glob watch is registered before compile', ready, vim.inspect(registrations))
    append('compiled-java-watch-registration.jsonl', { registrations = registrations,
      boundary = 'client readiness only; executeCommand validates exact CURRENT model ACK on server' })
    if not ready then return end
    barrier('current model watch response delivered before compile')
    local compiled = compile('clean-initial')
    local useful = getter('initial-first-generated-completion', true)
    authored('clean-initial')
    if not compiled or not useful then return end
    local uri = vim.uri_from_bufnr(source(consumer).buf)
    local params = { textDocument = { uri = uri }, position = position(consumer, getter_anchor, 'getName') }
    local hover = rpc('compiled-getter-hover', 'textDocument/hover', params)
    check('compiled getter has exact String signature', type(hover) == 'table' and type(hover.contents) == 'table'
      and hover.contents.value == '```java\ngetName(): String\n```', vim.inspect(hover))
    completion('compiled-getter-String-result-chain', chain_anchor, 'length', 'le', true)
    local definition = rpc('source-less-getter-definition', 'textDocument/definition', params)
    check('source-less getter has no fabricated declaration target', definition == nil or definition == vim.NIL
      or type(definition) == 'table' and next(definition) == nil, vim.inspect(definition))
    local references = rpc('source-less-getter-references', 'textDocument/references',
      vim.tbl_extend('force', params, { context = { includeDeclaration = true } }))
    check('source-less getter has no fabricated source references', references == nil or references == vim.NIL
      or type(references) == 'table' and next(references) == nil, vim.inspect(references))
    local prepared = rpc('source-less-getter-prepareRename', 'textDocument/prepareRename', params)
    check('source-less getter rename is honestly unavailable', prepared == nil or prepared == vim.NIL, vim.inspect(prepared))

    configure('configuration-revision-change', true, 1)
    getter('configuration-change-retracts-receipt', false); authored('configuration-change')
    configure('provider-disabled', false, 2)
    if not compile('disabled-command-does-not-run', 'provider-disabled') then return end
    getter('disabled-provider-withholds-getter', false); authored('disabled-provider')
    configure('provider-enabled-again', true, 3)
    getter('re-enabled-without-build-keeps-receipt-retracted', false)
    if not compile('fresh-receipt-for-owner-edit') then return end
    if not getter('fresh-owner-phase-first-getter', true) then return end
    local original_person = table.concat(source(person).disk, '\n')
    local edited_person, replacements = original_person:gsub('String name = "generated"', 'String name = "unsaved"', 1)
    assert(replacements == 1)
    set(person, edited_person, 'owner-unsaved-edit')
    getter('owner-edit-retracts-getter', false); authored('owner-edit')
    if not compile('owner-dirty-command-does-not-run', 'unsaved-module-source') then return end
    restore(person, 'owner-A-B-A-restoration')
    getter('owner-A-B-A-does-not-revive-old-receipt', false)
    if not compile('fresh-receipt-for-nonretained-consumer') then return end
    if not getter('fresh-consumer-phase-first-getter', true) then return end
    local original_consumer = table.concat(source(consumer).disk, '\n')
    local partial, replacements = original_consumer:gsub('return person%.getName%(%)%;', 'return person.ge', 1)
    assert(replacements == 1)
    set(consumer, partial, 'nonretained-consumer-incomplete-edit')
    completion('incomplete-consumer-invalidates-whole-module-receipt',
      'public String read(Person person) { return person.ge }', 'ge', 'ge', false, 'getName')
    authored('incomplete-consumer')
    if not compile('incomplete-consumer-command-does-not-run', 'unsaved-module-source') then return end
    restore(consumer, 'consumer-A-B-A-restoration')
    getter('consumer-A-B-A-does-not-revive-old-receipt', false)
    if not compile('fresh-final-clean-receipt') then return end
    getter('fresh-final-first-getter', true); authored('fresh-final')
  end
  local ok, err = xpcall(external_build and run_external or run, debug.traceback)
  if external_restore then
    local restored, restore_error = pcall(external_restore)
    check('external build fixture restoration completed', restored, tostring(restore_error))
    if not restored then ok, err = false, restore_error end
  end
  for _, path in ipairs({ person, consumer }) do
    local value = source(path)
    if not vim.deep_equal(lines(path), value.disk) then restore(path, 'failure-cleanup-' .. vim.fs.basename(path)) end
    vim.bo[value.buf].modified = false
    check('disk golden and clean buffer preserved: ' .. path,
      vim.deep_equal(vim.fn.readfile(path), value.disk) and not vim.bo[value.buf].modified)
  end
  if not external_build or initial_getter_queries > 0 then
    check('initial getter queried exactly once', initial_getter_queries == 1, tostring(initial_getter_queries))
  end
  client.rpc.notify = original_notify
  local not_run = {}
  if external_build then
    local ran = {}
    for _, phase in ipairs(phases) do ran[phase.name] = true end
    for _, name in ipairs({ 'initial', 'config', 'source', 'restored' }) do if not ran[name] then not_run[#not_run + 1] = name end end
  end
  vim.fn.writefile({ vim.json.encode({ status = ok and 'completed' or 'probe-error', error = err,
    checks = checks, phases = phases, delivery = delivery, initial_getter_queries = initial_getter_queries,
    mode = external_build and 'external-Gradle-build' or 'explicit-LSP-compile-command',
    lsp_generation_commands = external_build and 0 or nil,
    native_compiles_maximum = external_build and 0 or 4, external_builds_planned = external_build and 4 or 0,
    external_builds_not_run = external_build and not_run or nil,
    watched_changes = external_build and watched_changes or nil, semantic_retry_attempts = 0,
    scope = external_build and 'Actual external Java/Lombok classes builds, live output ingestion and preserved unsaved caller; no LSP generation command'
      or 'clean pure Java only; original mixed/unsaved useful generated completion remains unresolved',
    configuration_control = external_build and 'actual lombok.config and source changes, external builds and native client watches; no artificial notification'
      or 'explicit standard didChangeConfiguration, not a native lombok.config watcher test',
    source_less_navigation = 'None expected, never a stand-in owner or fabricated source span',
    performance = 'correctness only; no timing acceptance' }) }, ctx.artifacts .. '/compiled-java-results.json')
  if not ok then error(err) end
end
