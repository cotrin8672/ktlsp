-- Invoked only through KTLSP_GRADLE_RELOAD=1 dev/ktlsp-harness.sh gradle-live.
local dev = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h')
local repo = vim.fs.dirname(dev)
local fixture = dofile(dev .. '/gradle_reload_project.lua')
local probe = dofile(dev .. '/nvim_lsp_probe.lua')
local root, binary, artifacts = assert(arg[1]), assert(arg[2]), assert(arg[3])
root, artifacts = vim.fs.normalize(root), vim.fs.normalize(artifacts)
local automatic_watches = (vim.env.KTLSP_GRADLE_RELOAD_AUTOWATCH or '1') == '1'
local report = { checks = {}, phases = {}, messages = {}, requests = {}, engine = binary,
  watches = { mode = automatic_watches and 'automatic Neovim' or 'explicit notification control',
    registrations = {}, notifications = {} },
  established = { Java = {}, Kotlin = {} }, compiler_metadata = {},
  fixture = { gradle = '9.6.1', kotlin = '2.4.0', module = ':app', project_dir = 'modules/editing',
    artifact_semantics = 'compiled Java binary and published source JAR; no member claims from class-name stubs',
    generated_semantics = 'ordinary Gradle task; compiler-plugin generation is a separate acceptance case' } }
local function save()
  vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/gradle-reload.json')
end
local function check(name, passed, detail, observation)
  local item = { name = name, passed = passed == true, detail = detail }
  if observation then
    item.outcome = probe.request_outcome(observation.response, observation.error, function() return passed end, observation.attempted)
    item.measured = item.outcome == 'matched' or item.outcome == 'mismatch'
    item.observation, item.passed = observation, item.passed and item.measured
  end
  report.checks[#report.checks + 1] = item
  print((item.measured == false and 'UNMEASURED  ' or (item.passed and 'PASS  ' or 'FAIL  ')) .. name)
  save()
end

if not automatic_watches then
  local reason = 'canonical diagnostics require real client watch registration; set KTLSP_GRADLE_RELOAD_AUTOWATCH=1'
  report.precondition = { passed = false, reason = reason }
  save()
  print('PRECONDITION: ' .. reason)
  vim.cmd('cquit 1')
  return
end

-- Redirect the build in the native shell so a reused Gradle daemon cannot retain the editor's
-- capture pipe. Every build has a real exit status and a bounded wait.
local runner = artifacts .. '/gradle-build.ps1'
vim.fn.writefile(vim.split([=[param([string]$Project, [string]$Log)
$buildArgs = '/d /c call ' + [char]34 + (Join-Path $Project 'gradlew.bat') + [char]34 + ' fixtureJars :app:classes --offline --console=plain --max-workers=2 > ' + [char]34 + $Log + [char]34 + ' 2>&1'
$buildJob = Start-Process -FilePath $env:ComSpec -ArgumentList $buildArgs -WorkingDirectory $Project -WindowStyle Hidden -PassThru
$processHandle = $buildJob.Handle
if (-not $buildJob.WaitForExit(120000)) {
    & taskkill.exe /PID $buildJob.Id /T /F | Out-Null
    $buildJob.WaitForExit()
    exit 124
}
$buildExit = $buildJob.ExitCode
if ($null -eq $buildExit) { throw 'Gradle process exit code unavailable' }
exit $buildExit
]=], '\n', { plain = true }), runner)
local function build(phase)
  local log = artifacts .. '/gradle-phase' .. phase .. '-compile.out'
  local command
  if vim.fn.has('win32') == 1 then
    command = { 'powershell.exe', '-NoProfile', '-NonInteractive', '-File', runner, root, log }
  else
    command = { 'sh', '-c', 'exec "$1" fixtureJars :app:classes --offline --console=plain --max-workers=2 > "$2" 2>&1',
      'gradle-reload', root .. '/gradlew', log }
  end
  local start = vim.uv.hrtime()
  local result = vim.system(command, { cwd = root, text = true }):wait(125000)
  report.phases[#report.phases + 1] = { phase = phase, compile_exit = result.code,
    compile_signal = result.signal, elapsed_ms = (vim.uv.hrtime() - start) / 1e6,
    capture_stderr = result.stderr, log = log }
  check('phase ' .. phase .. ' actual offline Gradle Java/Kotlin compilation', result.code == 0 and result.signal == 0,
    result.stderr)
  return result.code == 0 and result.signal == 0
end

fixture.write(root, repo)
if not build(0) then vim.cmd('cquit 1'); return end

local module = root .. '/modules/editing'
local java_text = [[package app;
import vendor.Versioned;
import vendor.Added;
import generated.StandardGenerated;
import generated.CustomGenerated;

public class JavaEditor {
    void probe(Versioned versioned, Added added, StandardGenerated standard, CustomGenerated custom, KotlinHelper helper) {
        versioned.oldApi();
        versioned.newApi();
        added.addedApi();
        standard.generatedValue();
        custom.customValue();
        helper.assistUnsaved();
    }
}]]
local kotlin_text = [[package app
import vendor.Versioned
import vendor.Added
import generated.StandardGenerated
import generated.CustomGenerated

fun probe(versioned: Versioned, added: Added, standard: StandardGenerated, custom: CustomGenerated, helper: KotlinHelper) {
    versioned.oldApi()
    versioned.newApi()
    added.addedApi()
    standard.generatedValue()
    custom.customValue()
    helper.assistUnsaved()
}]]
local function open(relative, language, text)
  local buffer = vim.fn.bufadd(module .. '/' .. relative)
  vim.fn.bufload(buffer)
  vim.bo[buffer].filetype = language
  if text then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(text, '\n', { plain = true })) end
  return buffer
end
local java = open('src/main/java/app/JavaEditor.java', 'java', java_text)
local kotlin = open('src/main/kotlin/app/Editor.kt', 'kotlin', kotlin_text)
local helper = open('src/main/kotlin/app/KotlinHelper.kt', 'kotlin')
local main = open('src/main/java/app/Main.java', 'java')
local use = open('src/main/kotlin/app/Use.kt', 'kotlin')
local default_registration = vim.lsp.handlers['client/registerCapability']
local capabilities = vim.lsp.protocol.make_client_capabilities()
if not automatic_watches then
  capabilities.workspace.didChangeWatchedFiles.dynamicRegistration = false
end
local config = { name = 'ktlsp', root_dir = root, cmd = { binary },
  capabilities = capabilities,
  cmd_env = { HOME = root .. '/home', KTLSP_JDK_SRC = vim.env.KTLSP_JDK_SRC or root .. '/build/fake-jdk-src.zip' },
  handlers = {
    ['window/logMessage'] = function(_, message)
      report.messages[#report.messages + 1] = message
    end,
    ['client/registerCapability'] = function(error, params, context, config)
      local registration = { params = params, error = error, phase = report.current_phase or 0 }
      report.watches.registrations[#report.watches.registrations + 1] = registration
      local result = default_registration(error, params, context, config)
      registration.handled = true
      return result
    end,
  } }
local shutdown_state = probe.observe_shutdown(config)
local id = vim.lsp.start(config, { bufnr = java })
local initialized = vim.wait(10000, function()
  local client = vim.lsp.get_client_by_id(id)
  return client and client.initialized
end, 50)
check('one real Neovim LSP connection initialized', initialized)
local client = assert(vim.lsp.get_client_by_id(id))
report.watches.capability = vim.tbl_get(client.capabilities, 'workspace', 'didChangeWatchedFiles') or {}
local explicit_notification = false
local notify = client.rpc.notify
client.rpc.notify = function(method, params)
  local sent = notify(method, params)
  if method == 'workspace/didChangeWatchedFiles' then
    report.watches.notifications[#report.watches.notifications + 1] = {
      phase = report.current_phase or 0, explicit = explicit_notification, sent = sent, params = params,
    }
  end
  return sent
end
for _, buffer in ipairs({ kotlin, helper, main, use }) do vim.lsp.buf_attach_client(buffer, id) end
local function flush() vim.lsp._changetracking.flush(client) end
local function text(buffer) return vim.api.nvim_buf_get_lines(buffer, 0, -1, false) end
local function request(buffer, method, anchor, token, prefix)
  flush()
  local params = { textDocument = { uri = vim.uri_from_bufnr(buffer) } }
  if anchor then
    params.position = probe.find(text(buffer), anchor, token)
    if prefix then params.position.character = params.position.character + #prefix end
  end
  if method == 'textDocument/completion' then params.context = { triggerKind = 1 } end
  local reply, error = client:request_sync(method, params, 4000, buffer)
  local result = reply and reply.result
  report.requests[#report.requests + 1] = { phase = report.current_phase or 0, method = method,
    uri = params.textDocument.uri, position = params.position, result = result,
    error = error or (reply and reply.err) }
  return result
end
local function has(buffer, anchor, member)
  return probe.completion_has(request(buffer, 'textDocument/completion', anchor, member, member:sub(1, 3)), member)
end
local function go(buffer, anchor, member)
  return probe.locations(request(buffer, 'textDocument/definition', anchor, member))
end
local function exact_goto(buffer, anchor, member, suffix)
  local locations = go(buffer, anchor, member)
  if #locations ~= 1 then return false, locations end
  local location = locations[1]
  local uri, range = location.uri or location.targetUri, location.range or location.targetSelectionRange
  local path = uri and vim.uri_to_fname(uri)
  if not path or not vim.fs.normalize(path):lower():find(suffix:lower(), 1, true) then return false, locations end
  local lines = vim.fn.readfile(path)
  local _, expected = probe.find(lines, 'public int ' .. member .. '(', member)
  return vim.deep_equal(range, expected), locations
end
local function wait_for(condition, timeout)
  local next_check = 0
  return vim.wait(timeout, function()
    local now = vim.uv.hrtime()
    if now < next_check then return false end
    next_check = now + 250e6
    return condition()
  end, 50)
end
local function refresh_state(events)
  local requested, finished
  for _, event in ipairs(events) do
    if event.args and probe.path_key(event.args.root or '') == probe.path_key(root) then
      if event.name == 'workspace.refresh_requested' then requested = event.args.generation end
      if event.name == 'workspace.accepted_finish' then finished = event.args end
    end
  end
  return requested, finished
end
local function read_refresh_state()
  local events = {}
  local file = io.open(vim.env.KTLSP_TRACE or artifacts .. '/trace-events.jsonl', 'rb')
  if file then
    for line in file:lines() do
      local ok, event = pcall(vim.json.decode, line)
      if ok and (event.name == 'workspace.refresh_requested' or event.name == 'workspace.accepted_finish') then
        events[#events + 1] = event
      end
    end
    file:close()
  end
  return refresh_state(events)
end
local function refresh_complete(requested, finished, before)
  return requested ~= nil and requested > (before or 0) and finished ~= nil
    and finished.generation == requested and finished.failed == 0 and finished.missingSources == 0
    and finished.classpathComplete == true
end
assert(not refresh_complete(2, { generation = 1, failed = 0, missingSources = 0, classpathComplete = true }, 0))
assert(not refresh_complete(2, { generation = 2, failed = 0, missingSources = 0, classpathComplete = true }, 2))
assert(not refresh_complete(2, { generation = 2, failed = 1, missingSources = 0, classpathComplete = true }, 1))
assert(not refresh_complete(2, { generation = 2, failed = 0, missingSources = 0, classpathComplete = false }, 1))
assert(refresh_complete(2, { generation = 2, failed = 0, missingSources = 0, classpathComplete = true }, 1))
local function wait_refresh(before, timeout)
  local requested, finished
  local ready = wait_for(function()
    requested, finished = read_refresh_state()
    return refresh_complete(requested, finished, before)
  end, timeout)
  report.phases[#report.phases].refresh = { before = before, requested = requested, accepted_finish = finished, ready = ready }
  return ready
end
-- Read each published model after the existing useful v1/v2 requests, without extra readiness gates.
local function check_kotlin_metadata(phase, version)
  local capture = { matches = {}, read_errors = {}, model_exports = {}, jdk_selections = {},
    identity_limit = 'cache bytes and completed SDK-selection traces observed after phase requests; not an atomic model/request snapshot' }
  local cache = vim.env.KTLSP_CACHE_DIR
  local files = cache and vim.fn.globpath(cache .. '/classpath', '*.json', false, true) or {}
  table.sort(files)
  for _, path in ipairs(files) do
    local raw
    local ok, model = pcall(function()
      local file = assert(io.open(path, 'rb'))
      raw = file:read('*a')
      file:close()
      return vim.json.decode(raw)
    end)
    if ok and type(model) == 'table' then
      for _, item in ipairs(model.modules or {}) do
        if item.module == ':app' and type(item.project_dir) == 'string'
          and probe.path_key(item.project_dir) == probe.path_key(module) then
          local export = artifacts .. '/gradle-phase-' .. phase .. '-model-' .. (#capture.model_exports + 1) .. '.json'
          local output = assert(io.open(export, 'wb'))
          assert(output:write(raw))
          output:close()
          capture.model_exports[#capture.model_exports + 1] = { path = export, bytes = #raw, sha256 = vim.fn.sha256(raw) }
          for _, source_set in ipairs(item.source_sets or {}) do
            if source_set.name == 'main' then
              local java_home, java_task
              for _, task in ipairs(source_set.compiler_tasks or {}) do
                if task.task == ':app:compileJava' and task.language == 'java' and task.complete then
                  java_home = task.java_home
                  java_task = task
                end
              end
              for _, task in ipairs(source_set.compiler_tasks or {}) do
                if task.task == ':app:compileKotlin' and task.language == 'kotlin' then
                  capture.matches[#capture.matches + 1] = { cache_file = path,
                    model_version = model.model_version, module = item.module, build = item.build,
                    project_dir = item.project_dir, source_set = source_set.name, task = task,
                    kotlin_compile_task = source_set.kotlin_compile_task,
                    java_compile_home = java_home, java_compile_task = java_task }
                end
              end
            end
          end
        end
      end
    else
      capture.read_errors[#capture.read_errors + 1] = { path = path, error = tostring(model) }
    end
  end
  local trace = io.open(vim.env.KTLSP_TRACE or artifacts .. '/trace-events.jsonl', 'rb')
  if trace then
    for line in trace:lines() do
      local ok, event = pcall(vim.json.decode, line)
      if ok and event.name == 'deps.jdk_selection' then
        capture.jdk_selections[#capture.jdk_selections + 1] = { ts = event.ts, args = event.args }
      end
    end
    trace:close()
  end
  report.compiler_metadata[tostring(phase)] = capture
  local selected = #capture.matches == 1 and capture.matches[1] or nil
  local task = selected and selected.task or {}
  local prefix = 'phase ' .. phase .. ' Kotlin compiler metadata '
  check(prefix .. 'selects complete model11 :app/main public primary task', selected ~= nil
    and selected.model_version == 14 and selected.kotlin_compile_task == task.task
    and task.complete == true, capture)

  local sources, missing = {}, {}
  for _, path in ipairs(task.source_files or {}) do sources[probe.path_key(path)] = true end
  for _, path in ipairs({ 'src/main/kotlin/app/Use.kt', 'src/main/kotlin/app/KotlinHelper.kt',
    'src/main/java/app/Main.java' }) do
    if not sources[probe.path_key(module .. '/' .. path)] then missing[#missing + 1] = path end
  end
  check(prefix .. 'includes actual Kotlin and Java sources', #missing == 0, missing)

  local expected = probe.path_key(root .. '/home/.m2/repository/fixture/reload/versioned/'
    .. version .. '/versioned-' .. version .. '.jar')
  local versioned, stdlib, tooling = {}, {}, {}
  -- Preserve the configured classpath order; serialized arguments omit -classpath for this KGP.
  for index, path in ipairs(task.classpath or {}) do
    local name = vim.fs.basename(vim.fs.normalize(path))
    if name:match('^versioned%-.*%.jar$') then
      versioned[#versioned + 1] = { index = index, path = path }
    end
    if name:match('^kotlin%-stdlib[%w%.%-]*%.jar$') then stdlib[#stdlib + 1] = path end
    if name:match('^kotlin%-compiler') or name:match('^kotlin%-daemon')
      or name:match('^kotlin%-gradle%-plugin') or name:match('^kotlin%-build%-tools')
      or name:match('^kotlin%-scripting%-compiler') then tooling[#tooling + 1] = path end
  end
  check(prefix .. 'selects only Versioned ' .. version, #versioned == 1
    and probe.path_key(versioned[1].path) == expected, versioned)
  check(prefix .. 'includes stdlib without compiler tooling', #stdlib > 0 and #tooling == 0,
    { stdlib = stdlib, tooling = tooling })
  local expected_home = selected and selected.java_compile_home
  check(prefix .. 'uses the fixture configured Java toolchain', type(task.java_home) == 'string'
    and type(expected_home) == 'string' and probe.path_key(task.java_home) == probe.path_key(expected_home),
    { selected = task.java_home, expected_java_compile_home = expected_home })
end
local wait_ms = tonumber(vim.env.KTLSP_GRADLE_RELOAD_WAIT_MS) or 60000
local ready = wait_refresh(0, 120000)
check('initial dependency indexing reports completion', ready)
if automatic_watches then
  check('Neovim advertises automatic watched-file registration', report.watches.capability.dynamicRegistration)
  check('Neovim handles actual server watch registration', vim.iter(report.watches.registrations):any(function(item)
    return item.handled and not item.error and vim.iter(item.params.registrations):any(function(registration)
      return registration.method == 'workspace/didChangeWatchedFiles'
    end)
  end))
end
for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
  local useful = has(buffer, 'versioned.oldApi()', 'oldApi')
  check('initial ' .. label .. ' v1 member completion is useful', useful)
  local ok, locations = exact_goto(buffer, 'versioned.oldApi()', 'oldApi', '/fixture.reload/versioned/1.0/vendor/Versioned.java')
  check('initial ' .. label .. ' v1 goto exact source declaration', ok, locations)
  report.established[label].oldApi = { completion = useful, goto_definition = ok }
end
check_kotlin_metadata(0, '1.0')

-- The dirty helper and two editor probes never go to disk, including during Gradle builds.
vim.api.nvim_buf_set_lines(helper, 0, -1, false,
  vim.split(table.concat(text(helper), '\n'):gsub('assist', 'assistUnsaved'), '\n', { plain = true }))
flush()
local function dirty_checks(phase)
  check('phase ' .. phase .. ' Java sees preserved unsaved Kotlin member', has(java, 'helper.assistUnsaved()', 'assistUnsaved'))
  check('phase ' .. phase .. ' Kotlin sees preserved unsaved Kotlin member', has(kotlin, 'helper.assistUnsaved()', 'assistUnsaved'))
  check('phase ' .. phase .. ' helper disk remains compiler-valid original',
    table.concat(vim.fn.readfile(module .. '/src/main/kotlin/app/KotlinHelper.kt'), '\n'):find('fun assist()', 1, true) ~= nil)
end
dirty_checks(0)

local function watch(phase)
  local changes = {}
  for _, path in ipairs({ module .. '/build.gradle', root .. '/phase.txt',
    module .. '/src/main/java/app/Main.java', module .. '/src/main/kotlin/app/Use.kt' }) do
    changes[#changes + 1] = { uri = vim.uri_from_fname(path), type = 2 }
  end
  for _, path in ipairs({ module .. '/build/generated/sources/fixture/java/main/generated/StandardGenerated.java',
    module .. '/build/ktlsp-custom-java/generated/CustomGenerated.java' }) do
    changes[#changes + 1] = { uri = vim.uri_from_fname(path), type = phase == 3 and 3 or (phase == 1 and 1 or 2) }
  end
  if automatic_watches then
    local observed = wait_for(function()
      return vim.iter(report.watches.notifications):any(function(item)
        return item.phase == phase and not item.explicit and item.sent == true
      end)
    end, 10000)
    check('phase ' .. phase .. ' automatic standard watched-file notification sent on same client',
      client.id == id and observed)
    if phase == 1 then
      for _, suffix in ipairs({ '/build/generated/sources/fixture/java/main/generated/StandardGenerated.java',
        '/build/ktlsp-custom-java/generated/CustomGenerated.java' }) do
        local delivered = wait_for(function()
          return vim.iter(report.watches.notifications):any(function(item)
            return not item.explicit and item.sent == true and vim.iter(item.params.changes):any(function(change)
              return vim.fs.normalize(vim.uri_to_fname(change.uri)):sub(-#suffix) == suffix
            end)
          end)
        end, 10000)
        check('new generated root automatic notification: ' .. suffix, delivered)
      end
    end
  else
    explicit_notification = true
    local sent = client:notify('workspace/didChangeWatchedFiles', { changes = changes })
    explicit_notification = false
    check('phase ' .. phase .. ' standard watched-file notification sent on same client', client.id == id and sent)
  end
  for _, buffer in ipairs({ main, use }) do
    local path = vim.api.nvim_buf_get_name(buffer)
    local content = table.concat(vim.fn.readfile(path), '\n'):gsub('%.assist%(', '.assistUnsaved(')
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.split(content, '\n', { plain = true }))
  end
  flush()
end
local function clean_current_diagnostics(phase)
  for label, buffer in pairs({ Java = main, Kotlin = use }) do
    local result = request(buffer, 'textDocument/diagnostic')
    local valid = result and type(result.items) == 'table'
    for _, diagnostic in ipairs(result and result.items or {}) do
      if diagnostic.severity == 1 then valid = false end
    end
    check('phase ' .. phase .. ' current compiler-valid ' .. label .. ' buffer has no error diagnostics', valid, result)
  end
end

for phase = 1, 3 do
  report.current_phase = phase
  local before = read_refresh_state()
  fixture.phase(root, phase)
  if not build(phase) then break end
  watch(phase)
  report.phases[#report.phases].analysis_updated = wait_refresh(before, wait_ms)
  if phase == 1 then
    for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
      for _, item in ipairs({ { 'added.addedApi()', 'addedApi', '/fixture.reload/added/1.0/vendor/Added.java' },
        { 'standard.generatedValue()', 'generatedValue', '/build/generated/sources/fixture/java/main/generated/StandardGenerated.java' },
        { 'custom.customValue()', 'customValue', '/build/ktlsp-custom-java/generated/CustomGenerated.java' } }) do
        local useful = has(buffer, item[1], item[2])
        check('added ' .. label .. ' ' .. item[2] .. ' completion', useful)
        local ok, locations = exact_goto(buffer, item[1], item[2], item[3])
        check('added ' .. label .. ' ' .. item[2] .. ' exact goto', ok, locations)
        report.established[label][item[2]] = { completion = useful, goto_definition = ok }
      end
    end
  elseif phase == 2 then
    for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
      local useful = has(buffer, 'versioned.newApi()', 'newApi')
      check('updated ' .. label .. ' v2 new member completion', useful)
      check('updated ' .. label .. ' v1 member removed', report.established[label].oldApi.completion
        and not has(buffer, 'versioned.oldApi()', 'oldApi'))
      local ok, locations = exact_goto(buffer, 'versioned.newApi()', 'newApi', '/fixture.reload/versioned/2.0/vendor/Versioned.java')
      check('updated ' .. label .. ' goto exact v2 source declaration', ok, locations)
      report.established[label].newApi = { completion = useful, goto_definition = ok }
      local diagnostic, observation
      if label == 'Kotlin' then
        diagnostic, observation = probe.canonical_kotlin_diagnostics(client, buffer, {
          observation_timeout_ms = wait_ms, request_timeout_ms = 4000, compiler_version = '2.4.0',
          edit_label = 'one phase 2 comment in the unsaved fixture buffer; original sources stay on disk',
          mutate = function()
            vim.api.nvim_buf_set_lines(buffer, -1, -1, false, { '// canonical obsolete-member probe phase 2' })
          end,
          flush = flush,
        })
        report.requests[#report.requests + 1] = { phase = phase, method = observation.method,
          uri = observation.uri, result = diagnostic, error = observation.error, response = observation.response }
      else
        diagnostic = request(buffer, 'textDocument/diagnostic')
      end
      local position = probe.find(text(buffer), 'versioned.oldApi()', 'oldApi')
      local found = false
      for _, item in ipairs(diagnostic and diagnostic.items or {}) do
        if type(item) == 'table' and type(item.range) == 'table' and type(item.range.start) == 'table'
          and item.range.start.line == position.line and type(item.message) == 'string'
          and item.message:find('oldApi', 1, true) then found = true end
      end
      check('updated ' .. label .. ' stale member has a diagnostic', found, diagnostic, observation)
    end
    check_kotlin_metadata(2, '2.0')
  else
    for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
      for _, item in ipairs({ { 'versioned.oldApi()', 'oldApi' }, { 'versioned.newApi()', 'newApi' },
        { 'added.addedApi()', 'addedApi' }, { 'standard.generatedValue()', 'generatedValue' }, { 'custom.customValue()', 'customValue' } }) do
        local established = report.established[label][item[2]]
        local absent = not has(buffer, item[1], item[2])
        local locations = go(buffer, item[1], item[2])
        check('removed ' .. label .. ' ' .. item[2] .. ' has no stale completion', established.completion and absent,
          { previously_useful = established.completion, observed_absent = absent })
        check('removed ' .. label .. ' ' .. item[2] .. ' has no stale goto', established.goto_definition and #locations == 0,
          { previously_exact = established.goto_definition, locations = locations })
      end
    end
  end
  dirty_checks(phase)
  clean_current_diagnostics(phase)
  save()
end

-- Retire deliberately invalid editors before checking Java against complete Kotlin output.
report.current_phase = 'native-java'
for _, buffer in ipairs({ java, kotlin }) do
  vim.lsp.buf_detach_client(buffer, id)
  vim.api.nvim_buf_delete(buffer, { force = true })
end
for _, buffer in ipairs({ main, use, helper }) do
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, vim.fn.readfile(vim.api.nvim_buf_get_name(buffer)))
end
local helper_disk = table.concat(text(helper), '\n')
local valid_helper, replacements = helper_disk:gsub('fun assist%(%)%: Int = 1',
  'fun assist(): Int = 1\n    fun assistValidUnsaved(): Int = 42', 1)
assert(replacements == 1)
vim.api.nvim_buf_set_lines(helper, 0, -1, false, vim.split(valid_helper, '\n', { plain = true }))
local good = [[// 日本語😀 UTF-16 prefix
package app;
public class NativeJavacCertificate {
    public int probe(KotlinHelper helper) {
        int known = new String("x").length();
        int peer = helper.assistValidUnsaved();
        /* 日本語😀 */ return known + peer;
    }
}]]
local bad = good:gsub('return known %+ peer;', 'return known + peer + new String("x").missingJavacCertificateMethod();', 1)
local _, expected = probe.find(vim.split(bad, '\n', { plain = true }),
  'new String("x").missingJavacCertificateMethod()', 'missingJavacCertificateMethod')
assert(vim.deep_equal(expected, { start = { line = 6, character = 58 }, ['end'] = { line = 6, character = 87 } }))
local native_path = module .. '/src/main/java/app/NativeJavacCertificate.java'
assert(vim.fn.filereadable(native_path) == 0)
local native = open('src/main/java/app/NativeJavacCertificate.java', 'java', good)
assert(vim.lsp.buf_attach_client(native, id))
check('new Java sees valid unsaved Kotlin member', has(native, 'helper.assistValidUnsaved()', 'assistValidUnsaved'))
local notify_before, publish_before = client.rpc.notify, client.handlers['textDocument/publishDiagnostics']
local positive, observed = probe.canonical_kotlin_diagnostics(client, native, {
  observation_timeout_ms = wait_ms, request_timeout_ms = 4000, compiler_version = '2.4.0', require_complete = false,
  edit_label = 'one new unsaved Java body error with valid Kotlin sources',
  mutate = function() vim.api.nvim_buf_set_lines(native, 0, -1, false, vim.split(bad, '\n', { plain = true })) end,
  flush = flush,
})
report.requests[#report.requests + 1] = { phase = 'native-positive', method = observed.method,
  uri = observed.uri, result = positive, error = observed.error, response = observed.response }
report.native_java = { expected_range = expected, positive = { result = positive, observation = observed },
  whole_api_authority = false }
local errors = vim.tbl_filter(function(item) return item.severity == 1 end, positive and positive.items or {})
check('new Java one canonical Javac body error has exact Unicode token range', observed.canonical_certified
  and observed.accepted.count == 1 and #errors == 1 and errors[1].code ~= 'unresolved_reference'
  and errors[1].message:find('missingJavacCertificateMethod', 1, true) ~= nil
  and vim.deep_equal(errors[1].range, expected), positive, observed)
local _, finish = probe.observe_diagnostics(client, native)
vim.api.nvim_buf_set_lines(native, 0, -1, false, vim.split(good, '\n', { plain = true }))
flush()
local _, delivery = finish(wait_ms)
local delivered = delivery.error == nil and delivery.generation_certified and delivery.wire_matches_snapshot
local fixed
if delivered then fixed = request(native, 'textDocument/diagnostic') end
local function no_errors(items)
  return type(items) == 'table' and vim.iter(items):all(function(item) return item.severity ~= 1 end)
end
report.native_java.fixed = { delivery = delivery, result = fixed, canonical_certified = false, whole_api_authority = false }
check('fixed Java current-wire push and first standard pull remove the old error', delivered
  and no_errors(delivery.response.result.diagnostics)
  and fixed and fixed.kind == 'full' and no_errors(fixed.items), report.native_java.fixed)
check('native Java case restores observers and leaves both sources unwritten',
  client.rpc.notify == notify_before and client.handlers['textDocument/publishDiagnostics'] == publish_before
    and vim.fn.filereadable(native_path) == 0
    and table.concat(vim.fn.readfile(module .. '/src/main/kotlin/app/KotlinHelper.kt'), '\n') == helper_disk)

report.shutdown = probe.shutdown(client, shutdown_state, 10000)
check('server exits cleanly after the same-connection lifecycle', report.shutdown.clean, report.shutdown)
local failures = 0
for _, item in ipairs(report.checks) do if not item.passed then failures = failures + 1 end end
report.failed, report.total = failures, #report.checks
report.readiness_failures = 0
for _, phase in ipairs(report.phases) do
  if phase.refresh and not phase.refresh.ready then report.readiness_failures = report.readiness_failures + 1 end
end
save()
print(('Gradle reload: %d/%d checks passed'):format(#report.checks - failures, #report.checks))
vim.cmd(failures == 0 and report.readiness_failures == 0 and 'qa!' or 'cquit 1')
