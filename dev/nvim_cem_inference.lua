-- Existing corpus extra hook: preserve K2 controls, then inspect a separate unsaved subclass.
return function(ctx)
  dofile(ctx.repo .. '/dev/nvim_k2_java.lua')(ctx)
  local probe = dofile(ctx.repo .. '/dev/nvim_lsp_probe.lua')
  local fixture = ctx.repo .. '/crates/ktlsp/tests/fixtures/cem-selected/CemSuperInference120.kt'
  local lines = vim.fn.readfile(fixture)
  local path = vim.fs.abspath(ctx.project .. '/src/main/kotlin/ktlsp/cem/inference120/CemSuperInference120.kt')
  assert(vim.fn.filereadable(path) == 0 and vim.fn.bufnr(path) == -1,
    'inference control must be a new disposable buffer')
  local originals = {}
  for _, source in pairs(ctx.sources) do
    originals[source.buf] = { text = probe.buffer_full_text(source.buf), tick = vim.api.nvim_buf_get_changedtick(source.buf),
      version = vim.lsp.util.buf_versions[source.buf] }
  end
  local buf = vim.api.nvim_create_buf(true, false)
  vim.api.nvim_buf_set_name(buf, path)
  vim.bo[buf].filetype = 'kotlin'
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  local row = { case = 'cem-super-return-inlay', method = 'textDocument/inlayHint', attempts = 1,
    scope = 'disposable compiler-checked abstract subclass; original corpus buffers unchanged' }
  local ok, failure = xpcall(function()
    assert(vim.lsp.buf_attach_client(buf, ctx.client.id), 'inference control attachment failed')
    require('vim.lsp._changetracking').flush(ctx.client)
    row.source = probe.buffer_full_text(buf)
    row.source_sha256 = vim.fn.sha256(row.source)
    row.version, row.changedtick = vim.lsp.util.buf_versions[buf], vim.api.nvim_buf_get_changedtick(buf)
    for line, text in ipairs(lines) do
      local start = text:find('val speed =', 1, true)
      if start then row.expected_position = { line = line - 1, character = start - 1 + #'val speed' } end
    end
    assert(row.expected_position, 'missing speed declaration')
    row.params = { textDocument = { uri = vim.uri_from_bufnr(buf) },
      range = { start = { line = 0, character = 0 }, ['end'] = { line = #lines, character = 0 } } }
    row.response, row.error = ctx.client:request_sync(row.method, row.params, 5000, buf)
    row.outcome = probe.request_outcome(row.response, row.error, function(value)
      if type(value) ~= 'table' then return false end
      for _, hint in ipairs(value) do
        if vim.deep_equal(hint.position, row.expected_position) and hint.label == ': Float' then return true end
      end
      return false
    end)
    for original, before in pairs(originals) do
      assert(probe.buffer_full_text(original) == before.text and vim.api.nvim_buf_get_changedtick(original) == before.tick
        and vim.lsp.util.buf_versions[original] == before.version, 'original corpus buffer changed')
    end
    assert(probe.buffer_full_text(buf) == row.source and vim.api.nvim_buf_get_changedtick(buf) == row.changedtick
      and vim.lsp.util.buf_versions[buf] == row.version, 'inference buffer changed during request')
    ctx.request_outcomes[#ctx.request_outcomes + 1] = row
    ctx.check('CEM super return inlay at speed is Float', row.outcome == 'matched', vim.inspect(row), 'semantic', row.outcome)
  end, debug.traceback)
  vim.api.nvim_buf_delete(buf, { force = true })
  vim.fn.writefile({ vim.json.encode(row) }, ctx.artifacts .. '/cem-super-inference.json')
  assert(ok, failure)
end
