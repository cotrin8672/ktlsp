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
}) do
  local buffer = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(buffer, vim.fn.tempname() .. '.kt')
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'before' })
  vim.lsp.util.buf_versions[buffer] = 1
  local forwarded = 0
  local old_handler = function() forwarded = forwarded + 1 end
  local old_notify = function() return not case.send_failed end
  local client = { handlers = { ['textDocument/publishDiagnostics'] = old_handler }, rpc = { notify = old_notify } }
  local row, finish = probe.observe_diagnostics(client, buffer)
  local function publish(version, diagnostics)
    client.handlers['textDocument/publishDiagnostics'](case.error,
      { uri = vim.uri_from_bufnr(buffer), version = version, diagnostics = diagnostics }, {})
  end
  if case.before then publish(1, {}) end
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'after' })
  vim.lsp.util.buf_versions[buffer] = 2
  client.rpc.notify('textDocument/didChange', { textDocument = { uri = vim.uri_from_bufnr(buffer), version = 2 } })
  if case.changed then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'another edit' }) end
  if not case.silent then publish(case.version, case.diagnostics) end
  if case.later then publish(2, { { message = 'required' } }) end
  if case.reticked then vim.api.nvim_buf_set_lines(buffer, 0, -1, false, { 'after' }) end
  if case.version_changed then vim.lsp.util.buf_versions[buffer] = 3 end
  if case.resent then
    client.rpc.notify('textDocument/didChange', { textDocument = { uri = vim.uri_from_bufnr(buffer), version = 2 } })
  end
  local _, observed = finish(5)
  check(case.name, probe.request_outcome(observed.response, observed.error,
    function(result) return #result.diagnostics > 0 end, observed.attempted), case.expected)
  check(case.name .. ': restores notification handler', client.handlers['textDocument/publishDiagnostics'] == old_handler, true)
  check(case.name .. ': restores outbound hook', client.rpc.notify == old_notify, true)
  check(case.name .. ': records outbound version', row.outbound[1].version, 2)
  if case.limited then
    check(case.name .. ': versionless generation is explicitly limited', row.generation_certified == false
      and type(row.generation_limit) == 'string', true)
  end
  if not case.silent or case.before then check(case.name .. ': original handler receives notifications', forwarded > 0, true) end
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
