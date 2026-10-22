-- The member-role slice of the existing Java scenario, on its existing LSP connection.
return function(project, client, check)
  local script_dir = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
  local probe = dofile(script_dir .. '/nvim_lsp_probe.lua')
  local files = { reader = 'selection/Reader.java', payload = 'selection/Payload.java', unrelated = 'selection/Unrelated.java',
    java = 'selection/MemberUse.java', kotlin = 'selection/Callable.kt' }
  local buffers, original, notifications, sent = {}, {}, {}, {}
  local report = { requests = {}, diagnostics = {}, outbound = {} }
  local previous_buffer = vim.api.nvim_get_current_buf()
  local previous_handler = client.handlers['textDocument/publishDiagnostics']
  local diagnostic_handler = previous_handler or vim.lsp.handlers['textDocument/publishDiagnostics']
  client.handlers['textDocument/publishDiagnostics'] = function(err, result, ctx, config)
    if result then
      notifications[probe.path_key(vim.uri_to_fname(result.uri))] = result
      report.diagnostics[#report.diagnostics + 1] = vim.deepcopy(result)
    end
    diagnostic_handler(err, result, ctx, config)
  end
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local ok = notify(method, params)
    if ok and method == 'textDocument/didChange' then
      sent[params.textDocument.uri] = params.textDocument.version
      report.outbound[#report.outbound + 1] = vim.deepcopy(params.textDocument)
    end
    return ok
  end
  local function lines(name) return vim.api.nvim_buf_get_lines(buffers[name], 0, -1, false) end
  local function uri(name) return vim.uri_from_bufnr(buffers[name]) end
  local function site(name, anchor, token)
    local _, range = probe.find(lines(name), anchor, token)
    return probe.location_key(uri(name), range)
  end
  local function flush(name, phase)
    require('vim.lsp._changetracking').flush(client)
    if phase then
      local expected = vim.lsp.util.buf_versions[buffers[name]]
      check(phase .. ' change is delivered before first request', sent[uri(name)] == expected,
        ('expected=%s sent=%s'):format(expected, sent[uri(name)]))
    end
  end
  local function once(name, method, anchor, token, extra, position)
    local params = { textDocument = { uri = uri(name) }, position = position or probe.find(lines(name), anchor, token) }
    for key, value in pairs(extra or {}) do params[key] = value end
    local response, err = client:request_sync(method, params, 3000, buffers[name])
    local result = response and response.result
    report.requests[#report.requests + 1] = { method = method, params = params, attempts = 1,
      result = result, error = response and response.err or err, sent_versions = vim.deepcopy(sent) }
    return result
  end
  local function definition(name, anchor, token, target, declaration, target_token)
    local _, range = probe.find(lines(target), declaration, target_token or token)
    local result = once(name, 'textDocument/definition', anchor, token)
    check(name .. ' ' .. anchor .. ' goes to exact ' .. declaration,
      probe.exact_definition(result, project .. '/' .. files[target], { range }), vim.inspect(result))
  end
  local function partial(name, anchor, text, inspect)
    local position = probe.find(lines(name), anchor, anchor)
    local before = vim.api.nvim_buf_get_lines(buffers[name], position.line, position.line + 1, false)
    local edited = before[1]:match('^%s*') .. text
    vim.api.nvim_buf_set_lines(buffers[name], position.line, position.line + 1, false, { edited })
    flush(name, name .. ' ' .. text)
    local result = once(name, 'textDocument/completion', nil, nil, nil,
      { line = position.line, character = #edited })
    inspect(result)
    vim.api.nvim_buf_set_lines(buffers[name], position.line, position.line + 1, false, before)
    flush(name)
  end
  local function completion(name, anchor, text, expected, excluded)
    partial(name, anchor, text, function(result)
      check(name .. ' unfinished ' .. text .. ' offers ' .. expected .. ' and excludes ' .. excluded,
        probe.completion_has(result, expected) and not probe.completion_has(result, excluded), vim.inspect(result))
    end)
  end
  local function references(name, declaration, token, expected)
    local result = once(name, 'textDocument/references', declaration, token, { context = { includeDeclaration = true } })
    local found = {}
    for _, location in ipairs(probe.locations(result)) do
      found[#found + 1] = probe.location_key(location.uri or location.targetUri, location.targetSelectionRange or location.range)
    end
    table.sort(found)
    table.sort(expected)
    check('exact member references for ' .. declaration, vim.deep_equal(found, expected), vim.inspect(result))
  end
  local function restore()
    for name, buf in pairs(buffers) do vim.api.nvim_buf_set_lines(buf, 0, -1, false, original[name]) end
    flush('reader')
  end
  local function rename(name, declaration, token, new_name, expected, substitutions)
    local edit = once(name, 'textDocument/rename', declaration, token, { newName = new_name })
    local found, valid = {}, type(edit) == 'table'
    local function collect(document_uri, changes)
      for _, change in ipairs(changes or {}) do
        found[#found + 1] = probe.location_key(document_uri, change.range)
        valid = valid and change.newText == new_name
      end
    end
    if valid then
      for document_uri, changes in pairs(edit.changes or {}) do collect(document_uri, changes) end
      for _, change in ipairs(edit.documentChanges or {}) do
        if change.textDocument then collect(change.textDocument.uri, change.edits) else valid = false end
      end
    end
    table.sort(found)
    table.sort(expected)
    valid = valid and vim.deep_equal(found, expected)
    check('rename ' .. declaration .. ' edits exact selected sites', valid, vim.inspect(edit))
    if valid then
      local applied, err = pcall(vim.lsp.util.apply_workspace_edit, edit, client.offset_encoding)
      check('rename ' .. declaration .. ' applies in Neovim', applied, tostring(err))
      for key, before in pairs(original) do
        local wanted = vim.deepcopy(before)
        for i, line in ipairs(wanted) do
          for _, substitution in ipairs(substitutions[key] or {}) do line = line:gsub(substitution[1], substitution[2]) end
          wanted[i] = line
        end
        check('rename ' .. declaration .. ' preserves exact ' .. key .. ' buffer',
          vim.deep_equal(lines(key), wanted), vim.inspect(lines(key)))
      end
      flush(name)
    end
    restore()
  end
  local function run()
    for _, name in ipairs({ 'reader', 'payload', 'unrelated', 'java', 'kotlin' }) do
      local buf = vim.fn.bufadd(project .. '/' .. files[name])
      vim.fn.bufload(buf)
      vim.bo[buf].filetype = name == 'kotlin' and 'kotlin' or 'java'
      buffers[name], original[name] = buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false)
      assert(vim.lsp.buf_attach_client(buf, client.id), 'member fixture attach failed: ' .. name)
    end
    for _, name in ipairs({ 'java', 'kotlin' }) do
      local key = probe.path_key(project .. '/' .. files[name])
      local received = vim.wait(5000, function() return notifications[key] ~= nil end, 50)
      local result = notifications[key]
      check(name .. ' complete member source receives diagnostics', received, vim.inspect(result))
      check(name .. ' valid member calls have no false diagnostics', received and #result.diagnostics == 0, vim.inspect(result))
      definition(name, 'reader.body()', 'body', 'reader', 'public Payload body()')
      definition(name, 'value = reader.body', 'body', 'reader', 'public int body;')
      definition(name, 'Reader.Factory', 'Factory', 'reader', 'public static class Factory')
      partial(name, 'reader.body()', 'reader.bo', function(completed)
        local method, field = false, false
        for _, item in ipairs(type(completed) == 'table' and (completed.items or completed) or {}) do
          if item.label == 'body' or item.label:match('^body%f[^%w_]') then
            method = method or item.kind == 2 or item.kind == 3
            field = field or item.kind == 5 or item.kind == 10
          end
        end
        check(name .. ' bare completion retains both body method and field', method and field, vim.inspect(completed))
      end)
      completion(name, 'reader.body()', 'reader.body().us', 'useful', 'propertyOnly')
    end
    -- Inferred declarations must supply useful current types to both language consumers.
    definition('java', 'box.getValue()', 'getValue', 'kotlin', 'var value = input', 'value')
    definition('java', 'box.setValue(', 'setValue', 'kotlin', 'var value = input', 'value')
    definition('java', 'callables.getCallback()', 'getCallback', 'kotlin', 'val callback =', 'callback')
    definition('java', 'callables.inferredProduct()', 'inferredProduct', 'kotlin', 'fun inferredProduct()')
    completion('java', 'box.getValue()', 'box.getV', 'getValue', 'ping')
    completion('java', 'box.setValue(', 'box.setV', 'setValue', 'setInput')
    completion('java', 'callables.getCallback()', 'callables.getCall', 'getCallback', 'setCallback')
    for _, case in ipairs({
      { 'java', 'box.getValue()', 'box.getValue().pr', 'propertyOnly', 'methodOnly' },
      { 'java', 'callables.inferredProduct()', 'callables.inferredProduct().pr', 'propertyOnly', 'methodOnly' },
      { 'java', 'arrays.getValues()[0]', 'arrays.getValues()[0].pr', 'propertyOnly', 'methodOnly' },
      { 'java', 'arrays.getInts().length', 'arrays.getInts().le', 'length', 'propertyOnly' },
      { 'kotlin', 'box.value.propertyOnly()', 'box.value.pr', 'propertyOnly', 'methodOnly' },
      { 'kotlin', 'callables.inferredProduct()', 'callables.inferredProduct().pr', 'propertyOnly', 'methodOnly' },
      { 'kotlin', 'callables.callback(item)', 'callables.callback(item).pi', 'ping', 'propertyOnly' },
    }) do
      completion(unpack(case))
    end
    completion('java', 'String directName = Runnable.class.getName();', 'Runnable.class.getN', 'getName', 'run')
    for _, receiver in ipairs({ 'literalClass', 'primitiveClass', 'arrayClass' }) do
      completion('java', receiver .. '.getName()', receiver .. '.getN', 'getName', 'run')
    end
    local class_definition = once('java', 'textDocument/definition', 'literalClass.getName()', 'getName')
    local class_locations = probe.locations(class_definition)
    local exact_class_method = false
    if #class_locations == 1 then
      local target = vim.fs.normalize(vim.uri_to_fname(class_locations[1].uri or class_locations[1].targetUri))
      if target:match('/java/lang/Class%.java$') and vim.fn.filereadable(target) == 1 then
        local _, range = probe.find(vim.fn.readfile(target), 'public String getName()', 'getName')
        exact_class_method = probe.exact_definition(class_definition, target, { range })
      end
    end
    check('Java class literal local goes to the exact SDK Class.getName declaration',
      exact_class_method, vim.inspect(class_definition))
    partial('java', 'String arrayName = arrayClass.getName();', 'T.class.getN', function(result)
      local position = probe.find(lines('java'), 'String literalName = literalClass.getName();', 'getName')
      position.character = position.character + #'getN'
      local control = once('java', 'textDocument/completion', nil, nil, nil, position)
      check('Java invalid type-parameter class literal keeps a useful same-phase Class control',
        probe.completion_has(control, 'getName'), vim.inspect(control))
      check('Java invalid type-parameter class literal invents no Class method',
        not probe.completion_has(result, 'getName') and not probe.completion_has(result, 'run'), vim.inspect(result))
    end)
    local property_edit = lines('kotlin')
    for i, line in ipairs(property_edit) do
      property_edit[i] = line:gsub('var value = input', 'val value = input')
    end
    vim.api.nvim_buf_set_lines(buffers.kotlin, 0, -1, false, property_edit)
    flush('kotlin', 'unsaved inferred property mutability')
    partial('java', 'box.setValue(', 'box.', function(result)
      check('unsaved val withdraws inferred setter while keeping useful typed and inferred getters',
        probe.completion_has(result, 'getInput') and probe.completion_has(result, 'getValue')
          and not probe.completion_has(result, 'setValue'), vim.inspect(result))
    end)
    restore()
    completion('java', 'box.setValue(', 'box.setV', 'setValue', 'setInput')
    definition('java', 'box.setValue(', 'setValue', 'kotlin', 'var value = input', 'value')
    definition('kotlin', 'callables.action(1)', 'action', 'kotlin', 'val action:')
    definition('kotlin', 'callables.action("x")', 'action', 'kotlin', 'fun action(value: String)')
    definition('kotlin', 'callables.optional', 'optional', 'kotlin', 'val optional:')
    completion('kotlin', 'callables.action(1)', 'callables.action(1).pr', 'propertyOnly', 'methodOnly')
    completion('kotlin', 'callables.action("x")', 'callables.action("x").me', 'methodOnly', 'propertyOnly')
    completion('kotlin', 'callables.maybe(null)', 'callables.maybe(null)?.pr', 'propertyOnly', 'methodOnly')
    partial('kotlin', 'callables.action(1)', 'callables.action(missing).pr', function(result)
      check('unknown argument does not invent either result type',
        not probe.completion_has(result, 'propertyOnly') and not probe.completion_has(result, 'methodOnly'), vim.inspect(result))
    end)
    for _, case in ipairs({
      { 'callables.action(1)', 'action(p1: Int): Product' },
      { 'callables.action("x")', 'action(p1: String): OtherResult' },
      { 'callables.maybe(null)', 'maybe(p1: String?): Product?' },
    }) do
      local position = probe.find(lines('kotlin'), case[1], case[1])
      position.character = position.character + #case[1] - 1
      local result = once('kotlin', 'textDocument/signatureHelp', nil, nil, nil, position)
      check('signature follows selected ' .. case[1], type(result) == 'table'
        and #(result.signatures or {}) == 1 and result.signatures[1].label == case[2], vim.inspect(result))
    end
    local method_sites = { site('reader', 'public Payload body()', 'body'),
      site('java', 'reader.body()', 'body'), site('kotlin', 'reader.body()', 'body') }
    local field_sites = { site('reader', 'public int body;', 'body'),
      site('java', 'value = reader.body', 'body'), site('kotlin', 'value = reader.body', 'body') }
    local property_sites = { site('kotlin', 'val action:', 'action'), site('kotlin', 'callables.action(1)', 'action') }
    references('reader', 'public Payload body()', 'body', method_sites)
    references('reader', 'public int body;', 'body', field_sites)
    references('kotlin', 'val action:', 'action', property_sites)
    references('kotlin', 'fun action(value: String)', 'action', {
      site('kotlin', 'fun action(value: String)', 'action'), site('kotlin', 'callables.action("x")', 'action') })
    rename('reader', 'public Payload body()', 'body', 'readBody', method_sites, {
      reader = { { 'body%(', 'readBody(' } }, java = { { 'body%(', 'readBody(' } }, kotlin = { { 'body%(', 'readBody(' } } })
    rename('reader', 'public int body;', 'body', 'stored', field_sites, {
      reader = { { 'int body;', 'int stored;' } }, java = { { 'reader%.body;', 'reader.stored;' } },
      kotlin = { { 'reader%.body$', 'reader.stored' } } })
    rename('kotlin', 'val action:', 'action', 'transform', property_sites, { kotlin = {
      { 'val action:', 'val transform:' }, { 'callables%.action%(1%)', 'callables.transform(1)' } } })
    local edited = lines('kotlin')
    for i, line in ipairs(edited) do edited[i] = line:gsub('fun action%(value: String%)', 'fun action(value: Int)') end
    vim.api.nvim_buf_set_lines(buffers.kotlin, 0, -1, false, edited)
    flush('kotlin', 'unsaved callable signature')
    definition('kotlin', 'callables.action(1)', 'action', 'kotlin', 'fun action(value: Int)')
    completion('kotlin', 'callables.action(1)', 'callables.action(1).me', 'methodOnly', 'propertyOnly')
    restore()
    for name in pairs(files) do
      check('member probes keep ' .. name .. ' disk source unchanged',
        vim.deep_equal(vim.fn.readfile(project .. '/' .. files[name]), original[name]))
    end
  end
  local ok, err = xpcall(run, debug.traceback)
  client.rpc.notify = notify
  client.handlers['textDocument/publishDiagnostics'] = previous_handler
  vim.api.nvim_set_current_buf(previous_buffer)
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. '/member-selection.json')
  end
  assert(ok, err)
end
