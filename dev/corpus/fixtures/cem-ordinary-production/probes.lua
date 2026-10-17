-- Existing corpus extra hook; one first-response hover and signature per ordinary production call.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local manifest = ctx.repo .. '/dev/corpus/cem-ordinary-production.json'
  local spec = vim.json.decode(table.concat(vim.fn.readfile(manifest), '\n'))
  local requests = {}
  local function contains(text, wanted, excluded)
    if type(text) ~= 'string' then return false end
    for _, item in ipairs(wanted or {}) do if not text:find(item, 1, true) then return false end end
    for _, item in ipairs(excluded or {}) do if text:find(item, 1, true) then return false end end
    return true
  end
  local function hover_text(value)
    if type(value) ~= 'table' then return nil end
    local content = value.contents
    if type(content) == 'string' then return content end
    if type(content) ~= 'table' then return nil end
    if type(content.value) == 'string' then return content.value end
    local parts = {}
    for _, part in ipairs(content) do
      if type(part) == 'string' then parts[#parts + 1] = part
      elseif type(part) == 'table' and type(part.value) == 'string' then parts[#parts + 1] = part.value end
    end
    return table.concat(parts, '\n')
  end
  for _, case in ipairs(spec.probes) do
    local path = vim.fs.abspath(ctx.project .. '/' .. case.source)
    local source = assert(ctx.sources[path], 'ordinary caller must be opened by original corpus probes')
    local lines = vim.api.nvim_buf_get_lines(source.buf, 0, -1, false)
    local snapshot = probe.buffer_full_text(source.buf)
    local tick, version = vim.api.nvim_buf_get_changedtick(source.buf), vim.lsp.util.buf_versions[source.buf]
    assert(source.sha256 == case.source_sha256, 'normalized disk oracle changed')
    local raw_file = assert(io.open(path, 'rb'))
    local disk_bytes = raw_file:read('*a')
    raw_file:close()
    assert(vim.fn.sha256(disk_bytes) == case.source_raw_sha256, 'exact disk bytes changed')
    assert(snapshot == disk_bytes and vim.fn.sha256(snapshot) == case.source_raw_sha256,
      'wire body differs from exact pinned disk bytes')
    local position = probe.find(lines, case.anchor, case.token)
    for _, method in ipairs({ 'textDocument/hover', 'textDocument/signatureHelp' }) do
      local at = vim.deepcopy(position)
      if method == 'textDocument/signatureHelp' then
        at.character = at.character + vim.str_utfindex(case.token, 'utf-16') + 1 -- inside first argument
      end
      local row = { case = case.id .. ':' .. method, method = method, attempts = 1,
        phase = 'first-response-after-indexing', source_sha256 = vim.fn.sha256(snapshot),
        expected_version = version, changedtick = tick }
      ctx.request_outcomes[#ctx.request_outcomes + 1] = row
      requests[#requests + 1] = row
      row.params = { textDocument = { uri = vim.uri_from_bufnr(source.buf) }, position = at }
      row.response, row.error = ctx.client:request_sync(method, row.params, 5000, source.buf)
      local function accepts(value)
        if method == 'textDocument/hover' then
          return contains(hover_text(value), case.hover_contains, case.signature_excludes)
        end
        if type(value) ~= 'table' or type(value.signatures) ~= 'table' then return false end
        local active = value.activeSignature or 0
        if type(active) ~= 'number' or active < 0 or active % 1 ~= 0 then return false end
        local selected = value.signatures[active + 1]
        return type(selected) == 'table' and type(selected.parameters) == 'table'
          and #selected.parameters == case.signature_parameters
          and contains(selected.label, case.signature_contains, case.signature_excludes)
          and (value.activeParameter == nil or value.activeParameter == 0)
      end
      row.outcome = probe.request_outcome(row.response, row.error, accepts)
      if probe.buffer_full_text(source.buf) ~= snapshot or vim.api.nvim_buf_get_changedtick(source.buf) ~= tick
        or vim.lsp.util.buf_versions[source.buf] ~= version
        or not vim.deep_equal(vim.fn.readfile(path), source.disk) then
        row.correlation_error = 'source-changed-during-request'
        row.outcome = 'source-changed'
      end
      row.response_received = row.response ~= nil and row.error == nil and row.response.err == nil
      ctx.check(row.case, row.outcome == 'matched', vim.inspect(row), 'semantic', row.outcome)
    end
  end
  vim.fn.writefile({ vim.json.encode({ scope = 'six ordinary production hover/signature first responses; exact raw/wire equality, no buffer edits or retries', requests = requests }) },
    ctx.artifacts .. '/ordinary-hover-signature.json')
end
