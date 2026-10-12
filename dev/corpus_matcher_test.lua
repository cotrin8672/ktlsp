-- Pure boundary checks; nvim --headless -l dev/corpus_matcher_test.lua
local directory = vim.fs.dirname(debug.getinfo(1, 'S').source:gsub('^@', ''))
local probe = dofile(directory .. '/nvim_lsp_probe.lua')
local target = vim.fs.abspath(directory .. '/corpus-matcher-target.kt')
local other = vim.fs.abspath(directory .. '/corpus-matcher-other.kt')
local identifier = { start = { line = 0, character = 6 }, ['end'] = { line = 0, character = 10 } }
local constructor = { start = { line = 0, character = 10 }, ['end'] = { line = 2, character = 1 } }
local lines = { 'class Test(', '    value: String', ')' }
local count = 0
local function check(name, actual, expected)
  assert(actual == expected, name .. ': expected ' .. tostring(expected) .. ', got ' .. tostring(actual))
  count = count + 1
end
local function location(path, range)
  return { uri = vim.uri_from_fname(path), range = range }
end
local ranges = { identifier, constructor }
check('identifier still matches', probe.exact_definition(location(target, identifier), target, ranges), true)
check('reviewed constructor matches', probe.exact_definition(location(target, constructor), target, ranges), true)
check('constructor needs explicit alternative',
  probe.exact_definition(location(target, constructor), target, { identifier }), false)
check('wrong identifier file rejected', probe.exact_definition(location(other, identifier), target, ranges), false)
check('wrong constructor file rejected', probe.exact_definition(location(other, constructor), target, ranges), false)
local shifted = vim.deepcopy(constructor)
shifted.start.character = shifted.start.character + 1
check('shifted range rejected', probe.exact_definition(location(target, shifted), target, ranges), false)
local enclosing = { start = { line = 0, character = 0 }, ['end'] = { line = 2, character = 1 } }
check('enclosing declaration rejected', probe.exact_definition(location(target, enclosing), target, ranges), false)
check('extra target rejected', probe.exact_definition({ location(target, identifier), location(other, identifier) },
  target, ranges), false)
check('selection range normalized', probe.exact_definition({ targetUri = vim.uri_from_fname(target),
  targetRange = enclosing, targetSelectionRange = constructor }, target, ranges), true)
check('archive source matches', probe.exact_definition({ uri = 'jar:file:///cache/source.jar!/pkg/Test.kt',
  range = constructor }, target, ranges, 'pkg/Test.kt'), true)
check('wrong archive source rejected', probe.exact_definition({ uri = 'jar:file:///cache/source.jar!/other/Test.kt',
  range = constructor }, target, ranges, 'pkg/Test.kt'), false)
check('missing target rejected', probe.exact_definition({}, target, ranges), false)
check('constructor bounds valid', probe.range_in_source(constructor, lines), true)
local outside = vim.deepcopy(constructor)
outside['end'].line = #lines
check('line outside source rejected', probe.range_in_source(outside, lines), false)
outside = vim.deepcopy(constructor)
outside['end'].character = 2
check('column outside source rejected', probe.range_in_source(outside, lines), false)
check('empty range rejected', probe.range_in_source({ start = identifier.start, ['end'] = identifier.start }, lines), false)
check('UTF-16 bounds valid', probe.range_in_source({ start = { line = 0, character = 0 },
  ['end'] = { line = 0, character = 2 } }, { '😀' }), true)
check('UTF-16 column outside source rejected', probe.range_in_source({ start = { line = 0, character = 0 },
  ['end'] = { line = 0, character = 3 } }, { '😀' }), false)
local outcome_cases = {
  { name = 'unstarted is not measured', attempted = false, expected = 'not-run' },
  { name = 'deadline without RPC is unanswered', err = 'timeout', expected = 'unanswered' },
  { name = 'missing RPC is unanswered', expected = 'unanswered' },
  { name = 'send failure is operational', err = 'request failed', expected = 'protocol-error' },
  { name = 'RPC error cannot use its result', response = { result = { useful = true }, err = { code = -32603 } },
    accepted = true, expected = 'protocol-error' },
  { name = 'transport error cannot use a result', response = { result = {} }, err = 'interrupted',
    accepted = true, expected = 'protocol-error' },
  { name = 'valid nil positive is a functional mismatch', response = {}, expected = 'mismatch' },
  { name = 'valid JSON null positive is a functional mismatch', response = { result = vim.NIL }, expected = 'mismatch' },
  { name = 'valid empty positive is a functional mismatch', response = { result = {} }, expected = 'mismatch' },
  { name = 'valid wrong result is a functional mismatch', response = { result = { wrong = true } }, expected = 'mismatch' },
  { name = 'valid useful result is matched', response = { result = { useful = true } }, accepted = true, expected = 'matched' },
  { name = 'valid nil reviewed negative is matched', response = {}, accepted = true, expected = 'matched' },
  { name = 'valid JSON null reviewed negative is matched', response = { result = vim.NIL }, accepted = true, expected = 'matched' },
}
for _, case in ipairs(outcome_cases) do
  local evaluated = false
  local actual = probe.request_outcome(case.response, case.err, function()
    evaluated = true
    return case.accepted == true
  end, case.attempted)
  check(case.name, actual, case.expected)
  check(case.name .. ': predicate requires a valid RPC', evaluated,
    case.expected == 'matched' or case.expected == 'mismatch')
end

-- Independently written full-text payloads follow the installed Neovim LSP encoding.
for _, case in ipairs({
  { format = 'unix', eol = true, text = 'first😀\nsecond\n' },
  { format = 'unix', eol = false, text = 'first😀\nsecond' },
  { format = 'dos', eol = true, text = 'first😀\r\nsecond\r\n' },
  { format = 'dos', eol = false, text = 'first😀\r\nsecond' },
  { format = 'mac', eol = true, text = 'first😀\rsecond\r' },
  { format = 'mac', eol = false, text = 'first😀\rsecond' },
}) do
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'first😀', 'second' })
  vim.bo[buffer].fileformat, vim.bo[buffer].eol = case.format, case.eol
  local name = case.format .. (case.eol and ' with final separator' or ' without final separator')
  check(name .. ': exact full text', probe.buffer_full_text(buffer), case.text)
  check(name .. ': exact full-text hash', probe.buffer_sha256(buffer), vim.fn.sha256(case.text))
  vim.api.nvim_buf_delete(buffer, { force = true })
end

local function wire_change(text, start_line, start_character, end_line, end_character, length)
  return { text = text, rangeLength = length, range = {
    start = { line = start_line, character = start_character },
    ['end'] = { line = end_line, character = end_character },
  } }
end

-- Wire payloads and expected whole texts are independent literals, including shifted later ranges.
for _, case in ipairs({
  { name = 'incremental ASCII replacement', before = 'before\n',
    changes = { wire_change('after', 0, 0, 0, 6, 6) }, expected = 'after\n' },
  { name = 'incremental UTF-16 Japanese and surrogate pair', before = '甲😀乙\n次行\n',
    changes = { wire_change('文', 0, 1, 0, 3, 2) }, expected = '甲文乙\n次行\n' },
  { name = 'incremental zero-length insertion', before = '甲😀乙',
    changes = { wire_change('文', 0, 1, 0, 1, 0) }, expected = '甲文😀乙' },
  { name = 'incremental deletion', before = '甲😀乙',
    changes = { wire_change('', 0, 1, 0, 3, 2) }, expected = '甲乙' },
  { name = 'incremental sequential ranges use preceding edits', before = '甲😀乙\n次行\n',
    changes = { wire_change('文', 0, 1, 0, 1, 0), wire_change('', 0, 2, 0, 4, 2),
      wire_change('!\n新', 0, 2, 1, 1, 3), wire_change('終', 2, 0, 2, 0, 0) },
    expected = '甲文!\n新行\n終' },
  { name = 'incremental CRLF range preserves wire bytes', before = '甲😀乙\r\n次行\r\n',
    changes = { wire_change('文\r\n新', 0, 1, 1, 1, 6) }, expected = '甲文\r\n新行\r\n' },
  { name = 'incremental CR range preserves wire bytes', before = '甲😀乙\r次行\r',
    changes = { wire_change('文\r新', 0, 1, 1, 1, 5) }, expected = '甲文\r新行\r' },
  { name = 'incremental empty document insertion', before = '',
    changes = { wire_change('日本語', 0, 0, 0, 0, 0) }, expected = '日本語' },
  { name = 'incremental insertion at final empty line', before = '甲\n',
    changes = { wire_change('乙', 1, 0, 1, 0, 0) }, expected = '甲\n乙' },
  { name = 'full replacement remains supported', before = 'old\r\n',
    changes = { { text = 'new\n' } }, expected = 'new\n' },
  { name = 'full then ranged changes use replacement text', before = 'old\r\n',
    changes = { { text = '再😀\n' }, wire_change('検', 0, 1, 0, 3, 2) }, expected = '再検\n' },
  { name = 'ranged then full changes use array order', before = '甲😀乙',
    changes = { wire_change('文', 0, 1, 0, 3, 2), { text = '' } }, expected = '' },
  { name = 'UTF-8 negotiated byte positions', encoding = 'utf-8', before = '甲😀乙',
    changes = { wire_change('文', 0, 3, 0, 7, 4) }, expected = '甲文乙' },
  { name = 'UTF-32 negotiated character positions', encoding = 'utf-32', before = '甲😀乙',
    changes = { wire_change('文', 0, 1, 0, 2, 1) }, expected = '甲文乙' },
}) do
  local text, error = probe.apply_content_changes(case.before, case.changes, case.encoding)
  check(case.name .. ': exact reconstructed text', text, case.expected)
  check(case.name .. ': valid payload has no error', error, nil)
end
for _, case in ipairs({
  { name = 'surrogate start boundary', changes = { wire_change('x', 0, 2, 0, 3) } },
  { name = 'surrogate end boundary', changes = { wire_change('x', 0, 1, 0, 2) } },
  { name = 'column beyond source', changes = { wire_change('x', 0, 1, 0, 5) } },
  { name = 'line beyond source', changes = { wire_change('x', 3, 0, 3, 0) } },
  { name = 'negative position', changes = { wire_change('x', -1, 0, 0, 0) } },
  { name = 'fractional position', changes = { wire_change('x', 0, 1.5, 0, 3) } },
  { name = 'reversed same-line range', changes = { wire_change('x', 0, 3, 0, 1) } },
  { name = 'reversed multiline range', changes = { wire_change('x', 1, 0, 0, 1) } },
  { name = 'wrong replaced UTF-16 length', changes = { wire_change('x', 0, 1, 0, 3, 1) } },
  { name = 'negative replaced length', changes = { wire_change('x', 0, 1, 0, 3, -1) } },
  { name = 'missing range endpoint', changes = { { text = 'x', range = { start = { line = 0, character = 1 } } } } },
  { name = 'missing change text', changes = { { range = identifier } } },
  { name = 'empty changes', changes = {} },
  { name = 'missing changes' },
  { name = 'sparse changes', changes = { [2] = { text = 'x' } } },
  { name = 'non-array changes', changes = { [1] = { text = 'x' }, other = { text = 'y' } } },
  { name = 'unknown encoding', encoding = 'latin1', changes = { { text = 'x' } } },
  { name = 'UTF-8 codepoint split', encoding = 'utf-8', changes = { wire_change('x', 0, 4, 0, 7) } },
  { name = 'later invalid change rejects entire reconstruction',
    changes = { wire_change('文', 0, 1, 0, 3, 2), wire_change('x', 0, 20, 0, 20) } },
}) do
  local text, error = probe.apply_content_changes('甲😀乙\n次行\n', case.changes, case.encoding)
  check(case.name .. ': no reconstructed snapshot', text, nil)
  check(case.name .. ': explicit wire error', type(error), 'string')
end

for _, case in ipairs({
  { name = 'versioned diagnostic', version = 2, diagnostics = { { message = 'required' } }, expected = 'matched' },
  { name = 'versionless empty diagnostic', diagnostics = {}, expected = 'mismatch', limited = true },
  { name = 'old version ignored', version = 1, diagnostics = {}, expected = 'unanswered' },
  { name = 'no diagnostic observed', silent = true, expected = 'unanswered' },
  { name = 'pre-edit diagnostic ignored', before = true, silent = true, expected = 'unanswered' },
  { name = 'later useful diagnostic cannot replace first empty', version = 2, diagnostics = {}, later = true, expected = 'mismatch' },
  { name = 'changed buffer cannot certify diagnostic', version = 2, diagnostics = {}, changed = true, expected = 'protocol-error' },
  { name = 'same text with another edit cannot certify diagnostic', version = 2, diagnostics = {}, reticked = true, expected = 'protocol-error' },
  { name = 'later buffer version cannot certify diagnostic', version = 2, diagnostics = {}, version_changed = true, expected = 'protocol-error' },
  { name = 'multiple outbound changes cannot certify diagnostic', version = 2, diagnostics = {}, resent = true, expected = 'protocol-error' },
  { name = 'failed change send is not measured', send_failed = true, silent = true, expected = 'not-run' },
  { name = 'diagnostic error is not measured', version = 2, diagnostics = {}, error = { code = -32603 }, expected = 'protocol-error' },
  { name = 'wrong full-text wire hash cannot certify diagnostic', version = 2, diagnostics = { { message = 'required' } },
    wrong_wire = true, expected = 'protocol-error' },
  { name = 'incremental notification proves exact wire correlation', version = 2, diagnostics = { { message = 'required' } },
    incremental = true, expected = 'matched' },
  { name = 'multiple incremental changes prove exact wire correlation', version = 2,
    diagnostics = { { message = 'required' } }, expected = 'matched',
    changes = { wire_change('aft', 0, 0, 0, 3, 3), wire_change('er', 0, 3, 0, 6, 3) } },
  { name = 'UTF-16 incremental notification proves exact wire correlation', version = 2,
    old_lines = { '甲😀乙' }, new_lines = { '甲文乙' }, diagnostics = { { message = 'required' } }, expected = 'matched',
    changes = { wire_change('文', 0, 1, 0, 3, 2) } },
  { name = 'valid wrong incremental wire cannot certify diagnostic', version = 2,
    diagnostics = { { message = 'required' } }, expected = 'protocol-error',
    changes = { wire_change('wrong', 0, 0, 0, 6, 6) } },
  { name = 'invalid incremental range cannot certify diagnostic', version = 2,
    diagnostics = { { message = 'required' } }, expected = 'protocol-error',
    changes = { wire_change('after', 0, 6, 0, 10) } },
}) do
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(buffer, vim.fn.tempname() .. '.kt')
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, case.old_lines or { 'before' })
  vim.bo[buffer].fileformat, vim.bo[buffer].eol = 'unix', true
  vim.lsp.util.buf_versions[buffer] = 1
  local forwarded = 0
  local old_handler = function() forwarded = forwarded + 1 end
  local old_notify = function() return not case.send_failed end
  local client = { offset_encoding = 'utf-16',
    handlers = { ['textDocument/publishDiagnostics'] = old_handler }, rpc = { notify = old_notify } }
  local row, finish = probe.observe_diagnostics(client, buffer)
  local function publish(version, diagnostics)
    client.handlers['textDocument/publishDiagnostics'](case.error,
      { uri = vim.uri_from_bufnr(buffer), version = version, diagnostics = diagnostics }, {})
  end
  if case.before then publish(1, {}) end
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, case.new_lines or { 'after' })
  vim.lsp.util.buf_versions[buffer] = 2
  local changes = { { text = case.wrong_wire and 'after\r\n' or 'after\n' } }
  if case.incremental then changes = { wire_change('after', 0, 0, 0, 6, 6) } end
  if case.changes then changes = case.changes end
  client.rpc.notify('textDocument/didChange', { textDocument = { uri = vim.uri_from_bufnr(buffer), version = 2 },
    contentChanges = changes })
  if case.changed then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'another edit' }) end
  if not case.silent then publish(case.version, case.diagnostics) end
  if case.later then publish(2, { { message = 'required' } }) end
  if case.reticked then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'after' }) end
  if case.version_changed then vim.lsp.util.buf_versions[buffer] = 3 end
  if case.resent then
    client.rpc.notify('textDocument/didChange', { textDocument = { uri = vim.uri_from_bufnr(buffer), version = 2 },
      contentChanges = changes })
  end
  local _, observed = finish(5)
  check(case.name, probe.request_outcome(observed.response, observed.error,
    function(result) return #result.diagnostics > 0 end, observed.attempted), case.expected)
  check(case.name .. ': restores notification handler', client.handlers['textDocument/publishDiagnostics'] == old_handler, true)
  check(case.name .. ': restores outbound hook', client.rpc.notify == old_notify, true)
  check(case.name .. ': records outbound version', row.outbound[1].version, 2)
  if case.expected == 'matched' or case.expected == 'mismatch' then
    check(case.name .. ': reconstructed wire matches actual buffer', row.wire_matches_snapshot, true)
    check(case.name .. ': reconstructed wire SHA matches actual buffer', row.wire_source_sha256, probe.buffer_sha256(buffer))
    check(case.name .. ': records armed old snapshot', row.before_source_sha256,
      vim.fn.sha256(table.concat(case.old_lines or { 'before' }, '\n') .. '\n'))
  end
  if case.limited then
    check(case.name .. ': versionless generation is explicitly limited', row.generation_certified == false
      and type(row.generation_limit) == 'string', true)
  end
  if not case.silent or case.before then check(case.name .. ': original handler receives notifications', forwarded > 0, true) end
  vim.lsp.util.buf_versions[buffer] = nil
  vim.api.nvim_buf_delete(buffer, { force = true })
end


-- Canonical compiler receipt controls reuse the real buffer/outbound observer and RPC envelopes.
for _, case in ipairs({
  { name = 'fast versionless empty then accepted error', expected = 'matched', final_push = true },
  { name = 'incremental wire permits canonical diagnostic pull', expected = 'matched',
    changes = { wire_change('aft', 0, 0, 0, 3, 3), wire_change('er', 0, 3, 0, 6, 3) } },
  { name = 'wrong incremental wire prevents canonical diagnostic pull', expected = 'not-run',
    changes = { wire_change('wrong', 0, 0, 0, 6, 6) } },
  { name = 'clean canonical empty is a positive mismatch', clean = true, empty = true, expected = 'mismatch' },
  { name = 'partial canonical wrong empty is a positive mismatch', empty = true, expected = 'mismatch' },
  { name = 'pre-arm accepted receipt is stale', preexisting = true, no_accept = true, expected = 'not-run' },
  { name = 'other source SHA cannot certify result', wrong_sha = true, expected = 'not-run' },
  { name = 'other file receipt cannot certify result', wrong_path = true, expected = 'not-run' },
  { name = 'stale canonical store epoch is unmeasured', stale_epoch = true, expected = 'protocol-error' },
  { name = 'receipt fields must match its epoch', bad_epoch = true, expected = 'not-run' },
  { name = 'body edit during acceptance wait is unmeasured', changed_wait = true, expected = 'not-run' },
  { name = 'source ABA during acceptance wait is unmeasured', aba_wait = true, expected = 'not-run' },
  { name = 'body edit during diagnostic RPC is unmeasured', changed_rpc = true, expected = 'protocol-error' },
  { name = 'unavailable trace is unmeasured', no_trace = true, expected = 'not-run' },
  { name = 'unanswered diagnostic RPC is unmeasured', no_reply = true, expected = 'unanswered' },
  { name = 'error diagnostic RPC is unmeasured', rpc_error = true, expected = 'protocol-error' },
  { name = 'missing pull trace is unmeasured', no_pull = true, expected = 'protocol-error' },
  { name = 'partial compiler pass cannot certify absence', require_complete = true, empty = true,
    negative = true, expected = 'protocol-error' },
  { name = 'clean compiler pass can certify selected diagnostic absence', require_complete = true, clean = true,
    empty = true, negative = true, expected = 'matched' },
  { name = 'unrelated fast index revision preserves canonical epoch', index_changed = true, expected = 'matched' },
  { name = 'DOS full-text receipt matches wire SHA', fileformat = 'dos', lines = { 'after', 'peer' },
    wire_text = 'after\r\npeer\r\n', expected = 'matched' },
  { name = 'old Mac full-text receipt matches wire SHA', fileformat = 'mac', lines = { 'after', 'peer' },
    wire_text = 'after\rpeer\r', expected = 'matched' },
  { name = 'DOS without final separator matches wire SHA', fileformat = 'dos', eol_off = true, lines = { 'after', 'peer' },
    wire_text = 'after\r\npeer', expected = 'matched' },
  { name = 'Unix without final separator matches wire SHA', eol_off = true, lines = { 'after', 'peer' },
    wire_text = 'after\npeer', expected = 'matched' },
  { name = 'wrong full-text wire cannot certify canonical receipt', fileformat = 'dos', lines = { 'after', 'peer' },
    wire_text = 'after\r\npeer\r\n', wrong_wire = true, expected = 'not-run' },
  { name = 'missing full-text wire cannot certify canonical receipt', missing_wire = true, expected = 'not-run' },
  { name = 'format change during acceptance wait is unmeasured', format_wait = true, expected = 'not-run' },
}) do
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(buffer, vim.fn.tempname() .. '.kt')
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'before' })
  vim.bo[buffer].fileformat, vim.bo[buffer].eol = case.fileformat or 'unix', not case.eol_off
  vim.lsp.util.buf_versions[buffer] = 1
  local trace, uri = vim.fn.tempname(), vim.uri_from_bufnr(buffer)
  vim.fn.writefile({}, trace)
  local wire_text, after_lines = case.wire_text or 'after\n', case.lines or { 'after' }
  local source_sha256, requests = vim.fn.sha256(wire_text), 0
  local items = case.empty and {} or { { message = 'required', severity = 1,
    range = { start = { line = 0, character = 0 }, ['end'] = { line = 0, character = 5 } } } }
  local accepted_id = 'kotlin:7:4:3:5'
  local result_id = case.stale_epoch and 'kotlin:7:4:2:5'
    or (case.index_changed and 'kotlin:7:4:3:6' or accepted_id)
  local function append(name, category, args)
    vim.fn.writefile({ vim.json.encode({ name = name, cat = category, ph = 'X', ts = 1, dur = 1, args = args }) }, trace, 'a')
  end
  local function accept()
    append('kotlin_diagnostics_accepted', 'compiler', {
      path = case.wrong_path and vim.api.nvim_buf_get_name(buffer) .. '.other' or vim.api.nvim_buf_get_name(buffer),
      source_sha256 = case.wrong_sha and string.rep('0', 64) or source_sha256,
      result_id = accepted_id, source_generation = 4, store_epoch = case.bad_epoch and 4 or 3,
      count = case.clean and #items or 1, clean = case.clean == true,
      compiler_version = '2.4.0', compiler_inputs_sha256 = string.rep('b', 64),
    })
  end
  if case.preexisting then accept() end
  local old_handler, old_notify = function() end, function() return true end
  local client = { handlers = { ['textDocument/publishDiagnostics'] = old_handler }, rpc = { notify = old_notify } }
  local function publish(diagnostics)
    client.handlers['textDocument/publishDiagnostics'](nil, { uri = uri, diagnostics = diagnostics }, {})
  end
  function client:request_sync(method, params, timeout, target_buffer)
    requests = requests + 1
    assert(method == 'textDocument/diagnostic' and params.textDocument.uri == uri and target_buffer == buffer)
    assert(timeout == 5, 'RPC budget must be preserved')
    if case.no_reply then return nil, 'timeout' end
    if case.rpc_error then return { err = { code = -32603 } } end
    if case.changed_rpc then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'during RPC' }) end
    if not case.no_pull then append('diagnostics_pull', 'lsp', { path = vim.api.nvim_buf_get_name(buffer),
      result_id = result_id, count = #items }) end
    if case.final_push then publish(items) end
    return { result = { kind = 'full', resultId = result_id, items = items } }
  end
  local result, observation = probe.canonical_kotlin_diagnostics(client, buffer, {
    trace_path = case.no_trace and '' or trace, observation_timeout_ms = 30, request_timeout_ms = 5,
    compiler_version = '2.4.0', require_complete = case.require_complete,
    mutate = function()
      vim.api.nvim_buf_set_lines(buffer, 0, -1, false, after_lines)
      vim.lsp.util.buf_versions[buffer] = 2
      local changes = not case.missing_wire and { { text = case.wrong_wire and 'after\npeer\n' or wire_text } } or nil
      if case.changes then changes = case.changes end
      client.rpc.notify('textDocument/didChange', { textDocument = { uri = uri, version = 2 }, contentChanges = changes })
      publish({})
      if case.changed_wait or case.aba_wait or case.format_wait then
        vim.defer_fn(function()
          if case.format_wait then vim.bo[buffer].fileformat = 'dos'
          else
            vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'different body' })
            if case.aba_wait then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, after_lines) end
          end
          accept()
        end, 5)
      elseif not case.no_accept then accept() end
    end,
    flush = function() end,
  })
  local evaluated = false
  local outcome = probe.request_outcome(observation.response, observation.error, function(report)
    evaluated = true
    return case.negative and #report.items == 0 or (not case.negative and #report.items > 0)
  end, observation.attempted)
  check(case.name, outcome, case.expected)
  check(case.name .. ': semantic predicate needs a certified response', evaluated,
    case.expected == 'matched' or case.expected == 'mismatch')
  check(case.name .. ': at most one standard pull', requests, observation.attempted and 1 or 0)
  check(case.name .. ': original handler restored', client.handlers['textDocument/publishDiagnostics'] == old_handler, true)
  check(case.name .. ': original outbound hook restored', client.rpc.notify == old_notify, true)
  check(case.name .. ': producer closure stays unknown', observation.producer_closure_complete, false)
  if case.final_push then
    check(case.name .. ': first fast empty stays delivery evidence', #observation.push.response.result.diagnostics, 0)
    check(case.name .. ': accepted partial error is usable positive evidence', observation.canonical_certified, true)
    check(case.name .. ': partial pass does not certify absence', observation.absence_certified, false)
  end
  if case.index_changed then check(case.name .. ': canonical epoch excludes fast revision', observation.canonical_epoch, '7:4:3') end
  if case.expected == 'matched' or case.expected == 'mismatch' then
    check(case.name .. ': exposes the correlated RPC items', result == observation.response.result, true)
    check(case.name .. ': records actual full-text wire SHA', observation.wire_source_sha256, source_sha256)
    check(case.name .. ': buffer full text agrees with sent wire', observation.wire_matches_snapshot, true)
  end
  vim.fn.delete(trace)
  vim.lsp.util.buf_versions[buffer] = nil
  vim.api.nvim_buf_delete(buffer, { force = true })
end

local _, retrofit = dofile(directory .. '/nvim_retrofit_mixed_probes.lua')
local function service_references(length, include, members)
  local sites, result = {}, {}
  -- Independently reviewed Retrofit identifier and Java member-expression positions.
  for index, point in ipairs({ { 82, 18 }, { 41, 37 }, { 57, 23, 12 }, { 75, 26, 15 } }) do
    if include or index ~= 1 then
      local site = location(directory .. '/corpus-matcher-retrofit-' .. index,
        { start = { line = point[1], character = point[2] }, ['end'] = { line = point[1], character = point[2] + length } })
      site.member_start = point[3]
      sites[#sites + 1] = site
      local actual = vim.deepcopy(site)
      actual.member_start = nil
      if members and point[3] then actual.range.start.character = point[3] end
      result[#result + 1] = actual
    end
  end
  return retrofit.references(probe, sites), result
end
for _, length in ipairs({ 7, 11 }) do
  local accepts, identifiers = service_references(length, true, false)
  local _, members = service_references(length, true, true)
  check('service identifiers length ' .. length, accepts(identifiers), true)
  check('reviewed Java member expressions length ' .. length, accepts(members), true)
  local duplicate = vim.deepcopy(members)
  duplicate[4] = identifiers[3]
  check('identifier/member duplicate length ' .. length, accepts(duplicate), false)
end
local accepts, member_refs = service_references(7, true, true)
local wrong = vim.deepcopy(member_refs)
wrong[3].uri = vim.uri_from_fname(other)
check('service reference wrong file rejected', accepts(wrong), false)
wrong = vim.deepcopy(member_refs)
wrong[3].range['end'].character = wrong[3].range['end'].character + 2
check('service call parentheses rejected', accepts(wrong), false)
wrong = vim.deepcopy(member_refs)
wrong[3].range.start.character = wrong[3].range.start.character - 1
check('service wider receiver rejected', accepts(wrong), false)
wrong = vim.deepcopy(member_refs)
wrong[2].range.start.character = wrong[2].range.start.character - 11
check('Kotlin receiver widening rejected', accepts(wrong), false)
local without_declaration, uses = service_references(7, false, true)
check('service uses without declaration accepted', without_declaration(uses), true)
check('unexpected service declaration rejected', without_declaration(member_refs), false)
check('missing service declaration rejected', accepts(uses), false)
check('missing service references rejected', accepts(vim.NIL), false)
local wrapper = { uri = 'jar:file:///cache/kotlin-stdlib-1.9.23-sources.jar!/jvmMain/kotlin/text/StringsJVM.kt',
  range = { start = { line = 403, character = 32 }, ['end'] = { line = 403, character = 41 } } }
check('reviewed one-argument substring wrapper accepted', retrofit.substring(probe, wrapper), true)
check('reviewed JDK8 substring accepted', retrofit.substring(probe,
  { uri = 'jar:file:///cache/jdk8/src.zip!/java/lang/String.java',
    range = { start = { line = 1924, character = 18 }, ['end'] = { line = 1924, character = 27 } } }), true)
wrong = vim.deepcopy(wrapper)
wrong.range.start.line, wrong.range['end'].line = 412, 412
check('two-argument substring wrapper rejected', retrofit.substring(probe, wrong), false)
wrong = vim.deepcopy(wrapper)
wrong.uri = wrong.uri:gsub('1%.9%.23', '1.9.24')
check('unreviewed substring source version rejected', retrofit.substring(probe, wrong), false)
check('multiple substring targets rejected', retrofit.substring(probe, { wrapper, wrapper }), false)
check('missing substring target rejected', retrofit.substring(probe, vim.NIL), false)
print(('PASS corpus matcher and request outcomes: %d checks'):format(count))
