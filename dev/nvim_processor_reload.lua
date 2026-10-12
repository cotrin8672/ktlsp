-- Real KAPT/KSP option changes on one client, through the gradle-live harness.
local dev = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(dev .. '/nvim_lsp_probe.lua')
local root, binary, artifacts = vim.fs.abspath(assert(arg[1])), assert(arg[2]), vim.fs.abspath(assert(arg[3]))
probe.outside_project(root, artifacts)
assert(vim.fn.isdirectory(root) == 0, 'processor reload fixture must be new')
local report = { checks = {}, phases = {}, requests = {}, messages = {}, notifications = {}, delivery = {},
  canonical = {}, refresh_ordering = {}, snapshots = {}, selected_source_exports = {}, source_notifications = {}, open_authority = {
    Java = "AP and Kotlin-produced class authority remain unsupported",
    unsaved_regeneration = "producer execution is not certified by this compiler overlay",
  } }
local client, client_id, shutdown_state
local function check(name, passed, detail, observation)
  local item = { name = name, passed = passed == true, detail = detail }
  if observation then
    item.outcome = probe.request_outcome(observation.response, observation.error, function() return passed end, observation.attempted)
    item.measured = item.outcome == "matched" or item.outcome == "mismatch"
    item.observation, item.passed = observation, item.passed and item.measured
  end
  report.checks[#report.checks + 1] = item
  vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/processor-reload.json')
  print((item.measured == false and 'UNMEASURED  ' or item.passed and 'PASS  ' or 'FAIL  ') .. name)
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
  local versions, wire, source_buffers, notification_role = {}, {}, {}, "automatic"
  for _, buffer in pairs(buffers) do
    local uri = vim.uri_from_bufnr(buffer)
    wire[uri], source_buffers[uri] = probe.buffer_full_text(buffer), buffer
  end
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == 'workspace/didChangeWatchedFiles' then
      report.notifications[#report.notifications + 1] = { phase = report.phase, sent = sent, params = params, role = notification_role }
    elseif sent and (method == 'textDocument/didOpen' or method == 'textDocument/didChange') then
      local document = params.textDocument
      versions[document.uri] = document.version
      local buffer = source_buffers[document.uri]
      if buffer then
        local text, error
        if method == 'textDocument/didOpen' then text = document.text
        else text, error = probe.apply_content_changes(wire[document.uri], params.contentChanges, client.offset_encoding) end
        wire[document.uri] = text
        report.source_notifications[#report.source_notifications + 1] = {
          phase = report.phase, method = method, uri = document.uri, version = document.version,
          changedtick = vim.api.nvim_buf_get_changedtick(buffer), source_sha256 = probe.buffer_sha256(buffer),
          wire_source_sha256 = text and vim.fn.sha256(text), wire_matches_source = text == probe.buffer_full_text(buffer),
          content_changes = params.contentChanges, wire_error = error,
        }
      end
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
  local phase_traces = {}
  local function observe_refresh_trace()
    local path = assert(vim.env.KTLSP_TRACE, 'missing refresh trace')
    local initial = assert(vim.uv.fs_stat(path), 'missing refresh trace file')
    local offset, pending, row, skip_partial = initial.size, '', {}, false
    if offset > 0 then
      local baseline = assert(io.open(path, 'rb'), 'refresh trace baseline unavailable')
      baseline:seek('set', offset - 1)
      skip_partial = baseline:read(1) ~= '\n'
      baseline:close()
    end
    local function read()
      local stat = vim.uv.fs_stat(path)
      if not stat or stat.size < offset or stat.ino ~= initial.ino or stat.dev ~= initial.dev then
        row.error = 'refresh-trace-replaced-or-truncated'; return false
      end
      local file = io.open(path, 'rb')
      if not file then row.error = 'refresh-trace-unavailable'; return false end
      local seek = file:seek('set', offset)
      local bytes = seek and file:read('*a')
      local cursor = file:seek()
      file:close()
      if not bytes or not cursor then row.error = 'refresh-trace-read-failed'; return false end
      offset, pending = cursor, pending .. bytes
      while true do
        local newline = pending:find('\n', 1, true)
        if not newline then break end
        local line = pending:sub(1, newline - 1):gsub('\r$', '')
        pending = pending:sub(newline + 1)
        if skip_partial then skip_partial = false
        elseif line ~= '' then
          local ok, event = pcall(vim.json.decode, line)
          if not ok or type(event) ~= 'table' then row.error = 'invalid-refresh-trace-event'; return false end
          local args = type(event.args) == 'table' and event.args or {}
          if type(args.root) == 'string' and probe.path_key(args.root) == probe.path_key(root) then
            if event.name == 'workspace.project_scan' and event.cat == 'workspace' and args.forcedModel == true
              and type(args.generation) == 'number' and (not row.scan or args.generation >= row.scan.generation) then
              row.scan, row.deps = args, nil
            elseif event.name == 'deps.index_workspace' and event.cat == 'deps' and row.scan
              and args.generation == row.scan.generation then row.deps = args end
          end
        end
      end
      return row.scan ~= nil and row.deps ~= nil
    end
    return read, row
  end
  local function stamp(stat)
    return stat and (tostring(stat.ino) .. ':' .. stat.mtime.sec .. ':' .. stat.mtime.nsec) or nil
  end
  local function cache_stamps()
    local values = {}
    for _, path in ipairs(vim.fn.globpath(assert(vim.env.KTLSP_CACHE_DIR) .. '/classpath', '*.json', false, true)) do
      values[path] = stamp(vim.uv.fs_stat(path))
    end
    return values
  end
  local function refreshed_model(before)
    local property = root .. '/gradle.properties'
    local property_stat = assert(vim.uv.fs_stat(property))
    local expected_mtime = property_stat.mtime.sec * 1e9 + property_stat.mtime.nsec
    for _, path in ipairs(vim.fn.globpath(assert(vim.env.KTLSP_CACHE_DIR) .. '/classpath', '*.json', false, true)) do
      local file_stamp = stamp(vim.uv.fs_stat(path))
      if file_stamp and file_stamp ~= before[path] then
        local ok, raw = pcall(function() return table.concat(vim.fn.readfile(path, 'b'), '\n') end)
        local decoded, model
        if ok then decoded, model = pcall(vim.json.decode, raw) end
        local actual_mtime
        if decoded and type(model) == 'table' and type(model.mtimes) == 'table' then
          for key, mtime in pairs(model.mtimes) do
            if type(key) == 'string' and probe.path_key(key) == probe.path_key(property) then
              actual_mtime = mtime
              break
            end
          end
        end
        if decoded and type(model) == 'table' and stamp(vim.uv.fs_stat(path)) == file_stamp
          and model.model_version == 10 and type(model.mtimes) == 'table'
          and actual_mtime == expected_mtime then
          for _, module in ipairs(model.modules or {}) do
            if module.module == ':app' and probe.path_key(module.project_dir) == probe.path_key(root .. '/app') then
              for _, source_set in ipairs(module.source_sets or {}) do
                if source_set.name == 'main' and not source_set.compiler_metadata_incomplete
                  and not source_set.classpath_incomplete and #(source_set.unresolved or {}) == 0 then
                  local selected, count, auxiliary, java, java_count = nil, 0, 0, nil, 0
                  for _, task in ipairs(source_set.compiler_tasks or {}) do
                    if task.language == 'kotlin' and task.task == source_set.kotlin_compile_task then
                      selected, count = task, count + 1
                    elseif task.language == 'kotlin' then auxiliary = auxiliary + 1
                    elseif task.language == 'java' then
                      for _, source_file in ipairs(task.source_files or {}) do
                        if probe.path_key(source_file) == probe.path_key(vim.api.nvim_buf_get_name(buffers.Java)) then
                          java, java_count = task, java_count + 1
                          break
                        end
                      end
                    end
                  end
                  local recipe = selected and selected.kotlin_compiler
                  if count == 1 and selected.complete and recipe and recipe.compiler_version == '1.8.21'
                    and recipe.gradle_plugin_version == '1.8.21' then
                    return { cache_file = path, cache_stamp = file_stamp, cache_sha256 = vim.fn.sha256(raw), cache_text = raw,
                      selected_kotlin_task = selected, selected_java_task_count = java_count,
                      selected_java_task = java_count == 1 and java or nil,
                      properties_mtime_ns = expected_mtime, model_version = model.model_version,
                      primary_task = selected.task, compiler_version = recipe.compiler_version,
                      gradle_plugin_version = recipe.gradle_plugin_version,
                      runtime_count = #recipe.default_classpath, declared_classpath_count = #selected.classpath,
                      source_count = #selected.source_files, auxiliary_kotlin_tasks = auxiliary,
                      producer_closure_complete = false }
                  end
                end
              end
            end
          end
        end
      end
    end
  end
  local function refresh_after_build(number, version, enabled)
    build(number, version, enabled)
    check('phase ' .. number .. ' native watched-file notification delivered', wait_for(function()
      for _, notification in ipairs(report.notifications) do
        if notification.phase == number and notification.sent and notification.role == 'automatic' then return true end
      end
      return false
    end, 10000))
    if mode ~= 'ktlsp' then
      local ready = probe.import_ready(client, 'gradle', 120000, check)
      report.refresh_ordering[tostring(number)] = { mode = mode, ready = ready,
        generation_certified = false, role = 'existing IDEA adapter import observation' }
      return ready
    end
    -- Deliver a final build batch after native compilation/output checks, so a mid-build import
    -- cannot be mistaken for the final model. Automatic watch delivery is checked separately above.
    local before, changes = cache_stamps(), {}
    local read_trace, trace_row = observe_refresh_trace()
    phase_traces[number] = { read = read_trace, row = trace_row }
    for _, path in ipairs({ 'gradle.properties', 'app/build/classes/kotlin/main',
      'app/build/classes/java/main', 'app/build/generated/ksp/main/classes' }) do
      changes[#changes + 1] = { uri = vim.uri_from_fname(root .. '/' .. path),
        type = vim.uv.fs_stat(root .. '/' .. path) and 2 or 3 }
    end
    notification_role = 'post-build final model control'
    local sent = client:notify('workspace/didChangeWatchedFiles', { changes = changes })
    notification_role = 'automatic'
    local observation = { sent = not not sent, native_build_closed = true,
      build_exit = report.phases[#report.phases].compile_exit,
      properties_sha256 = vim.fn.sha256(table.concat(vim.fn.readfile(root .. '/gradle.properties'), '\n')) }
    local observed = sent and wait_for(function()
      local traced = read_trace()
      if trace_row.error then return true end
      local selected = refreshed_model(before)
      if not traced or not selected then return false end
      observation.model = selected
      return true
    end, 120000)
    local ready = observed and not trace_row.error and observation.model ~= nil
    observation.ready, observation.trace = not not ready, trace_row
    if not ready then observation.error = trace_row.error or 'refresh-observation-unanswered' end
    report.refresh_ordering[tostring(number)] = observation
    return ready
  end
  -- This is an artifact observer; it contributes no assertions, requests, or producer authority.
  local function strict_utf8(text)
    local i = 1
    while i <= #text do
      local b, count, low, high = text:byte(i), 1, 0x80, 0xbf
      if b <= 0x7f then count = 1
      elseif b >= 0xc2 and b <= 0xdf then count = 2
      elseif b >= 0xe0 and b <= 0xef then
        count = 3
        if b == 0xe0 then low = 0xa0 elseif b == 0xed then high = 0x9f end
      elseif b >= 0xf0 and b <= 0xf4 then
        count = 4
        if b == 0xf0 then low = 0x90 elseif b == 0xf4 then high = 0x8f end
      else return false end
      if i + count - 1 > #text then return false end
      for offset = 1, count - 1 do
        local next_byte = text:byte(i + offset)
        if next_byte < (offset == 1 and low or 0x80) or next_byte > (offset == 1 and high or 0xbf) then return false end
      end
      i = i + count
    end
    return true
  end
  local function capture_selected_sources(number, model)
    local row = { phase = number, availability = 'unmeasured', tasks = {}, disk_files = {}, wire_overlays = {},
      started_ns = vim.uv.hrtime(), producer_closure_complete = false, diagnostic_authority = false,
      capture_boundary = number == 0 and 'initial index readiness before first acceptance sweep'
        or 'accepted final post-build model before first phase diagnostic/acceptance RPC' }
    report.selected_source_exports[tostring(number)] = row
    local ok, error = pcall(function()
      assert(model, 'accepted selected task model unavailable')
      row.model_cache_sha256, row.model_reference = model.cache_sha256, number == 0 and 'initial current-cache observation'
        or 'refresh_ordering.' .. number .. '.model'
      row.refresh_generation = report.refresh_ordering[tostring(number)]
        and vim.tbl_get(report.refresh_ordering[tostring(number)], 'trace', 'scan', 'generation')
      local function read(path)
        local file = assert(io.open(path, 'rb'), 'source/export bytes unavailable: ' .. path)
        local bytes = file:read('*a')
        assert(file:close() and bytes ~= nil, 'source/export read failed: ' .. path)
        return bytes
      end
      local function stat(path)
        local value = assert(vim.uv.fs_stat(path), 'source/export stat unavailable: ' .. path)
        return { ino = value.ino, dev = value.dev, size = value.size, type = value.type,
          mtime = value.mtime, ctime = value.ctime }
      end
      local function cache_current()
        local before = stamp(vim.uv.fs_stat(model.cache_file))
        return before == model.cache_stamp and read(model.cache_file) == model.cache_text
          and stamp(vim.uv.fs_stat(model.cache_file)) == before
          and vim.fn.sha256(model.cache_text) == model.cache_sha256
      end
      assert(cache_current(), 'selected cache changed before byte export')
      local destination = artifacts .. '/processor-selected-sources/phase-' .. number
      assert(vim.fn.isdirectory(destination) == 0, 'source export phase must be fresh')
      vim.fn.mkdir(destination, 'p')
      local function preserve(bytes, relative)
        local path = destination .. '/' .. relative
        assert(vim.fn.filereadable(path) == 0, 'source export must be absent')
        local file = assert(io.open(path, 'wb'), 'source export unavailable: ' .. path)
        local written = file:write(bytes)
        assert(file:close() and written, 'source export write failed: ' .. path)
        assert(read(path) == bytes, 'source export bytes changed')
        -- Blob hashing preserves NUL, CRLF and non-UTF8 bytes; it never hashes a decoded Vim string.
        local hash = vim.fn.eval('sha256(readblob(' .. vim.fn.string(path) .. '))')
        assert(type(hash) == 'string' and #hash == 64 and hash:match('^[0-9a-f]+$'), 'raw SHA256 unavailable')
        local valid = strict_utf8(bytes)
        return { artifact = path, relative_artifact = relative, bytes = #bytes, sha256 = hash,
          strict_utf8 = valid, utf8_text = valid and bytes or nil }
      end
      local all, positions, base = {}, {}, probe.path_key(root) .. '/'
      local function task(role, selected)
        assert(selected and type(selected.source_files) == 'table', 'selected ' .. role .. ' source inventory unavailable')
        local entry = { role = role, task = selected.task, language = selected.language,
          source_files = vim.deepcopy(selected.source_files), file_indices = {} }
        row.tasks[#row.tasks + 1] = entry
        for index, path in ipairs(selected.source_files) do
          local normalized = vim.fs.normalize(vim.fs.abspath(path))
          local key = probe.path_key(normalized)
          assert(key:sub(1, #base) == base, 'selected source escapes owned fixture: ' .. path)
          local position = positions[key]
          if not position then
            position = #all + 1
            positions[key], all[position] = position, { origin_path = path, normalized_path = normalized,
              real_path = assert(vim.uv.fs_realpath(path), 'selected source realpath unavailable'),
              source_key = key, memberships = {} }
          end
          all[position].memberships[#all[position].memberships + 1] = { role = role, task = selected.task,
            source_index = index, original_path = path }
          entry.file_indices[#entry.file_indices + 1] = position
        end
      end
      task('kotlin-primary', model.selected_kotlin_task)
      assert(model.selected_java_task_count == 1, 'selected Java source task is not unique')
      task('java-primary', model.selected_java_task)
      for index, entry in ipairs(all) do
        row.disk_files[index] = entry
        entry.before = stat(entry.origin_path)
        assert(entry.before.type == 'file', 'selected source is not a regular file')
        local bytes = read(entry.origin_path)
        entry.after = stat(entry.origin_path)
        entry.stable_while_read = vim.deep_equal(entry.before, entry.after) and entry.after.size == #bytes
          and probe.path_key(entry.origin_path) == entry.source_key
        assert(entry.stable_while_read, 'selected source changed while reading')
        entry.content = preserve(bytes, ('source-%04d.bin'):format(index))
      end
      for label, buffer in pairs(buffers) do
        local path, uri = vim.api.nvim_buf_get_name(buffer), vim.uri_from_bufnr(buffer)
        local text, outbound = probe.buffer_full_text(buffer), wire[uri]
        local entry = { label = label, path = path, uri = uri, source_key = probe.path_key(path),
          version = vim.lsp.util.buf_versions[buffer], sent_version = versions[uri],
          changedtick = vim.api.nvim_buf_get_changedtick(buffer), wire_matches_source = text == outbound,
          selected_file_index = positions[probe.path_key(path)] }
        row.wire_overlays[#row.wire_overlays + 1] = entry
        entry.buffer_content = preserve(text, 'overlay-' .. label .. '-buffer.bin')
        assert(type(outbound) == 'string', 'outbound source overlay unavailable')
        entry.wire_content = preserve(outbound, 'overlay-' .. label .. '-wire.bin')
        entry.identity_complete = entry.selected_file_index ~= nil and entry.wire_matches_source
          and entry.version ~= nil and entry.version == entry.sent_version
        assert(entry.identity_complete, 'outbound source overlay identity incomplete')
      end
      assert(cache_current(), 'selected cache changed during byte export')
      row.exact_selected_membership = true
      row.consistency = 'current accepted cache and per-file stable observations; not an atomic compiler snapshot'
      row.availability = 'captured'
    end)
    row.completed_ns, row.elapsed_ns = vim.uv.hrtime(), vim.uv.hrtime() - row.started_ns
    if not ok then row.error, row.availability = tostring(error), 'unmeasured' end
    local destination = artifacts .. '/processor-selected-sources/phase-' .. number
    if vim.fn.isdirectory(destination) == 1 then
      local saved, save_error = pcall(function()
        assert(vim.fn.writefile({ vim.json.encode(row) }, destination .. '/manifest.json') == 0, 'source manifest write failed')
      end)
      if not saved then row.error, row.availability = tostring(save_error), 'unmeasured' end
    end
  end
  local function capture_snapshot(number)
    local row = { phase = number, sources = {}, source_identity_complete = true, producer_closure_complete = false }
    for label, buffer in pairs(buffers) do
      local uri, text = vim.uri_from_bufnr(buffer), probe.buffer_full_text(buffer)
      local source = { path = vim.api.nvim_buf_get_name(buffer), uri = uri, text = text,
        source_sha256 = vim.fn.sha256(text), wire_text = wire[uri], wire_source_sha256 = wire[uri] and vim.fn.sha256(wire[uri]),
        version = vim.lsp.util.buf_versions[buffer], sent_version = versions[uri],
        changedtick = vim.api.nvim_buf_get_changedtick(buffer), wire_matches_source = text == wire[uri] }
      row.sources[label] = source
      row.source_identity_complete = row.source_identity_complete and source.wire_matches_source
        and source.version ~= nil and source.version == source.sent_version
    end
    local ordering = report.refresh_ordering[tostring(number)]
    local model = ordering and ordering.model
    row.refresh_generation = ordering and ordering.trace and ordering.trace.scan and ordering.trace.scan.generation
    row.model_reference = 'refresh_ordering.' .. number .. '.model'
    row.model_identity_limit = 'accepted cache and refresh generation; server trace has no model hash'
    if model then
      local before = stamp(vim.uv.fs_stat(model.cache_file))
      local file = io.open(model.cache_file, 'rb')
      local current = file and file:read('*a')
      if file then file:close() end
      row.model_cache_current = before == model.cache_stamp and stamp(vim.uv.fs_stat(model.cache_file)) == before
        and current == model.cache_text and vim.fn.sha256(model.cache_text) == model.cache_sha256
    else row.model_error = 'accepted-model-unavailable' end
    row.availability = row.source_identity_complete and row.model_cache_current and row.refresh_generation ~= nil
      and 'captured' or 'unmeasured'
    capture_selected_sources(number, model)
    row.captured_ns = vim.uv.hrtime()
    report.snapshots[tostring(number)] = row
    vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/processor-reload.json')
  end
  local request_prerequisite
  local function request(buffer, method, receiver, member, role)
    if request_prerequisite then
      local row = { phase = report.phase, role = role or 'acceptance', method = method, attempted = false,
        uri = vim.uri_from_bufnr(buffer), receiver = receiver, member = member, error = request_prerequisite }
      report.requests[#report.requests + 1] = row
      return nil, row
    end
    vim.lsp._changetracking.flush(client)
    local position = receiver and probe.find(lines(buffer), receiver .. '.' .. member .. '()', member)
    if method == 'textDocument/completion' then position.character = position.character + 3 end
    local response, error = client:request_sync(method, {
      textDocument = { uri = vim.uri_from_bufnr(buffer) }, position = position,
    }, 5000, buffer)
    local result = response and response.result
    local row = { phase = report.phase, role = role or 'acceptance', method = method, attempted = true,
      uri = vim.uri_from_bufnr(buffer), receiver = receiver, member = member, result = result, response = response,
      error = error or (response and response.err) }
    report.requests[#report.requests + 1] = row
    return result, row
  end
  local function current(version, enabled, role)
    local member = version == 'v1' and 'versionOne' or 'versionTwo'
    local obsolete = version == 'v1' and 'versionTwo' or 'versionOne'
    local correct, missing = true, nil
    for _, buffer in ipairs({ buffers.Java, buffers.Kotlin }) do
      for _, target in ipairs(generated) do
        local result, observation = request(buffer, 'textDocument/completion', target.receiver, member, role)
        if probe.request_outcome(observation.response, observation.error, function() return true end, observation.attempted) ~= 'matched' then
          missing = missing or observation
        end
        correct = (probe.completion_has(result, member) == enabled) and not probe.completion_has(result, obsolete) and correct
      end
    end
    return correct, missing
  end
  local established = false
  local original_generated = {}
  for phase, state in ipairs({ { 'v1', true }, { 'v2', true }, { 'v2', false }, { 'v1', true } }) do
    local version, enabled = state[1], state[2]
    local number = phase - 1
    local canonical, observation, refresh_ready
    if number > 0 then
      if mode == 'ktlsp' and (number == 1 or not enabled) then
        canonical, observation = probe.canonical_kotlin_diagnostics(client, buffers.Kotlin, {
          observation_timeout_ms = 120000, request_timeout_ms = 5000, compiler_version = '1.8.21',
          edit_label = 'one unsaved phase ' .. number .. ' comment after final native build/model ordering',
          mutate = function()
            refresh_ready = refresh_after_build(number, version, enabled)
            assert(refresh_ready, 'final processor model ordering unavailable')
            vim.api.nvim_buf_set_lines(buffers.Kotlin, -1, -1, false,
              { '// canonical generated API probe phase ' .. number })
          end,
          flush = function()
            vim.lsp._changetracking.flush(client)
            capture_snapshot(number)
          end,
        })
        local ordering = phase_traces[number]
        local accepted = type(observation.accepted) == 'table' and observation.accepted or nil
        local accepted_generation = accepted and type(accepted.result_id) == 'string'
          and tonumber(accepted.result_id:match('^kotlin:(%d+):%d+:%d+:%d+$')) or nil
        if observation.canonical_certified then
          local traced = ordering and ordering.read()
          if not traced or ordering.row.error or accepted_generation == nil
            or accepted_generation ~= ordering.row.scan.generation then
            canonical, observation.canonical_certified = nil, false
            observation.error, observation.availability = 'canonical-refresh-generation-mismatch', 'protocol-error'
          end
          observation.refresh_generation = ordering and ordering.row.scan and ordering.row.scan.generation
        end
        local snapshot = report.snapshots[tostring(number)]
        if snapshot then
          snapshot.sources_unchanged_through_diagnostic_rpc = true
          for label, buffer in pairs(buffers) do
            local source = snapshot.sources[label]
            source.unchanged_through_diagnostic_rpc = probe.buffer_full_text(buffer) == source.text
              and wire[source.uri] == source.wire_text and vim.lsp.util.buf_versions[buffer] == source.version
              and versions[source.uri] == source.sent_version and vim.api.nvim_buf_get_changedtick(buffer) == source.changedtick
            snapshot.sources_unchanged_through_diagnostic_rpc = snapshot.sources_unchanged_through_diagnostic_rpc
              and source.unchanged_through_diagnostic_rpc
          end
          snapshot.captured_before_diagnostic_rpc = observation.rpc_started_ns ~= nil
            and snapshot.captured_ns < observation.rpc_started_ns
          snapshot.canonical_source_matches = accepted ~= nil and accepted.source_sha256 == snapshot.sources.Kotlin.source_sha256
          snapshot.canonical_generation_matches = accepted_generation ~= nil
            and accepted_generation == snapshot.refresh_generation
          snapshot.kotlin_receipt_correlated = observation.canonical_certified == true and snapshot.availability == 'captured'
            and snapshot.sources_unchanged_through_diagnostic_rpc and snapshot.captured_before_diagnostic_rpc
            and snapshot.canonical_source_matches and snapshot.canonical_generation_matches
        end
        report.canonical[tostring(number)] = observation
        report.requests[#report.requests + 1] = { phase = number, role = 'canonical acceptance',
          method = observation.method, uri = observation.uri, result = canonical,
          error = observation.error, response = observation.response, attempted = observation.attempted }
      else refresh_ready = refresh_after_build(number, version, enabled) end
    end
    if (number == 1 or number == 2) and not report.snapshots[tostring(number)] then
      vim.lsp._changetracking.flush(client)
      capture_snapshot(number)
    end
    if not report.selected_source_exports[tostring(number)] then
      local model = number == 0 and refreshed_model({}) or vim.tbl_get(report.refresh_ordering, tostring(number), 'model')
      capture_selected_sources(number, model)
    end
    -- One acceptance sweep after build/model ordering; desired completion contents are not polled.
    request_prerequisite = number > 0 and not refresh_ready and 'final-model-refresh-unavailable' or nil
    local phase_ok, phase_missing = current(version, enabled)
    if number > 0 then
      check('phase ' .. number .. ' shared analysis observes processor build', refresh_ready and phase_ok,
        report.refresh_ordering[tostring(number)], phase_missing)
    end
    check('phase ' .. number .. ' both languages expose only current generated API',
      phase_ok and (enabled or established), nil, phase_missing)
    if enabled then established = established or (phase_ok and not phase_missing) end
    local member = version == 'v1' and 'versionOne' or 'versionTwo'
    for language, buffer in pairs({ Java = buffers.Java, Kotlin = buffers.Kotlin }) do
      for _, target in ipairs(generated) do
        local result, request_observation = request(buffer, 'textDocument/definition', target.receiver, member)
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
        check('phase ' .. number .. ' ' .. language .. ' exact generated source or proved removal ' .. target.name,
          matched, result, request_observation)
      end
      local unsaved, unsaved_observation = request(buffer, 'textDocument/completion', 'source', 'editorUnsaved')
      check('phase ' .. number .. ' ' .. language .. ' preserves unsaved source member',
        probe.completion_has(unsaved, 'editorUnsaved'), unsaved, unsaved_observation)
      if number == 1 or not enabled then
        local diagnostics, diagnostic_observation
        if language == 'Kotlin' and mode == 'ktlsp' then
          diagnostics, diagnostic_observation = canonical, observation
        else diagnostics, diagnostic_observation = request(buffer, 'textDocument/diagnostic') end
        check('phase ' .. number .. ' ' .. language .. ' returns diagnostic report',
          type(diagnostics) == 'table' and type(diagnostics.items) == 'table', diagnostics, diagnostic_observation)
        for _, target in ipairs(generated) do
          local position = enabled
            and probe.find(lines(buffer), target.receiver .. '.versionOne()', 'versionOne')
            or probe.find(lines(buffer), 'import generated.' .. target.name, target.name)
          local found = vim.iter(diagnostics and diagnostics.items or {}):any(function(item)
            local range = item.range
            return item.severity == 1 and range.start.line == position.line and range['end'].line == position.line
              and range.start.character <= position.character and range['end'].character > position.character
          end)
          if not enabled and language == 'Kotlin' then
            local content = lines(buffer)
            local _, type_range = probe.find(content, target.receiver .. ': ' .. target.name, target.name)
            local _, qualifier_range = probe.find(content, 'import generated.' .. target.name, 'generated')
            local _, class_range = probe.find(content, 'import generated.' .. target.name, target.name)
            local function error_covers(expected)
              return vim.iter(diagnostics and diagnostics.items or {}):any(function(item)
                local range = item.range
                return item.severity == 1 and range.start.line == expected.start.line
                  and range['end'].line == expected['end'].line
                  and range.start.character <= expected.start.character
                  and range['end'].character >= expected['end'].character
              end)
            end
            found = error_covers(type_range) and (error_covers(qualifier_range) or error_covers(class_range))
          end
          check('phase ' .. number .. ' ' .. language .. ' diagnoses obsolete or removed API ' .. target.name,
            found, diagnostics, diagnostic_observation)
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
