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
print(('PASS corpus matcher and request outcomes: %d checks'):format(count))
