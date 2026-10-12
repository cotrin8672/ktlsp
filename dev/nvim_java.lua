---@diagnostic disable: undefined-global
-- Headless Neovim check for Java project support: open a .java file, goto definition into
-- another .java file in the same project.

local project = arg[1]
assert(project and project ~= "", "usage: nvim -l dev/nvim_java.lua <project-dir> [ktlsp-bin]")

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
    print("UNMEASURED: " .. name .. " (" .. outcome .. ")")
  elseif ok then
    print("PASS: " .. name)
  else
    print("FAIL: " .. name)
    if detail then
      print("  " .. detail)
    end
  end
  if not ok or not measured then
    table.insert(failures, name)
  end
end

vim.cmd("edit " .. vim.fn.fnameescape(project .. "/app/Main.java"))
local bufnr = vim.api.nvim_get_current_buf()
vim.bo[bufnr].filetype = "java"

local id = vim.lsp.start({ name = "ktlsp", cmd = { bin }, root_dir = project,
  on_exit = function(code, signal) shutdown.exit = { code = code, signal = signal } end })
assert(id, "vim.lsp.start returned nil")
vim.wait(8000, function()
  local c = vim.lsp.get_client_by_id(id)
  return c ~= nil and (c.initialized == true or (c.server_capabilities and c.server_capabilities.definitionProvider))
end, 50)

local client = vim.lsp.get_client_by_id(id)
check("advertises definitionProvider", client and client.server_capabilities.definitionProvider ~= nil)
check("advertises referencesProvider", client and client.server_capabilities.referencesProvider ~= nil)
check("advertises documentSymbolProvider", client and client.server_capabilities.documentSymbolProvider ~= nil)
check("advertises workspaceSymbolProvider", client and client.server_capabilities.workspaceSymbolProvider ~= nil)
check("advertises completionProvider", client and client.server_capabilities.completionProvider ~= nil)
check("advertises renameProvider", client and client.server_capabilities.renameProvider ~= nil)
check("advertises signatureHelpProvider", client and client.server_capabilities.signatureHelpProvider ~= nil)
check("advertises implementationProvider", client and client.server_capabilities.implementationProvider ~= nil)
check("advertises typeDefinitionProvider", client and client.server_capabilities.typeDefinitionProvider ~= nil)
check("advertises semanticTokensProvider", client and client.server_capabilities.semanticTokensProvider ~= nil)
check("advertises inlayHintProvider", client and client.server_capabilities.inlayHintProvider ~= nil)
check("advertises callHierarchyProvider", client and client.server_capabilities.callHierarchyProvider ~= nil)
check("advertises typeHierarchyProvider", client and client.server_capabilities.typeHierarchyProvider ~= nil)
check("advertises foldingRangeProvider", client and client.server_capabilities.foldingRangeProvider ~= nil)
check("advertises selectionRangeProvider", client and client.server_capabilities.selectionRangeProvider ~= nil)

local function find(anchor, token)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  for l, line in ipairs(lines) do
    if line:find(anchor, 1, true) then
      local c = line:find(token, 1, true)
      if c then
        return l - 1, c - 1
      end
    end
  end
  return nil
end

local function request_rpc(method, params, buffer, timeout)
  local row = { method = method, params = vim.deepcopy(params), attempted = client ~= nil,
    attempts = client and 1 or 0, timeout_ms = timeout or 3000, started_ns = vim.uv.hrtime() }
  if client then
    local ok, response, err = pcall(client.request_sync, client, method, params, timeout or 3000, buffer or bufnr)
    if ok then row.response, row.error = response, err else row.error = response end
  end
  row.finished_ns = vim.uv.hrtime()
  row.availability = probe.request_outcome(row.response, row.error, function() return true end, row.attempted)
  if row.availability == "matched" then row.availability = "responded" end
  requests[#requests + 1] = row
  return row.availability == "responded" and row.response or nil, row
end

local function request(method, params)
  local response, row = request_rpc(method, params)
  local result = response and response.result
  if result == vim.NIL then result = nil end
  return result, row
end

local client_request = request

local uri = vim.uri_from_bufnr(bufnr)

local function result_mentions_uri(res, needle)
  if type(res) ~= "table" then
    return false, "empty"
  end
  if res.uri and res.uri:find(needle, 1, true) then
    return true, res.uri
  end
  if res.targetUri and res.targetUri:find(needle, 1, true) then
    return true, res.targetUri
  end
  for _, item in ipairs(res) do
    if type(item) == "table" then
      if item.uri and item.uri:find(needle, 1, true) then
        return true, item.uri
      end
      if item.targetUri and item.targetUri:find(needle, 1, true) then
        return true, item.targetUri
      end
    end
  end
  return false, vim.inspect(res)
end

-- documentSymbol should list the Main class and its run method.
do
  local res, res_rpc = request("textDocument/documentSymbol", { textDocument = { uri = uri } })
  local has_main = false
  local has_run = false
  if type(res) == "table" then
    for _, s in ipairs(res) do
      if s.name == "Main" then
        has_main = true
      end
      if s.name == "run" then
        has_run = true
      end
    end
  end
  check("documentSymbol includes Main and run", has_main and has_run, vim.inspect(res), res_rpc)
end

-- workspace/symbol should find Helper from the other file.
do
  local res, res_rpc = request("workspace/symbol", { query = "Helper" })
  local has_helper = false
  if type(res) == "table" then
    for _, s in ipairs(res) do
      if s.name == "Helper" then
        has_helper = true
      end
    end
  end
  check("workspace/symbol finds Helper", has_helper, vim.inspect(res), res_rpc)
end

-- Goto definition from Main.java to Helper.java.
do
  local l, c = find("new Helper()", "Helper")
  check("found Helper reference in Main.java", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/definition", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" then
      -- LSP may return a single Location {uri, range} or an array of LocationLink {targetUri, ...}.
      if res.uri and res.uri:find("Helper.java", 1, true) then
        ok = true
        detail = res.uri
      elseif res.targetUri and res.targetUri:find("Helper.java", 1, true) then
        ok = true
        detail = res.targetUri
      elseif #res > 0 then
        local helper_count = 0
        for _, d in ipairs(res) do
          if d.uri and d.uri:find("Helper.java", 1, true) then
            helper_count = helper_count + 1
            detail = d.uri
          end
          if d.targetUri and d.targetUri:find("Helper.java", 1, true) then
            helper_count = helper_count + 1
            detail = d.targetUri
          end
        end
        ok = helper_count == 1 and #res == 1
        if not ok then
          detail = "expected exactly one Helper.java definition, got " .. vim.inspect(res)
        end
      end
    end
    check("goto-definition lands in Helper.java", ok, detail, res_rpc)
  end
end

-- Goto definition on an overloaded Java method call should use the known argument count.
do
  local l, c = find("helper.waitFor(1);", "waitFor")
  check("found overloaded waitFor call in Main.java", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/definition", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local locations = {}
    if type(res) == "table" then
      if res.uri or res.targetUri then
        locations = { res }
      else
        locations = res
      end
    end
    local expected_line
    local helper_lines = vim.fn.readfile(project .. "/app/Helper.java")
    for idx, line in ipairs(helper_lines) do
      if line:find("void waitFor(int seconds)", 1, true) then
        expected_line = idx - 1
        break
      end
    end
    local loc = locations[1]
    local loc_uri = loc and (loc.uri or loc.targetUri) or nil
    local loc_range = loc and (loc.range or loc.targetSelectionRange) or nil
    local ok = #locations == 1
      and loc_uri
      and loc_uri:find("Helper.java", 1, true)
      and loc_range
      and expected_line ~= nil
      and loc_range.start.line == expected_line
    check("goto-definition narrows overloaded Java method by arity", ok, vim.inspect(res), res_rpc)
  end
end

-- Type definition from a Java value usage should resolve to the value's declared type.
do
  local l, c = find("helper.assist();", "helper")
  check("found helper value for typeDefinition", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/typeDefinition", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok, detail = result_mentions_uri(res, "Helper.java")
    check("typeDefinition(helper) returns Helper", ok, detail, res_rpc)
  end
end

-- Find references on Helper: declaration + usage.
do
  local l, c = find("new Helper()", "Helper")
  if l then
    local res, res_rpc = request("textDocument/references", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
      context = { includeDeclaration = true },
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" and #res >= 2 then
      ok = true
      detail = tostring(#res) .. " refs"
    end
    check("references returns Helper declaration and usage", ok, detail, res_rpc)
  else
    check("references returns Helper declaration and usage", false, "could not find Helper")
  end
end

-- Folding ranges should expose Java class/method blocks.
do
  local res, res_rpc = request("textDocument/foldingRange", { textDocument = { uri = uri } })
  local ok = type(res) == "table" and #res >= 2
  check("foldingRange returns Java block folds", ok, vim.inspect(res), res_rpc)
end

-- Selection range should expand from a Java identifier through its method invocation.
do
  local l, c = find("helper.assist();", "assist")
  check("found selectionRange target in Main.java", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/selectionRange", {
      textDocument = { uri = uri },
      positions = { { line = l, character = c } },
    })
    local item = type(res) == "table" and res[1] or nil
    check("selectionRange returns Java parent chain", item ~= nil and item.parent ~= nil, vim.inspect(res), res_rpc)
  end
end

-- Semantic tokens should classify Java-specific declarations, not just Kotlin-shaped identifiers.
do
  local res, res_rpc = request("textDocument/semanticTokens/full", { textDocument = { uri = uri } })
  local has_function = false
  local has_parameter = false
  if type(res) == "table" and type(res.data) == "table" then
    for i = 4, #res.data, 5 do
      if res.data[i] == 5 then
        has_function = true
      end
      if res.data[i] == 8 then
        has_parameter = true
      end
    end
  end
  check("semanticTokens/full includes Java function tokens", has_function, vim.inspect(res), res_rpc)
  check("semanticTokens/full includes Java parameter tokens", has_parameter, vim.inspect(res), res_rpc)
end

-- Inlay hints should report Java `var` inferred local types.
do
  local res, res_rpc = request("textDocument/inlayHint", {
    textDocument = { uri = uri },
    range = {
      start = { line = 0, character = 0 },
      ["end"] = { line = vim.api.nvim_buf_line_count(bufnr), character = 0 },
    },
  })
  local has_helper_hint = false
  if type(res) == "table" then
    for _, hint in ipairs(res) do
      local label = hint.label
      if type(label) == "table" then
        label = vim.inspect(label)
      end
      if label == ": Helper" then
        has_helper_hint = true
        break
      end
    end
  end
  check("inlayHint returns Java var type", has_helper_hint, vim.inspect(res), res_rpc)
end

-- Call hierarchy should report Java method callers and callees.
do
  local al, ac = find("helper.assist();", "assist")
  check("found callHierarchy assist target in Main.java", al ~= nil, "line/col")
  if al then
    local prepared, prepared_rpc = request("textDocument/prepareCallHierarchy", {
      textDocument = { uri = uri },
      position = { line = al, character = ac },
    })
    local item = type(prepared) == "table" and prepared[1] or nil
    check("prepareCallHierarchy(assist) returns item", item ~= nil and item.name == "assist", vim.inspect(prepared), prepared_rpc)
    if item then
      local incoming, incoming_rpc = request("callHierarchy/incomingCalls", { item = item })
      local has_run = false
      if type(incoming) == "table" then
        for _, call in ipairs(incoming) do
          if call.from and call.from.name == "run" then
            has_run = true
            break
          end
        end
      end
      check("callHierarchy/incomingCalls(assist) returns run", has_run, vim.inspect(incoming), incoming_rpc)
    end
  end

  local rl, rc = find("public void run", "run")
  check("found callHierarchy run target in Main.java", rl ~= nil, "line/col")
  if rl then
    local prepared, prepared_rpc = request("textDocument/prepareCallHierarchy", {
      textDocument = { uri = uri },
      position = { line = rl, character = rc },
    })
    local item = type(prepared) == "table" and prepared[1] or nil
    check("prepareCallHierarchy(run) returns item", item ~= nil and item.name == "run", vim.inspect(prepared), prepared_rpc)
    if item then
      local outgoing, outgoing_rpc = request("callHierarchy/outgoingCalls", { item = item })
      local has_assist = false
      if type(outgoing) == "table" then
        for _, call in ipairs(outgoing) do
          if call.to and call.to.name == "assist" then
            has_assist = true
            break
          end
        end
      end
      check("callHierarchy/outgoingCalls(run) returns assist", has_assist, vim.inspect(outgoing), outgoing_rpc)
    end
  end
end

-- Observe one fresh notification; empty diagnostics are measured mismatches, not missing responses.
do
  local observation, finish = probe.observe_diagnostics(client, bufnr)
  diagnostic_probes[#diagnostic_probes + 1] = observation
  local count = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, count, count, false, { "" })
  require("vim.lsp._changetracking").flush(client)
  local result = finish(5000)
  for _, expected in ipairs({
    { "publishes Java unused-import diagnostic", "Unused import: List" },
    { "publishes Java unresolved-reference diagnostic", "Unresolved reference: missingCall" },
    { "publishes Java call-shape diagnostic", "No overload of combine accepts 1 argument" },
    { "publishes Java argument-type diagnostic", "No overload of adopt accepts argument type (Dog)" },
  }) do
    local found = false
    for _, diagnostic in ipairs(result and result.diagnostics or {}) do
      if (diagnostic.message or ""):find(expected[2], 1, true) then
        found = true
        break
      end
    end
    check(expected[1], found, vim.inspect(result), observation)
  end
  vim.api.nvim_buf_set_lines(bufnr, count, -1, false, {})
  require("vim.lsp._changetracking").flush(client)
end

-- Code actions should offer the Java unused-import quickfix as an LSP WorkspaceEdit.
do
  local l, c = find("import java.util.List;", "List")
  check("found Java unused import for codeAction", l ~= nil, "line/col")
  if l then
    local actions, actions_rpc = request("textDocument/codeAction", {
      textDocument = { uri = uri },
      range = {
        start = { line = l, character = c },
        ["end"] = { line = l, character = c + 4 },
      },
      context = { diagnostics = {} },
    })
    local found = false
    if type(actions) == "table" then
      for _, action in ipairs(actions) do
        if action.title == "Remove unused import `List`" and action.edit ~= nil then
          found = true
          break
        end
      end
    end
    check("codeAction removes Java unused import", found, vim.inspect(actions), actions_rpc)
  end
end

-- Implementation should use Java extends/implements edges.
do
  local l, c = find("Worker worker", "Worker")
  check("found Worker type in Main.java", l ~= nil, "line/col")
  if l then
    local impls, impls_rpc = request("textDocument/implementation", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok, detail = result_mentions_uri(impls, "Helper.java")
    check("implementation(Worker) returns Helper", ok, detail, impls_rpc)
  end
end

-- Implementation on an interface member should return the overriding member.
do
  local l, c = find("worker.assist()", "assist")
  check("found Worker.assist member usage in Main.java", l ~= nil, "line/col")
  if l then
    local impls, impls_rpc = request("textDocument/implementation", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok, detail = result_mentions_uri(impls, "Helper.java")
    check("implementation(Worker.assist) returns Helper.assist", ok, detail, impls_rpc)
  end
end

-- Type hierarchy should expose Java extends/implements edges.
do
  local l, c = find("Worker worker", "Worker")
  check("found Worker typeHierarchy target in Main.java", l ~= nil, "line/col")
  if l then
    local prepared, prepared_rpc = request("textDocument/prepareTypeHierarchy", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local item = type(prepared) == "table" and prepared[1] or nil
    check("prepareTypeHierarchy(Worker) returns item", item ~= nil and item.name == "Worker", vim.inspect(prepared), prepared_rpc)
    if item then
      local subtypes, subtypes_rpc = client_request("typeHierarchy/subtypes", { item = item })
      local has_helper = false
      if type(subtypes) == "table" then
        for _, subtype in ipairs(subtypes) do
          if subtype.name == "Helper" then
            has_helper = true
            break
          end
        end
      end
      check("typeHierarchy/subtypes(Worker) returns Helper", has_helper, vim.inspect(subtypes), subtypes_rpc)
    end
  end
end

-- Completion at the first Helper reference should offer Helper.
do
  local l, c = find("Helper helper", "Helper")
  check("found completion target in Main.java", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/completion", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" then
      local items = res.items or res
      if type(items) == "table" then
        for _, item in ipairs(items) do
          if item.label == "Helper" then
            ok = true
            detail = item.label
            break
          end
        end
      end
    end
    check("completion offers Helper", ok, detail, res_rpc)
  end
end

-- Member completion must use incomplete Java text and current shared Kotlin declarations.
do
  local function completion_after_edit(anchor, text)
    local line = assert(find(anchor, anchor), "completion anchor not found: " .. anchor)
    local original = vim.api.nvim_buf_get_lines(bufnr, line, line + 1, false)
    vim.api.nvim_buf_set_lines(bufnr, line, line + 1, false, { text })
    vim.wait(200)
    local result, result_rpc = request("textDocument/completion", {
      textDocument = { uri = uri },
      position = { line = line, character = #text },
    })
    local labels = {}
    for _, item in ipairs(result and (result.items or result) or {}) do
      labels[item.label] = true
    end
    vim.api.nvim_buf_set_lines(bufnr, line, line + 1, false, original)
    vim.wait(200)
    return labels, result_rpc
  end

  local labels, labels_rpc = completion_after_edit("helper.assist();", "        helper.as")
  check("unsaved Java helper.as offers assist", labels.assist, vim.inspect(labels), labels_rpc)
  check("Java member completion excludes unrelated Other.assistUnrelated", not labels.assistUnrelated, vim.inspect(labels), labels_rpc)

  labels, labels_rpc = completion_after_edit("this.runAgain();", "        this.")
  check("bare Java this. offers runAgain", labels.runAgain, vim.inspect(labels), labels_rpc)

  labels, labels_rpc = completion_after_edit("helper.inherited();", "        helper.inh")
  check("Java helper.inh offers inherited member", labels.inherited, vim.inspect(labels), labels_rpc)

  labels, labels_rpc = completion_after_edit("kotlinHelper.assist();", "        kotlinHelper.as")
  check("Java kotlinHelper.as offers Kotlin assist", labels.assist, vim.inspect(labels), labels_rpc)

  labels, labels_rpc = completion_after_edit("helper.assist();", "        unknown.as")
  check("unknown Java receiver has no unrelated completion", next(labels) == nil, vim.inspect(labels), labels_rpc)

  local kotlin_buf = vim.fn.bufadd(project .. "/app/KotlinHelper.kt")
  vim.fn.bufload(kotlin_buf)
  vim.bo[kotlin_buf].filetype = "kotlin"
  assert(vim.lsp.buf_attach_client(kotlin_buf, id), "Kotlin buffer did not attach to shared client")
  local original = vim.api.nvim_buf_get_lines(kotlin_buf, 0, -1, false)
  vim.api.nvim_buf_set_lines(kotlin_buf, 0, -1, false, {
    "package app", "", "class KotlinHelper {", "    fun assistChanged() { }", "}",
  })
  vim.wait(200)
  labels, labels_rpc = completion_after_edit("kotlinHelper.assist();", "        kotlinHelper.as")
  check("unsaved Kotlin member edit appears in Java completion", labels.assistChanged, vim.inspect(labels), labels_rpc)
  check("Java completion removes the old unsaved Kotlin member", not labels.assist, vim.inspect(labels), labels_rpc)
  check("Kotlin declaration remains unsaved", vim.bo[kotlin_buf].modified)
  vim.api.nvim_buf_set_lines(kotlin_buf, 0, -1, false, original)
  vim.wait(200)
end

-- Java resources keep their Kotlin body type through incomplete calls and lexical scopes.
do
  local path = project .. "/resourcecase/ResourceUse.java"
  local kotlin_path = project .. "/resourcecase/Bodies.kt"
  local resource_buf = vim.fn.bufadd(path)
  vim.fn.bufload(resource_buf)
  vim.bo[resource_buf].filetype = "java"
  assert(vim.lsp.buf_attach_client(resource_buf, id), "resource fixture did not attach")
  local original = vim.api.nvim_buf_get_lines(resource_buf, 0, -1, false)
  local kotlin_original = vim.fn.readfile(kotlin_path)
  local original_modified = vim.bo[resource_buf].modified
  local original_hash = probe.buffer_sha256(resource_buf)
  local function disk_sha(file)
    return vim.fn.sha256(table.concat(vim.fn.readfile(file, "b"), "\n"))
  end
  local original_disk = { java = disk_sha(path), kotlin = disk_sha(kotlin_path) }
  local resource_uri = vim.uri_from_bufnr(resource_buf)
  local wire_text, latest = probe.buffer_full_text(resource_buf), nil
  local report = { requests = {}, outbound = {}, expected_probe_count = 14,
    original_source_sha256 = original_hash, original_disk_sha256 = original_disk,
    valid_var_status = "OPEN unless the first response offers Body.string; empty is a mismatch" }
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == "textDocument/didChange" and params.textDocument.uri == resource_uri then
      local reconstructed, err = probe.apply_content_changes(wire_text, params.contentChanges,
        client.offset_encoding or "utf-16")
      latest = { sent = not not sent, version = params.textDocument.version,
        buffer_version = vim.lsp.util.buf_versions[resource_buf], source_sha256 = probe.buffer_sha256(resource_buf),
        wire_source_sha256 = reconstructed and vim.fn.sha256(reconstructed), wire_error = err }
      if sent and reconstructed then wire_text = reconstructed end
      report.outbound[#report.outbound + 1] = vim.deepcopy(latest)
    end
    return sent
  end
  local function flush()
    require("vim.lsp._changetracking").flush(client)
  end
  local function once(phase, method, anchor, token, text)
    local position = probe.find(original, anchor, token)
    if text then
      vim.api.nvim_buf_set_lines(resource_buf, position.line, position.line + 1, false, { text })
      flush()
      if method == "textDocument/completion" then
        position.character = vim.str_utfindex(text, "utf-16")
      else
        position = probe.find(vim.api.nvim_buf_get_lines(resource_buf, 0, -1, false), text, token)
      end
    end
    local params = { textDocument = { uri = resource_uri }, position = position }
    local sha = probe.buffer_sha256(resource_buf)
    local delivered = latest and latest.sent and latest.version == vim.lsp.util.buf_versions[resource_buf]
      and latest.source_sha256 == sha and latest.wire_source_sha256 == sha and not latest.wire_error
    local response, row
    if delivered then
      response, row = request_rpc(method, params, resource_buf)
    else
      row = { method = method, params = params, attempted = false, attempts = 0,
        availability = "not-run", delivery_error = "current edited/restored source was not observed outbound" }
      requests[#requests + 1] = row
    end
    row.resource_phase, row.source_sha256, row.delivery = phase, sha, vim.deepcopy(latest)
    report.requests[#report.requests + 1] = row
    if text then
      vim.api.nvim_buf_set_lines(resource_buf, position.line, position.line + 1, false,
        { original[position.line + 1] })
      flush()
    end
    local result = response and response.result
    if result == vim.NIL then result = nil end
    return result, row
  end
  local function completion(phase, anchor, token, text, expected, excluded)
    local result, row = once(phase, "textDocument/completion", anchor, token, text)
    check("Java resource " .. phase .. " offers " .. expected .. " and excludes " .. excluded,
      probe.completion_has(result, expected) and not probe.completion_has(result, excluded), vim.inspect(result), row)
  end
  local function definition(phase, anchor, token, text, target, lines, declaration)
    local result, row = once(phase, "textDocument/definition", anchor, token, text)
    local _, range = probe.find(lines, declaration, token)
    check("Java resource " .. phase .. " goes to exact " .. token .. " declaration",
      probe.exact_definition(result, target, { range }), vim.inspect(result), row)
  end
  local ok, err = xpcall(function()
    completion("ordinary local unfinished println control", "System.out.println(localBody.string());", "localBody",
      "        System.out.println(localBody.", "string", "enqueue")
    completion("typed body.st", "System.out.println(body.string());", "body",
      "            body.st", "string", "enqueue")
    completion("typed unfinished println body.", "System.out.println(body.string());", "body",
      "            System.out.println(body.", "string", "enqueue")
    definition("unfinished println Kotlin member", "System.out.println(body.string());", "string",
      "            System.out.println(body.string()", kotlin_path, kotlin_original, "fun string():")
    definition("try body binding", "System.out.println(body.string());", "body", nil,
      path, original, "try (Body body = supply)")
    for _, scope in ipairs({ "before", "caught", "finished", "after" }) do
      local anchor = "/* " .. scope .. " */ body.enqueue();"
      completion(scope .. " outer field body.en", anchor, "body",
        "        /* " .. scope .. " */ body.en", "enqueue", "string")
      definition(scope .. " outer field binding", anchor, "body", nil,
        path, original, "private Other body")
    end
    completion("valid var unfinished println positive", "System.out.println(inferredBody.string());", "inferredBody",
      "            System.out.println(inferredBody.", "string", "enqueue")
  end, debug.traceback)
  if not vim.deep_equal(vim.api.nvim_buf_get_lines(resource_buf, 0, -1, false), original) then
    vim.api.nvim_buf_set_lines(resource_buf, 0, -1, false, original)
    flush()
  end
  client.rpc.notify = notify
  vim.bo[resource_buf].modified = original_modified
  report.disk_unchanged = disk_sha(path) == original_disk.java and disk_sha(kotlin_path) == original_disk.kotlin
  report.restored = probe.buffer_sha256(resource_buf) == original_hash and report.disk_unchanged
  report.error = not ok and err or nil
  check("Java resource buffers restore exactly without disk edits", report.restored, vim.inspect(report))
  if not ok then check("Java resource probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-resource-body.json")
  end
end

-- Imported superclasses must keep their declaration context across both languages and edits.
do
  local script_dir = vim.fs.dirname(debug.getinfo(1, "S").source:gsub("^@", ""))
  local probe = dofile(script_dir .. "/nvim_lsp_probe.lua")
  local files = {
    base = "support/ImportedBase.java", other = "app/ImportedBase.java",
    child = "app/ImportedChild.java", kotlin = "app/KotlinImportedChild.kt",
    java_use = "app/SuperUse.java", kotlin_use = "app/SuperUse.kt",
  }
  local buffers, original, notifications, sent_versions = {}, {}, {}, {}
  local report = { requests = {}, diagnostics = {}, outbound = {} }
  local previous_handler = client.handlers["textDocument/publishDiagnostics"]
  local diagnostic_handler = previous_handler or vim.lsp.handlers["textDocument/publishDiagnostics"]
  client.handlers["textDocument/publishDiagnostics"] = function(err, result, ctx, config)
    if result then
      local key = probe.path_key(vim.uri_to_fname(result.uri))
      notifications[key] = result
      report.diagnostics[#report.diagnostics + 1] = vim.deepcopy(result)
    end
    diagnostic_handler(err, result, ctx, config)
  end
  local rpc_notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = rpc_notify(method, params)
    if sent and method == "textDocument/didChange" then
      local document = params.textDocument
      sent_versions[document.uri] = document.version
      report.outbound[#report.outbound + 1] = vim.deepcopy(document)
    end
    return sent
  end
  for _, name in ipairs({ "base", "other", "child", "kotlin", "java_use", "kotlin_use" }) do
    local buf = vim.fn.bufadd(project .. "/" .. files[name])
    vim.fn.bufload(buf)
    vim.bo[buf].filetype = files[name]:match("%.kt$") and "kotlin" or "java"
    buffers[name] = buf
    original[name] = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    assert(vim.lsp.buf_attach_client(buf, id), "superclass fixture did not attach: " .. name)
  end
  local function lines(name)
    return vim.api.nvim_buf_get_lines(buffers[name], 0, -1, false)
  end
  local function doc_uri(name)
    return vim.uri_from_bufnr(buffers[name])
  end
  local function flush(name, assertion)
    require("vim.lsp._changetracking").flush(client)
    if assertion then
      local expected = vim.lsp.util.buf_versions[buffers[name]]
      check(assertion .. " didChange is outbound before the first request",
        sent_versions[doc_uri(name)] == expected,
        ("expected=%s sent=%s"):format(expected, sent_versions[doc_uri(name)]))
    end
  end
  local function once(name, method, position, phase)
    local params = { textDocument = { uri = doc_uri(name) }, position = position }
    local response, row = request_rpc(method, params, buffers[name])
    local result = response and response.result
    report.requests[#report.requests + 1] = {
      phase = phase, method = method, params = params, attempts = 1,
      result = result, response = row.response, error = row.error,
      sent_versions = vim.deepcopy(sent_versions),
    }
    if result == vim.NIL then result = nil end
    return result, row
  end
  -- A notification on complete valid source is required; an empty local diagnostic store is insufficient.
  for _, name in ipairs({ "java_use", "kotlin_use" }) do
    local key = probe.path_key(vim.api.nvim_buf_get_name(buffers[name]))
    local received = vim.wait(5000, function() return notifications[key] ~= nil end, 50)
    local result = notifications[key]
    check(name .. " receives diagnostics for complete valid superclass calls", received, vim.inspect(result))
    check(name .. " valid imported superclass calls have no false diagnostics",
      received and #result.diagnostics == 0, vim.inspect(result))
  end
  local function definition(name, receiver, target, declaration, phase)
    local position = probe.find(lines(name), receiver .. '.accept("ok")', "accept")
    local _, range = probe.find(lines(target), declaration, "accept")
    local expected = probe.location_key(doc_uri(target), range)
    local result, result_rpc = once(name, "textDocument/definition", position, phase)
    local locations = probe.locations(result)
    local found = locations[1]
    check(name .. " " .. phase .. " " .. receiver .. ".accept goes to exact declaration",
      #locations == 1 and probe.location_key(found.uri or found.targetUri,
        found.targetSelectionRange or found.range) == expected, vim.inspect(result), result_rpc)
  end
  local function completion(name, receiver, expected, excluded, phase)
    local position = probe.find(lines(name), receiver .. ".baseOnly()", "baseOnly")
    local before = vim.api.nvim_buf_get_lines(buffers[name], position.line, position.line + 1, false)
    local text = before[1]:match("^%s*") .. receiver .. ".ba"
    vim.api.nvim_buf_set_lines(buffers[name], position.line, position.line + 1, false, { text })
    flush(name, name .. " " .. phase .. " " .. receiver .. ".ba")
    local result, result_rpc = once(name, "textDocument/completion",
      { line = position.line, character = #text }, phase)
    check(name .. " " .. phase .. " " .. receiver .. ".ba offers " .. expected .. " and excludes " .. excluded,
      probe.completion_has(result, expected) and not probe.completion_has(result, excluded), vim.inspect(result), result_rpc)
    vim.api.nvim_buf_set_lines(buffers[name], position.line, position.line + 1, false, before)
    flush(name)
  end
  for _, name in ipairs({ "java_use", "kotlin_use" }) do
    for _, receiver in ipairs({ "child", "kotlin" }) do
      definition(name, receiver, "base", "public void accept(String value)", "initial")
      completion(name, receiver, "baseOnly", "baseWrong", "initial")
    end
  end
  for _, receiver in ipairs({ "child", "kotlin" }) do
    local position = probe.find(lines(receiver), "import support.ImportedBase", "support")
    local text = lines(receiver)[position.line + 1]:gsub("support%.ImportedBase", "app.ImportedBase")
    vim.api.nvim_buf_set_lines(buffers[receiver], position.line, position.line + 1, false, { text })
    local phase = "unsaved " .. receiver .. " import switch"
    flush(receiver, phase)
    check(phase .. " keeps the declaration unsaved and disk unchanged",
      vim.bo[buffers[receiver]].modified and vim.deep_equal(vim.fn.readfile(project .. "/" .. files[receiver]),
        original[receiver]))
    for _, name in ipairs({ "java_use", "kotlin_use" }) do
      completion(name, receiver, "baseWrong", "baseOnly", phase)
      definition(name, receiver, "other", "public void accept(int value)", phase)
    end
  end
  -- After switching both imports, valid syntax must still report the two String-to-int errors.
  for _, name in ipairs({ "java_use", "kotlin_use" }) do
    local observation, finish = probe.observe_diagnostics(client, buffers[name])
    diagnostic_probes[#diagnostic_probes + 1] = observation
    local edited = lines(name)
    for i, text in ipairs(edited) do edited[i] = text:gsub("%.baseOnly%(", ".baseWrong(") end
    vim.api.nvim_buf_set_lines(buffers[name], 0, -1, false, edited)
    flush(name)
    local result = finish(5000)
    local received = result ~= nil and #result.diagnostics == 2
    for _, diagnostic in ipairs(result and result.diagnostics or {}) do
      received = received and diagnostic.code == "call_shape_mismatch"
    end
    check(name .. " changed superclass reports exactly two invalid accept arguments",
      received, vim.inspect(result), observation)
  end
  for name, buf in pairs(buffers) do
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, original[name])
  end
  flush("child")
  client.rpc.notify = rpc_notify
  client.handlers["textDocument/publishDiagnostics"] = previous_handler
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-superclasses.json")
  end
end

-- Shared call/value roles, callable Kotlin properties and qualified Java nested types.
do
  local script_dir = vim.fs.dirname(debug.getinfo(1, "S").source:gsub("^@", ""))
  dofile(script_dir .. "/nvim_member_selection.lua")(project, client, check)
end

-- Signature help on a Java method call should include parameter and return types.
do
  local l, c = find('helper.combine("Ada", 2)', "2")
  check("found signatureHelp target in Main.java", l ~= nil, "line/col")
  if l then
    local res, res_rpc = request("textDocument/signatureHelp", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" and type(res.signatures) == "table" then
      for _, sig in ipairs(res.signatures) do
        if sig.label == "combine(p1: String, p2: int): String" and res.activeParameter == 1 then
          ok = true
          detail = sig.label
          break
        end
      end
      if not ok then
        detail = vim.inspect(res)
      end
    end
    check("signatureHelp(combine) returns Java signature", ok, detail, res_rpc)
  end
end

-- prepareRename on Helper should return the identifier range.
do
  local l, c = find("new Helper()", "Helper")
  if l then
    local res, res_rpc = request("textDocument/prepareRename", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" and (res.range or res.start) then
      ok = true
      detail = "range"
    end
    check("prepareRename returns range for Helper", ok, detail, res_rpc)
  else
    check("prepareRename returns range for Helper", false, "could not find Helper")
  end
end

-- Rename Helper to Renamed should produce edits.
do
  local l, c = find("new Helper()", "Helper")
  if l then
    local res, res_rpc = request("textDocument/rename", {
      textDocument = { uri = uri },
      position = { line = l, character = c },
      newName = "Renamed",
    })
    local ok = false
    local detail = "empty"
    if type(res) == "table" and res.changes then
      local count = 0
      for _, _ in pairs(res.changes) do
        count = count + 1
      end
      if count >= 1 then
        ok = true
        detail = tostring(count) .. " files"
      end
    end
    check("rename returns edits for Helper", ok, detail, res_rpc)
  else
    check("rename returns edits for Helper", false, "could not find Helper")
  end
end

-- Applied method renames must preserve interface contracts and hidden static dispatch.
for _, case in ipairs({
  {
    directory = "renamecase", target = "Provider.java", anchor = "void assist()", renamed_anchor = "void support()",
    files = { "Contract.java", "Provider.java", "Unrelated.java", "JavaUse.java", "KotlinUse.kt" },
    replacements = {
      ["Contract.java"] = { { "assist%(%)", "support()" } },
      ["Provider.java"] = { { "void assist%(%)", "void support()" } },
      ["JavaUse.java"] = { { "provider%.assist%(%)", "provider.support()" }, { "contract%.assist%(%)", "contract.support()" } },
      ["KotlinUse.kt"] = { { "provider%.assist%(%)", "provider.support()" }, { "contract%.assist%(%)", "contract.support()" } },
    },
    sites = {
      { "Contract.java", "void assist()" }, { "Provider.java", "void assist()" },
      { "JavaUse.java", "provider.assist()" }, { "JavaUse.java", "contract.assist()" },
      { "KotlinUse.kt", "provider.assist()" }, { "KotlinUse.kt", "contract.assist()" },
    },
    count = 6, label = "override rename", kotlin_call = "provider.support()",
    sites_label = "override rename includes both declarations and four Java/Kotlin calls",
    goto_label = "renamed Kotlin call navigates to current implementation", snapshot = "override-rename-snapshots.json",
  },
  {
    directory = "barecase", target = "Child.java", anchor = "int assist()", renamed_anchor = "int support()",
    files = { "Base.java", "Child.java", "Decoy.java", "JavaUse.java", "KotlinUse.kt" },
    replacements = {
      ["Child.java"] = { { "int assist%(%)", "int support()" }, { "return assist%(%)", "return support()" } },
      ["JavaUse.java"] = { { "Child%.assist%(%)", "Child.support()" } },
      ["KotlinUse.kt"] = { { "Child%.assist%(%)", "Child.support()" } },
    },
    sites = {
      { "Child.java", "int assist()" }, { "Child.java", "return assist()" },
      { "JavaUse.java", "Child.assist()" }, { "KotlinUse.kt", "Child.assist()" },
    },
    count = 4, label = "static bare rename", bare_call = "return assist()", kotlin_call = "Child.support()",
    sites_label = "static bare rename includes declaration, bare call and Java/Kotlin calls",
    goto_label = "renamed Kotlin static call navigates to current implementation", snapshot = "static-bare-rename-snapshots.json",
  },
}) do
  local probe = dofile(vim.fn.getcwd() .. "/dev/nvim_lsp_probe.lua")
  if arg[3] then probe.outside_project(project, arg[3]) end
  local files, by_path, delivered = {}, {}, {}
  local original_notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    if method == "textDocument/didChange" then
      delivered[probe.path_key(vim.uri_to_fname(params.textDocument.uri))] = {
        version = params.textDocument.version, changes = vim.deepcopy(params.contentChanges),
      }
    end
    return original_notify(method, params)
  end
  for _, name in ipairs(case.files) do
    local path = project .. "/" .. case.directory .. "/" .. name
    local buffer = vim.fn.bufadd(path)
    vim.fn.bufload(buffer)
    vim.bo[buffer].filetype = name:match("%.kt$") and "kotlin" or "java"
    vim.lsp.buf_attach_client(buffer, id)
    local file = { name = name, buffer = buffer, uri = vim.uri_from_bufnr(buffer), path = path }
    file.before = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
    file.disk = vim.fn.readfile(path)
    file.expected = vim.deepcopy(file.before)
    for line, text in ipairs(file.expected) do
      for _, replacement in ipairs(case.replacements[name] or {}) do
        text = text:gsub(replacement[1], replacement[2])
      end
      file.expected[line] = text
    end
    files[#files + 1] = file
    by_path[probe.path_key(path)] = file
  end
  local provider = by_path[probe.path_key(project .. "/" .. case.directory .. "/" .. case.target)]
  local position, target_range = probe.find(provider.before, case.anchor, "assist")
  if case.bare_call then
    local bare_position = probe.find(provider.before, case.bare_call, "assist")
    local target, target_rpc = request_rpc("textDocument/definition", {
      textDocument = { uri = provider.uri }, position = bare_position,
    }, provider.buffer, 5000)
    check("bare Java call resolves to hidden static declaration",
      probe.exact_definition(target and target.result, provider.path, { target_range }), vim.inspect(target), target_rpc)
  end
  local result, rename_rpc = request_rpc("textDocument/rename", {
    textDocument = { uri = provider.uri }, position = position, newName = "support",
  }, provider.buffer, 5000)
  local edit = result and result.result
  local safe, count, sites = type(edit) == "table", 0, {}
  local expected_sites = {}
  for _, site in ipairs(case.sites) do
    local file = by_path[probe.path_key(project .. "/" .. case.directory .. "/" .. site[1])]
    local _, range = probe.find(file.before, site[2], "assist")
    expected_sites[probe.location_key(file.uri, range)] = true
  end
  local normalized = vim.deepcopy(edit or {})
  if normalized.changes then normalized.changes = {} end
  local function collect(uri, edits)
    local file = by_path[probe.path_key(vim.uri_to_fname(uri))]
    if not file then safe = false; return end
    for _, item in ipairs(edits) do
      local range = item.range
      if not probe.range_in_source(range, file.before) then safe = false; return end
      local key = probe.location_key(file.uri, range)
      safe = safe and item.newText == "support"
        and expected_sites[key] == true and not sites[key]
      sites[key], count = true, count + 1
    end
    return file.uri
  end
  for uri, edits in pairs(edit and edit.changes or {}) do
    local canonical = collect(uri, edits)
    if canonical then normalized.changes[canonical] = vim.list_extend(normalized.changes[canonical] or {}, edits) end
  end
  for index, change in ipairs(edit and edit.documentChanges or {}) do
    if change.textDocument then
      local canonical = collect(change.textDocument.uri, change.edits)
      if canonical then normalized.documentChanges[index].textDocument.uri = canonical end
    else safe = false end
  end
  safe = safe and count == case.count
  check(case.sites_label, safe, vim.inspect(result), rename_rpc)
  local applied, message = false, "unsafe or incomplete edits"
  if safe then
    applied, message = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding)
    applied = applied and message ~= false
  end
  check(case.label .. " applies in Neovim", applied, tostring(message))
  local snapshots = { request = result, files = {}, applied = applied }
  for _, file in ipairs(files) do
    local actual = vim.api.nvim_buf_get_lines(file.buffer, 0, -1, false)
    check(case.label .. " preserves exact " .. file.name, vim.deep_equal(actual, file.expected), vim.inspect(actual))
    snapshots.files[file.name] = { before = file.before, expected = file.expected, actual = actual }
  end
  require("vim.lsp._changetracking").flush(client)
  snapshots.delivered = vim.deepcopy(delivered)
  for _, file in ipairs(files) do
    if not vim.deep_equal(file.before, file.expected) then
      local sent = delivered[probe.path_key(file.path)]
      local text = table.concat(file.expected, "\n") .. (vim.bo[file.buffer].eol and "\n" or "")
      check(case.label .. " changes delivered for " .. file.name, sent ~= nil and #sent.changes == 1
        and sent.changes[1].range == nil and sent.changes[1].text == text, vim.inspect(sent))
    end
  end
  if applied then
    local use = by_path[probe.path_key(project .. "/" .. case.directory .. "/KotlinUse.kt")]
    local current = vim.api.nvim_buf_get_lines(use.buffer, 0, -1, false)
    local cursor = probe.find(current, case.kotlin_call, "support")
    local target, target_rpc = request_rpc("textDocument/definition", {
      textDocument = { uri = use.uri }, position = cursor,
    }, use.buffer, 5000)
    local _, range = probe.find(provider.expected, case.renamed_anchor, "support")
    check(case.goto_label,
      probe.exact_definition(target and target.result, provider.path, { range }), vim.inspect(target), target_rpc)
  else
    local skipped = { method = "textDocument/definition", attempted = false, attempts = 0,
      availability = "not-run", reason = "rename was not safely applied" }
    requests[#requests + 1] = skipped
    check(case.goto_label, false, skipped.reason, skipped)
  end
  for _, file in ipairs(files) do
    vim.api.nvim_buf_set_lines(file.buffer, 0, -1, false, file.before)
    check(case.label .. " leaves disk unchanged for " .. file.name, vim.deep_equal(vim.fn.readfile(file.path), file.disk))
  end
  require("vim.lsp._changetracking").flush(client)
  client.rpc.notify = original_notify
  if arg[3] then
    vim.fn.writefile({ vim.json.encode(snapshots) }, arg[3] .. "/" .. case.snapshot)
  end
end

-- Java parser diagnostics should be published for syntax errors.
do
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local edited = false
  for i, line in ipairs(lines) do
    if line:find('helper.combine("Ada", 2);', 1, true) then
      lines[i] = '        helper.combine("Ada", );'
      edited = true
      break
    end
  end
  if edited then
    local observation, finish = probe.observe_diagnostics(client, bufnr)
    diagnostic_probes[#diagnostic_probes + 1] = observation
    vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
    require("vim.lsp._changetracking").flush(client)
    local result = finish(5000)
    local ok = false
    for _, diagnostic in ipairs(result and result.diagnostics or {}) do
      if diagnostic.severity == vim.diagnostic.severity.ERROR then ok = true end
    end
    check("publishes Java syntax diagnostics", ok, vim.inspect(result), observation)
  else
    check("publishes Java syntax diagnostics", false, "could not edit combine call")
  end
end

shutdown.force_requested = true
shutdown.stop_ok, shutdown.stop_error = pcall(function() client:stop(true) end)
vim.wait(300)
if vim.env.KTLSP_TRACE then
  vim.fn.writefile({ vim.json.encode({ checks = checks, requests = requests,
    diagnostic_probes = diagnostic_probes, shutdown = shutdown, failures = failures,
    observation_phase = "ordinary editor smoke; deadlines bound observation, not speed acceptance" }) },
    vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-results.json")
end

if #failures == 0 then
  print("\nALL PASS")
  os.exit(0)
else
  print("\nFAILED: " .. table.concat(failures, ", "))
  os.exit(1)
end
