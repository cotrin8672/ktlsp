-- Run with nvim --headless -u NONE -i NONE -l dev/nvim_lsp_probe_selfcheck.lua [artifact.json].
-- Client mocks use real Neovim timers; no server, compiler, or index is started.
local directory = vim.fs.dirname(debug.getinfo(1, 'S').source:sub(2))
local helper = dofile(directory .. '/nvim_lsp_probe.lua')
local names = { 'nvim_lsp_probe.lua', 'nvim_corpus.lua', 'nvim_mixed.lua',
  'nvim_completion_ui.lua', 'nvim_processor_reload.lua', 'nvim_gradle_reload.lua' }
local function hashes()
  local result = {}
  for _, name in ipairs(names) do
    local file = assert(io.open(directory .. '/' .. name, 'rb'))
    result[name] = vim.fn.sha256(file:read('*a'))
    file:close()
  end
  return result
end
local before = hashes()
for _, name in ipairs(names) do assert(loadfile(directory .. '/' .. name)) end
local results = {}
local cases = {
  { name = 'null-ACK/native0/function-callback', clean = true },
  { name = 'vim-NIL-ACK/native0/callback-list', clean = true, result = vim.NIL, callback_list = true },
  { name = 'native1-is-failure', clean = false, exit_code = 1 },
  { name = 'signal15-is-failure', clean = false, exit_signal = 15 },
  { name = 'non-null-ACK-is-failure', clean = false, result = {} },
  { name = 'error-ACK-observes-client-termination', clean = false, error = { code = -32600 } },
  { name = 'no-ACK-timeout-forces-only-this-mock', clean = false, no_ack = true },
  { name = 'forced-native0-is-still-failure', clean = false, no_ack = true, forced_code = 0 },
  { name = 'stopping-is-not-exited', clean = false, no_exit = true },
  { name = 'failed-shutdown-send-is-failure', clean = false, shutdown_send_failed = true },
  { name = 'failed-exit-send-is-failure', clean = false, exit_send_failed = true },
  { name = 'duplicate-exit-is-failure', clean = false, duplicate_exit = true },
  { name = 'duplicate-shutdown-is-failure', clean = false, duplicate_shutdown = true },
  { name = 'already-exited-without-handshake-is-failure', clean = false, already_exited = true },
}
for _, case in ipairs(cases) do
  local old_calls, stop_calls, custom_calls, terminate_calls, is_stopped_calls = 0, 0, 0, 0, 0
  local first = function(code, signal, client_id)
    assert(type(code) == 'number' and type(signal) == 'number' and client_id == 123)
    old_calls = old_calls + 1
  end
  local second = function() old_calls = old_calls + 1 end
  local original_list = { first, second }
  local config = { on_exit = case.callback_list and original_list or first }
  local state = helper.observe_shutdown(config)
  assert(config.on_exit[2] == first)
  if case.callback_list then assert(config.on_exit[3] == second and #original_list == 2) end
  local function emit(code, signal)
    for _, callback in ipairs(config.on_exit) do callback(code, signal, 123) end
  end
  local notify_reply = function() end
  local client = { rpc = {} }
  client.rpc.request = function(method, params, callback, extra)
    if method == 'probe/custom' then
      assert(params.value == 1 and extra == notify_reply)
      callback(nil, 'custom-result', 42)
      return true, 42
    end
    assert(method == 'shutdown' and params == nil)
    if not case.no_ack then vim.schedule(function() callback(case.error, case.result, 7) end) end
    return not case.shutdown_send_failed, 7
  end
  client.rpc.notify = function(method)
    assert(method == 'exit')
    if not case.no_exit and not case.exit_send_failed then
      vim.defer_fn(function() emit(case.exit_code or 0, case.exit_signal or 0) end, 5)
    end
    return not case.exit_send_failed
  end
  client.rpc.terminate = function()
    terminate_calls = terminate_calls + 1
    emit(case.forced_code or 99, 0)
  end
  function client:is_stopped() is_stopped_calls = is_stopped_calls + 1; return true end
  function client:stop(force)
    stop_calls = stop_calls + 1
    if force then self.rpc.terminate(); return end
    local sent, id = self.rpc.request('probe/custom', { value = 1 }, function(err, value, returned_id)
      assert(err == nil and value == 'custom-result' and returned_id == 42)
      custom_calls = custom_calls + 1
    end, notify_reply)
    assert(sent and id == 42)
    local function shutdown()
      self.rpc.request('shutdown', nil, function(err)
        if err == nil then
          self.rpc.notify('exit')
          if case.duplicate_exit then self.rpc.notify('exit') end
        else
          self.rpc.terminate()
        end
      end)
    end
    shutdown()
    if case.duplicate_shutdown then shutdown() end
  end
  if case.already_exited then emit(0, 0) end
  local observed = helper.shutdown(client, state, 50)
  assert(observed.clean == case.clean, case.name .. ': ' .. vim.inspect(observed))
  assert(is_stopped_calls == 0, 'must not consult the stopping-only predicate')
  assert(observed.terminate_calls == terminate_calls)
  assert(observed.exit ~= nil, 'mock cleanup must be reaped')
  assert(old_calls >= (case.callback_list and 2 or 1), 'preserve previous callback(s)')
  if not case.already_exited then assert(custom_calls == 1) end
  local previous_stop_calls = stop_calls
  assert(helper.shutdown(client, state, 50) == state and stop_calls == previous_stop_calls,
    'idempotent finished stop')
  results[#results + 1] = { name = case.name, expected_clean = case.clean, observed_clean = observed.clean,
    shutdown_requests = #observed.shutdown_requests, exit_notifications = #observed.exit_notifications,
    terminate_calls = terminate_calls, timed_out = observed.timed_out, old_callback_calls = old_calls }
  vim.wait(10, function() return false end, 5) -- Drain duplicate mock exit events before the next case.
end
local after = hashes()
assert(vim.deep_equal(before, after))
if arg[1] then
  assert(vim.fn.filereadable(arg[1]) == 0, 'fresh artifact required')
  vim.fn.writefile({ vim.json.encode({ checks = results, before = before, after = after,
    syntax_checked = names, inputs_unchanged = true, real_server_processes = 0 }) }, arg[1])
end
print(('PASS %d lifecycle controls; six Lua files parse; no server/index/compiler'):format(#results))
vim.cmd('qa!')
