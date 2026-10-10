-- The mixed harness runs this in Nvim's input loop, rather than in a blocking -l script.
local script_dir = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(script_dir .. '/nvim_lsp_probe.lua')
local project = assert(vim.env.KTLSP_UI_PROJECT)
local artifacts = assert(vim.env.KTLSP_UI_ARTIFACTS)
probe.outside_project(project, artifacts)
local cycles = tonumber(vim.env.KTLSP_UI_SAMPLES) or 1
assert(cycles >= 1 and cycles <= 1000 and cycles == math.floor(cycles), 'invalid KTLSP_UI_SAMPLES')
local ready_timeout = tonumber(vim.env.KTLSP_MIXED_READY_TIMEOUT_MS) or 240000
local report = { samples = {}, cycles = cycles, neovim = vim.version(),
  measurement = 'insert-mode dot to popupmenu_show handoff, separately from RPC; no physical-screen timing',
  performance_acceptance = false }
local files = { java = 'src/main/java/mixed/JavaApi.java',
  kotlin = 'src/main/kotlin/mixed/KotlinApi.kt', box = 'src/main/java/mixed/JavaBox.java' }
local cases = {
  { name = 'java', anchor = 'return api.kotlinGreeting();', before = '        return api',
    member = 'kotlinGreeting', excluded = 'javaGreeting', after = '        return api.kotlinGreeting();',
    post_input = ';' },
  { name = 'kotlin', anchor = 'fun kotlinGreeting(): String = peer.javaGreeting()',
    before = '    fun kotlinGreeting(): String = peer', member = 'javaGreeting', excluded = 'kotlinGreeting',
    after = '    fun kotlinGreeting(): String = peer.javaGreeting()' },
}
local build_tool = 'maven'
local custom
if vim.env.KTLSP_UI_CASES then
  custom = vim.json.decode(table.concat(vim.fn.readfile(vim.env.KTLSP_UI_CASES), '\n'))
  files, cases, build_tool = assert(custom.files), assert(custom.cases), assert(custom.build_tool)
  assert(#cases > 0, 'custom UI manifest needs at least one case')
  report.manifest = vim.env.KTLSP_UI_CASES
end
local buffers, original = {}, {}
local client, sample, done
vim.o.hidden = true
vim.o.completeopt = 'menuone,noselect'
vim.o.showmode = false
vim.lsp.log.set_level(vim.log.levels.WARN)
for name, file in pairs(files) do
  local buf = vim.fn.bufadd(project .. '/' .. file)
  vim.fn.bufload(buf)
  vim.bo[buf].filetype = name == 'kotlin' and 'kotlin' or 'java'
  buffers[name], original[name] = buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  if custom then
    assert(vim.fn.sha256(table.concat(original[name], '\n')) == assert(custom.source_sha256[name]),
      'custom UI source changed: ' .. file)
  end
end
-- The mixed probe already created an isolated Maven launcher for the official plugin.
local shim = artifacts .. '/maven-bin'
if vim.fn.isdirectory(shim) == 1 then
  vim.env.PATH = shim .. (vim.fn.has('win32') == 1 and ';' or ':') .. (vim.env.PATH or '')
end
vim.env.MAVEN_USER_HOME = artifacts .. '/maven-home'
local function finish(err)
  if done then return end
  done = true
  report.error, report.disk_unchanged = err, true
  if vim.snippet.active() then vim.snippet.stop() end
  for name, buf in pairs(buffers) do
    report.disk_unchanged = report.disk_unchanged and vim.deep_equal(
      vim.fn.readfile(project .. '/' .. files[name]), original[name])
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, original[name])
  end
  if client then
    client:stop()
    local exited = vim.wait(15000, function() return report.server_exit_code ~= nil end, 50)
    report.clean_shutdown = exited and client:is_stopped() and report.shutdown_acknowledged
      and report.exit_sent and report.server_exit_code == 0
    if not client:is_stopped() then client:stop(true) end
  end
  local ok = not err and report.disk_unchanged and report.clean_shutdown
  vim.fn.writefile({ vim.json.encode(report) }, artifacts .. '/completion-ui.json')
  print((ok and 'PASS ' or 'FAIL ') .. 'builtin completion UI: input, useful menu, exact accepted text, unchanged files, shutdown')
  if err then print(err) end
  vim.cmd(ok and 'qa!' or 'cq!')
end
local function guarded(fn)
  return function(...)
    if done then return end
    local ok, err = pcall(fn, ...)
    if not ok then vim.schedule(function() finish(tostring(err)) end) end
  end
end
local next_case
local function matches(label, token)
  return label == token or label:match('^' .. token .. '%f[^%w_]')
end
vim.api.nvim_create_autocmd('InsertCharPre', { callback = guarded(function()
  if sample and vim.v.char == '.' then sample.insert_char_ns = vim.uv.hrtime() end
end) })
vim.api.nvim_create_autocmd('CompleteDone', { callback = guarded(function()
  if sample and sample.accept_ns then sample.complete_done_reason = vim.v.event.reason end
end) })
local ns = vim.api.nvim_create_namespace('ktlsp-completion-ui')
vim.ui_attach(ns, { ext_popupmenu = true }, guarded(function(event, items)
  if event ~= 'popupmenu_show' or not sample or sample.menu_ns then return end
  sample.menu_ns, sample.items = vim.uv.hrtime(), items
  sample.input_to_menu_ms = (sample.menu_ns - assert(sample.insert_char_ns)) / 1e6
  sample.enqueue_to_menu_ms = (sample.menu_ns - sample.enqueue_ns) / 1e6
  local selected
  for i, item in ipairs(items) do
    assert(not sample.case.excluded or not matches(item[1], sample.case.excluded), 'wrong receiver member in popup')
    if matches(item[1], sample.case.member) then selected = i end
  end
  assert(selected, 'expected LSP member missing from popup')
  sample.selected = selected
  vim.schedule(guarded(function()
    assert(vim.fn.pumvisible() == 1, 'popup is not visible')
    local info = vim.fn.complete_info({ 'items' }).items
    sample.lsp_item = info[selected].user_data.nvim.lsp.completion_item
    assert(matches(sample.lsp_item.label, sample.case.member), 'popup is not populated by LSP')
    assert(sample.request_count == 1 and not sample.rpc_error, 'completion required more than one response or failed')
    sample.accept_ns = vim.uv.hrtime()
    vim.api.nvim_input(string.rep('<C-n>', selected) .. '<C-y>')
    local function accepted()
      -- Native completion may resolve the item and execute a server edit command.
      -- Typing the semicolon earlier invalidates Nvim's pending resolve by changedtick.
      local resolving = report.resolve_provider and not sample.resolve_response_ns
      local command = sample.lsp_item.command or (sample.resolve_result or {}).command
      if not sample.complete_done_reason or resolving or (command and not sample.command_response_ns) then
        assert(vim.uv.hrtime() - sample.accept_ns < 5e9, 'completion acceptance did not finish')
        vim.defer_fn(guarded(accepted), 20)
        return
      end
      assert(sample.complete_done_reason == 'accept', 'completion was not accepted')
      assert(not sample.resolve_error and not sample.command_error, 'completion acceptance failed')
      if sample.case.post_input then vim.api.nvim_input(sample.case.post_input) end
      vim.defer_fn(guarded(function()
        sample.result_line = vim.api.nvim_get_current_line()
        assert(sample.result_line == sample.case.after, 'accepted completion produced wrong text')
        sample.accept_to_text_ms = (vim.uv.hrtime() - sample.accept_ns) / 1e6
        sample.accepted = true
        vim.api.nvim_input('<Esc>')
        vim.defer_fn(guarded(next_case), 60)
      end), 20)
    end
    vim.defer_fn(guarded(accepted), 20)
  end))
end))
local config, mode = probe.config(project, assert(vim.env.KTLSP_BIN), artifacts, build_tool)
report.mode, report.binary = mode, vim.env.KTLSP_BIN
report.removed_environment_names = probe.removed_environment
local index_state = { project = false, libraries = false }
report.index_state = index_state
local previous_log = config.handlers and config.handlers['window/logMessage']
config.handlers = config.handlers or {}
config.handlers['window/logMessage'] = function(err, params, ctx, handler_config)
  local message = params and params.message or ''
  if message:match('^ktlsp indexed %d+ project files') then index_state.project = true end
  if message:match('^ktlsp indexed %d+ library files') then
    index_state.libraries = true
    local failed, missing = message:match('(%d+) failed, (%d+) missing sources')
    index_state.failed, index_state.missing_sources = tonumber(failed), tonumber(missing)
  end
  if previous_log then return previous_log(err, params, ctx, handler_config) end
end
local apply_edit = config.handlers['workspace/applyEdit'] or vim.lsp.handlers['workspace/applyEdit']
config.handlers['workspace/applyEdit'] = function(err, params, ctx, handler_config)
  local observed = { params = vim.deepcopy(params), received_ns = vim.uv.hrtime() }
  local result = apply_edit(err, params, ctx, handler_config)
  observed.result = result
  if sample then
    sample.apply_edits = sample.apply_edits or {}
    sample.apply_edits[#sample.apply_edits + 1] = observed
  end
  return result
end
local previous_exit = config.on_exit
config.on_exit = function(code, signal, client_id)
  report.server_exit_code, report.server_exit_signal = code, signal
  if previous_exit then previous_exit(code, signal, client_id) end
end
local initial_buf = assert(buffers[cases[1].name], 'missing initial UI buffer')
vim.api.nvim_set_current_buf(initial_buf)
local id = assert(vim.lsp.start(config, { bufnr = initial_buf }))
local number = 0
next_case = function()
  if sample then
    if vim.snippet.active() then vim.snippet.stop() end
    vim.api.nvim_buf_set_lines(buffers[sample.case.name], 0, -1, false, original[sample.case.name])
  end
  number = number + 1
  if number > cycles * #cases then finish(); return end
  local case = cases[(number - 1) % #cases + 1]
  local buf = buffers[case.name]
  vim.api.nvim_set_current_buf(buf)
  local pos = probe.find(original[case.name], case.anchor, case.source_token or case.member)
  vim.api.nvim_buf_set_lines(buf, pos.line, pos.line + 1, false, { case.before })
  require('vim.lsp._changetracking').flush(client)
  vim.api.nvim_win_set_cursor(0, { pos.line + 1, #case.before })
  sample = { case = case, cycle = math.ceil(number / #cases), request_count = 0,
    phase = number <= #cases and 'first-after-readiness' or 'warm-repetition' }
  report.samples[#report.samples + 1] = sample
  local current = sample
  vim.cmd('startinsert!')
  vim.defer_fn(guarded(function()
    assert(vim.api.nvim_get_mode().mode == 'i', 'did not enter insert mode')
    sample.enqueue_ns = vim.uv.hrtime()
    vim.api.nvim_input('.')
  end), 40)
  vim.defer_fn(function()
    if not done and sample == current and not current.menu_ns then
      finish('no useful completion UI for ' .. case.name .. ' cycle ' .. current.cycle)
    end
  end, 5000)
end
local function wait_ready()
  client = vim.lsp.get_client_by_id(id)
  if not (client and client.initialized) then vim.defer_fn(guarded(wait_ready), 50); return end
  if mode == 'ktlsp' and not (index_state.project and index_state.libraries) then
    vim.defer_fn(guarded(wait_ready), 50); return
  end
  if mode == 'intellij' then
    local progress = require('intellij-lsp.progress')
    if not progress.is_ready(id) then vim.defer_fn(guarded(wait_ready), 100); return end
    report.import_state = progress.import_state(id)
    assert(report.import_state and report.import_state.folders and #report.import_state.folders > 0,
      'IntelliJ import state is missing')
    for _, folder in ipairs(report.import_state.folders) do
      assert(folder.status == 'SUCCESS', 'IntelliJ project import failed')
    end
  end
  report.ready_ns = vim.uv.hrtime()
  report.command_provider = client.server_capabilities.executeCommandProvider
  report.resolve_provider = (client.server_capabilities.completionProvider or {}).resolveProvider
  for _, buf in pairs(buffers) do
    assert(vim.lsp.buf_attach_client(buf, id))
    -- Nvim keeps the options from the first enable call; the RPC oracle disabled autotrigger.
    vim.lsp.completion.enable(false, id, buf)
    vim.lsp.completion.enable(true, id, buf, { autotrigger = true })
  end
  local request = client.rpc.request
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == 'exit' then report.exit_sent = not not sent end
    return sent
  end
  client.rpc.request = function(method, params, callback, ...)
    local current = sample
    if method == 'textDocument/completion' and current then
      current.request_count = current.request_count + 1
      current.request_ns, current.params = vim.uv.hrtime(), params
      local previous_callback = callback
      callback = function(err, result)
        current.response_ns = vim.uv.hrtime()
        current.rpc_ms = (current.response_ns - current.request_ns) / 1e6
        current.rpc_error = err
        previous_callback(err, result)
      end
    elseif method == 'completionItem/resolve' and current and current.accept_ns then
      current.resolve_request_ns = vim.uv.hrtime()
      local previous_callback = callback
      callback = function(err, result)
        current.resolve_response_ns = vim.uv.hrtime()
        current.resolve_error, current.resolve_result = err, result
        previous_callback(err, result)
      end
    elseif method == 'workspace/executeCommand' and current then
      current.command_ns, current.command_params = vim.uv.hrtime(), params
      local previous_callback = callback
      callback = function(err, result)
        current.command_response_ns = vim.uv.hrtime()
        current.command_error, current.command_result = err, result
        previous_callback(err, result)
      end
    elseif method == 'shutdown' then
      local previous_callback = callback
      callback = function(err, result)
        report.shutdown_acknowledged = err == nil
        previous_callback(err, result)
      end
    end
    return request(method, params, callback, ...)
  end
  next_case()
end
vim.defer_fn(guarded(wait_ready), 50)
vim.defer_fn(function()
  if not report.ready_ns then finish('project import/indexing did not finish') end
end, ready_timeout)
