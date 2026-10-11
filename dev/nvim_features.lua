---@diagnostic disable: undefined-global
-- Headless Neovim checks for S1 (incremental reparse via did_change) and S3 (references),
-- driven through Neovim's real LSP client against dev/sample. Run via dev/smoke_features.sh.
--
--     nvim -l dev/nvim_features.lua <project-dir> [ktlsp-bin]

local project = arg[1]
assert(project and project ~= "", "usage: nvim -l dev/nvim_features.lua <project-dir> [ktlsp-bin]")

local bin = arg[2] or os.getenv("KTLSP_BIN")
if not bin or bin == "" then
  bin = vim.fn.getcwd() .. "/target/release/ktlsp"
  if vim.fn.filereadable(bin) == 0 then
    bin = vim.fn.getcwd() .. "/target/debug/ktlsp"
  end
end
assert(vim.fn.filereadable(bin) == 1, "ktlsp binary not found — run `cargo build` first")

local script_dir = vim.fs.dirname(debug.getinfo(1, "S").source:gsub("^@", ""))
local probe = dofile(script_dir .. "/nvim_lsp_probe.lua")
local failures, checks, requests, diagnostic_probes, shutdown = {}, {}, {}, {}, {}
local function check(name, ok, detail, observation)
  local outcome = observation and probe.request_outcome(observation.response, observation.error,
    function() return ok end, observation.attempted) or (ok and "matched" or "mismatch")
  local measured = outcome == "matched" or outcome == "mismatch"
  checks[#checks + 1] = { name = name, ok = not not ok, outcome = outcome, measured = measured,
    kind = observation and "semantic" or "assertion", observation = observation }
  if not measured then
    print(("UNMEASURED %s  (%s)"):format(name, outcome))
  elseif ok then
    print(("PASS  %s"):format(name))
  else
    print(("FAIL  %s%s"):format(name, detail and ("  (" .. tostring(detail) .. ")") or ""))
  end
  if not ok or not measured then
    table.insert(failures, name)
  end
end

vim.cmd("edit " .. vim.fn.fnameescape(project .. "/Main.kt"))
local bufnr = vim.api.nvim_get_current_buf()
vim.bo[bufnr].filetype = "kotlin"

local id = vim.lsp.start({ name = "ktlsp", cmd = { bin }, root_dir = project,
  on_exit = function(code, signal) shutdown.exit = { code = code, signal = signal } end })
assert(id, "vim.lsp.start returned nil")
vim.wait(8000, function()
  local c = vim.lsp.get_client_by_id(id)
  return c ~= nil and (c.initialized == true or (c.server_capabilities and c.server_capabilities.definitionProvider))
end, 50)

local client = vim.lsp.get_client_by_id(id)
check("advertises referencesProvider", client and client.server_capabilities.referencesProvider ~= nil)
check("advertises completionProvider", client and client.server_capabilities.completionProvider ~= nil)
check("advertises hoverProvider", client and client.server_capabilities.hoverProvider ~= nil)
check("advertises documentHighlightProvider", client and client.server_capabilities.documentHighlightProvider ~= nil)
check("advertises documentSymbolProvider", client and client.server_capabilities.documentSymbolProvider ~= nil)
check("advertises workspaceSymbolProvider", client and client.server_capabilities.workspaceSymbolProvider ~= nil)
check("advertises codeActionProvider", client and client.server_capabilities.codeActionProvider ~= nil)
check("advertises foldingRangeProvider", client and client.server_capabilities.foldingRangeProvider ~= nil)
check("advertises selectionRangeProvider", client and client.server_capabilities.selectionRangeProvider ~= nil)
check("advertises semanticTokensProvider", client and client.server_capabilities.semanticTokensProvider ~= nil)
check("advertises inlayHintProvider", client and client.server_capabilities.inlayHintProvider ~= nil)
check("advertises renameProvider", client and client.server_capabilities.renameProvider ~= nil)
check("advertises signatureHelpProvider", client and client.server_capabilities.signatureHelpProvider ~= nil)
check("advertises implementationProvider", client and client.server_capabilities.implementationProvider ~= nil)
check("advertises typeDefinitionProvider", client and client.server_capabilities.typeDefinitionProvider ~= nil)
check("advertises callHierarchyProvider", client and client.server_capabilities.callHierarchyProvider ~= nil)
check("advertises executeCommandProvider", client and client.server_capabilities.executeCommandProvider ~= nil)

-- locate (0-indexed) line/col of `token` on the buffer line containing `anchor`
local function find(anchor, token)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  for i, line in ipairs(lines) do
    if line:find(anchor, 1, true) then
      local s = line:find(token, 1, true)
      if s then
        return i - 1, s - 1
      end
    end
  end
  return nil
end

local function request(method, params)
  local row = { method = method, params = vim.deepcopy(params), attempted = client ~= nil,
    attempts = client and 1 or 0, timeout_ms = 4000, started_ns = vim.uv.hrtime() }
  if client then
    local ok, response, err = pcall(client.request_sync, client, method, params, 4000, bufnr)
    if ok then row.response, row.error = response, err else row.error = response end
  end
  row.finished_ns = vim.uv.hrtime()
  row.availability = probe.request_outcome(row.response, row.error, function() return true end, row.attempted)
  if row.availability == "matched" then row.availability = "responded" end
  requests[#requests + 1] = row
  local result = row.availability == "responded" and row.response.result or nil
  if result == vim.NIL then result = nil end
  return result, row
end

local request_raw = request

local uri = vim.uri_from_bufnr(bufnr)

-- Add one unused import through the editor so code actions exercise the real didChange path.
vim.api.nvim_buf_set_lines(bufnr, 1, 1, false, {
  "import widgets.Decorator",
  "import widgets.Decorator.Badge",
  "import a.b.Unused",
})
do
  local n = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, n, n, false, {
    "",
    "fun usesNestedType(x: Decorator.Badge) {}",
  })
end
vim.wait(800)

-- ---- Goto-definition from import targets and imported nested types ----
do
  local l, c = find("import widgets.Decorator", "Decorator")
  check("found Decorator import target", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("goto on imported type -> Decor.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Decor%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end

  l, c = find("import widgets.Decorator.Badge", "Badge")
  check("found Badge import target", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("goto on imported nested type -> Decor.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Decor%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end

  l, c = find("fun usesNestedType", "Badge")
  check("found nested Badge type usage", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("goto on imported outer nested type -> Decor.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Decor%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end
end

-- ---- Passive symbol surface: document symbols, hover, highlights, workspace symbols ----
do
  local res, res_rpc = request("textDocument/documentSymbol", { textDocument = { uri = uri } })
  local has_helper, has_main = false, false
  for _, item in ipairs(res or {}) do
    if item.name == "helper" then has_helper = true end
    if item.name == "main" then has_main = true end
  end
  check("documentSymbol includes helper", has_helper, vim.inspect(res), res_rpc)
  check("documentSymbol includes main", has_main, vim.inspect(res), res_rpc)
end

do
  local l, c = find("showGreeter(helper())", "helper")
  check("found helper() call for passive requests", l ~= nil)
  if l then
    local hover, hover_rpc = request("textDocument/hover", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local hover_text = hover and hover.contents and (hover.contents.value or hover.contents)
    check("hover(helper) reports function facts", hover_text ~= nil and tostring(hover_text):find("helper", 1, true) ~= nil, vim.inspect(hover), hover_rpc)
    check("hover(helper) includes KDoc", hover_text ~= nil and tostring(hover_text):find("Helpful hover docs.", 1, true) ~= nil, vim.inspect(hover), hover_rpc)

    local highlights, highlights_rpc = request("textDocument/documentHighlight", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    check("documentHighlight(helper) returns declaration + usage", highlights ~= nil and #highlights >= 2, vim.inspect(highlights), highlights_rpc)
  end
end

do
  local res, res_rpc = request("workspace/symbol", { query = "helper" })
  local has_helper = false
  for _, item in ipairs(res or {}) do
    if item.name == "helper" then
      has_helper = true
      break
    end
  end
  check("workspace/symbol finds helper", has_helper, vim.inspect(res), res_rpc)
end

-- ---- Source/code actions: remove unused import as an LSP WorkspaceEdit ----
do
  local l, c = find("import a.b.Unused", "Unused")
  check("found unused import for codeAction", l ~= nil)
  if l then
    local actions, actions_rpc = request("textDocument/codeAction", {
      textDocument = { uri = uri },
      range = {
        start = { line = l, character = c },
        ["end"] = { line = l, character = c + #"Unused" },
      },
      context = {
        diagnostics = {},
        only = { "quickfix" },
      },
    })
    local found = false
    for _, action in ipairs(actions or {}) do
      if action.title == "Remove unused import `Unused`" and action.edit ~= nil then
        found = true
        break
      end
    end
    check("codeAction removes unused import", found, vim.inspect(actions), actions_rpc)
  end
end

-- ---- Parser diagnostics: broken syntax is published, semantic hints wait for a clean parse ----
do
  local start_line = vim.api.nvim_buf_line_count(bufnr)
  local observation, finish = probe.observe_diagnostics(client, bufnr)
  diagnostic_probes[#diagnostic_probes + 1] = observation
  vim.api.nvim_buf_set_lines(bufnr, start_line, start_line, false, {
    "",
    "fun syntaxProbe( { )",
  })
  require("vim.lsp._changetracking").flush(client)
  local notification = finish(4000)
  local syntax_diag = nil
  for _, d in ipairs(notification and notification.diagnostics or {}) do
    if d.source == "ktlsp" and d.severity == vim.diagnostic.severity.ERROR and d.message:find("Syntax error", 1, true) then
      syntax_diag = d
      break
    end
  end
  check("publishDiagnostics reports tree-sitter syntax error", syntax_diag ~= nil, vim.inspect(notification), observation)

  local has_unused_during_parse_error = false
  for _, d in ipairs(notification and notification.diagnostics or {}) do
    if d.source == "ktlsp" and d.message:find("Unused import", 1, true) then
      has_unused_during_parse_error = true
      break
    end
  end
  check("syntax error suppresses semantic diagnostics", not has_unused_during_parse_error, vim.inspect(notification), observation)

  local restored, finish_restore = probe.observe_diagnostics(client, bufnr)
  diagnostic_probes[#diagnostic_probes + 1] = restored
  vim.api.nvim_buf_set_lines(bufnr, start_line, start_line + 2, false, {})
  require("vim.lsp._changetracking").flush(client)
  finish_restore(4000)
end

-- ---- Indexed diagnostics: missing simple type is published only after complete indexing ----
do
  local start_line = vim.api.nvim_buf_line_count(bufnr)
  local observation, finish = probe.observe_diagnostics(client, bufnr)
  diagnostic_probes[#diagnostic_probes + 1] = observation
  vim.api.nvim_buf_set_lines(bufnr, start_line, start_line, false, {
    "",
    "fun unresolvedTypeProbe(x: MissingHarnessType) {}",
  })
  require("vim.lsp._changetracking").flush(client)
  local notification = finish(8000)
  local unresolved_diag = nil
  for _, d in ipairs(notification and notification.diagnostics or {}) do
    local is_missing_type = d.source == "ktlsp"
      and d.severity == vim.diagnostic.severity.ERROR
      and d.message == "Unresolved reference: MissingHarnessType"
    if is_missing_type then
      unresolved_diag = d
      break
    end
  end
  check(
    "publishDiagnostics reports complete-index missing type",
    unresolved_diag ~= nil,
    vim.inspect(notification), observation
  )

  local command_result, command_result_rpc = request("workspace/executeCommand", {
    command = "ktlsp.explainResolution",
    arguments = {
      {
        uri = uri,
        position = { line = start_line + 1, character = #"fun unresolvedTypeProbe(x: " },
      },
    },
  })
  check(
    "executeCommand explainResolution reports definitely-absent for closed missing type",
    command_result ~= nil
      and command_result.status == "definitely-absent"
      and command_result.kind == "type"
      and vim.tbl_isempty(command_result.reasons or {}),
    vim.inspect(command_result), command_result_rpc
  )
end

-- ---- Visual editor features: folding, selection ranges, semantic tokens, inlay hints ----
do
  local folds, folds_rpc = request("textDocument/foldingRange", { textDocument = { uri = uri } })
  check("foldingRange returns body folds", folds ~= nil and #folds >= 1, vim.inspect(folds), folds_rpc)

  local l, c = find("showGreeter(helper())", "helper")
  if l then
    local selections, selections_rpc = request("textDocument/selectionRange", {
      textDocument = { uri = uri },
      positions = { { line = l, character = c } },
    })
    check("selectionRange returns parent chain", selections ~= nil and selections[1] ~= nil and selections[1].parent ~= nil, vim.inspect(selections), selections_rpc)
  end

  local semantic, semantic_rpc = request("textDocument/semanticTokens/full", { textDocument = { uri = uri } })
  check("semanticTokens/full returns encoded tokens", semantic ~= nil and semantic.data ~= nil and #semantic.data > 0, vim.inspect(semantic), semantic_rpc)

  local hints, hints_rpc = request("textDocument/inlayHint", {
    textDocument = { uri = uri },
    range = {
      start = { line = 0, character = 0 },
      ["end"] = { line = vim.api.nvim_buf_line_count(bufnr), character = 0 },
    },
  })
  local has_type_hint = false
  for _, hint in ipairs(hints or {}) do
    if hint.label == ": String" or hint.label == ": Greeter" then
      has_type_hint = true
      break
    end
  end
  check("inlayHint returns local type hint", has_type_hint, vim.inspect(hints), hints_rpc)
end

-- ---- Expanded editor surface: rename, graph navigation, signature help, commands ----
do
  local n0 = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, n0, n0, false, {
    "",
    "interface FeatureSurface {",
    "    fun renderFeature()",
    "}",
    "class FeatureImpl : FeatureSurface {",
    "    override fun renderFeature() {}",
    "}",
    "fun addForSignature(a: Int, b: Int): Int = a + b",
    "fun callSignature(): Int = addForSignature(1, 2)",
  })
  vim.wait(800)

  local l, c = find("class FeatureImpl : FeatureSurface", "FeatureSurface")
  check("found FeatureSurface supertype usage", l ~= nil)
  if l then
    local impls, impls_rpc = request("textDocument/implementation", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local loc = impls and (impls[1] or impls)
    check("implementation(FeatureSurface) returns FeatureImpl", loc ~= nil and loc.range ~= nil, vim.inspect(impls), impls_rpc)

    local type_params = {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    }
    local types, types_rpc = request_raw("textDocument/prepareTypeHierarchy", type_params)
    local item = types and types[1]
    check("prepareTypeHierarchy returns FeatureSurface", item ~= nil and item.name == "FeatureSurface", vim.inspect(types), types_rpc)
    if item then
      local subtypes, subtypes_rpc = request_raw("typeHierarchy/subtypes", { item = item })
      local has_impl = false
      for _, subtype in ipairs(subtypes or {}) do
        if subtype.name == "FeatureImpl" then
          has_impl = true
          break
        end
      end
      check("typeHierarchy/subtypes returns FeatureImpl", has_impl, vim.inspect(subtypes), subtypes_rpc)
    end
  end

  local ml, mc = find("fun renderFeature()", "renderFeature")
  check("found FeatureSurface member", ml ~= nil)
  if ml then
    local impls, impls_rpc = request("textDocument/implementation", {
      textDocument = { uri = uri },
      position = { line = ml, character = mc },
    })
    local loc = impls and (impls[1] or impls)
    check("implementation(renderFeature) returns overriding member", loc ~= nil and loc.range ~= nil, vim.inspect(impls), impls_rpc)
  end

  local gl, gc = find("showGreeter(g.greet())", "g")
  check("found g for typeDefinition", gl ~= nil)
  if gl then
    local type_def, type_def_rpc = request("textDocument/typeDefinition", {
      textDocument = { uri = uri },
      position = { line = gl, character = gc },
    })
    local loc = type_def and (type_def[1] or type_def)
    check("typeDefinition(g) returns Greeter", loc ~= nil and loc.uri ~= nil and loc.uri:match("Greeter%.kt$") ~= nil, loc and loc.uri, type_def_rpc)
  end

  local sl, sc = find("fun callSignature(): Int = addForSignature(1, 2)", "2")
  check("found addForSignature call for signatureHelp", sl ~= nil)
  if sl then
    local sig, sig_rpc = request("textDocument/signatureHelp", {
      textDocument = { uri = uri },
      position = { line = sl, character = sc },
    })
    local ok = sig ~= nil and sig.signatures ~= nil and sig.signatures[1] ~= nil and sig.signatures[1].label:find("addForSignature", 1, true) ~= nil
    check("signatureHelp(addForSignature) returns signature", ok, vim.inspect(sig), sig_rpc)
  end

  local rl, rc = find("fun callSignature(): Int = addForSignature(1, 2)", "addForSignature")
  check("found addForSignature call for rename/hierarchy", rl ~= nil)
  if rl then
    local prepared, prepared_rpc = request("textDocument/prepareRename", {
      textDocument = { uri = uri },
      position = { line = rl, character = rc },
    })
    check("prepareRename(addForSignature) returns placeholder", prepared ~= nil and prepared.placeholder == "addForSignature", vim.inspect(prepared), prepared_rpc)

    local renamed, renamed_rpc = request("textDocument/rename", {
      textDocument = { uri = uri },
      position = { line = rl, character = rc },
      newName = "sumForSignature",
    })
    local changes = renamed and renamed.changes
    local edit_count = 0
    if changes then
      for _, edits in pairs(changes) do
        edit_count = edit_count + #edits
      end
    end
    check("rename(addForSignature) returns workspace edit", edit_count >= 2, vim.inspect(renamed), renamed_rpc)

    local prepared_calls, prepared_calls_rpc = request("textDocument/prepareCallHierarchy", {
      textDocument = { uri = uri },
      position = { line = rl, character = rc },
    })
    local call_item = prepared_calls and prepared_calls[1]
    check("prepareCallHierarchy returns addForSignature", call_item ~= nil and call_item.name == "addForSignature", vim.inspect(prepared_calls), prepared_calls_rpc)
    if call_item then
      local incoming, incoming_rpc = request("callHierarchy/incomingCalls", { item = call_item })
      local has_caller = false
      for _, call in ipairs(incoming or {}) do
        if call.from and call.from.name == "callSignature" then
          has_caller = true
          break
        end
      end
      check("callHierarchy/incomingCalls returns caller", has_caller, vim.inspect(incoming), incoming_rpc)
    end
  end

  local command_result, command_result_rpc = request("workspace/executeCommand", {
    command = "ktlsp.explainResolution",
    arguments = {
      {
        uri = uri,
        position = { line = rl or 0, character = rc or 0 },
      },
    },
  })
  check("executeCommand explainResolution returns ok", command_result ~= nil and command_result.status == "ok", vim.inspect(command_result), command_result_rpc)
end

-- ---- S3: references on `helper` (declaration + the call in main) ----
do
  local l, c = find("showGreeter(helper())", "helper")
  check("found helper() usage", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/references", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
      context = { includeDeclaration = true },
    })
    local n = res and #res or 0
    check("references(helper) returns >= 2 sites", n >= 2, "got " .. n, res_rpc)
    if res then
      check("  references are in Main.kt", vim.tbl_count(res) > 0 and res[1].uri:match("Main%.kt$") ~= nil, res[1] and res[1].uri, res_rpc)
    end
  end
end

-- ---- Stage A: scope/name completion offers a visible top-level name at an unqualified prefix ----
do
  -- Position the cursor one char into the `helper` token of the `showGreeter(helper())` call, so the
  -- completion prefix is `h` (an unqualified scope-name position).
  local l, c = find("showGreeter(helper())", "helper")
  check("found helper() call for completion", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/completion", {
      textDocument = { uri = uri },
      position = { line = l, character = c + 1 },
    })
    -- The result may be a CompletionList ({ items = {...} }) or a bare item array.
    local items = res and (res.items or res) or {}
    local has_helper = false
    for _, item in ipairs(items) do
      if item.label == "helper" then
        has_helper = true
        break
      end
    end
    check("completion offers `helper`", has_helper, "got " .. #items .. " items", res_rpc)
    local completion_explained, completion_explained_rpc = request("workspace/executeCommand", {
      command = "ktlsp.explainCompletion",
      arguments = {
        { uri = uri, position = { line = l, character = c + 1 } },
      },
    })
    check(
      "executeCommand explainCompletion returns scope-name ok",
      completion_explained ~= nil
        and completion_explained.status == "ok"
        and completion_explained.context == "scope-name",
      vim.inspect(completion_explained), completion_explained_rpc
    )
  end
end

-- ---- Stage C: member completion after `g.` offers Greeter members with a function snippet ----
do
  -- Position the cursor right after the `.` in `g.greet()` (the start of the `greet` selector).
  local l, c = find("g.greet()", "greet")
  check("found g.greet() for member completion", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/completion", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local items = res and (res.items or res) or {}
    local has_greet, has_potato = false, false
    local potato_item = nil
    for _, item in ipairs(items) do
      if item.label == "greet" then has_greet = true end
      if item.label == "potato" then
        has_potato = true
        potato_item = item
      end
    end
    check("member completion offers `greet`", has_greet, "got " .. #items .. " items", res_rpc)
    check("member completion offers `potato`", has_potato, "got " .. #items .. " items", res_rpc)
    if potato_item then
      -- `potato` is a zero-arg function: SNIPPET format (2), insertText `potato()$0`, kind FUNCTION (3).
      check("  potato insertTextFormat == Snippet (2)", potato_item.insertTextFormat == 2, tostring(potato_item.insertTextFormat), res_rpc)
      check("  potato insertText == potato()$0", potato_item.insertText ~= nil and potato_item.insertText:match("potato%(%)%$0") ~= nil, potato_item.insertText, res_rpc)
      check("  potato kind == Function (3)", potato_item.kind == 3, tostring(potato_item.kind), res_rpc)
    end
  end
end

-- ---- Stage C: an unimported, indexed type is offered WITH an auto-import additionalTextEdits ----
do
  -- `AutoImportTarget` is declared in dev/sample/Decor.kt under package `widgets` (no import in
  -- Main.kt), and referenced unimported below; completion must carry an import edit.
  local n0 = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, n0, n0, false, {
    "",
    "fun tryAutoImport() { AutoImportTarge }",
  })
  vim.wait(800)
  local l, c = find("fun tryAutoImport() { AutoImportTarge }", "AutoImportTarge")
  check("found AutoImportTarge reference for auto-import", l ~= nil)
  if l then
    -- Retain the first response; later retries cannot turn a measured mismatch into a pass.
    local auto_import_item = nil
    local res, res_rpc = request("textDocument/completion", {
      textDocument = { uri = uri },
      position = { line = l, character = c + #"AutoImportTarge" },
    })
    local items = res and (res.items or res) or {}
    for _, item in ipairs(items) do
      if item.label == "AutoImportTarget" then
        auto_import_item = item
        break
      end
    end
    check("completion offers `AutoImportTarget`", auto_import_item ~= nil, "got " .. #items .. " items", res_rpc)
    if auto_import_item then
      local edits = auto_import_item.additionalTextEdits
      local ok = edits ~= nil and edits[1] ~= nil and edits[1].newText ~= nil and edits[1].newText:match("^import ") ~= nil
      check("  AutoImportTarget carries an `import ` additionalTextEdit", ok, edits and edits[1] and edits[1].newText, res_rpc)
    end
  end
end

-- ---- S6: member access `g.greet()` resolves into Greeter.kt via the receiver's inferred type ----
do
  local l, c = find("g.greet()", "greet")
  check("found g.greet() member usage", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("member goto g.greet -> Greeter.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Greeter%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end
end

do
  local l, c = find("        greet()", "greet")
  check("found bare greet() inside receiver lambda", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("implicit receiver goto greet() -> Greeter.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Greeter%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end
end

do
  local l, c = find("        setTag()", "setTag")
  check("found bare setTag() inside custom receiver lambda", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", { textDocument = { uri = uri }, position = { line = l, character = c } })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("implicit receiver goto setTag() -> Main.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Main%.kt$") ~= nil, loc and loc.uri, res_rpc)
  end
end

-- ---- S1: edit the buffer (did_change -> incremental reparse), then goto on the new code ----
do
  local n = vim.api.nvim_buf_line_count(bufnr)
  -- append a fresh declaration + a call to it; this fires textDocument/didChange to ktlsp
  vim.api.nvim_buf_set_lines(bufnr, n, n, false, {
    "",
    "fun freshlyAdded(): Int = 7",
    "fun callsFresh() { freshlyAdded() }",
  })
  vim.wait(800) -- let change-tracking flush didChange and ktlsp reparse incrementally

  local l, c = find("fun callsFresh() { freshlyAdded() }", "freshlyAdded")
  check("found edited usage of freshlyAdded", l ~= nil)
  if l then
    local res, res_rpc = request("textDocument/definition", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("goto after edit resolves freshlyAdded", loc ~= nil and loc.uri ~= nil, nil, res_rpc)
    if loc and loc.range then
      -- the definition line is where `fun freshlyAdded` was inserted
      local dl = find("fun freshlyAdded(): Int = 7", "freshlyAdded")
      check("  resolves to the edited definition line", loc.range.start.line == dl, ("got %s want %s"):format(loc.range.start.line, dl), res_rpc)
    end
  end
end

shutdown.force_requested = true
shutdown.stop_ok, shutdown.stop_error = pcall(function() client:stop(true) end)
vim.wait(300)
if vim.env.KTLSP_TRACE then
  vim.fn.writefile({ vim.json.encode({ checks = checks, requests = requests,
    diagnostic_probes = diagnostic_probes, shutdown = shutdown, failures = failures,
    observation_phase = "ordinary editor smoke; deadlines bound observation, not speed acceptance" }) },
    vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/features-results.json")
end

if #failures == 0 then
  print("\nALL PASS")
  os.exit(0)
else
  print("\nFAILED: " .. table.concat(failures, ", "))
  os.exit(1)
end
