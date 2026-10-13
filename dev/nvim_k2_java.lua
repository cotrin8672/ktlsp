-- Temp EXTRA probe: ordinary corpus12 remain unchanged; root audits the raw backend trace.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local client = ctx.client
  local target_path = ctx.project .. '/src/main/java/binaryuse/JavaUse.java'
  local target, originals = nil, {}
  local function disk(path)
    local file = assert(io.open(path, 'rb'))
    local raw = file:read('*a'); file:close()
    return raw
  end
  for path, source in pairs(ctx.sources) do
    originals[path] = { buf = source.buf, lines = vim.api.nvim_buf_get_lines(source.buf, 0, -1, false),
      source = probe.buffer_full_text(source.buf), disk = disk(path), modified = vim.bo[source.buf].modified,
      fileformat = vim.bo[source.buf].fileformat, eol = vim.bo[source.buf].eol,
      version = vim.lsp.util.buf_versions[source.buf], tick = vim.api.nvim_buf_get_changedtick(source.buf) }
    if probe.path_key(path) == probe.path_key(target_path) then target = originals[path] end
  end
  assert(target, 'original corpus JavaUse buffer is required')
  local previous_notify, previous_publish = client.rpc.notify, client.handlers['textDocument/publishDiagnostics']
  local report = { expected = { import = 'import ktlsp_missing_native_control.Absent;',
    severity = vim.diagnostic.severity.ERROR, message_contains = 'ktlsp_missing_native_control' },
    assertions = {}, originals = originals, original_corpus_cases = 12, producer_closure_complete = false,
    backend_audit = 'root must correlate compiler.java.editor and the complete raw javac receipt' }
  local result, observation, edited_source
  local unchanged, expected_position = true, nil
  local executed, failure = xpcall(function()
    require('vim.lsp._changetracking').flush(client)
    assert(not target.source:find('ktlsp_missing_native_control', 1, true), 'control name already exists')
    assert(target.lines[1] == 'package binaryuse;', 'unexpected JavaUse package')
    result, observation = probe.canonical_kotlin_diagnostics(client, target.buf, {
      observation_timeout_ms = 120000, request_timeout_ms = 5000, require_complete = false,
      edit_label = 'one unsaved unused missing-package import in original JavaUse',
      mutate = function()
        vim.api.nvim_buf_set_lines(target.buf, 1, 1, false, { report.expected.import })
        edited_source = probe.buffer_full_text(target.buf)
        expected_position = probe.find(vim.api.nvim_buf_get_lines(target.buf, 0, -1, false),
          report.expected.import, 'Absent')
      end,
      flush = function() require('vim.lsp._changetracking').flush(client) end,
    })
    report.observation, report.edited_source, report.expected.position = observation, edited_source, expected_position
    report.after = {}
    for path, old in pairs(originals) do
      local source = probe.buffer_full_text(old.buf)
      report.after[path] = { source = source, source_sha256 = vim.fn.sha256(source),
        version = vim.lsp.util.buf_versions[old.buf], tick = vim.api.nvim_buf_get_changedtick(old.buf) }
      unchanged = unchanged and disk(path) == old.disk
      if old.buf == target.buf then unchanged = unchanged and source == edited_source
      else
        unchanged = unchanged and source == old.source and vim.bo[old.buf].modified == old.modified
          and vim.lsp.util.buf_versions[old.buf] == old.version
          and vim.api.nvim_buf_get_changedtick(old.buf) == old.tick
      end
    end
  end, debug.traceback)
  report.error = not executed and failure or nil
  report.restore_errors = {}
  client.rpc.notify, client.handlers['textDocument/publishDiagnostics'] = previous_notify, previous_publish
  for path, old in pairs(originals) do
    local ok, err = pcall(function()
      if not vim.deep_equal(vim.api.nvim_buf_get_lines(old.buf, 0, -1, false), old.lines) then
        vim.api.nvim_buf_set_lines(old.buf, 0, -1, false, old.lines)
      end
      vim.bo[old.buf].fileformat, vim.bo[old.buf].eol, vim.bo[old.buf].modified =
        old.fileformat, old.eol, old.modified
    end)
    if not ok then report.restore_errors[path] = tostring(err) end
  end
  local flushed, flush_error = pcall(function() require('vim.lsp._changetracking').flush(client) end)
  if not flushed then report.restore_errors.flush = tostring(flush_error) end
  local restored = flushed and next(report.restore_errors) == nil
  for path, old in pairs(originals) do
    local ok, valid = pcall(function()
      return probe.buffer_full_text(old.buf) == old.source and disk(path) == old.disk
        and vim.bo[old.buf].modified == old.modified and vim.bo[old.buf].fileformat == old.fileformat
        and vim.bo[old.buf].eol == old.eol
    end)
    restored = restored and ok and valid
  end
  report.hooks_restored = client.rpc.notify == previous_notify
    and client.handlers['textDocument/publishDiagnostics'] == previous_publish
  report.restored = restored and report.hooks_restored
  local canonical = executed and observation and observation.canonical_certified == true
  local errors, matching = {}, false
  for _, item in ipairs(result and result.items or {}) do
    if item.severity == report.expected.severity then
      errors[#errors + 1] = item
      matching = matching or (type(item.message) == 'string'
        and item.message:find(report.expected.message_contains, 1, true) ~= nil
        and type(item.range) == 'table' and vim.deep_equal(item.range.start, expected_position))
    end
  end
  report.error_items, report.manual_pull_count = errors, observation and observation.attempted and 1 or 0
  local counts = { total = 0, matched = 0, mismatch = 0, unmeasured = 0 }
  local function check(name, valid, outcome)
    outcome = outcome or (valid and 'matched' or 'mismatch')
    counts.total = counts.total + 1
    local bucket = (outcome == 'matched' or outcome == 'mismatch') and outcome or 'unmeasured'
    counts[bucket] = counts[bucket] + 1
    report.assertions[#report.assertions + 1] = { name = name, ok = not not valid, outcome = outcome }
    ctx.check('K2 Java backend probe: ' .. name, valid, nil, 'semantic', outcome)
  end
  local unavailable = observation and observation.availability or 'not-run'
  if unavailable == 'matched' then unavailable = 'protocol-error' end
  check('canonical current Java pull', canonical, canonical and 'matched' or unavailable)
  check('one missing-package ERROR at selector', canonical and #errors == 1 and matching,
    not canonical and unavailable or nil)
  check('other buffers and all disk bytes preserved', executed and unchanged)
  check('buffers flags and hooks restored', report.restored)
  report.assert_counts = counts
  vim.fn.writefile({ vim.json.encode(report) }, ctx.artifacts .. '/k2-java-backend.json')
  print('K2_JAVA_BACKEND_PROBE_ASSERTIONS ' .. vim.json.encode(counts)
    .. ' manual_pulls=' .. tostring(report.manual_pull_count))
end
