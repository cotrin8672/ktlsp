-- Shared editor boundary for the mixed fixture and real-project corpus scenarios.
local M = {}

function M.sanitize_environment()
  local removed = {}
  local suffixes = { 'TOKEN', 'API_KEY', 'SECRET', 'PASSWORD', 'PASSWD', 'ACCESS_KEY', 'PRIVATE_KEY' }
  for name in pairs(vim.fn.environ()) do
    local upper = name:upper()
    for _, suffix in ipairs(suffixes) do
      if upper == suffix or upper:sub(-#suffix - 1) == '_' .. suffix then
        -- Clear this disposable editor process too: removing a cmd_env override
        -- alone would still let its child inherit the original credential.
        vim.env[name] = nil
        removed[#removed + 1] = name
        break
      end
    end
  end
  table.sort(removed)
  return removed
end

function M.path_key(path)
  path = vim.fs.normalize(vim.uv.fs_realpath(path) or path)
  return vim.fn.has('win32') == 1 and path:lower() or path
end

function M.location_key(uri, range)
  if not uri or not range then return '?' end
  return ('%s:%d:%d-%d:%d'):format(M.path_key(vim.uri_to_fname(uri)),
    range.start.line, range.start.character, range['end'].line, range['end'].character)
end

function M.locations(result)
  if type(result) ~= 'table' then return {} end
  return (result.uri or result.targetUri) and { result } or result
end

function M.range_in_source(range, lines)
  local function inside(position)
    if type(position) ~= 'table' then return false end
    for _, key in ipairs({ 'line', 'character' }) do
      local value = position[key]
      if type(value) ~= 'number' or value < 0 or value ~= math.floor(value) then return false end
    end
    local line = lines[position.line + 1]
    return line ~= nil and position.character <= vim.str_utfindex(line, 'utf-16')
  end
  if type(range) ~= 'table' or not inside(range.start) or not inside(range['end']) then return false end
  return range.start.line < range['end'].line
    or (range.start.line == range['end'].line and range.start.character < range['end'].character)
end

function M.exact_definition(result, target, ranges, uri_suffix)
  local found = M.locations(result)
  if #found ~= 1 or type(found[1]) ~= 'table' then return false end
  local location = found[1]
  local uri = location.uri or location.targetUri
  local range = location.targetSelectionRange or location.range
  if type(uri) ~= 'string' or type(range) ~= 'table' then return false end
  if uri_suffix then
    -- Archive and extracted URIs must name the same qualified source file.
    local decoded = uri:gsub('%%(%x%x)', function(hex) return string.char(tonumber(hex, 16)) end)
    local suffix = '/' .. uri_suffix:gsub('\\', '/')
    if decoded:gsub('\\', '/'):sub(-#suffix) ~= suffix then return false end
  else
    local ok, path = pcall(vim.uri_to_fname, uri)
    if not ok or M.path_key(path) ~= M.path_key(target) then return false end
  end
  for _, expected in ipairs(ranges) do
    if vim.deep_equal(range, expected) then return true end
  end
  return false
end

function M.completion_has(result, token)
  if type(result) ~= 'table' then return false end
  for _, item in ipairs(result.items or result) do
    if item.label == token or item.label:match('^' .. token .. '%f[^%w_]') then return true end
  end
  return false
end

function M.find(lines, anchor, token)
  local position, range
  for index, line in ipairs(lines) do
    local start = line:find(anchor, 1, true)
    if start then
      assert(not position, 'ambiguous anchor ' .. anchor)
      local column = assert(line:find(token, start, true), 'missing token ' .. token .. ' after ' .. anchor) - 1
      position = { line = index - 1, character = vim.str_utfindex(line, 'utf-16', column) }
      range = { start = position,
        ['end'] = { line = index - 1, character = vim.str_utfindex(line, 'utf-16', column + #token) } }
    end
  end
  assert(position, 'missing anchor ' .. anchor)
  return position, range
end

function M.outside_project(project, artifacts)
  project, artifacts = M.path_key(project), M.path_key(artifacts)
  assert(artifacts ~= project and artifacts:sub(1, #project + 1) ~= project .. '/',
    'artifact directory must be outside the source project')
end

function M.config(project, bin, artifacts, build_tool)
  M.removed_environment = M.sanitize_environment()
  local mode = vim.env.KTLSP_PROBE_SERVER or vim.env.KTLSP_MIXED_SERVER or 'ktlsp'
  assert(mode == 'ktlsp' or mode == 'intellij', 'probe server must be ktlsp or intellij')
  if mode == 'ktlsp' then
    return { name = 'ktlsp', root_dir = project, cmd = { (assert(bin, 'missing ktlsp binary')) } }, mode
  end
  vim.opt.runtimepath:prepend(assert(vim.env.IJLS_PLUGIN, 'set IJLS_PLUGIN to intellij-lsp.nvim checkout'))
  local server_home = artifacts .. '/server-home'
  vim.fn.mkdir(server_home, 'p')
  require('intellij-lsp.progress').configure({ build_output = 'never' })
  return require('intellij-lsp.client').config(project, {
    server_path = assert(vim.env.IJLS_SERVER, 'set IJLS_SERVER to the IntelliJ server executable'),
    accept_eula = true, build_tool = build_tool,
    default_sdk = assert(vim.env.IJLS_JDK or vim.env.JAVA_HOME, 'set IJLS_JDK or JAVA_HOME'),
    jvm_args = { '-Duser.home=' .. server_home },
    autotrigger = false, inlay_hints = false, file_templates = false,
    run = false, references = false,
  }), mode
end

function M.import_ready(client, build_tool, timeout, check)
  local progress = require('intellij-lsp.progress')
  local ready = vim.wait(timeout, function() return progress.is_ready(client.id) end, 200)
  check('IntelliJ index ready', ready)
  local state = progress.import_state(client.id)
  local imported = state and state.folders and #state.folders > 0
  for _, folder in ipairs(state and state.folders or {}) do
    imported = imported and folder.status == 'SUCCESS'
  end
  check('IntelliJ ' .. build_tool:gsub('^%l', string.upper) .. ' import succeeds', imported, vim.inspect(state))
  return ready and imported, state
end

return M
