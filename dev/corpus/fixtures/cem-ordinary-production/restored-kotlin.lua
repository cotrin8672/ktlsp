return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local case = vim.json.decode(table.concat(vim.fn.readfile(ctx.repo .. '/dev/corpus/fixtures/cem-ordinary-production/restored-kotlin.json'), '\n')).probes[1]
  local path = vim.fs.abspath(ctx.project .. '/' .. case.source)
  assert(path:find('/src/main/kotlin/', 1, true) and not path:find('/mixin/', 1, true), 'ordinary Kotlin caller required')
  local target = assert(ctx.sources[path])
  local function raw_disk()
    local file = assert(io.open(path, 'rb'))
    local text = file:read('*a'); file:close(); return text
  end
  local old = {lines=vim.api.nvim_buf_get_lines(target.buf, 0, -1, false),
    text=probe.buffer_full_text(target.buf), disk=raw_disk(), modified=vim.bo[target.buf].modified,
    ff=vim.bo[target.buf].fileformat, eol=vim.bo[target.buf].eol,
    version=vim.lsp.util.buf_versions[target.buf], tick=vim.api.nvim_buf_get_changedtick(target.buf)}
  assert(old.text == old.disk and vim.fn.sha256(old.disk) == case.source_raw_sha256, 'exact initial wire/disk mismatch')
  local previous_notify, previous_publish = ctx.client.rpc.notify, ctx.client.handlers['textDocument/publishDiagnostics']
  local report = {scope='One ordinary Kotlin restored-valid canonical pull, not Java absence authority or Mixin coverage',
    path=path, original=old, planned_semantic_pulls=1, producer_closure_complete=false, java_absence_certified=false}
  local row = {case='cem-ordinary-restored-canonical', method='textDocument/diagnostic', attempts=0, outcome='not-run', response_received=false}
  ctx.request_outcomes[#ctx.request_outcomes + 1] = row
  local result, observation
  local ok, failure = xpcall(function()
    require('vim.lsp._changetracking').flush(ctx.client)
    local delivery, finish_delivery = probe.observe_diagnostics(ctx.client, target.buf)
    vim.api.nvim_buf_set_lines(target.buf, -1, -1, false, {'// ktlsp ordinary restored-input readiness control'})
    require('vim.lsp._changetracking').flush(ctx.client)
    local _, ack = finish_delivery(15000)
    report.temporary_edit_delivery = ack
    assert(ack.attempted and #ack.outbound == 1 and ack.wire_matches_snapshot
      and ack.generation_certified and ack.response and not ack.error, 'temporary didChange version acknowledgement unavailable')
    assert(ack.expected_version > old.version and raw_disk() == old.disk, 'temporary edit not buffer-only/current')
    -- Observe any current canonical result, then classify errors on this valid input as mismatches.
    result, observation = probe.canonical_kotlin_diagnostics(ctx.client, target.buf, {
      observation_timeout_ms=120000, request_timeout_ms=5000, compiler_version='2.4.0', require_complete=false,
      edit_label='Restore exact ordinary Kotlin disk/wire bytes after one buffer-only comment; wait for canonical acceptance then one pull',
      mutate=function()
        vim.api.nvim_buf_set_lines(target.buf, 0, -1, false, old.lines)
        vim.bo[target.buf].fileformat, vim.bo[target.buf].eol, vim.bo[target.buf].modified = old.ff, old.eol, old.modified
      end,
      flush=function() require('vim.lsp._changetracking').flush(ctx.client) end,
    })
    report.observation = observation
    assert(observation.expected_version > ack.expected_version, 'restoration version did not advance')
  end, debug.traceback)
  report.execution_error = not ok and failure or nil
  ctx.client.rpc.notify, ctx.client.handlers['textDocument/publishDiagnostics'] = previous_notify, previous_publish
  if not vim.deep_equal(vim.api.nvim_buf_get_lines(target.buf,0,-1,false),old.lines) then
    vim.api.nvim_buf_set_lines(target.buf,0,-1,false,old.lines)
  end
  vim.bo[target.buf].fileformat, vim.bo[target.buf].eol, vim.bo[target.buf].modified = old.ff, old.eol, old.modified
  require('vim.lsp._changetracking').flush(ctx.client)
  report.restored = probe.buffer_full_text(target.buf) == old.text and raw_disk() == old.disk
    and vim.bo[target.buf].modified == old.modified and vim.bo[target.buf].fileformat == old.ff
    and vim.bo[target.buf].eol == old.eol
  report.hooks_restored = ctx.client.rpc.notify == previous_notify
    and ctx.client.handlers['textDocument/publishDiagnostics'] == previous_publish
  local complete = ok and observation and observation.canonical_certified and observation.absence_certified
    and observation.accepted.clean and observation.accepted.compiler_version == '2.4.0'
    and observation.source_sha256 == case.source_raw_sha256
    and observation.wire_source_sha256 == case.source_raw_sha256
    and observation.accepted.source_sha256 == case.source_raw_sha256
    and observation.accepted.compiler_inputs_sha256:match('^%x+$')
    and #observation.accepted.compiler_inputs_sha256 == 64
    and report.restored and report.hooks_restored
  local errors = {}
  for _, item in ipairs(result and result.items or {}) do
    if item.severity == vim.diagnostic.severity.ERROR then errors[#errors+1] = item end
  end
  report.error_items = errors
  report.current = {text_sha256=probe.buffer_sha256(target.buf), version=vim.lsp.util.buf_versions[target.buf],
    tick=vim.api.nvim_buf_get_changedtick(target.buf), disk_sha256=vim.fn.sha256(raw_disk())}
  row.attempts = observation and observation.attempted and 1 or 0
  row.response_received = observation and observation.response ~= nil and observation.response.err == nil and observation.error == nil or false
  row.outcome = complete and #errors == 0 and 'matched'
    or (row.response_received and 'mismatch' or observation and observation.availability or 'not-run')
  row.expected_version, row.source_sha256 = observation and observation.expected_version, case.source_raw_sha256
  row.result_id, row.compiler_inputs_sha256 = result and result.resultId, observation and observation.accepted and observation.accepted.compiler_inputs_sha256
  report.request = row
  vim.fn.writefile({vim.json.encode(report)}, ctx.artifacts .. '/ordinary-restored-canonical.json')
  ctx.check('ordinary Kotlin restored valid current canonical pull', row.outcome == 'matched',
    vim.inspect({error=report.execution_error, observation_error=observation and observation.error, row=row, errors=errors}), 'semantic', row.outcome)
end
