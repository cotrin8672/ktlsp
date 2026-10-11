-- The same semantic assertions run against ktlsp and intellij-lsp.nvim.
-- Use dev/ktlsp-harness.sh mixed; all projects, compiler outputs and caches are disposable.
local script_dir = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(script_dir .. '/nvim_lsp_probe.lua')
local project = assert(vim.uv.fs_realpath((assert(arg[1], 'missing project directory'))))
local artifacts = vim.fs.normalize((assert(arg[3], 'missing artifact directory')))
probe.outside_project(project, artifacts)
local mode = vim.env.KTLSP_PROBE_SERVER or vim.env.KTLSP_MIXED_SERVER or 'ktlsp'
assert(mode == 'ktlsp' or mode == 'intellij', 'KTLSP_MIXED_SERVER must be ktlsp or intellij')
vim.fn.mkdir(artifacts, 'p')
vim.opt.hidden = true
vim.cmd('filetype plugin indent on')
vim.lsp.set_log_level('debug')

local checks, failures, buffers = {}, {}, {}
local files = {
  java = 'src/main/java/mixed/JavaApi.java',
  kotlin = 'src/main/kotlin/mixed/KotlinApi.kt',
  box = 'src/main/java/mixed/JavaBox.java',
}
local disk = {}
local function check(name, ok, detail)
  checks[#checks + 1] = { name = name, ok = not not ok, detail = detail }
  print((ok and 'PASS ' or 'FAIL ') .. name .. (detail and (' — ' .. detail) or ''))
  if not ok then failures[#failures + 1] = name end
  return ok
end
local function write(path, lines)
  vim.fn.mkdir(vim.fs.dirname(path), 'p')
  vim.fn.writefile(lines, path)
end
local path_key, location_key = probe.path_key, probe.location_key
for name, relative in pairs(files) do
  local path = project .. '/' .. relative
  disk[name] = vim.fn.readfile(path)
  local buf = vim.fn.bufadd(path)
  vim.fn.bufload(buf)
  vim.bo[buf].filetype = name == 'kotlin' and 'kotlin' or 'java'
  buffers[name] = buf
end
local function buffer_uri(uri)
  local file = path_key(vim.uri_to_fname(uri))
  for _, buf in pairs(buffers) do
    if path_key(vim.api.nvim_buf_get_name(buf)) == file then return vim.uri_from_bufnr(buf) end
  end
end
local function lines(name)
  return vim.api.nvim_buf_get_lines(buffers[name], 0, -1, false)
end
local function find(name, anchor, token)
  return probe.find(lines(name), anchor, token)
end
local function expected_location(name, anchor, token)
  local _, range = find(name, anchor, token)
  return location_key(vim.uri_from_bufnr(buffers[name]), range)
end
local function before_or_equal(a, b)
  return a.line < b.line or a.line == b.line and a.character <= b.character
end
local function contains(outer, inner)
  return before_or_equal(outer.start, inner.start) and before_or_equal(inner['end'], outer['end'])
end
local function matching_site(uri, range, expected, edit)
  if not range then return nil end
  local file, match, count = path_key(vim.uri_to_fname(uri)), nil, 0
  for _, key in ipairs(expected) do
    local path, sl, sc, el, ec = key:match('^(.*):(%d+):(%d+)%-(%d+):(%d+)$')
    local site = { start = { line = tonumber(sl), character = tonumber(sc) },
      ['end'] = { line = tonumber(el), character = tonumber(ec) } }
    if file == path and (edit and contains(site, range) or not edit and contains(range, site)) then
      count = count + 1
      if edit or vim.deep_equal(range['end'], site['end']) then match = key end
    end
  end
  return count == 1 and match or nil
end

-- Compilation happens in copies, so neither the source fixture nor editor buffers are saved.
local compiler = vim.env.KTLSP_MIXED_MVN
  or vim.fn.exepath(vim.fn.has('win32') == 1 and 'mvn.cmd' or 'mvn')
if compiler ~= '' then
  local resolved = vim.fn.exepath(compiler)
  if resolved ~= '' then compiler = resolved end
end
local maven_repo = vim.fs.normalize(vim.env.KTLSP_MIXED_MAVEN_REPO or (artifacts .. '/m2'))
local maven_home = artifacts .. '/maven-home'
vim.fn.mkdir(maven_repo, 'p')
vim.fn.mkdir(maven_home, 'p')
local function jvm_option(name, value)
  local option = '-D' .. name .. '=' .. value
  return option:find('%s') and ('"' .. option .. '"') or option
end
vim.env.MAVEN_OPTS = (vim.env.MAVEN_OPTS or '')
  .. ' ' .. jvm_option('maven.repo.local', maven_repo) .. ' ' .. jvm_option('user.home', maven_home)
vim.env.MAVEN_USER_HOME = maven_home
local function compile_snapshot(name)
  local root = artifacts .. '/compiler-' .. name
  write(root .. '/pom.xml', vim.fn.readfile(project .. '/pom.xml'))
  for key, relative in pairs(files) do write(root .. '/' .. relative, lines(key)) end
  if compiler == '' then return check('compiler ' .. name, false, 'set KTLSP_MIXED_MVN or put Maven on PATH') end
  local command = { compiler, '-B', '-ntp', '-Dstyle.color=never', '-Dmaven.repo.local=' .. maven_repo, 'compile' }
  if vim.fn.has('win32') == 1 and compiler:lower():match('%.cmd$') then
    -- Windows CreateProcess cannot launch a batch file itself.
    command = vim.list_extend({ vim.env.COMSPEC or 'cmd.exe', '/d', '/c' }, command)
  end
  print('COMPILER ' .. name .. ' -> ' .. root)
  local ok, result = pcall(function()
    return vim.system(command, { cwd = root, text = true,
      timeout = tonumber(vim.env.KTLSP_MIXED_COMPILE_TIMEOUT_MS) or 180000 }):wait()
  end)
  local output = ok and ((result.stdout or '') .. (result.stderr or '')) or tostring(result)
  write(artifacts .. '/compiler-' .. name .. '.out', vim.split(output, '\n', { plain = true }))
  return check('compiler ' .. name, ok and result.code == 0,
    ok and ('exit=' .. result.code) or tostring(result))
end
local compiler_original = compile_snapshot('original')

vim.api.nvim_set_current_buf(buffers.kotlin)
if mode == 'intellij' then
  -- The server supplies its own MAVEN_OPTS to child imports. Keep their repository isolated too.
  local shim = artifacts .. '/maven-bin'
  if vim.fn.has('win32') == 1 then
    local function batch(value) return value:gsub('%%', '%%%%') end
    write(shim .. '/mvn.cmd', { '@echo off',
      'set "MAVEN_OPTS=%MAVEN_OPTS% ' .. batch(jvm_option('user.home', maven_home)) .. '"',
      'call "' .. batch(compiler) .. '" "-Dmaven.repo.local=' .. batch(maven_repo) .. '" %*',
      'exit /b %ERRORLEVEL%',
    })
    vim.env.PATH = shim .. ';' .. (vim.env.PATH or '')
  else
    local function shell(value) return "'" .. value:gsub("'", "'\\''") .. "'" end
    write(shim .. '/mvn', { '#!/bin/sh',
      'MAVEN_OPTS="$MAVEN_OPTS "' .. shell(jvm_option('user.home', maven_home)),
      'export MAVEN_OPTS',
      'exec ' .. shell(compiler) .. ' ' .. shell('-Dmaven.repo.local=' .. maven_repo) .. ' "$@"',
    })
    assert(vim.uv.fs_chmod(shim .. '/mvn', 493))
    vim.env.PATH = shim .. ':' .. (vim.env.PATH or '')
  end
end
local config = probe.config(project, vim.env.KTLSP_BIN or arg[2], artifacts, 'maven')
local shutdown_state = probe.observe_shutdown(config)
local id = vim.lsp.start(config, { bufnr = buffers.kotlin })
local client
local function finish()
  if client then
    local stopped = probe.shutdown(client, shutdown_state, 15000)
    check('client shuts down gracefully', stopped.clean, vim.inspect(stopped))
  end
  write(artifacts .. '/mixed-results.json', { vim.json.encode({ mode = mode, checks = checks,
    failures = failures, server = client and client.server_info, shutdown = shutdown_state, compiler = compiler,
    binary = vim.env.KTLSP_BIN or arg[2], plugin = vim.env.IJLS_PLUGIN,
    server_executable = vim.env.IJLS_SERVER, jdk = vim.env.IJLS_JDK or vim.env.JAVA_HOME,
    removed_environment_names = probe.removed_environment,
    project = project, maven_repository = maven_repo, neovim = vim.version() }) })
  print(#failures == 0 and '\nALL MIXED CHECKS PASSED' or ('\nFAILED: ' .. table.concat(failures, ', ')))
  vim.cmd(#failures == 0 and 'qa!' or 'cq!')
end
local initialized = vim.wait(120000, function()
  client = id and vim.lsp.get_client_by_id(id)
  return client and client.initialized
end, 100)
check('one client initializes', initialized, mode)
if not initialized then finish() end
for _, buf in pairs(buffers) do vim.lsp.buf_attach_client(buf, id) end
check('one client serves Java and Kotlin', vim.lsp.buf_is_attached(buffers.java, id)
  and vim.lsp.buf_is_attached(buffers.kotlin, id))
print('SERVER ' .. vim.inspect(client.server_info))
if mode == 'intellij' then
  if not probe.import_ready(client, 'maven', tonumber(vim.env.KTLSP_MIXED_READY_TIMEOUT_MS) or 240000, check) then finish() end
end

-- Observe the transport after the plugin's notification wrapper, before server processing.
local sent_versions, outbound_changes = {}, {}
local rpc_notify = client.rpc.notify
client.rpc.notify = function(method, params)
  local sent = rpc_notify(method, params)
  if sent and method == 'textDocument/didChange' then
    local document = params.textDocument
    sent_versions[document.uri] = document.version
    outbound_changes[#outbound_changes + 1] = { uri = document.uri, version = document.version }
  end
  return sent
end

-- Startup checks may retry indexing. Freshness checks below use exactly one response.
-- No request in this scenario is a timing sample.
local function request(name, method, anchor, token, extra, accepts, options)
  options = options or {}
  local position = find(name, anchor, token)
  local params = { textDocument = { uri = vim.uri_from_bufnr(buffers[name]) }, position = position }
  params = vim.tbl_extend('force', params, extra or {})
  local sent_before = vim.deepcopy(sent_versions)
  local result, last_error, attempts, first_result, first_error, first_matched
  attempts = 0
  local ready, deadline = false, vim.uv.hrtime() + 15000000000
  repeat
    attempts = attempts + 1
    local response, err = client:request_sync(method, params, 2500, buffers[name])
    result = response and response.result
    last_error = response and response.err or err
    write(artifacts .. '/last-response.json', { vim.json.encode({ method = method, params = params,
      result = result, error = last_error, attempts = attempts }) })
    ready = result ~= nil and (not accepts or accepts(result))
    if attempts == 1 then first_result, first_error, first_matched = result, last_error, ready end
    if not ready and not options.once then vim.wait(200) end
  until ready or options.once or vim.uv.hrtime() >= deadline
  print(('REQUEST %s %s attempts=%d first_matched=%s matched=%s'):format(
    name, method, attempts, tostring(first_matched), tostring(ready)))
  vim.fn.writefile({ vim.json.encode({ method = method, params = params, result = result,
    error = last_error, attempts = attempts, matched = ready,
    first = { result = first_result, error = first_error, matched = first_matched },
    phase = options.phase, sent_before = sent_before, sent_after = vim.deepcopy(sent_versions) }) },
    artifacts .. '/mixed-requests.jsonl', 'a')
  return result
end
local locations = probe.locations
local function definition(name, anchor, token, target, declaration, context, options)
  local expected = expected_location(target, declaration, token)
  local function matches(result)
    local found = locations(result)
    if #found ~= 1 then return false end
    local loc = found[1]
    return location_key(loc.uri or loc.targetUri, loc.targetSelectionRange or loc.range) == expected
  end
  local result = request(name, 'textDocument/definition', anchor, token, nil, matches, options)
  check(name .. ' ' .. (context or 'direct') .. ' goto ' .. token .. ' -> exact declaration',
    matches(result), vim.inspect(result))
end
local function completion(name, anchor, token, context, options)
  local function has_member(result)
    return probe.completion_has(result, token)
  end
  local result = request(name, 'textDocument/completion', anchor, token,
    { context = { triggerKind = 1 } }, has_member, options)
  if options and options.observe then
    print('OBSERVE ' .. name .. ' ' .. context .. ' completion offers ' .. token .. '=' .. tostring(has_member(result)))
  else
    check(name .. ' ' .. (context or 'direct') .. ' completion offers ' .. token, result and has_member(result))
  end
end
local function reference_set(name, declaration, token, expected)
  table.sort(expected)
  local function matches(result)
    local actual = {}
    for _, loc in ipairs(locations(result)) do
      local key = matching_site(loc.uri, loc.range, expected, false)
      if not key then return false end
      actual[#actual + 1] = key
    end
    table.sort(actual)
    return vim.deep_equal(actual, expected)
  end
  local result = request(name, 'textDocument/references', declaration, token,
    { context = { includeDeclaration = true } }, matches)
  check(name .. ' references to ' .. token .. ' are exact', matches(result), vim.inspect(result))
end

completion('java', 'return api.kotlinGreeting()', 'kotlinGreeting')
completion('kotlin', 'fun kotlinGreeting()', 'javaGreeting')
definition('java', 'return api.kotlinGreeting()', 'kotlinGreeting', 'kotlin', 'fun kotlinGreeting()')
definition('kotlin', 'fun kotlinGreeting()', 'javaGreeting', 'java', 'public String javaGreeting()')
completion('java', 'return box.value().kotlinGreeting()', 'kotlinGreeting', 'generic')
completion('kotlin', 'fun useBox(', 'javaGreeting', 'generic')
definition('java', 'return box.value().kotlinGreeting()', 'kotlinGreeting', 'kotlin', 'fun kotlinGreeting()', 'generic')
definition('kotlin', 'fun useBox(', 'javaGreeting', 'java', 'public String javaGreeting()', 'generic')

-- Both declarations and their cross-language uses exist only in the open buffers.
vim.api.nvim_buf_set_lines(buffers.java, -2, -2, false, {
  '', '    public String javaFresh() {', '        return "fresh java";', '    }',
  '', '    public String useFreshKotlin(KotlinApi api) {', '        return api.kotlinFresh();', '    }',
})
vim.api.nvim_buf_set_lines(buffers.kotlin, -2, -2, false, {
  '', '    fun kotlinFresh(): String = "fresh kotlin"',
  '', '    fun useFreshJava(): String = peer.javaFresh()',
})
check('Java and Kotlin edits remain unsaved', vim.bo[buffers.java].modified and vim.bo[buffers.kotlin].modified
  and vim.deep_equal(vim.fn.readfile(project .. '/' .. files.java), disk.java)
  and vim.deep_equal(vim.fn.readfile(project .. '/' .. files.kotlin), disk.kotlin))
local expected_versions = {}
for _, name in ipairs({ 'java', 'kotlin' }) do
  expected_versions[vim.uri_from_bufnr(buffers[name])] = vim.lsp.util.buf_versions[buffers[name]]
end
-- A normal request only flushes its own buffer. Keep that editing-UX observation separate.
completion('java', 'return api.kotlinFresh()', 'kotlinFresh', 'before all-buffer delivery',
  { once = true, observe = true, phase = 'before-all-buffer-delivery' })
-- Neovim's existing all-buffer flush is a test delivery barrier, not a server workaround.
-- It sends pending changes synchronously; there is no sleep or warm-up query here.
require('vim.lsp._changetracking').flush(client)
for _, name in ipairs({ 'java', 'kotlin' }) do
  local uri = vim.uri_from_bufnr(buffers[name])
  check(name .. ' current didChange is outbound before freshness queries',
    sent_versions[uri] == expected_versions[uri],
    ('expected=%s sent=%s'):format(expected_versions[uri], sent_versions[uri]))
end
write(artifacts .. '/mixed-didchange.json', { vim.json.encode({ expected = expected_versions,
  outbound = outbound_changes, sent = sent_versions, barrier = 'vim.lsp._changetracking.flush(client)' }) })
local first = { once = true, phase = 'first-response-after-all-buffer-delivery' }
completion('java', 'return api.kotlinFresh()', 'kotlinFresh', 'unsaved first response after delivery', first)
completion('kotlin', 'fun useFreshJava()', 'javaFresh', 'unsaved first response after delivery', first)
definition('java', 'return api.kotlinFresh()', 'kotlinFresh', 'kotlin', 'fun kotlinFresh()',
  'unsaved first response after delivery', first)
definition('kotlin', 'fun useFreshJava()', 'javaFresh', 'java', 'public String javaFresh()',
  'unsaved first response after delivery', first)
local compiler_edited = compile_snapshot('edited')

local function java_references(token)
  return {
    expected_location('java', 'public String ' .. token .. '()', token),
    expected_location('kotlin', 'fun kotlinGreeting()', token),
    expected_location('kotlin', 'fun useBox(', token),
  }
end
local function kotlin_references(token)
  return {
    expected_location('kotlin', 'fun ' .. token .. '()', token),
    expected_location('java', 'return api.' .. token .. '()', token),
    expected_location('java', 'return box.value().' .. token .. '()', token),
  }
end
reference_set('java', 'public String javaGreeting()', 'javaGreeting', java_references('javaGreeting'))
reference_set('kotlin', 'fun kotlinGreeting()', 'kotlinGreeting', kotlin_references('kotlinGreeting'))

local function rename(name, anchor, token, new_name, expected)
  local before, wanted = {}, {}
  for key in pairs(files) do
    before[key] = lines(key)
    wanted[key] = {}
    for _, line in ipairs(before[key]) do
      wanted[key][#wanted[key] + 1] = line:gsub('%f[%w_]' .. token .. '%f[^%w_]', new_name)
    end
  end
  local edit = request(name, 'textDocument/rename', anchor, token, { newName = new_name })
  if type(edit) ~= 'table' then edit = nil end
  local actual, seen, intervals, safe = {}, {}, {}, type(edit) == 'table'
  local function collect(uri, edits)
    safe = safe and buffer_uri(uri) ~= nil
    local file = path_key(vim.uri_to_fname(uri))
    intervals[file] = intervals[file] or {}
    for _, item in ipairs(edits) do
      local site = matching_site(uri, item.range, expected, true)
      safe = safe and site ~= nil and type(item.newText) == 'string'
      if site and not seen[site] then actual[#actual + 1], seen[site] = site, true end
      intervals[file][#intervals[file] + 1] = item.range
    end
  end
  for uri, edits in pairs(edit and edit.changes or {}) do collect(uri, edits) end
  for _, change in ipairs(edit and edit.documentChanges or {}) do
    if change.textDocument then collect(change.textDocument.uri, change.edits) else safe = false end
  end
  for _, ranges in pairs(intervals) do
    table.sort(ranges, function(a, b) return not before_or_equal(b.start, a.start) end)
    for i = 2, #ranges do
      safe = safe and before_or_equal(ranges[i - 1]['end'], ranges[i].start)
        and not vim.deep_equal(ranges[i - 1].start, ranges[i].start)
    end
  end
  table.sort(expected)
  table.sort(actual)
  local correct = check('rename ' .. token .. ' edits exact declarations and uses',
    safe and vim.deep_equal(actual, expected),
    vim.inspect(edit))
  local applied, err = false, 'no safe workspace edit'
  if safe then
    local normalized = vim.deepcopy(edit)
    if normalized.changes then
      local changes = {}
      for uri, edits in pairs(normalized.changes) do
        local canonical = buffer_uri(uri)
        changes[canonical] = vim.list_extend(changes[canonical] or {}, edits)
      end
      normalized.changes = changes
    end
    for _, change in ipairs(normalized.documentChanges or {}) do
      change.textDocument.uri = buffer_uri(change.textDocument.uri)
    end
    if mode == 'intellij' then
      -- Its ledger uses didOpen URIs as keys. Equivalent Windows drive spellings must share a key.
      require('intellij-lsp.versions').fix_workspace_edit(id, normalized)
    end
    write(artifacts .. '/rename-' .. token .. '-apply.json', { vim.json.encode(normalized) })
    applied, err = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding)
  end
  local detail
  if not applied then detail = tostring(err) end
  correct = check('rename ' .. token .. ' applies in Neovim', applied, detail) and correct
  for key in pairs(files) do
    correct = check('rename ' .. token .. ' preserves exact ' .. key .. ' contents',
      vim.deep_equal(lines(key), wanted[key])) and correct
  end
  return correct
end
local renamed_java = rename('kotlin', 'fun kotlinGreeting()', 'javaGreeting', 'javaHello', java_references('javaGreeting'))
local renamed_kotlin = rename('java', 'return api.kotlinGreeting()', 'kotlinGreeting', 'kotlinHello', kotlin_references('kotlinGreeting'))
local compiler_renamed = compile_snapshot('renamed')
if compiler_original and compiler_edited and compiler_renamed and renamed_java and renamed_kotlin then
  print('COMPILER verified original, unsaved, and applied-rename snapshots')
end
for key, relative in pairs(files) do
  check('source fixture stays unchanged: ' .. key, vim.deep_equal(vim.fn.readfile(project .. '/' .. relative), disk[key]))
end

finish()
