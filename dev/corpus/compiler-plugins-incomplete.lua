-- Extra probe for the compiler-plugins corpus, using only unsaved source edits.
return function(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  for _, name in ipairs({ 'Model', 'OpenBase' }) do
  local path = vim.fs.abspath(ctx.project .. '/src/main/kotlin/pluginfixture/' .. name .. '.kt')
  local disk = vim.fn.readfile(path)
  local buf = vim.fn.bufadd(path)
  vim.fn.bufload(buf)
  vim.bo[buf].filetype = 'kotlin'
  ctx.sources[path] = { buf = buf, disk = disk, sha256 = vim.fn.sha256(table.concat(disk, '\n')) }
  assert(vim.lsp.buf_attach_client(buf, ctx.client.id))
  local function flush() require('vim.lsp._changetracking').flush(ctx.client) end
  local ok, err = xpcall(function()
    local lines = vim.deepcopy(disk)
    lines[#lines + 1] = ''
    lines[#lines + 1] = 'fun incompleteSameFile() = Model.'
    local diagnostics, observation = probe.canonical_kotlin_diagnostics(ctx.client, buf, {
      observation_timeout_ms = 120000, request_timeout_ms = 5000, compiler_version = '1.9.23',
      edit_label = 'unfinished package function beside valid ' .. name .. ' classes',
      mutate = function() vim.api.nvim_buf_set_lines(buf, 0, -1, true, lines) end, flush = flush,
    })
    vim.fn.writefile({ vim.json.encode(observation) }, ctx.artifacts .. '/incomplete-' .. name .. '-diagnostics.json')
    local retained = false
    for _, diagnostic in ipairs(diagnostics and diagnostics.items or {}) do
      if diagnostic.severity == 1 and diagnostic.range.start.line == #lines - 1
        and diagnostic.message:lower():find('expecting an element', 1, true) then retained = true end
    end
    ctx.check('same-file ' .. name .. ': current compiler syntax diagnostic retained',
      observation.canonical_certified and retained, observation.error, 'semantic', observation.availability)
    if not observation.canonical_certified then return end
    for _, consumer in ipairs({
      { 'Java', 'src/main/java/pluginfixture/JavaUse.java', 'Model.Companion.serializer()' },
      { 'Kotlin', 'src/main/kotlin/pluginfixture/KotlinUse.kt', 'Model.serializer()' },
    }) do
    local source = assert(ctx.sources[vim.fs.abspath(ctx.project .. '/' .. consumer[2])],
      'consumer must be an existing corpus probe')
    local position = probe.find(source.disk, consumer[3], 'serializer')
    position.character = position.character + #'serializer'
    -- The unchanged Java consumer also calls NeedsLabel(); its namespace requires actual Javac success.
    local response, request_error = ctx.client:request_sync('textDocument/completion', {
      textDocument = { uri = vim.uri_from_bufnr(source.buf) }, position = position, context = { triggerKind = 1 },
    }, 5000, source.buf)
    local outcome = probe.request_outcome(response, request_error, function(value)
      return probe.completion_has(value, 'serializer')
    end)
    local row = { case = 'same-file-' .. name .. '-' .. consumer[1], method = 'textDocument/completion', attempts = 1,
      outcome = outcome, response = response, error = request_error }
    ctx.request_outcomes[#ctx.request_outcomes + 1] = row
    vim.fn.writefile({ vim.json.encode(row) }, ctx.artifacts .. '/incomplete-' .. name .. '-' .. consumer[1] .. '.json')
    ctx.check('same-file ' .. name .. ': ' .. consumer[1] .. ' consumer keeps actual generated API', outcome == 'matched',
      request_error, 'semantic', outcome)
    end
  end, debug.traceback)
  vim.api.nvim_buf_set_lines(buf, 0, -1, true, disk)
  flush()
  vim.bo[buf].modified = false
  assert(ok, err)
  end
end
