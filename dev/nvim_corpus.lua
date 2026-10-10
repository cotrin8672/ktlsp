-- Exact, read-only real-project probes through the existing editor harness.
-- nvim -l dev/nvim_corpus.lua <root> <manifest.json> <ktlsp-bin> <artifacts>
local script_dir = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(script_dir .. '/nvim_lsp_probe.lua')
local project = assert(vim.uv.fs_realpath((assert(arg[1], 'missing project root'))))
local manifest_path = assert(arg[2], 'missing probe manifest')
local artifacts = vim.fs.normalize((assert(arg[4], 'missing artifact directory')))
probe.outside_project(project, artifacts)
local manifest = vim.json.decode(table.concat(vim.fn.readfile(manifest_path), '\n'))
assert(type(manifest.probes) == 'table' and #manifest.probes > 0, 'manifest needs at least one probe')
vim.fn.mkdir(artifacts, 'p')
vim.opt.hidden = true
vim.cmd('filetype plugin indent on')
vim.lsp.set_log_level('debug')

local checks, failures, sources, prepared = {}, {}, {}, {}
local function check(name, ok, detail)
  checks[#checks + 1] = { name = name, ok = not not ok, detail = detail }
  print((ok and 'PASS ' or 'FAIL ') .. name .. (detail and (' — ' .. detail) or ''))
  if not ok then failures[#failures + 1] = name end
  return ok
end
local function write(name, value)
  vim.fn.writefile({ vim.json.encode(value) }, artifacts .. '/' .. name)
end
local function full_path(path)
  if not path:match('^[/\\]') and not path:match('^%a:[/\\]') then path = project .. '/' .. path end
  return vim.fs.abspath(path)
end
-- A target is read as oracle data, not opened/indexed merely to make a result available.
for _, case in ipairs(manifest.probes) do
  local path = full_path(assert(case.source, 'probe needs source'))
  if not sources[path] then
    local disk = vim.fn.readfile(path)
    local buf = vim.fn.bufadd(path)
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = path:match('%.java$') and 'java' or 'kotlin'
    sources[path] = { buf = buf, disk = disk, sha256 = vim.fn.sha256(table.concat(disk, '\n')) }
  end
  local source = sources[path]
  local position = probe.find(source.disk, assert(case.anchor), assert(case.token))
  local target = full_path(assert(case.target_file or case.target, 'probe needs target'))
  local _, expected_range = probe.find(vim.fn.readfile(target), assert(case.target_anchor),
    case.target_token or case.token)
  local expected = probe.location_key(vim.uri_from_fname(target), expected_range)
  prepared[#prepared + 1] = { case = case, buf = source.buf, position = position,
    expected = expected, range = expected_range, target = target }
end

local function git_snapshot()
  local function run(args)
    local command = vim.list_extend({ 'git', '-C', project }, args)
    local result = vim.system(command, { text = true, timeout = 15000 }):wait()
    return { code = result.code, stdout = result.stdout, stderr = result.stderr }
  end
  return { revision = run({ 'rev-parse', 'HEAD' }), status = run({ 'status', '--porcelain', '--untracked-files=no' }) }
end
local before_git = git_snapshot()
local config, mode = probe.config(project, vim.env.KTLSP_BIN or arg[3], artifacts, manifest.build_tool or 'gradle')
vim.api.nvim_set_current_buf(prepared[1].buf)
local id = vim.lsp.start(config, { bufnr = prepared[1].buf })
local client, import_state
local function finish()
  if client then
    client:stop()
    check('client shuts down gracefully', vim.wait(15000, function() return client:is_stopped() end, 100))
    if not client:is_stopped() then client:stop(true) end
  end
  for path, source in pairs(sources) do
    check('source remains unchanged: ' .. path, vim.deep_equal(vim.fn.readfile(path), source.disk)
      and not vim.bo[source.buf].modified)
  end
  local after_git = git_snapshot()
  if before_git.status.code == 0 and after_git.status.code == 0 then
    check('tracked working-tree status remains unchanged', before_git.status.stdout == after_git.status.stdout)
  end
  local hashes = {}
  for path, source in pairs(sources) do hashes[path] = source.sha256 end
  write('corpus-results.json', { mode = mode, project = project, manifest = manifest,
    manifest_path = manifest_path, checks = checks, failures = failures, source_sha256 = hashes,
    server = client and client.server_info, binary = vim.env.KTLSP_BIN or arg[3],
    plugin = vim.env.IJLS_PLUGIN, server_executable = vim.env.IJLS_SERVER,
    jdk = vim.env.IJLS_JDK or vim.env.JAVA_HOME, gradle_user_home = vim.env.GRADLE_USER_HOME,
    import_state = import_state, git_before = before_git, git_after = after_git, neovim = vim.version(),
    performance = 'bounded indexing retries; not timing or freshness acceptance' })
  print(#failures == 0 and '\nALL CORPUS CHECKS PASSED' or ('\nFAILED: ' .. table.concat(failures, ', ')))
  vim.cmd(#failures == 0 and 'qa!' or 'cq!')
end
local initialized = vim.wait(120000, function()
  client = id and vim.lsp.get_client_by_id(id)
  return client and client.initialized
end, 100)
check('one client initializes', initialized, mode)
if not initialized then finish() end
for _, source in pairs(sources) do vim.lsp.buf_attach_client(source.buf, id) end
print('SERVER ' .. vim.inspect(client.server_info))
if mode == 'intellij' then
  local imported
  imported, import_state = probe.import_ready(client, manifest.build_tool or 'gradle',
    tonumber(vim.env.KTLSP_CORPUS_READY_TIMEOUT_MS) or 600000, check)
  if not imported then finish() end
end

local function request(prep, method, accepts)
  local params = { textDocument = { uri = vim.uri_from_bufnr(prep.buf) }, position = prep.position }
  if method == 'textDocument/completion' then params.context = { triggerKind = 1 } end
  local deadline = vim.uv.hrtime() + (tonumber(vim.env.KTLSP_CORPUS_REQUEST_TIMEOUT_MS) or 15000) * 1000000
  local attempts, result, error, matched, first = 0, nil, nil, false, nil
  repeat
    attempts = attempts + 1
    local response, err = client:request_sync(method, params, 5000, prep.buf)
    result, error = response and response.result, response and response.err or err
    matched = result ~= nil and accepts(result)
    if not first then first = { result = result, error = error, matched = matched } end
    if not matched then vim.wait(200) end
  until matched or vim.uv.hrtime() >= deadline
  vim.fn.writefile({ vim.json.encode({ case = prep.case.id, method = method, params = params,
    attempts = attempts, matched = matched, result = result, error = error, first = first }) },
    artifacts .. '/corpus-requests.jsonl', 'a')
  print(('REQUEST %s %s attempts=%d first_matched=%s matched=%s'):format(
    prep.case.id, method, attempts, tostring(first.matched), tostring(matched)))
  return matched, result
end

local function exact_definition(prep, result)
  local found = probe.locations(result)
  if #found ~= 1 then return false end
  local loc = found[1]
  local uri, range = loc.uri or loc.targetUri, loc.targetSelectionRange or loc.range
  if prep.case.target_uri_suffix then
    -- A dependency may be exposed as a jar URI or an extracted file in either engine.
    -- Its qualified source path AND exact declaration range must match the verified archive source.
    local decoded = uri and uri:gsub('%%(%x%x)', function(hex) return string.char(tonumber(hex, 16)) end)
    local suffix = '/' .. prep.case.target_uri_suffix:gsub('\\', '/')
    return decoded and decoded:gsub('\\', '/'):sub(-#suffix) == suffix and vim.deep_equal(range, prep.range)
  end
  return probe.location_key(uri, range) == prep.expected
end
for _, prep in ipairs(prepared) do
  if prep.case.completion ~= false then
    local token = prep.case.completion_token or prep.case.token
    local matched = request(prep, 'textDocument/completion', function(result)
      return probe.completion_has(result, token)
    end)
    check(prep.case.id .. ': completion offers ' .. token, matched)
  end
  local matched, result = request(prep, 'textDocument/definition', function(value)
    return exact_definition(prep, value)
  end)
  check(prep.case.id .. ': goto exact declaration', matched, matched and nil or vim.inspect(result))
end
finish()
