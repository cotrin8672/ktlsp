-- This is an EXTRA_PROBES callback for the existing corpus client, never a second LSP client.
-- All semantic requests have exactly one attempt. Compiles have distinct lifecycle purposes.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local client, sources = ctx.client, ctx.sources
  local checks, phases, delivery, sent_versions = {}, {}, {}, {}
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
  end
  client.rpc.notify = function(method, params)
    local sent = original_notify(method, params)
    if sent and (method == 'textDocument/didChange' or method == 'textDocument/didOpen') then
      sent_versions[params.textDocument.uri] = params.textDocument.version
    end
    if sent and method == 'workspace/didChangeConfiguration' then config_serial = config_serial + 1 end
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
  local ok, err = xpcall(run, debug.traceback)
  for _, path in ipairs({ person, consumer }) do
    local value = source(path)
    if not vim.deep_equal(lines(path), value.disk) then restore(path, 'failure-cleanup-' .. vim.fs.basename(path)) end
    vim.bo[value.buf].modified = false
    check('disk golden and clean buffer preserved: ' .. path,
      vim.deep_equal(vim.fn.readfile(path), value.disk) and not vim.bo[value.buf].modified)
  end
  check('initial getter queried exactly once', initial_getter_queries == 1, tostring(initial_getter_queries))
  client.rpc.notify = original_notify
  vim.fn.writefile({ vim.json.encode({ status = ok and 'completed' or 'probe-error', error = err,
    checks = checks, phases = phases, delivery = delivery, initial_getter_queries = initial_getter_queries,
    native_compiles_maximum = 4, semantic_retry_attempts = 0,
    scope = 'clean pure Java only; original mixed/unsaved useful generated completion remains unresolved',
    configuration_control = 'explicit standard didChangeConfiguration, not a native lombok.config watcher test',
    source_less_navigation = 'None expected, never a stand-in owner or fabricated source span',
    performance = 'correctness only; no timing acceptance' }) }, ctx.artifacts .. '/compiled-java-results.json')
  if not ok then error(err) end
end
