-- Existing corpus extra hook: one exact notification, positive server branch, one normal RPC.
return function(ctx)
  local client = ctx.client
  local path = vim.fs.normalize(ctx.project .. '/build/moddev/artifacts/neoforge-21.1.248-sources.jar')
  local binary = vim.fs.normalize(ctx.project .. '/build/moddev/artifacts/neoforge-21.1.248.jar')
  local log = vim.lsp.log.get_filename()
  local before_lines = #vim.fn.readfile(log)
  local function stamp(file)
    local value = assert(vim.uv.fs_stat(file), 'selected archive must exist: ' .. file)
    assert(value.type == 'file', 'selected archive must be a regular file')
    return { size = value.size, mtime_sec = value.mtime.sec, mtime_nsec = value.mtime.nsec }
  end
  local before = { binary = stamp(binary), sources = stamp(path) }
  local uri = vim.uri_from_fname(path)
  local report = { scope = 'one separate finite watch case; zero original60 golden requests',
    target = path, uri = uri, before = before, log = log, log_start_line = before_lines + 1,
    outgoing = {}, request = {}, branch = 'unobserved',
    classification_evidence = 'positive exact-path DEBUG branch; post-notify RPC is not a notification barrier' }
  local notify = client.rpc.notify
  local synthetic = false
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if sent and method == 'workspace/didChangeWatchedFiles' then
      for _, change in ipairs(params.changes or {}) do
        if change.uri == uri then
          report.outgoing[#report.outgoing + 1] = { sent = sent, synthetic = synthetic,
            method = method, change = change, sent_ns = vim.uv.hrtime() }
        end
      end
    end
    return sent
  end
  local ok, error = pcall(function()
    synthetic = true
    report.sent = client:notify('workspace/didChangeWatchedFiles', { changes = { { uri = uri, type = 2 } } })
    synthetic = false
    assert(report.sent, 'exact standard Changed notification was not outbound')
    -- Reuse the existing 5000ms request budget; timeout is unmeasured, never a quiet PASS.
    report.ack_budget_ms = 5000
    report.observed = vim.wait(5000, function()
      for number, line in ipairs(vim.fn.readfile(log)) do
        if number > before_lines and line:find('path="' .. path .. '"', 1, true) then
          if line:find('unchanged source archive watch ignored', 1, true) then
            report.branch, report.branch_line, report.branch_text = 'ignored-return-None', number, line
            return true
          end
          if line:find('project refresh requested by client watch', 1, true)
            and line:find('force=Some(true)', 1, true) then
            report.branch, report.branch_line, report.branch_text = 'forced-model-refresh', number, line
            return true
          end
        end
      end
      return false
    end, 100)
    if report.observed then
      report.request.method = 'workspace/executeCommand'
      report.request.params = { command = 'ktlsp.tracePath' }
      report.request.attempts = 1
      report.request.response, report.request.error = client:request_sync(report.request.method, report.request.params, 5000)
    end
    report.after = { binary = stamp(binary), sources = stamp(path) }
  end)
  client.rpc.notify = notify
  report.error = not ok and tostring(error) or nil
  report.single_delivery = #report.outgoing == 1 and report.outgoing[1].synthetic
    and report.outgoing[1].change.type == 2
  report.archives_unchanged = report.after ~= nil and vim.deep_equal(before, report.after)
  local response = report.request.response
  report.normal_rpc_ok = response ~= nil and response.err == nil and report.request.error == nil
    and type(response.result) == 'string' and response.result == vim.env.KTLSP_TRACE
  local correlated = ok and report.observed and report.single_delivery and report.archives_unchanged
  local outcome = correlated and (report.branch == 'ignored-return-None' and 'matched' or 'mismatch') or 'unanswered'
  report.classification_outcome = outcome
  report.no_gradle_dump = 'Root audits finalized existing trace after normal shutdown; not inferred from a quiet interval.'
  vim.fn.writefile({ vim.json.encode(report) }, ctx.artifacts .. '/source-watch-proof.json')
  ctx.check('finite watch: one exact Changed notification and positive branch', correlated, vim.inspect(report), 'operational')
  ctx.check('finite watch: unchanged selected source archive returns None', outcome == 'matched', vim.inspect(report), 'semantic', outcome)
  ctx.check('finite watch: normal RPC after positive branch succeeds', report.normal_rpc_ok, vim.inspect(report.request), 'operational')
end
