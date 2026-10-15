-- Existing corpus EXTRA hook: selected parent edits must retract and restore child facts.
return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, requests, summaries = env.client, {}, {}
  local parent = env.project .. '/project-repo/org/ktlsp/control/encoding-parent/1.0/encoding-parent-1.0.pom'
  local saved = vim.fn.readfile(parent, 'b')
  local original = table.concat(saved, '\n')
  assert(original:find('ISO-8859-1', 1, true), 'expected selected ISO parent')
  local properties = table.concat(vim.fn.readfile(env.project .. '/gradle.properties'), '\n')
  assert(properties:find('repositoryPolicy=project', 1, true), 'parent probe requires project policy')
  local buffer = assert(vim.fn.bufnr(env.project .. '/src/main/java/Use.java'))
  assert(buffer > 0, 'corpus source buffer is missing')
  local position = probe.find(vim.api.nvim_buf_get_lines(buffer, 0, -1, false), 'return value.café', 'café')
  local completion_position = { line = position.line, character = position.character }
  local oracle = env.project .. '/build/policy-fixture/oracle/project/Policy.java'
  local oracle_lines = vim.fn.readfile(oracle)
  local _, declaration = probe.find(oracle_lines, 'public int café', 'café')
  local oracle_hash = vim.fn.sha256(table.concat(oracle_lines, '\n'))
  local previous = client.handlers['window/logMessage']
  client.handlers['window/logMessage'] = function(err, message, ctx, config)
    if message and message.message and message.message:match('^ktlsp indexed %d+ library files') then
      summaries[#summaries + 1] = message.message
    end
    if previous then return previous(err, message, ctx, config) end
  end
  local function change_parent(lines, label)
    local before = #summaries
    assert(vim.fn.writefile(lines, parent, 'b') == 0)
    assert(client:notify('workspace/didChangeWatchedFiles', {
      changes = { { uri = vim.uri_from_fname(parent), type = 2 } },
    }))
    local ready = vim.wait(180000, function() return #summaries > before end, 50)
    env.check(label .. ': refreshed dependency generation', ready, nil, 'operational')
    assert(ready, 'dependency refresh did not finish')
  end
  local function request(name, method, at, accepts)
    local params = { textDocument = { uri = vim.uri_from_bufnr(buffer) }, position = at }
    local response, err = client:request_sync(method, params, 5000, buffer)
    local outcome = probe.request_outcome(response, err, accepts)
    local record = { case = name, method = method, attempts = 1, outcome = outcome }
    env.request_outcomes[#env.request_outcomes + 1] = record
    requests[#requests + 1] = { name = name, params = params, response = response, error = err, outcome = outcome }
    env.check(name, outcome == 'matched', outcome == 'matched' and nil or vim.inspect(response), 'semantic', outcome)
  end
  local source_uri
  local function exact_project_source(result)
    if not probe.exact_definition(result, oracle, { declaration }, 'org/ktlsp/control/Policy.java') then return false end
    local location = probe.locations(result)[1]
    local uri = location.uri or location.targetUri
    local path = vim.uri_to_fname(uri)
    if vim.fn.sha256(table.concat(vim.fn.readfile(path), '\n')) ~= oracle_hash then return false end
    source_uri = source_uri or uri
    return true
  end
  local ok, failure = xpcall(function()
    request('selected parent initial decoded source identity', 'textDocument/definition', position, exact_project_source)
    assert(source_uri, 'initial source declaration is unavailable')
    local changed, count = original:gsub('ISO%-8859%-1', 'UTF-8', 1)
    assert(count == 1)
    change_parent({ changed }, 'selected parent UTF-8 edit')
    request('parent edit retains binary known control', 'textDocument/completion', completion_position,
      function(result) return probe.completion_has(result, 'known') end)
    request('parent edit retracts the old decoded source declaration', 'textDocument/definition', position,
      function(result)
        for _, location in ipairs(probe.locations(result)) do
          if (location.uri or location.targetUri) == source_uri then return false end
        end
        return true
      end)
    change_parent(saved, 'selected parent ISO restore')
    request('parent restore recovers café and known', 'textDocument/completion', completion_position,
      function(result) return probe.completion_has(result, 'café') and probe.completion_has(result, 'known') end)
    request('restored parent navigation selects decoded project source', 'textDocument/definition', position, exact_project_source)
  end, debug.traceback)
  if table.concat(vim.fn.readfile(parent, 'b'), '\n') ~= original then
    local restored, restore_error = pcall(change_parent, saved, 'cleanup selected parent restore')
    if not restored then failure = tostring(failure) .. '\n' .. tostring(restore_error); ok = false end
  end
  client.handlers['window/logMessage'] = previous
  vim.fn.writefile({ vim.json.encode({ requests = requests, dependency_summaries = summaries,
    parent_restored = table.concat(vim.fn.readfile(parent, 'b'), '\n') == original }) },
    env.artifacts .. '/selected-parent-proof.json')
  assert(ok, failure)
end
