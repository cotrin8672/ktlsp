return function(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local files = { target = 'fixture/Target.java', alternate = 'fixture/SecondTarget.java', mixin = 'fixture/CaseMixin.java' }
  local buffers, paths, original, pending, outbound = {}, {}, {}, {}, {}
  local requests, snapshots = {}, {}
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then outbound[params.textDocument.uri] = params.textDocument.version end
    return sent
  end
  for key, file in pairs(files) do
    local path = vim.fs.abspath(env.project .. '/' .. file)
    local buf = vim.fn.bufadd(path)
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = 'java'
    paths[key], buffers[key], original[key] = path, buf, vim.fn.readfile(path)
    env.sources[path] = env.sources[path] or { buf = buf, disk = original[key], sha256 = vim.fn.sha256(table.concat(original[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
  local function set(key, value)
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, value)
    pending[buffers[key]] = true
  end
  local function replaced(key, before, after)
    return vim.split(table.concat(original[key], '\n'):gsub(before, after), '\n', { plain = true })
  end
  local function deliver(name)
    local expected = {}
    for buf in pairs(pending) do expected[vim.uri_from_bufnr(buf)] = vim.lsp.util.buf_versions[buf] end
    require('vim.lsp._changetracking').flush(client)
    for uri, version in pairs(expected) do check(name .. ': delivered change', outbound[uri] == version, uri) end
    pending = {}
  end
  local function request(name, key, anchor, token, method, extra, accepts)
    local position = probe.find(lines(key), anchor, token)
    if method == 'textDocument/completion' then position.character = position.character + #token end
    local params = vim.tbl_extend('force', { textDocument = { uri = vim.uri_from_bufnr(buffers[key]) }, position = position }, extra or {})
    deliver(name)
    local response, err = client:request_sync(method, params, 5000, buffers[key])
    local outcome = probe.request_outcome(response, err, accepts)
    local record = { case = name, method = method, attempts = 1, outcome = outcome }
    env.request_outcomes[#env.request_outcomes + 1] = record
    requests[#requests + 1] = { name = name, params = params, response = response, error = err, outcome = outcome }
    check(name, outcome == 'matched', outcome == 'matched' and nil or vim.inspect(response), 'semantic', outcome)
    return response and response.result
  end
  local function exact(name, key, anchor, token, target_token, destination)
    destination = destination or 'target'
    local _, range = probe.find(lines(destination), 'public int ' .. target_token, target_token)
    return request(name, key, anchor, token, 'textDocument/definition', nil,
      function(result) return probe.exact_definition(result, paths[destination], { range }) end)
  end
  local function no_target(name)
    request(name, 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'textDocument/definition', nil,
      function(result)
        for _, location in ipairs(probe.locations(result)) do
          if probe.path_key(vim.uri_to_fname(location.uri or location.targetUri)) == probe.path_key(paths.target) then return false end
        end
        return true
      end)
  end
  local function diagnostic(name, wanted)
    request(name, 'mixin', 'public abstract int', 'shadow$twice', 'textDocument/diagnostic', nil,
      function(result)
        if type(result) ~= 'table' or result.kind ~= 'full' or type(result.items) ~= 'table' then return false end
        local found = false
        for _, item in ipairs(result.items) do
          if item.message == 'External Shadow: target binding is incomplete' then found = true end
        end
        return found == wanted
      end)
  end
  local function configure(enabled, package)
    check('provider configuration delivered', client:notify('workspace/didChangeConfiguration', {
      settings = { analysisProviders = { ['fixture.external-shadow'] = {
        enabled = enabled, configuration = { mixinPackage = package or 'fixture' },
      } } },
    }))
  end
  request('prefixed completion uses shared target descriptor', 'mixin', 'public abstract int shadow$twice', 'shadow$twice',
    'textDocument/completion', { context = { triggerKind = 1 } }, function(result)
      for _, item in ipairs(result and result.items or result or {}) do
        if item.label == 'shadow$twice' and item.detail == 'fixture.Target (I)I' then
          local _, range = probe.find(lines('mixin'), 'public abstract int shadow$twice', 'twice')
          return item.textEdit and item.textEdit.newText == 'twice' and vim.deep_equal(item.textEdit.range, range)
        end
      end
      return false
    end)
  exact('shadow declaration navigates to actual target', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'twice')
  exact('ordinary shadow call shares target binding', 'mixin', 'return shadow$twice', 'shadow$twice', 'twice')
  diagnostic('valid shadow has no extension diagnostic', false)
  request('target references include exact prefixed declaration and call', 'target', 'public int twice', 'twice',
    'textDocument/references', { context = { includeDeclaration = true } }, function(result)
      local actual, expected = {}, {}
      for _, location in ipairs(probe.locations(result)) do actual[#actual + 1] = probe.location_key(location.uri, location.range) end
      for _, site in ipairs({ { 'target', 'public int twice', 'twice' }, { 'mixin', 'public abstract int shadow$twice', 'shadow$twice' }, { 'mixin', 'return shadow$twice', 'shadow$twice' } }) do
        local _, range = probe.find(lines(site[1]), site[2], site[3])
        expected[#expected + 1] = probe.location_key(vim.uri_from_bufnr(buffers[site[1]]), range)
      end
      table.sort(actual); table.sort(expected)
      return vim.deep_equal(actual, expected)
    end)
  local edit = request('target rename returns applicable edits', 'target', 'public int twice', 'twice',
    'textDocument/rename', { newName = 'doubled' }, function(result) return type(result) == 'table' and (result.changes or result.documentChanges) ~= nil end)
  local safe = type(edit) == 'table'
  local function known(uri)
    for key, path in pairs(paths) do if probe.path_key(vim.uri_to_fname(uri)) == probe.path_key(path) then return vim.uri_from_bufnr(buffers[key]) end end
  end
  if edit and edit.changes then
    local changes = {}
    for uri, edits in pairs(edit.changes) do local key = known(uri); safe = safe and key ~= nil; if key then changes[key] = edits end end
    edit.changes = changes
  end
  for _, change in ipairs(edit and edit.documentChanges or {}) do
    local uri = change.textDocument and known(change.textDocument.uri)
    safe = safe and uri ~= nil
    if uri then change.textDocument.uri = uri end
  end
  local applied = safe and pcall(vim.lsp.util.apply_workspace_edit, edit, client.offset_encoding)
  check('rename applies in Neovim', applied)
  for key in pairs(files) do
    local expected = key == 'alternate' and original[key] or replaced(key, 'twice', 'doubled')
    check('rename preserves prefix and exact whole ' .. key, vim.deep_equal(lines(key), expected))
    check('rename leaves disk unchanged ' .. key, vim.deep_equal(vim.fn.readfile(paths[key]), original[key]))
    snapshots[key] = lines(key)
    set(key, original[key])
  end
  deliver('rename restored')
  configure(false)
  no_target('disabled provider retracts target binding')
  diagnostic('disabled provider retracts diagnostics', false)
  configure(true, 'other')
  no_target('changed package configuration retracts binding')
  configure(true)
  exact('restored configuration restores binding', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'twice')
  set('mixin', replaced('mixin', '@Mixin%(Target%.class%)', '@Mixin(SecondTarget.class)'))
  exact('unsaved Mixin annotation switches exact target', 'mixin', 'public abstract int shadow$twice', 'shadow$twice', 'twice', 'alternate')
  set('mixin', original.mixin)
  set('target', replaced('target', 'twice', 'changed'))
  no_target('unsaved target rename retracts stale binding')
  diagnostic('unsaved missing target reports honest incomplete hint', true)
  set('mixin', replaced('mixin', 'twice', 'changed'))
  exact('unsaved shadow rename binds current target', 'mixin', 'public abstract int shadow$changed', 'shadow$changed', 'changed')
  for key in pairs(files) do set(key, original[key]); vim.bo[buffers[key]].modified = false end
  deliver('final restore')
  client.rpc.notify = notify
  vim.fn.writefile({ vim.json.encode({ requests = requests, renamed = snapshots, outbound = outbound }) }, env.artifacts .. '/external-shadow-proof.json')
end
