-- Existing corpus hook and canonical observer; Java-only open exercises the guarded queue.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local path = vim.fs.abspath(ctx.project .. '/src/main/java/app/PlainUse.java')
  local source = assert(ctx.sources[path], 'the ordinary Java caller must be open')
  local buf, client = source.buf, ctx.client
  local old = { lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false),
    text = probe.buffer_full_text(buf), modified = vim.bo[buf].modified,
    fileformat = vim.bo[buf].fileformat, eol = vim.bo[buf].eol,
    notify = client.rpc.notify, publish = client.handlers['textDocument/publishDiagnostics'] }
  local report = { scope = 'One controlled mixed module; current Java error then separately certified body repair',
    observations = {}, assertions = {}, original_sha256 = vim.fn.sha256(old.text), initial_open_files = {} }
  for name in pairs(ctx.sources) do report.initial_open_files[#report.initial_open_files + 1] = name end
  local function check(name, value, detail, outcome)
    report.assertions[#report.assertions + 1] = { name = name, ok = not not value, detail = detail, outcome = outcome }
    ctx.check('Java body receipt: ' .. name, value, detail, 'semantic', outcome)
  end
  local function call(phase, mutate, complete)
    local result, row = probe.canonical_kotlin_diagnostics(client, buf, {
      observation_timeout_ms = 120000, request_timeout_ms = 5000, require_complete = complete,
      edit_label = 'Java body ' .. phase,
      mutate = mutate,
      flush = function() require('vim.lsp._changetracking').flush(client) end,
    })
    report.observations[phase] = row
    local record = { case = 'java-body-' .. phase,
      method = 'textDocument/diagnostic', attempts = row.attempted and 1 or 0,
      response_received = row.response ~= nil, outcome = row.canonical_certified and 'matched' or row.availability,
      first_outcome = row.canonical_certified and 'matched' or row.availability }
    ctx.request_outcomes[#ctx.request_outcomes + 1] = record
    return result, row, record
  end
  -- Rust hashes the sorted JSON receipt. Keep the actual raw Javac evidence joined to
  -- the accepted body certificate, rather than trusting a clean notification.
  local function canonical_json(value)
    if type(value) ~= 'table' then return vim.json.encode(value) end
    local parts = {}
    if vim.islist(value) then
      for _, item in ipairs(value) do parts[#parts + 1] = canonical_json(item) end
      return '[' .. table.concat(parts, ',') .. ']'
    end
    for _, key in ipairs(vim.tbl_keys(value)) do assert(type(key) == 'string') end
    local keys = vim.tbl_keys(value)
    table.sort(keys)
    for _, key in ipairs(keys) do parts[#parts + 1] = vim.json.encode(key) .. ':' .. canonical_json(value[key]) end
    return '{' .. table.concat(parts, ',') .. '}'
  end
  local function raw_receipt(phase, row)
    local file = assert(io.open(assert(vim.env.KTLSP_TRACE), 'rb'))
    local found
    for line in file:lines() do
      local decoded, event = pcall(vim.json.decode, line)
      local args = decoded and type(event) == 'table' and event.args
      if event and event.name == 'compiler.java.editor' and type(args) == 'table'
        and type(args.java_receipt) == 'table' then
        for _, item in ipairs(args.java_receipt.sourcesBefore or {}) do
          local normalized = type(item.path) == 'string' and item.path:gsub('\\', '/') or ''
          if normalized:sub(-#'/src/main/java/app/PlainUse.java') == '/src/main/java/app/PlainUse.java'
            and item.sha256 == row.wire_source_sha256 then found = args end
        end
      end
    end
    file:close()
    local raw = found and found.java_receipt
    if raw then
      local bytes = canonical_json(raw)
      report.raw_receipts = report.raw_receipts or {}
      report.raw_receipts[phase] = { java_receipt = raw, header_receipt = found.header_receipt,
        body_receipt_sha256 = vim.fn.sha256(bytes), helper_input_sha256 = found.helper_input_sha256,
        canonical_output_reused = found.canonical_output_reused,
        k2_normal_output_matches_canonical = found.k2_normal_output_matches_canonical }
      vim.fn.writefile({ bytes }, ctx.artifacts .. '/java-body-' .. phase .. '-javac-receipt.json')
    end
    return found, raw
  end
  local executed, failure = xpcall(function()
    check('initially only Java is open', #report.initial_open_files == 1 and path:match('%.java$') ~= nil)
    assert(old.lines[1] == 'package app;', 'unexpected pinned package')
    local result, row, record = call('error', function()
      vim.api.nvim_buf_set_lines(buf, 1, 1, false, { 'import ktlsp_missing_native_control.Absent;' })
    end, false)
    local located = false
    for _, item in ipairs(result and result.items or {}) do
      if item.severity == 1 and type(item.message) == 'string'
        and item.message:find('ktlsp_missing_native_control', 1, true)
        and probe.range_in_source(item.range, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
        and item.range.start.line == 1 and item.range['end'].line == 1 then located = true end
    end
    check('current canonical Java error has source version and wire SHA', row.canonical_certified
      and row.wire_matches_snapshot and row.source_sha256 == row.wire_source_sha256,
      row.error, not row.canonical_certified and 'uncertified-current-java-error' or nil)
    check('Javac missing-package ERROR is located on the edited import', row.canonical_certified and located,
      nil, not row.canonical_certified and 'uncertified-current-java-error' or nil)
    if row.canonical_certified and not located then record.outcome, record.first_outcome = 'mismatch', 'mismatch' end
    local error_event, error_raw = raw_receipt('error', row)
    check('current error has a completed raw Javac receipt', row.canonical_certified and error_raw
      and error_raw.compileCompleted == true and error_raw.success == false
      and error_event.canonical_output_reused == true and error_event.k2_normal_output_matches_canonical == true)
    local repaired, repair, repair_record = call('repair', function()
      vim.api.nvim_buf_set_lines(buf, 0, -1, false, old.lines)
    end, true)
    local body = repair.accepted and repair.accepted.java_body
    local function digest(value) return type(value) == 'string' and #value == 64 and value:match('^%x+$') end
    local certified = repair.canonical_certified and repair.absence_certified and repair.wire_matches_snapshot
      and repair.source_sha256 == report.original_sha256 and repair.source_sha256 == repair.wire_source_sha256
      and repaired and #repaired.items == 0 and type(body) == 'table'
      and body.basis == 'javac-body-normal-kotlin-output' and body.task == ':compileJava'
      and digest(body.body_receipt_sha256) and digest(body.input_spec_sha256)
      and digest(body.compiler_inputs_sha256) and type(body.source_count) == 'number' and body.source_count >= 1
      and repair.pull and repair.pull.result_id == repaired.resultId
    check('repair has its own current full Javac body clean receipt', certified, repair.error,
      not certified and 'uncertified-current-java-body' or nil)
    if not certified then repair_record.outcome, repair_record.first_outcome = 'uncertified-current-java-body', 'uncertified-current-java-body' end
    local repair_event, repair_raw = raw_receipt('repair', repair)
    local actual = report.raw_receipts and report.raw_receipts.repair
    check('repair body digest and input identities match the raw Javac receipt', certified and actual
      and repair_raw.compileCompleted == true and repair_raw.success == true
      and actual.body_receipt_sha256 == body.body_receipt_sha256
      and repair_raw.inputSpecSha256 == body.input_spec_sha256
      and repair_event.helper_input_sha256 == body.compiler_inputs_sha256
      and repair_event.canonical_output_reused == true and repair_event.k2_normal_output_matches_canonical == true)
    check('error v3 and repair v4 are the delivered source versions', row.expected_version == 3 and repair.expected_version == 4)
    check('repair uses a fresh result identity and delivered version', certified and row.expected_version
      and repair.expected_version > row.expected_version and row.response
      and repaired.resultId ~= row.response.result.resultId, nil,
      not certified and 'uncertified-current-java-body' or nil)
  end, debug.traceback)
  report.error = executed and vim.NIL or tostring(failure)
  client.rpc.notify, client.handlers['textDocument/publishDiagnostics'] = old.notify, old.publish
  if probe.buffer_full_text(buf) ~= old.text then
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, old.lines)
    require('vim.lsp._changetracking').flush(client)
  end
  vim.bo[buf].fileformat, vim.bo[buf].eol, vim.bo[buf].modified = old.fileformat, old.eol, old.modified
  report.restored = probe.buffer_full_text(buf) == old.text and vim.deep_equal(vim.fn.readfile(path), source.disk)
  check('original buffer and all disk source bytes preserved', executed and report.restored)
  vim.fn.writefile({ vim.json.encode(report) }, ctx.artifacts .. '/java-body-diagnostics.json')
end
