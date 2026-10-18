return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local rows, outbound = {}, {}
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then
      outbound[params.textDocument.uri] = params.textDocument.version
    end
    return sent
  end
  local function request(name, source, anchor, token, method, accepts, after)
    local lines = vim.api.nvim_buf_get_lines(source.buf, 0, -1, false)
    local position = probe.find(lines, anchor, token)
    if after then position.character = position.character + #token end
    local params = { textDocument = { uri = vim.uri_from_bufnr(source.buf) }, position = position }
    if method == 'textDocument/completion' then params.context = { triggerKind = 1 } end
    local response, err = client:request_sync(method, params, 5000, source.buf)
    local outcome = probe.request_outcome(response, err, accepts)
    env.request_outcomes[#env.request_outcomes + 1] = { case = name, method = method, attempts = 1, outcome = outcome }
    rows[#rows + 1] = { name = name, params = params, response = response, error = err, outcome = outcome }
    check(name, outcome == 'matched', outcome == 'matched' and nil or vim.inspect(response), 'semantic', outcome)
  end
  local function edit(name, source, text)
    vim.api.nvim_buf_set_lines(source.buf, 0, -1, false, vim.split(text, '\n', { plain = true }))
    local uri, version = vim.uri_from_bufnr(source.buf), vim.lsp.util.buf_versions[source.buf]
    require('vim.lsp._changetracking').flush(client)
    check(name .. ': didChange delivered', outbound[uri] == version, vim.inspect({ expected = version, actual = outbound[uri] }))
  end
  local oracle = vim.fn.readfile(env.project .. '/oracle/Holder.stub.txt')
  local _, item_range = probe.find(oracle, ' ITEM;', 'ITEM')
  for _, language in ipairs({ 'java', 'kotlin' }) do
    local relative = language == 'java' and 'src/main/java/binarygenerics/JavaGenerics.java'
      or 'src/main/kotlin/binarygenerics/KotlinGenerics.kt'
    local path = vim.fs.abspath(env.project .. '/' .. relative)
    local source = assert(env.sources[path], 'manifest source missing: ' .. path)
    local original = table.concat(source.disk, '\n')
    local partial, count = original:gsub('Holder%.ITEM%.get%(%)%.ping%(%);?', 'Holder.ITEM.get().')
    assert(count == 1, 'static chain must be unique')
    local name = 'binary generics ' .. language
    edit(name .. ' incomplete chain', source, partial)
    request(name .. ': actual unfinished blank-prefix ping', source, 'Holder.ITEM.get().', 'Holder.ITEM.get().',
      'textDocument/completion', function(result)
        return probe.completion_has(result, 'ping') and not probe.completion_has(result, 'pong')
      end, true)
    request(name .. ': unfinished exact ITEM declaration', source, 'Holder.ITEM.get().', 'ITEM',
      'textDocument/definition', function(result)
        return probe.exact_definition(result, nil, { item_range }, 'genericfixture/Holder.java')
      end)
    request(name .. ': same-phase instance receiver stays useful', source, 'child.get().ping', 'ping',
      'textDocument/completion', function(result) return probe.completion_has(result, 'ping') end)
    if language == 'java' then
      local malformed, edits = partial:gsub('        Holder%.ITEM%.get%(%)%.', '        Object Holder = ;\n        Holder.ITEM.get().')
      assert(edits == 1)
      edit(name .. ' malformed local shadow', source, malformed)
      request(name .. ': malformed local does not prove a type receiver', source, 'Holder.ITEM.get().', 'Holder.ITEM.get().',
        'textDocument/completion', function(result) return not probe.completion_has(result, 'ping') end, true)
      request(name .. ': malformed-local known receiver stays useful', source, 'child.get().ping', 'ping',
        'textDocument/completion', function(result) return probe.completion_has(result, 'ping') end)
    end
    if language == 'java' then
      local unfinished, outer_edits = original:gsub('factory%.make%(%)%.outer%(%)%.special%(%);',
        'factory.make().outer().', 1)
      assert(outer_edits == 1)
      edit(name .. ' captured owner unfinished chain', source, unfinished)
      request(name .. ': unfinished owner keeps SpecialItem completion', source,
        'factory.make().outer().', 'factory.make().outer().', 'textDocument/completion',
        function(result)
          return probe.completion_has(result, 'special') and not probe.completion_has(result, 'pong')
        end, true)
      edit(name .. ' captured owner restore for navigation', source, original)
      local special_oracle = vim.fn.readfile(env.project .. '/oracle/SpecialItem.stub.txt')
      local _, special_range = probe.find(special_oracle, 'java.lang.String special()', 'special')
      request(name .. ': captured owner exact SpecialItem declaration', source,
        'factory.make().outer().special', 'special', 'textDocument/definition',
        function(result)
          return probe.exact_definition(result, nil, { special_range }, 'outerfixture/SpecialItem.java')
        end)
    end
    edit(name .. ' restore', source, original)
    vim.bo[source.buf].modified = false
    check(name .. ': source disk unchanged', vim.deep_equal(vim.fn.readfile(path), source.disk))
  end
  client.rpc.notify = notify
  vim.fn.writefile({ vim.json.encode(rows) }, env.artifacts .. '/binary-generic-requests.json')
end
