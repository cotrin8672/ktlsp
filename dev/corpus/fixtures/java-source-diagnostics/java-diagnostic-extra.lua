-- Existing corpus extra-probe hook; one unsaved Java edit, useful controls, one diagnostic pull.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local path = ctx.project .. '/src/main/java/probe/Use.java'
  local source
  for candidate, value in pairs(ctx.sources) do
    if probe.path_key(candidate) == probe.path_key(path) then source = value end
  end
  assert(source, 'known corpus controls must open the same Java caller')
  local buffer, client = source.buf, ctx.client
  local original = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
  assert(vim.deep_equal(original, source.disk) and not vim.bo[buffer].modified,
    'disposable source must start at its pinned disk snapshot')
  local original_sha = probe.buffer_sha256(buffer)
  local mixed = vim.uv.fs_stat(ctx.project .. '/src/main/kotlin/probe/KotlinCurrent.kt') ~= nil
  local body = table.concat(original, '\n')
  local anchor = '        // diagnostic-probe'
  local first, last = body:find(anchor, 1, true)
  assert(first and not body:find(anchor, last + 1, true), 'unique diagnostic edit anchor required')
  local inserted = '        value.removed();\n'
    .. (mixed and '        kotlin.removed();\n' or '')
    .. '        Missing missing = null;\n        Gone gone = null;'
  body = body:sub(1, first - 1) .. inserted .. body:sub(last + 1)
  body = body:gsub('^package probe;\n', 'package probe;\nimport missing.Gone;\n', 1)
  local changed = vim.split(body, '\n', { plain = true })
  local report = { compiler_authority = 'not-certified',
    scope = 'first standard Java pull after a stable unsaved buffer; no producer closure claim',
    original_sha256 = original_sha, mixed = mixed, requests = {} }
  local push, stop_push = probe.observe_diagnostics(client, buffer)
  local function unchanged()
    return push.attempted and #push.outbound == 1 and push.wire_matches_snapshot == true
      and probe.buffer_sha256(buffer) == push.source_sha256
      and vim.api.nvim_buf_get_changedtick(buffer) == push.changedtick
      and vim.lsp.util.buf_versions[buffer] == push.expected_version
  end
  local ok, failure = pcall(function()
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, changed)
    vim.lsp._changetracking.flush(client)
    ctx.check('Java reduction: edited source is outbound', unchanged(), vim.inspect(push), 'operational')
    local function request(method, params, accepts, label)
      local row = { case = 'plain-java-diagnostic-reduction', method = method, attempts = 0,
        outcome = 'not-run', source_sha256 = push.source_sha256, expected_version = push.expected_version }
      ctx.request_outcomes[#ctx.request_outcomes + 1] = row
      report.requests[#report.requests + 1] = row
      if unchanged() then
        row.attempts = 1
        row.response, row.error = client:request_sync(method, params, 5000, buffer)
        row.rpc_outcome = probe.request_outcome(row.response, row.error, accepts)
        row.outcome = row.rpc_outcome
        if not unchanged() then
          row.correlation_error = 'source-changed-during-request'
          row.outcome = probe.request_outcome(nil, row.correlation_error, accepts)
        end
      end
      ctx.check(label, row.outcome == 'matched', vim.inspect(row), 'semantic', row.outcome)
      return row
    end
    for _, receiver in ipairs(mixed and { 'value', 'kotlin' } or { 'value' }) do
      local position = probe.find(changed, receiver .. '.current()', 'current')
      position.character = position.character + 4
      request('textDocument/completion', { textDocument = { uri = vim.uri_from_bufnr(buffer) },
        position = position, context = { triggerKind = 1 } },
        function(value) return probe.completion_has(value, 'current') end,
        'Java reduction: same-phase useful member on ' .. receiver)
    end
    local pull = request('textDocument/diagnostic',
      { textDocument = { uri = vim.uri_from_bufnr(buffer) } },
      function(value) return type(value) == 'table' and value.kind == 'full' and type(value.items) == 'table' end,
      'Java reduction: standard diagnostic report is available')
    local expectations = {
      { 'Java source missing member', 'value.removed()', 'removed' },
      { 'unimported missing type', 'Missing missing', 'Missing' },
      { 'removed import', 'import missing.Gone;', 'Gone' },
    }
    if mixed then expectations[#expectations + 1] = { 'Kotlin source missing member', 'kotlin.removed()', 'removed' } end
    for _, expected in ipairs(expectations) do
      local position = probe.find(changed, expected[2], expected[3])
      local outcome = pull.outcome
      if outcome == 'matched' or outcome == 'mismatch' then
        outcome = probe.request_outcome(pull.response, pull.error, function(value)
          if type(value) ~= 'table' or value.kind ~= 'full' or type(value.items) ~= 'table' then return false end
          for _, item in ipairs(value.items) do
            local range = type(item) == 'table' and item.range or nil
            if type(item) == 'table' and item.severity == 1 and probe.range_in_source(range, changed)
              and range.start.line == position.line and range['end'].line == position.line
              and range.start.character <= position.character and range['end'].character > position.character then
              return true
            end
          end
          return false
        end)
      end
      ctx.check('Java reduction diagnoses ' .. expected[1], outcome == 'matched',
        vim.inspect(pull), 'semantic', outcome)
    end
  end)
  stop_push(0) -- UI push is recorded only; it cannot certify final Java compiler semantics.
  report.push = push
  local restored, stop_restore = probe.observe_diagnostics(client, buffer)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, original)
  vim.lsp._changetracking.flush(client)
  stop_restore(0)
  vim.bo[buffer].modified = false
  report.restoration = restored
  local restored_ok = restored.attempted and #restored.outbound == 1 and restored.wire_matches_snapshot == true
    and probe.buffer_sha256(buffer) == original_sha and vim.deep_equal(vim.fn.readfile(path), source.disk)
  ctx.check('Java reduction: original buffer restored and disk preserved', restored_ok, vim.inspect(restored), 'operational')
  report.completed, report.error = ok, ok and vim.NIL or tostring(failure)
  vim.fn.writefile({ vim.json.encode(report) }, ctx.artifacts .. '/java-diagnostic-reduction.json')
  assert(ok, failure)
end
