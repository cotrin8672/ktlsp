-- Invoked only through KTLSP_GRADLE_RELOAD=1 dev/ktlsp-harness.sh gradle-live.
local dev = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h')
local repo = vim.fs.dirname(dev)
local fixture = dofile(dev .. '/gradle_reload_project.lua')
local probe = dofile(dev .. '/nvim_lsp_probe.lua')
local root, binary, artifacts = assert(arg[1]), assert(arg[2]), assert(arg[3])
root, artifacts = vim.fs.normalize(root), vim.fs.normalize(artifacts)
local report = { checks = {}, phases = {}, messages = {}, requests = {}, engine = binary,
  established = { Java = {}, Kotlin = {} },
  fixture = { gradle = '9.6.1', kotlin = '2.4.0', module = ':app', project_dir = 'modules/editing',
    artifact_semantics = 'compiled Java binary and published source JAR; no member claims from class-name stubs',
    generated_semantics = 'ordinary Gradle task; compiler-plugin generation is a separate acceptance case' } }
local function save()
  vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/gradle-reload.json')
end
local function check(name, passed, detail)
  report.checks[#report.checks + 1] = { name = name, passed = passed == true, detail = detail }
  print((passed and 'PASS  ' or 'FAIL  ') .. name)
  save()
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
local exit
local id = vim.lsp.start({ name = 'ktlsp', root_dir = root, cmd = { binary },
  cmd_env = { HOME = root .. '/home', KTLSP_JDK_SRC = root .. '/build/fake-jdk-src.zip' },
  handlers = { ['window/logMessage'] = function(_, message)
    report.messages[#report.messages + 1] = message
  end }, on_exit = function(code, signal) exit = { code = code, signal = signal } end }, { bufnr = java })
local initialized = vim.wait(10000, function()
  local client = vim.lsp.get_client_by_id(id)
  return client and client.initialized
end, 50)
check('one real Neovim LSP connection initialized', initialized)
local client = assert(vim.lsp.get_client_by_id(id))
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
local wait_ms = tonumber(vim.env.KTLSP_GRADLE_RELOAD_WAIT_MS) or 60000
local ready = wait_for(function()
  for _, message in ipairs(report.messages) do
    if message.message:match('ktlsp indexed .* library files') then return true end
  end
  return false
end, 120000)
check('initial dependency indexing reports completion', ready)
for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
  local useful = has(buffer, 'versioned.oldApi()', 'oldApi')
  check('initial ' .. label .. ' v1 member completion is useful', useful)
  local ok, locations = exact_goto(buffer, 'versioned.oldApi()', 'oldApi', '/fixture.reload/versioned/1.0/vendor/Versioned.java')
  check('initial ' .. label .. ' v1 goto exact source declaration', ok, locations)
  report.established[label].oldApi = { completion = useful, goto_definition = ok }
end

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
  check('phase ' .. phase .. ' standard watched-file notification sent on same client',
    client.id == id and client:notify('workspace/didChangeWatchedFiles', { changes = changes }))
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
  fixture.phase(root, phase)
  if not build(phase) then break end
  watch(phase)
  if phase == 1 then
    report.phases[#report.phases].analysis_updated = wait_for(function()
      return has(java, 'added.addedApi()', 'addedApi') and has(kotlin, 'added.addedApi()', 'addedApi')
        and has(java, 'standard.generatedValue()', 'generatedValue') and has(kotlin, 'custom.customValue()', 'customValue')
    end, wait_ms)
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
    report.phases[#report.phases].analysis_updated = wait_for(function()
      return has(java, 'versioned.newApi()', 'newApi') and not has(java, 'versioned.oldApi()', 'oldApi')
        and has(kotlin, 'versioned.newApi()', 'newApi') and not has(kotlin, 'versioned.oldApi()', 'oldApi')
    end, wait_ms)
    for label, buffer in pairs({ Java = java, Kotlin = kotlin }) do
      local useful = has(buffer, 'versioned.newApi()', 'newApi')
      check('updated ' .. label .. ' v2 new member completion', useful)
      check('updated ' .. label .. ' v1 member removed', report.established[label].oldApi.completion
        and not has(buffer, 'versioned.oldApi()', 'oldApi'))
      local ok, locations = exact_goto(buffer, 'versioned.newApi()', 'newApi', '/fixture.reload/versioned/2.0/vendor/Versioned.java')
      check('updated ' .. label .. ' goto exact v2 source declaration', ok, locations)
      report.established[label].newApi = { completion = useful, goto_definition = ok }
      local diagnostic = request(buffer, 'textDocument/diagnostic')
      local position = probe.find(text(buffer), 'versioned.oldApi()', 'oldApi')
      local found = false
      for _, item in ipairs(diagnostic and diagnostic.items or {}) do
        if item.range.start.line == position.line and item.message:find('oldApi', 1, true) then found = true end
      end
      check('updated ' .. label .. ' stale member has a diagnostic', found, diagnostic)
    end
  else
    report.phases[#report.phases].analysis_updated = wait_for(function()
      return not has(java, 'versioned.newApi()', 'newApi') and not has(java, 'versioned.oldApi()', 'oldApi')
        and not has(kotlin, 'versioned.newApi()', 'newApi') and not has(kotlin, 'versioned.oldApi()', 'oldApi')
        and not has(java, 'added.addedApi()', 'addedApi') and not has(kotlin, 'custom.customValue()', 'customValue')
    end, wait_ms)
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

client:stop()
local stopped = vim.wait(10000, function() return exit ~= nil end, 50)
if not stopped then client:stop(true) end
check('server exits cleanly after the same-connection lifecycle', stopped and exit.code == 0 and exit.signal == 0, exit)
local failures = 0
for _, item in ipairs(report.checks) do if not item.passed then failures = failures + 1 end end
report.failed, report.total = failures, #report.checks
save()
print(('Gradle reload: %d/%d checks passed'):format(#report.checks - failures, #report.checks))
vim.cmd(failures == 0 and 'qa!' or 'cquit 1')
