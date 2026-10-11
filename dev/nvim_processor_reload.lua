-- Real KAPT/KSP option changes on one client, through the gradle-live harness.
local dev = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(dev .. '/nvim_lsp_probe.lua')
local root, binary, artifacts = vim.fs.abspath(assert(arg[1])), assert(arg[2]), vim.fs.abspath(assert(arg[3]))
probe.outside_project(root, artifacts)
assert(vim.fn.isdirectory(root) == 0, 'processor reload fixture must be new')
local report = { checks = {}, phases = {}, requests = {}, messages = {}, notifications = {}, delivery = {} }
local client, client_id, shutdown_state
local function check(name, passed, detail)
  report.checks[#report.checks + 1] = { name = name, passed = passed == true, detail = detail }
  vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/processor-reload.json')
  print((passed and 'PASS  ' or 'FAIL  ') .. name)
end
local function lines(buffer) return vim.api.nvim_buf_get_lines(buffer, 0, -1, false) end
local function wait_for(condition, timeout)
  local next_check = 0
  return vim.wait(timeout, function()
    local now = vim.uv.hrtime()
    if now < next_check then return false end
    next_check = now + 250e6
    return condition()
  end, 50)
end
local generated = {
  { name = 'KspKotlin', receiver = 'kotlin', path = 'app/build/generated/ksp/main/kotlin/generated/KspKotlin.kt' },
  { name = 'KspJava', receiver = 'java', path = 'app/build/generated/ksp/main/java/generated/KspJava.java' },
  { name = 'KaptJava', receiver = 'kapt', path = 'app/build/generated/source/kapt/main/generated/KaptJava.java' },
}
local runner = artifacts .. '/processor-build.ps1'
vim.fn.writefile(vim.split([=[param([string]$Project, [string]$Log)
$argsText = '/d /c call ' + [char]34 + (Join-Path $Project 'gradlew.bat') + [char]34 + ' :app:clean :app:classes --offline --no-daemon --console=plain --max-workers=2 > ' + [char]34 + $Log + [char]34 + ' 2>&1'
$job = Start-Process -FilePath $env:ComSpec -ArgumentList $argsText -WorkingDirectory $Project -WindowStyle Hidden -PassThru
$handle = $job.Handle
if (-not $job.WaitForExit(120000)) {
    & taskkill.exe /PID $job.Id /T /F | Out-Null
    $job.WaitForExit()
    exit 124
}
if ($null -eq $job.ExitCode) { throw 'Gradle exit unavailable' }
exit $job.ExitCode
]=], '\n', { plain = true }), runner)

local function run()
  report.removed_environment_names = probe.sanitize_environment()
  local created = vim.system({ vim.v.progpath, '-l', dev .. '/processor_project.lua', root }, { text = true }):wait(15000)
  check('fresh real processor fixture created', created.code == 0 and created.signal == 0, created.stderr)
  assert(created.code == 0, 'fixture creation failed')
  local properties = vim.fn.readfile(root .. '/gradle.properties')
  assert(properties[1] == 'generationLabel=v1', 'pinned initial generation option changed')
  local jdk = assert(vim.env.JAVA_HOME, 'JAVA_HOME must select JDK 17 for Gradle 7.4.2')
  local executable_suffix = vim.fn.has('win32') == 1 and '.exe' or ''
  local java = jdk .. '/bin/java' .. executable_suffix
  local separator = vim.fn.has('win32') == 1 and ';' or ':'
  local gradle_home = vim.env.GRADLE_USER_HOME or vim.fn.expand('~/.gradle')
  local stdlib = vim.fn.glob(gradle_home .. '/caches/modules-2/files-2.1/org.jetbrains.kotlin/kotlin-stdlib/1.8.21/*/kotlin-stdlib-1.8.21.jar', false, true)
  assert(#stdlib == 1, 'the pinned Kotlin stdlib must be cached')
  local classpath = table.concat({ root .. '/app/build/classes/kotlin/main', root .. '/app/build/classes/java/main',
    root .. '/kapt-processor/build/classes/java/main', stdlib[1] }, separator)
  local function build(phase, version, enabled)
    report.phase = phase
    local changed = vim.deepcopy(properties)
    changed[1] = 'generationLabel=' .. version
    if not enabled then changed[#changed + 1] = 'processingDisabled=true' end
    vim.fn.writefile(changed, root .. '/gradle.properties')
    local log = artifacts .. '/processor-phase-' .. phase .. '-compile.out'
    local command = vim.fn.has('win32') == 1
      and { 'powershell.exe', '-NoProfile', '-NonInteractive', '-File', runner, root, log }
      or { 'sh', '-c', 'exec "$1" :app:clean :app:classes --offline --no-daemon --console=plain --max-workers=2 > "$2" 2>&1',
        'processor-reload', root .. '/gradlew', log }
    local result = vim.system(command, { cwd = root, text = true }):wait(125000)
    report.phases[#report.phases + 1] = { phase = phase, enabled = enabled, version = version,
      compile_exit = result.code, compile_signal = result.signal, log = log, capture_stderr = result.stderr }
    local expected = enabled and result.code == 0 or not enabled and result.code == 1
    check('phase ' .. phase .. ' actual Gradle compiler control', expected and result.signal == 0, result)
    local log_text = table.concat(vim.fn.readfile(log), '\n')
    for _, target in ipairs(generated) do
      local exists = vim.fn.filereadable(root .. '/' .. target.path) == 1
      check('phase ' .. phase .. ' actual generated source ' .. target.name, exists == enabled)
      if not enabled then
        check('disabled Kotlin compiler rejects ' .. target.name,
          log_text:find('Unresolved reference: ' .. target.name, 1, true) ~= nil)
      end
    end
    if enabled and expected then
      local runtime = vim.system({ java, '-cp', classpath, root .. '/oracle/RuntimeCheck.java', version }, { text = true }):wait(30000)
      local ok, counts = pcall(vim.json.decode, runtime.stdout or '')
      check('phase ' .. phase .. ' twelve actual generated API runtime checks',
        runtime.code == 0 and runtime.signal == 0 and ok and counts.checks == 12 and counts.pass == 12
          and counts.generation == version, runtime)
    elseif not enabled then
      local destination = artifacts .. '/disabled-java-classes'
      vim.fn.mkdir(destination, 'p')
      local compiled = vim.system({ jdk .. '/bin/javac' .. executable_suffix, '-d', destination,
        root .. '/app/src/main/java/fixture/JavaUse.java' }, { text = true }):wait(30000)
      check('disabled actual Java compiler rejects generated API uses', compiled.code == 1 and compiled.signal == 0, compiled)
      for _, target in ipairs(generated) do
        check('disabled Java compiler names missing ' .. target.name, (compiled.stderr or ''):find(target.name, 1, true) ~= nil)
      end
    end
    return enabled and expected
  end
  assert(build(0, 'v1', true), 'initial processor compilation failed')

  local buffers, originals = {}, {}
  for language, path in pairs({ Java = 'app/src/main/java/fixture/JavaUse.java', Kotlin = 'app/src/main/kotlin/fixture/KotlinUse.kt',
    Source = 'app/src/main/kotlin/fixture/Source.kt' }) do
    local buffer = vim.fn.bufadd(root .. '/' .. path)
    vim.fn.bufload(buffer)
    vim.bo[buffer].filetype = language == 'Java' and 'java' or 'kotlin'
    buffers[language] = buffer
    originals[buffer] = vim.fn.readfile(root .. '/' .. path)
  end
  local config, mode = probe.config(root, binary, artifacts, 'gradle')
  report.mode = mode
  local log_handler = vim.tbl_get(config, 'handlers', 'window/logMessage')
  config.handlers = config.handlers or {}
  config.handlers['window/logMessage'] = function(error, message, context, options)
    report.messages[#report.messages + 1] = message
    if log_handler then return log_handler(error, message, context, options) end
  end
  shutdown_state = probe.observe_shutdown(config)
  client_id = assert(vim.lsp.start(config, { bufnr = buffers.Java }))
  check('one real Neovim client initializes', vim.wait(10000, function()
    client = vim.lsp.get_client_by_id(client_id)
    return client and client.initialized
  end, 50))
  assert(client and client.initialized, 'client initialization failed')
  local versions = {}
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == 'workspace/didChangeWatchedFiles' then
      report.notifications[#report.notifications + 1] = { phase = report.phase, sent = sent, params = params }
    elseif method == 'textDocument/didChange' and sent then
      versions[params.textDocument.uri] = params.textDocument.version
    end
    return sent
  end
  for _, buffer in pairs(buffers) do vim.lsp.buf_attach_client(buffer, client_id) end
  local imports = { 'import generated.KspKotlin', 'import generated.KspJava', 'import generated.KaptJava' }
  for language, buffer in pairs({ Java = buffers.Java, Kotlin = buffers.Kotlin }) do
    local content = { language == 'Java' and 'package fixture;' or 'package fixture' }
    for _, import in ipairs(imports) do content[#content + 1] = import .. (language == 'Java' and ';' or '') end
    content[#content + 1] = language == 'Java'
      and 'public class JavaUse { void probe(KspKotlin kotlin, KspJava java, KaptJava kapt, Source source) {'
      or 'fun probe(kotlin: KspKotlin, java: KspJava, kapt: KaptJava, source: Source) {'
    for _, target in ipairs(generated) do
      for _, member in ipairs({ 'versionOne', 'versionTwo' }) do
        content[#content + 1] = '    ' .. target.receiver .. '.' .. member .. '()' .. (language == 'Java' and ';' or '')
      end
    end
    content[#content + 1] = '    source.editorUnsaved()' .. (language == 'Java' and ';' or '')
    content[#content + 1] = language == 'Java' and '} }' or '}'
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, content)
  end
  local source = table.concat(originals[buffers.Source], '\n'):gsub('class Source', 'class Source { fun editorUnsaved(): Int = 7 }')
  vim.api.nvim_buf_set_lines(buffers.Source, 0, -1, false, vim.split(source, '\n', { plain = true }))
  vim.lsp._changetracking.flush(client)
  for label, buffer in pairs(buffers) do
    local uri = vim.uri_from_bufnr(buffer)
    local expected = vim.lsp.util.buf_versions[buffer]
    check('unsaved ' .. label .. ' change is outbound', versions[uri] == expected, { expected = expected, sent = versions[uri] })
    report.delivery[#report.delivery + 1] = { uri = uri, expected = expected, sent = versions[uri] }
  end
  if mode == 'intellij' then
    probe.import_ready(client, 'gradle', 180000, check)
  else
    check('initial dependency indexing finishes', wait_for(function()
      for _, message in ipairs(report.messages) do
        if message.message:match('^ktlsp indexed .* library files') then return true end
      end
      return false
    end, 120000))
  end
  local function request(buffer, method, receiver, member, role)
    vim.lsp._changetracking.flush(client)
    local position = receiver and probe.find(lines(buffer), receiver .. '.' .. member .. '()', member)
    if method == 'textDocument/completion' then position.character = position.character + 3 end
    local response, error = client:request_sync(method, {
      textDocument = { uri = vim.uri_from_bufnr(buffer) }, position = position,
    }, 5000, buffer)
    local result = response and response.result
    report.requests[#report.requests + 1] = { phase = report.phase, role = role or 'acceptance', method = method,
      uri = vim.uri_from_bufnr(buffer), receiver = receiver, member = member, result = result,
      error = error or (response and response.err) }
    return result
  end
  local function current(version, enabled, role)
    local member = version == 'v1' and 'versionOne' or 'versionTwo'
    local obsolete = version == 'v1' and 'versionTwo' or 'versionOne'
    local correct = true
    for _, buffer in ipairs({ buffers.Java, buffers.Kotlin }) do
      for _, target in ipairs(generated) do
        local result = request(buffer, 'textDocument/completion', target.receiver, member, role)
        correct = (probe.completion_has(result, member) == enabled) and not probe.completion_has(result, obsolete) and correct
      end
    end
    return correct
  end
  local established = false
  local original_generated = {}
  for phase, state in ipairs({ { 'v1', true }, { 'v2', true }, { 'v2', false }, { 'v1', true } }) do
    local version, enabled = state[1], state[2]
    local number = phase - 1
    if number > 0 then
      build(number, version, enabled)
      check('phase ' .. number .. ' native watched-file notification delivered', wait_for(function()
        for _, notification in ipairs(report.notifications) do
          if notification.phase == number and notification.sent then return true end
        end
        return false
      end, 10000))
      check('phase ' .. number .. ' shared analysis observes processor build',
        wait_for(function() return current(version, enabled, 'refresh-readiness') end, 60000))
    end
    local phase_ok = current(version, enabled)
    check('phase ' .. number .. ' both languages expose only current generated API', phase_ok and (enabled or established))
    if enabled then established = established or phase_ok end
    local member = version == 'v1' and 'versionOne' or 'versionTwo'
    for language, buffer in pairs({ Java = buffers.Java, Kotlin = buffers.Kotlin }) do
      for _, target in ipairs(generated) do
        local result = request(buffer, 'textDocument/definition', target.receiver, member)
        local path = root .. '/' .. target.path
        local matched
        if enabled and vim.fn.filereadable(path) == 1 then
          local target_lines = vim.fn.readfile(path)
          local anchor = (target.name == 'KspKotlin' and 'fun ' or 'String ') .. member .. '()'
          local _, range = probe.find(target_lines, anchor, member)
          matched = probe.exact_definition(result, path, { range })
          local hash = vim.fn.sha256(table.concat(target_lines, '\n'))
          if number == 0 then original_generated[target.name] = hash end
          if number == 3 then check('restored exact source hash ' .. target.name, hash == original_generated[target.name]) end
        else matched = established and #probe.locations(result) == 0 end
        check('phase ' .. number .. ' ' .. language .. ' exact generated source or proved removal ' .. target.name, matched, result)
      end
      check('phase ' .. number .. ' ' .. language .. ' preserves unsaved source member',
        probe.completion_has(request(buffer, 'textDocument/completion', 'source', 'editorUnsaved'), 'editorUnsaved'))
      if number == 1 or not enabled then
        local diagnostics = request(buffer, 'textDocument/diagnostic')
        check('phase ' .. number .. ' ' .. language .. ' returns diagnostic report',
          type(diagnostics) == 'table' and type(diagnostics.items) == 'table', diagnostics)
        for _, target in ipairs(generated) do
          local position = enabled
            and probe.find(lines(buffer), target.receiver .. '.versionOne()', 'versionOne')
            or probe.find(lines(buffer), 'import generated.' .. target.name, target.name)
          local found = vim.iter(diagnostics and diagnostics.items or {}):any(function(item)
            local range = item.range
            return item.severity == 1 and range.start.line == position.line and range['end'].line == position.line
              and range.start.character <= position.character and range['end'].character > position.character
          end)
          check('phase ' .. number .. ' ' .. language .. ' diagnoses obsolete or removed API ' .. target.name, found, diagnostics)
        end
      end
    end
    for label, buffer in pairs(buffers) do
      check('phase ' .. number .. ' ' .. label .. ' disk source remains original',
        vim.deep_equal(vim.fn.readfile(vim.api.nvim_buf_get_name(buffer)), originals[buffer]))
    end
    check('phase ' .. number .. ' keeps the same live client', client.id == client_id and not shutdown_state.exit)
  end
end
local ok, error = xpcall(run, debug.traceback)
check('processor reload probe completes', ok, not ok and error or nil)
if client then
  report.shutdown = probe.shutdown(client, shutdown_state, 10000)
  check('same client exits gracefully', report.shutdown.clean, report.shutdown)
end
local failed = vim.iter(report.checks):any(function(item) return not item.passed end)
vim.cmd(failed and 'cquit 1' or 'qa!')
