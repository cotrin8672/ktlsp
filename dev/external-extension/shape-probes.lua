return function(env)
  dofile(env.repo .. '/dev/external-extension/probes.lua')(env)
  local probe = dofile(env.repo .. '/dev/nvim_lsp_probe.lua')
  local client, check = env.client, env.check
  local files = { java = 'ShapeMixin.java', kotlin = 'ShapeKotlinMixin.kt', own = 'KotlinShapeMixin.kt', target = 'ShapeTarget.java', owntarget = 'KotlinShapeTarget.kt' }
  local buffers, paths, original, pending, outbound, requests = {}, {}, {}, {}, {}, {}
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'textDocument/didChange' then outbound[params.textDocument.uri] = params.textDocument.version end
    return sent
  end
  for key, file in pairs(files) do
    local path = vim.fs.abspath(env.project .. '/fixture/' .. file)
    local buf = vim.fn.bufadd(path)
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = file:match('%.kt$') and 'kotlin' or 'java'
    paths[key], buffers[key], original[key] = path, buf, vim.fn.readfile(path)
    env.sources[path] = env.sources[path] or { buf = buf, disk = original[key], sha256 = vim.fn.sha256(table.concat(original[key], '\n')) }
    vim.lsp.buf_attach_client(buf, client.id)
  end
  local function lines(key) return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false) end
  local function set(key, text)
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, vim.split(text, '\n', { plain = true }))
    pending[buffers[key]] = true
  end
  local function request(name, key, token, method, accepts)
    local position = probe.find(lines(key), token, token)
    if method == 'textDocument/completion' then position.character = position.character + #token end
    local params = { textDocument = { uri = vim.uri_from_bufnr(buffers[key]) }, position = position }
    if method == 'textDocument/completion' then params.context = { triggerKind = 1 } end
    local expected = {}
    for buf in pairs(pending) do expected[vim.uri_from_bufnr(buf)] = vim.lsp.util.buf_versions[buf] end
    require('vim.lsp._changetracking').flush(client)
    for uri, version in pairs(expected) do check('shape delivered change: ' .. name, outbound[uri] == version, uri) end
    pending = {}
    local response, err = client:request_sync(method, params, 5000, buffers[key])
    local outcome = probe.request_outcome(response, err, accepts)
    name = 'shape ' .. name
    env.request_outcomes[#env.request_outcomes + 1] = { case = name, method = method, attempts = 1, outcome = outcome }
    requests[#requests + 1] = { name = name, params = params, response = response, error = err, outcome = outcome }
    check(name, outcome == 'matched', outcome == 'matched' and nil or vim.inspect(response), 'semantic', outcome)
  end
  local function spelling(key, name) return key == 'java' and 'shadow$' .. name or name end
  local function target(key) return key == 'own' and 'owntarget' or 'target' end
  local function definition(label, key, name, wanted)
    local destination = target(key)
    local _, range = probe.find(lines(destination), name .. '(', name)
    request(label, key, spelling(key, name), 'textDocument/definition', function(result)
      if wanted then return probe.exact_definition(result, paths[destination], { range }) end
      for _, location in ipairs(probe.locations(result)) do
        if probe.path_key(vim.uri_to_fname(location.uri or location.targetUri)) == probe.path_key(paths[destination]) then return false end
      end
      return true
    end)
  end
  local function completion(label, key, name, descriptor, wanted)
    local owner = key == 'own' and 'KotlinShapeTarget' or 'ShapeTarget'
    request(label, key, spelling(key, name), 'textDocument/completion', function(result)
      local found = false
      for _, item in ipairs(result and result.items or result or {}) do
        if item.label == spelling(key, name) and item.detail == 'fixture.' .. owner .. ' ' .. descriptor then found = true end
      end
      return found == wanted
    end)
  end
  local function diagnostic(label, key, name, wanted)
    request(label, key, spelling(key, name), 'textDocument/diagnostic', function(result)
      if type(result) ~= 'table' or result.kind ~= 'full' or type(result.items) ~= 'table' then return false end
      local found = false
      for _, item in ipairs(result.items) do
        if item.message == 'External Shadow: target binding is incomplete' then found = true end
      end
      return found == wanted
    end)
  end
  local cases = {
    { 'java', 'axis', '(Lfixture/ShapeTarget$Axis;)Lfixture/ShapeTarget$Axis;' },
    { 'java', 'point', '(Lfixture/ShapeTarget$Point;)Lfixture/ShapeTarget$Point;' },
    { 'kotlin', 'axis', '(Lfixture/ShapeTarget$Axis;)Lfixture/ShapeTarget$Axis;' },
    { 'kotlin', 'point', '(Lfixture/ShapeTarget$Point;)Lfixture/ShapeTarget$Point;' },
    { 'kotlin', 'any', '(Ljava/lang/Object;)Ljava/lang/Object;' },
    { 'kotlin', 'text', '(Ljava/lang/String;)Ljava/lang/String;' },
    { 'own', 'axis', '(Lfixture/KotlinShapeTarget$Axis;)Lfixture/KotlinShapeTarget$Axis;' },
    { 'own', 'marker', '(Lfixture/KotlinShapeTarget$Marker;)Lfixture/KotlinShapeTarget$Marker;' },
  }
  for _, case in ipairs(cases) do
    local key, name, descriptor = unpack(case)
    definition(key .. ' exact ' .. name, key, name, true)
    completion(key .. ' descriptor ' .. name, key, name, descriptor, true)
  end
  for _, key in ipairs({ 'java', 'kotlin', 'own' }) do diagnostic(key .. ' valid bindings', key, 'axis', false) end
  set('target', table.concat(original.target, '\n'):gsub('enum Axis', 'enum MissingAxis'))
  for _, key in ipairs({ 'java', 'kotlin' }) do
    definition(key .. ' missing enum retracts binding', key, 'axis', false)
    diagnostic(key .. ' missing enum stays incomplete', key, 'axis', true)
  end
  set('target', table.concat(original.target, '\n'))
  for _, key in ipairs({ 'java', 'kotlin' }) do definition(key .. ' enum restored', key, 'axis', true) end
  set('owntarget', table.concat(original.owntarget, '\n'):gsub('object Marker', '@JvmInline value class Marker(val raw: Int)'))
  definition('value class retracts object binding', 'own', 'marker', false)
  diagnostic('value class stays incomplete', 'own', 'marker', true)
  set('owntarget', table.concat(original.owntarget, '\n'))
  definition('object restored', 'own', 'marker', true)
  check('shape provider disabled notification', client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { ['fixture.external-shadow'] = { enabled = false, configuration = {} } } } }))
  for _, case in ipairs({ cases[1], cases[3], cases[8] }) do
    local key, name, descriptor = unpack(case)
    definition(key .. ' disabled binding', key, name, false)
    completion(key .. ' disabled completion', key, name, descriptor, false)
    diagnostic(key .. ' disabled diagnostic', key, name, false)
  end
  check('shape provider reenabled notification', client:notify('workspace/didChangeConfiguration', { settings = { analysisProviders = { ['fixture.external-shadow'] = { enabled = true, configuration = {} } } } }))
  for _, case in ipairs({ cases[1], cases[3], cases[8] }) do definition(case[1] .. ' reenabled', case[1], case[2], true) end
  for key in pairs(files) do
    vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, original[key])
    vim.bo[buffers[key]].modified = false
  end
  require('vim.lsp._changetracking').flush(client)
  client.rpc.notify = notify
  vim.fn.writefile({ vim.json.encode({ requests = requests, outbound = outbound }) }, env.artifacts .. '/external-shape-proof.json')
end
