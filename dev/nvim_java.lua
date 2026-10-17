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

local index_state = { project = false, libraries = false, messages = {} }
local wait_java_index = vim.env.KTLSP_JAVA_WAIT_INDEX == "1"
local log_handler = vim.lsp.handlers["window/logMessage"]
local client_config = { name = "ktlsp", cmd = { bin }, root_dir = project,
  handlers = { ["window/logMessage"] = function(err, params, ctx, config)
    local message = params and params.message or ""
    local project_files = message:match("^ktlsp indexed (%d+) project files")
    local library_files = message:match("^ktlsp indexed (%d+) library files")
    if project_files or library_files then
      index_state.messages[#index_state.messages + 1] = { message = message, observed_ns = vim.uv.hrtime() }
    end
    if project_files then index_state.project, index_state.project_files = true, tonumber(project_files) end
    if library_files then
      index_state.libraries, index_state.library_files = true, tonumber(library_files)
      index_state.libraries_observed_ns = vim.uv.hrtime()
      local failed, missing = message:match("(%d+) failed, (%d+) missing sources")
      index_state.failed, index_state.missing_sources = tonumber(failed), tonumber(missing)
    end
    if log_handler then return log_handler(err, params, ctx, config) end
  end } }
shutdown = probe.observe_shutdown(client_config)
local id = vim.lsp.start(client_config)
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
  local ready_before_edit = index_state.project and index_state.libraries
    and index_state.failed == 0 and index_state.missing_sources == 0
  local count = vim.api.nvim_buf_line_count(bufnr)
  vim.api.nvim_buf_set_lines(bufnr, count, count, false, { "" })
  require("vim.lsp._changetracking").flush(client)
  local result = finish(5000)
  if not ready_before_edit then
    if vim.env.KTLSP_TRACE then
      vim.fn.writefile({ vim.json.encode({ observation = observation, index = vim.deepcopy(index_state),
        classification = "early notification; semantic expectations not inferred before index readiness" }) },
        vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-early-diagnostics.json")
    end
    local ready = vim.wait(30000, function()
      return index_state.project and index_state.libraries and index_state.failed == 0 and index_state.missing_sources == 0
    end, 50)
    check("Java diagnostic precondition observes complete project/library indexing", ready, vim.inspect(index_state))
    if ready then
      observation, finish = probe.observe_diagnostics(client, bufnr)
      vim.api.nvim_buf_set_lines(bufnr, count + 1, count + 1, false, { "" })
      require("vim.lsp._changetracking").flush(client)
      result = finish(5000)
    else
      observation, result = { attempted = false, error = "project/library readiness not observed" }, nil
    end
  end
  diagnostic_probes[#diagnostic_probes + 1] = observation
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
  local report = { requests = {}, outbound = {}, expected_probe_count = 26,
    array_requests = {}, expected_array_probe_count = 12,
    alias_requests = {}, expected_alias_probe_count = 26, alias_positive_probe_count = 24,
    alias_scalar_guard_count = 2, rank2_requests = {}, expected_rank2_probe_count = 8,
    array_coverage_gaps = "rank2 first-response intrinsics/chains measured separately; Kotlin/compiled array factories remain OPEN",
    original_source_sha256 = original_hash, original_disk_sha256 = original_disk,
    valid_var_status = "OPEN unless the first response offers Body.string; empty is a mismatch" }
  local active_requests = report.requests
  local alias_kotlin = {}
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
  local function once(phase, method, anchor, token, text, anchor_token)
    local position = probe.find(original, anchor, anchor_token or token)
    local restore_line = active_requests == report.alias_requests
      and vim.api.nvim_buf_get_lines(resource_buf, position.line, position.line + 1, false)[1]
      or original[position.line + 1]
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
    if active_requests == report.alias_requests or active_requests == report.rank2_requests then
      local producer = alias_kotlin.latest
      local producer_sha = probe.buffer_sha256(alias_kotlin.buf)
      delivered = delivered and producer and producer.sent
        and producer.version == vim.lsp.util.buf_versions[alias_kotlin.buf]
        and producer.source_sha256 == producer_sha and producer.wire_source_sha256 == producer_sha
        and not producer.wire_error
    end
    local rank2_sources
    if active_requests == report.rank2_requests or phase:find("alias rank2 scalar guard", 1, true) then
      rank2_sources = {
        java = { uri = resource_uri, text = probe.buffer_full_text(resource_buf), wire_text = wire_text,
          version = vim.lsp.util.buf_versions[resource_buf], tick = vim.api.nvim_buf_get_changedtick(resource_buf),
          sha256 = sha, wire_sha256 = vim.fn.sha256(wire_text) },
        kotlin = { uri = alias_kotlin.uri, text = probe.buffer_full_text(alias_kotlin.buf),
          wire_text = alias_kotlin.wire, sha256 = probe.buffer_sha256(alias_kotlin.buf),
          wire_sha256 = vim.fn.sha256(alias_kotlin.wire), version = vim.lsp.util.buf_versions[alias_kotlin.buf],
          tick = vim.api.nvim_buf_get_changedtick(alias_kotlin.buf) },
      }
    end
    local response, row
    if delivered then
      response, row = request_rpc(method, params, resource_buf)
    else
      row = { method = method, params = params, attempted = false, attempts = 0,
        availability = "not-run", delivery_error = "current edited/restored source was not observed outbound" }
      requests[#requests + 1] = row
    end
    row.resource_phase, row.source_sha256, row.delivery = phase, sha, vim.deepcopy(latest)
    row.rank2_sources = rank2_sources
    if active_requests == report.array_requests then row.array_phase = phase end
    if active_requests == report.alias_requests then
      row.alias_phase, row.kotlin_delivery = phase, vim.deepcopy(alias_kotlin.latest)
    elseif active_requests == report.rank2_requests then
      row.rank2_phase, row.kotlin_delivery = phase, vim.deepcopy(alias_kotlin.latest)
    end
    active_requests[#active_requests + 1] = row
    if text then
      vim.api.nvim_buf_set_lines(resource_buf, position.line, position.line + 1, false,
        { restore_line })
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
  local function definition(phase, anchor, token, text, target, lines, declaration, anchor_token)
    local result, row = once(phase, "textDocument/definition", anchor, token, text, anchor_token)
    local _, range = probe.find(lines, declaration, token)
    check("Java resource " .. phase .. " goes to exact " .. token .. " declaration",
      probe.exact_definition(result, target, { range }), vim.inspect(result), row)
  end
  local ok, err = xpcall(function()
    if wait_java_index then
      -- This separate control measures the first response after indexing, never a content retry.
      local ready = vim.wait(30000, function() return index_state.project and index_state.libraries end, 50)
      report.index_readiness = { requested = true, ready = ready, observed = vim.deepcopy(index_state),
        finished_ns = vim.uv.hrtime(), semantic_phase = "first-response-after-indexing" }
      assert(ready, "Java resource control did not observe project and library indexing completion")
    else
      report.index_readiness = { requested = false, observed = vim.deepcopy(index_state),
        finished_ns = vim.uv.hrtime(), semantic_phase = "startup-first-response" }
    end
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
    completion("typed foreach Kotlin bare receiver", "System.out.println(kotlinItem.string());", "kotlinItem",
      "            kotlinItem.", "string", "enqueue")
    completion("typed foreach Kotlin unfinished println", "System.out.println(kotlinItem.string());", "kotlinItem",
      "            System.out.println(kotlinItem.", "string", "enqueue")
    completion("typed foreach Java bare receiver", "System.out.println(javaItem.javaOnly());", "javaItem",
      "            javaItem.", "javaOnly", "string")
    completion("typed foreach Java unfinished println", "System.out.println(javaItem.javaOnly());", "javaItem",
      "            System.out.println(javaItem.", "javaOnly", "string")
    completion("var foreach Kotlin array", "System.out.println(varKotlinArrayItem.string());", "varKotlinArrayItem",
      "            System.out.println(varKotlinArrayItem.", "string", "enqueue")
    completion("var foreach Java array", "System.out.println(varJavaArrayItem.javaOnly());", "varJavaArrayItem",
      "            System.out.println(varJavaArrayItem.", "javaOnly", "string")
    completion("var foreach Kotlin iterable", "System.out.println(varKotlinIterableItem.string());", "varKotlinIterableItem",
      "            System.out.println(varKotlinIterableItem.", "string", "enqueue")
    completion("var foreach Java iterable", "System.out.println(varJavaIterableItem.javaOnly());", "varJavaIterableItem",
      "            System.out.println(varJavaIterableItem.", "javaOnly", "string")
    completion("valid var unfinished println positive", "System.out.println(inferredBody.string());", "inferredBody",
      "            System.out.println(inferredBody.", "string", "enqueue")
    completion("var factory unfinished println", "System.out.println(factoryBody.string());", "factoryBody",
      "        System.out.println(factoryBody.", "string", "enqueue")
    completion("authored selector suffix after cursor", "System.out.println(factoryBody.string());", "string",
      nil, "string", "enqueue")
    local chain = "supply"
    for _ = 1, 32 do chain = "identity(" .. chain .. ")" end
    completion("32 nested generic results", "identity(supply).string();", "string",
      "        " .. chain .. ".", "string", "enqueue")
    definition("32 nested generic results Kotlin member", "identity(supply).string();", "string",
      "        " .. chain .. ".string();", kotlin_path, kotlin_original, "fun string():")
    -- Preserve the original 26 first responses above; these 12 are a separate array denominator.
    active_requests = report.array_requests
    for _, case in ipairs({
      { name = "Kotlin method array", receiver = "kotlinMethodArrayItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { name = "Java method array", receiver = "javaMethodArrayItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
      { name = "Kotlin postfix array", receiver = "kotlinPostfixedArrayItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { name = "Java postfix array", receiver = "javaPostfixedArrayItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
      { name = "Kotlin new array", receiver = "kotlinCreatedArrayItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { name = "Java new array", receiver = "javaCreatedArrayItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
    }) do
      local anchor = "System.out.println(" .. case.receiver .. "." .. case.member .. "());"
      completion("array " .. case.name .. " unfinished println", anchor, case.receiver,
        "            System.out.println(" .. case.receiver .. ".", case.member, case.excluded)
      definition("array " .. case.name .. " exact member", anchor, case.member,
        "            System.out.println(" .. case.receiver .. "." .. case.member .. "()",
        case.target, case.lines, case.declaration)
    end
    -- Alias responses have their own denominator, after the original 26 and 12 requests.
    alias_kotlin.buf = vim.fn.bufadd(kotlin_path)
    vim.fn.bufload(alias_kotlin.buf)
    alias_kotlin.modified = vim.bo[alias_kotlin.buf].modified
    alias_kotlin.original = vim.api.nvim_buf_get_lines(alias_kotlin.buf, 0, -1, false)
    alias_kotlin.hash = probe.buffer_sha256(alias_kotlin.buf)
    assert(vim.deep_equal(alias_kotlin.original, kotlin_original), "alias Kotlin fixture was already edited")
    alias_kotlin.uri, alias_kotlin.wire = vim.uri_from_bufnr(alias_kotlin.buf),
      probe.buffer_full_text(alias_kotlin.buf)
    local previous_notify = client.rpc.notify
    client.rpc.notify = function(method, params)
      local sent = previous_notify(method, params)
      if params.textDocument and params.textDocument.uri == alias_kotlin.uri
        and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
        local wire, err
        if method == "textDocument/didOpen" then wire = params.textDocument.text
        else wire, err = probe.apply_content_changes(alias_kotlin.wire, params.contentChanges,
          client.offset_encoding or "utf-16") end
        if sent and wire then alias_kotlin.wire = wire end
        alias_kotlin.latest = { sent = not not sent, version = params.textDocument.version,
          buffer_version = vim.lsp.util.buf_versions[alias_kotlin.buf],
          source_sha256 = probe.buffer_sha256(alias_kotlin.buf),
          wire_source_sha256 = wire and vim.fn.sha256(wire), wire_error = err }
        report.alias_kotlin_outbound = report.alias_kotlin_outbound or {}
        report.alias_kotlin_outbound[#report.alias_kotlin_outbound + 1] = vim.deepcopy(alias_kotlin.latest)
      end
      return sent
    end
    vim.bo[alias_kotlin.buf].filetype = "kotlin"
    assert(vim.lsp.buf_attach_client(alias_kotlin.buf, id), "alias Kotlin fixture did not attach")
    -- An explicit full edit also observes the wire if this buffer was attached earlier.
    vim.api.nvim_buf_set_lines(alias_kotlin.buf, 0, -1, false, alias_kotlin.original)
    flush()
    active_requests = report.alias_requests
    local function alias_case(phase, case)
      local anchor = "System.out.println(" .. case.receiver .. "." .. (case.anchor_member or case.member) .. "());"
      completion("alias " .. phase .. " " .. case.receiver, anchor, case.receiver,
        "            System.out.println(" .. case.receiver .. ".", case.member, case.excluded)
      local member_line = "            System.out.println(" .. case.receiver .. "." .. case.member .. "());"
      if case.member == "enqueue" then member_line = "            " .. case.receiver .. ".enqueue();" end
      definition("alias " .. phase .. " " .. case.receiver, anchor, case.member, member_line,
        case.target, case.lines, case.declaration, case.anchor_member)
    end
    local initial_cases = {
      { receiver = "kotlinDeclaredAliasItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { receiver = "javaDeclaredAliasItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
      { receiver = "kotlinMethodAliasItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { receiver = "javaMethodAliasItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
      { receiver = "kotlinCreatedAliasItem", member = "string", excluded = "enqueue", target = kotlin_path, lines = kotlin_original, declaration = "fun string():" },
      { receiver = "javaCreatedAliasItem", member = "javaOnly", excluded = "string", target = path, lines = original, declaration = "public String javaOnly()" },
    }
    for _, case in ipairs(initial_cases) do alias_case("initial", case) end
    for _, receiver in ipairs({ "kotlinAliasRow", "javaAliasRow" }) do
      local result, row = once("alias rank2 scalar guard " .. receiver, "textDocument/completion",
        "System.out.println(" .. receiver .. ".length);", receiver,
        "            System.out.println(" .. receiver .. ".")
      check("Java resource alias rank2 " .. receiver .. " excludes scalar members",
        not probe.completion_has(result, "string") and not probe.completion_has(result, "javaOnly")
          and not probe.completion_has(result, "enqueue"), vim.inspect(result), row)
      check("Java resource rank2 first response " .. receiver .. " offers length and clone",
        probe.completion_has(result, "length") and probe.completion_has(result, "clone"), vim.inspect(result), row)
    end
    local kotlin_changed, java_changed = {}, {}
    for i, line in ipairs(alias_kotlin.original) do
      kotlin_changed[i] = line:gsub("fun string%(%):", "fun currentString():")
    end
    for i, line in ipairs(original) do
      java_changed[i] = line:gsub("%.string%(%)", ".currentString()")
        :gsub("var kotlinCreatedAlias = new Body%[0%];", "var kotlinCreatedAlias = new Other[0];")
        :gsub("System%.out%.println%(kotlinCreatedAliasItem%.currentString%(%)%);",
          "kotlinCreatedAliasItem.enqueue();")
    end
    vim.api.nvim_buf_set_lines(alias_kotlin.buf, 0, -1, false, kotlin_changed)
    vim.api.nvim_buf_set_lines(resource_buf, 0, -1, false, java_changed)
    flush()
    report.alias_changed_source_sha256 = { java = probe.buffer_sha256(resource_buf),
      kotlin = probe.buffer_sha256(alias_kotlin.buf) }
    alias_case("two-file unsaved", { receiver = "kotlinMethodAliasItem", member = "currentString",
      anchor_member = "string", excluded = "string", target = kotlin_path, lines = kotlin_changed,
      declaration = "fun currentString():" })
    alias_case("two-file unsaved", { receiver = "kotlinCreatedAliasItem", member = "enqueue",
      anchor_member = "string", excluded = "currentString", target = kotlin_path, lines = kotlin_changed,
      declaration = "fun enqueue()" })
    alias_case("two-file unsaved Java control", initial_cases[4])
    vim.api.nvim_buf_set_lines(alias_kotlin.buf, 0, -1, false, alias_kotlin.original)
    vim.api.nvim_buf_set_lines(resource_buf, 0, -1, false, original)
    flush()
    for _, i in ipairs({ 3, 5, 4 }) do alias_case("restored", initial_cases[i]) end
    active_requests = report.rank2_requests
    for _, case in ipairs({
      { receiver = "kotlinMemberRow", member = "string", excluded = "javaOnly", target = kotlin_path,
        lines = kotlin_original, declaration = "fun string():" },
      { receiver = "javaMemberRow", member = "javaOnly", excluded = "string", target = path,
        lines = original, declaration = "public String javaOnly()" },
    }) do
      for _, suffix in ipairs({ "", ".clone()" }) do
        local anchor = "System.out.println(" .. case.receiver .. suffix .. ".length);"
        local result, row = once("rank2 intrinsics " .. case.receiver .. suffix, "textDocument/completion",
          anchor, case.receiver, "            System.out.println(" .. case.receiver .. suffix .. ".")
        check("Java resource rank2 " .. case.receiver .. suffix .. " offers length and clone without scalar members",
          probe.completion_has(result, "length") and probe.completion_has(result, "clone")
            and not probe.completion_has(result, "string") and not probe.completion_has(result, "javaOnly"),
          vim.inspect(result), row)
      end
      local expression = case.receiver .. ".clone()[0]"
      local anchor = "System.out.println(" .. expression .. "." .. case.member .. "());"
      completion("rank2 cloned cell " .. case.receiver, anchor, case.receiver,
        "            System.out.println(" .. expression .. ".", case.member, case.excluded)
      definition("rank2 cloned cell " .. case.receiver, anchor, case.member, nil,
        case.target, case.lines, case.declaration)
    end
    active_requests = report.requests
  end, debug.traceback)
  if not vim.deep_equal(vim.api.nvim_buf_get_lines(resource_buf, 0, -1, false), original) then
    vim.api.nvim_buf_set_lines(resource_buf, 0, -1, false, original)
    flush()
  end
  if alias_kotlin.buf then
    if not vim.deep_equal(vim.api.nvim_buf_get_lines(alias_kotlin.buf, 0, -1, false), alias_kotlin.original) then
      vim.api.nvim_buf_set_lines(alias_kotlin.buf, 0, -1, false, alias_kotlin.original)
      flush()
    end
    report.alias_kotlin_restored = probe.buffer_sha256(alias_kotlin.buf) == alias_kotlin.hash
    vim.bo[alias_kotlin.buf].modified = alias_kotlin.modified
  end
  client.rpc.notify = notify
  vim.bo[resource_buf].modified = original_modified
  report.disk_unchanged = disk_sha(path) == original_disk.java and disk_sha(kotlin_path) == original_disk.kotlin
  report.restored = probe.buffer_sha256(resource_buf) == original_hash and report.disk_unchanged
  report.error = not ok and err or nil
  check("Java resource buffers restore exactly without disk edits", report.restored, vim.inspect(report))
  local array_once = #report.array_requests == report.expected_array_probe_count
  for _, row in ipairs(report.array_requests) do
    array_once = array_once and row.attempted and row.attempts == 1
  end
  check("Java resource 12 array requests each run exactly once", array_once, vim.inspect(report.array_requests))
  local alias_once = #report.alias_requests == report.expected_alias_probe_count
  for _, row in ipairs(report.alias_requests) do
    alias_once = alias_once and row.attempted and row.attempts == 1
  end
  check("Java resource 26 alias requests each run exactly once", alias_once, vim.inspect(report.alias_requests))
  check("Java resource alias both buffers restore exactly without disk edits",
    report.restored and report.alias_kotlin_restored, vim.inspect(report))
  local rank2_once = #report.rank2_requests == report.expected_rank2_probe_count
  for _, row in ipairs(report.rank2_requests) do
    rank2_once = rank2_once and row.attempted and row.attempts == 1
  end
  check("Java resource 8 rank2 requests each run exactly once", rank2_once, vim.inspect(report.rank2_requests))
  if not ok then check("Java resource probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-resource-body.json")
  end
end

-- Actual SDK Object members and File array returns: six additional first responses after the old 72.
do
  local path = project .. "/resourcecase/ArraySdkUse.java"
  local buffer = vim.fn.bufadd(path)
  vim.fn.bufload(buffer)
  vim.bo[buffer].filetype = "java"
  local original = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
  local modified, original_hash = vim.bo[buffer].modified, probe.buffer_sha256(buffer)
  local function disk_sha() return vim.fn.sha256(table.concat(vim.fn.readfile(path, "b"), "\n")) end
  local disk_hash, sdk_uri = disk_sha(), vim.uri_from_bufnr(buffer)
  local wire, latest = probe.buffer_full_text(buffer), nil
  local owner_buffer = vim.fn.bufnr(project .. "/resourcecase/Bodies.kt")
  assert(owner_buffer > 0, "existing Bodies fixture was not loaded")
  local function owner_snapshot()
    return { uri = vim.uri_from_bufnr(owner_buffer), text = probe.buffer_full_text(owner_buffer),
      sha256 = probe.buffer_sha256(owner_buffer), version = vim.lsp.util.buf_versions[owner_buffer],
      tick = vim.api.nvim_buf_get_changedtick(owner_buffer) }
  end
  local owner_original = owner_snapshot()
  local function sdk_source(entry, token)
    local selected = {}
    for _, marker_path in ipairs(vim.fn.globpath(vim.env.KTLSP_CACHE_DIR .. "/extracted/jdk", "*/.ktlsp-source-jar", false, true)) do
      local root = vim.fs.dirname(marker_path)
      local path = root .. "/" .. entry
      local marker = vim.json.decode(table.concat(vim.fn.readfile(marker_path), "\n"))
      if not vim.env.JAVA_HOME or vim.fs.normalize(marker.jar) == vim.fs.normalize(vim.env.JAVA_HOME .. "/lib/src.zip") then
        local extracted = vim.system({ "unzip", "-p", marker.jar, entry }):wait()
        assert(extracted.code == 0, "selected SDK archive cannot be read: " .. entry)
        local lines, ranges = vim.split(extracted.stdout, "\n", { plain = true }), {}
        for line, text in ipairs(lines) do
          if text:match("^%s*public%s+String%s+" .. token .. "%s*%(") then
            local column = assert(text:find(token, 1, true)) - 1
            ranges[#ranges + 1] = { start = { line = line - 1, character = column },
              ["end"] = { line = line - 1, character = column + #token } }
          end
        end
        assert(#ranges == 1, "SDK declaration is not unique: " .. entry)
        selected[#selected + 1] = { entry = entry, path = path, jar = marker.jar, range = ranges[1],
          sha256 = vim.fn.sha256(extracted.stdout) }
      end
    end
    assert(#selected == 1, "selected SDK source is not unique: " .. entry)
    return selected[1]
  end
  local report = { requests = {}, outbound = {}, expected_requests = 6,
    original_text = probe.buffer_full_text(buffer), original_source_sha256 = original_hash,
    original_disk_sha256 = disk_hash, owner_source = owner_original,
    fileformat = vim.bo[buffer].fileformat, eol = vim.bo[buffer].eol, expected_sdk_sources = {
      Object = sdk_source("java.base/java/lang/Object.java", "toString"),
      File = sdk_source("java.base/java/io/File.java", "getName") } }
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if params.textDocument and params.textDocument.uri == sdk_uri
      and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
      local current, err
      if method == "textDocument/didOpen" then current = params.textDocument.text
      else current, err = probe.apply_content_changes(wire, params.contentChanges, client.offset_encoding or "utf-16") end
      if sent and current then wire = current end
      latest = { sent = not not sent, version = params.textDocument.version,
        source_sha256 = probe.buffer_sha256(buffer), wire_source_sha256 = current and vim.fn.sha256(current),
        wire_error = err }
      report.outbound[#report.outbound + 1] = vim.deepcopy(latest)
    end
    return sent
  end
  local function flush() require("vim.lsp._changetracking").flush(client) end
  local function once(phase, method, anchor, token, text)
    local position = probe.find(original, anchor, token)
    if text then
      vim.api.nvim_buf_set_lines(buffer, position.line, position.line + 1, false, { text })
      flush()
      position.character = vim.str_utfindex(text, "utf-16")
    end
    local before = { uri = sdk_uri, text = probe.buffer_full_text(buffer), wire_text = wire,
      sha256 = probe.buffer_sha256(buffer), wire_sha256 = vim.fn.sha256(wire),
      version = vim.lsp.util.buf_versions[buffer], tick = vim.api.nvim_buf_get_changedtick(buffer) }
    local delivered = latest and latest.sent and latest.version == before.version
      and latest.source_sha256 == before.sha256 and latest.wire_source_sha256 == before.sha256
      and not latest.wire_error
    local params = { textDocument = { uri = sdk_uri }, position = position }
    local response, row
    if delivered then response, row = request_rpc(method, params, buffer)
    else
      row = { method = method, params = params, attempted = false, attempts = 0,
        availability = "not-run", delivery_error = "SDK-array source was not observed outbound" }
      requests[#requests + 1] = row
    end
    row.sdk_array_phase, row.source, row.delivery = phase, before, vim.deepcopy(latest)
    row.owner_source = owner_snapshot()
    row.after = { sha256 = probe.buffer_sha256(buffer), version = vim.lsp.util.buf_versions[buffer],
      tick = vim.api.nvim_buf_get_changedtick(buffer) }
    row.stable = before.sha256 == row.after.sha256 and before.version == row.after.version
      and before.tick == row.after.tick and vim.deep_equal(row.owner_source, owner_original)
    report.requests[#report.requests + 1] = row
    check("Java SDK array " .. phase .. " keeps current source throughout the first response", row.stable,
      vim.inspect(row.after), row)
    if text then
      vim.api.nvim_buf_set_lines(buffer, position.line, position.line + 1, false, { original[position.line + 1] })
      flush()
    end
    local result = response and response.result
    if result == vim.NIL then result = nil end
    return result, row
  end
  local ok, err = xpcall(function()
    assert(vim.lsp.buf_attach_client(buffer, id), "SDK array fixture did not attach")
    flush()
    for _, case in ipairs({
      { phase = "row public Object", anchor = "int rowHash = rows[0].hashCode();", token = "rows",
        text = "        rows[0].", expected = { "length", "clone", "toString", "hashCode", "equals" },
        excluded = { "finalize", "string", "getName" } },
      { phase = "cloned row element", anchor = "String cellText = rows[0].clone()[0].string();", token = "rows",
        text = "        rows[0].clone()[0].", expected = { "string" }, excluded = { "length", "getName" } },
      { phase = "File listFiles array", anchor = "int entryCount = directory.listFiles().length;", token = "directory",
        text = "        directory.listFiles().", expected = { "length", "clone", "toString", "hashCode", "equals" },
        excluded = { "finalize", "getName", "string" } },
      { phase = "File cloned array element", anchor = "String name = directory.listFiles().clone()[0].getName();", token = "directory",
        text = "        directory.listFiles().clone()[0].", expected = { "getName", "length" }, excluded = { "string", "finalize" } },
    }) do
      local result, row = once(case.phase, "textDocument/completion", case.anchor, case.token, case.text)
      local matched = row.stable
      for _, name in ipairs(case.expected) do matched = matched and probe.completion_has(result, name) end
      for _, name in ipairs(case.excluded) do matched = matched and not probe.completion_has(result, name) end
      if case.phase == "File cloned array element" then
        local file_length = false
        for _, item in ipairs((result or {}).items or result or {}) do
          file_length = file_length or (item.label == "length" and item.kind == 3)
        end
        matched = matched and file_length
      end
      row.expected, row.excluded = case.expected, case.excluded
      check("Java SDK array " .. case.phase .. " offers required members without unrelated/protected members",
        matched, vim.inspect(result), row)
    end
    for _, case in ipairs({
      { phase = "actual Object toString", anchor = "String rowText = rows[0].toString();", token = "toString",
        suffix = "java/lang/Object.java", source = report.expected_sdk_sources.Object },
      { phase = "actual File getName", anchor = "String name = directory.listFiles().clone()[0].getName();", token = "getName",
        suffix = "java/io/File.java", source = report.expected_sdk_sources.File },
    }) do
      local result, row = once(case.phase, "textDocument/definition", case.anchor, case.token)
      row.expected_source_suffix, row.expected_range = case.suffix, case.source.range
      check("Java SDK array " .. case.phase .. " goes to the exact SDK identifier",
        row.stable and probe.exact_definition(result, case.source.path, { case.source.range }), vim.inspect(result), row)
    end
  end, debug.traceback)
  local cleanup_ok, cleanup_error = pcall(function()
    if not vim.deep_equal(vim.api.nvim_buf_get_lines(buffer, 0, -1, false), original) then
      vim.api.nvim_buf_set_lines(buffer, 0, -1, false, original)
      flush()
    end
  end)
  client.rpc.notify = notify
  local modified_ok, modified_error = pcall(function() vim.bo[buffer].modified = modified end)
  report.restored = probe.buffer_sha256(buffer) == original_hash
  report.restored_text, report.restored_wire_text = probe.buffer_full_text(buffer), wire
  report.restored_wire_sha256, report.restored_version = vim.fn.sha256(wire), vim.lsp.util.buf_versions[buffer]
  report.wire_restored = latest and latest.sent and latest.version == report.restored_version
    and latest.source_sha256 == original_hash and latest.wire_source_sha256 == original_hash
    and not latest.wire_error and report.restored_wire_sha256 == original_hash
  report.owner_restored = vim.deep_equal(owner_snapshot(), owner_original)
  report.disk_unchanged, report.modified_restored = disk_sha() == disk_hash, vim.bo[buffer].modified == modified
  report.notify_restored, report.error = client.rpc.notify == notify, not ok and err or nil
  report.cleanup_error = not cleanup_ok and cleanup_error or not modified_ok and modified_error or nil
  check("Java SDK array source, wire, owner, disk, modified flag and notification hook restore exactly",
    cleanup_ok and modified_ok and report.restored and report.wire_restored and report.owner_restored
      and report.disk_unchanged and report.modified_restored and report.notify_restored, vim.inspect(report))
  local each_once = #report.requests == report.expected_requests
  for _, row in ipairs(report.requests) do each_once = each_once and row.attempted and row.attempts == 1 end
  check("Java SDK array six requests each run exactly once", each_once, vim.inspect(report.requests))
  if not ok then check("Java SDK array probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-array-sdk.json")
  end
end

-- Kotlin reads of Java arrays: five additional first responses after old 72 and SDK 6.
do
  local files, names = {}, { "factory", "kotlin" }
  for name, relative in pairs({ factory = "resourcecase/KotlinArrayFactory.java", kotlin = "resourcecase/KotlinArrayUse.kt" }) do
    local path = project .. "/" .. relative
    local buffer = vim.fn.bufadd(path)
    vim.fn.bufload(buffer)
    vim.bo[buffer].filetype = name == "factory" and "java" or "kotlin"
    files[name] = { path = path, buf = buffer, uri = vim.uri_from_bufnr(buffer),
      original = vim.api.nvim_buf_get_lines(buffer, 0, -1, false), hash = probe.buffer_sha256(buffer),
      modified = vim.bo[buffer].modified, wire = probe.buffer_full_text(buffer),
      disk = vim.fn.sha256(table.concat(vim.fn.readfile(path, "b"), "\n")) }
  end
  local body_path = project .. "/resourcecase/Bodies.kt"
  local body_buf = vim.fn.bufnr(body_path)
  assert(body_buf > 0, "existing Bodies fixture was not loaded")
  local function body_snapshot()
    return { uri = vim.uri_from_bufnr(body_buf), text = probe.buffer_full_text(body_buf),
      sha256 = probe.buffer_sha256(body_buf), version = vim.lsp.util.buf_versions[body_buf],
      tick = vim.api.nvim_buf_get_changedtick(body_buf), modified = vim.bo[body_buf].modified,
      disk_sha256 = vim.fn.sha256(table.concat(vim.fn.readfile(body_path, "b"), "\n")) }
  end
  local body_original = body_snapshot()
  local report = { requests = {}, outbound = {}, expected_requests = 5, original = {},
    owner_source = body_original, owner_wire_observed_by_this_block = false }
  for _, name in ipairs(names) do
    local file = files[name]
    report.original[name] = { text = file.wire, sha256 = file.hash, disk_sha256 = file.disk,
      fileformat = vim.bo[file.buf].fileformat, eol = vim.bo[file.buf].eol, modified = file.modified }
  end
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    for _, name in ipairs(names) do
      local file = files[name]
      if params.textDocument and params.textDocument.uri == file.uri
        and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
        local wire, err
        if method == "textDocument/didOpen" then wire = params.textDocument.text
        else wire, err = probe.apply_content_changes(file.wire, params.contentChanges, client.offset_encoding or "utf-16") end
        if sent and wire then file.wire = wire end
        file.latest = { sent = not not sent, version = params.textDocument.version,
          source_sha256 = probe.buffer_sha256(file.buf), wire_source_sha256 = wire and vim.fn.sha256(wire), error = err }
        report.outbound[#report.outbound + 1] = { file = name, delivery = vim.deepcopy(file.latest) }
      end
    end
    return sent
  end
  local function flush() require("vim.lsp._changetracking").flush(client) end
  local function sources()
    local out = { body = body_snapshot() }
    for _, name in ipairs(names) do
      local file = files[name]
      out[name] = { uri = file.uri, text = probe.buffer_full_text(file.buf), wire_text = file.wire,
        sha256 = probe.buffer_sha256(file.buf), wire_sha256 = vim.fn.sha256(file.wire),
        version = vim.lsp.util.buf_versions[file.buf], tick = vim.api.nvim_buf_get_changedtick(file.buf) }
    end
    return out
  end
  local function once(phase, method, anchor, token, text)
    local file = files.kotlin
    local position = probe.find(file.original, anchor, token)
    if text then
      vim.api.nvim_buf_set_lines(file.buf, position.line, position.line + 1, false, { text })
      flush()
      position.character = vim.str_utfindex(text, "utf-16")
    end
    local before, delivery, delivered = sources(), {}, true
    for _, name in ipairs(names) do
      local current, latest = before[name], files[name].latest
      delivered = delivered and latest ~= nil and latest.sent and not latest.error
        and latest.version == current.version and latest.source_sha256 == current.sha256
        and latest.wire_source_sha256 == current.sha256 and current.wire_sha256 == current.sha256
      delivery[name] = vim.deepcopy(latest)
    end
    local params = { textDocument = { uri = file.uri }, position = position }
    local response, row
    if delivered then response, row = request_rpc(method, params, file.buf)
    else
      row = { method = method, params = params, attempted = false, attempts = 0,
        availability = "not-run", delivery_error = "current Java/Kotlin array sources were not observed outbound" }
      requests[#requests + 1] = row
    end
    row.kotlin_array_phase, row.source, row.delivery = phase, before, delivery
    row.after = sources()
    row.stable = vim.deep_equal(before, row.after) and vim.deep_equal(before.body, body_original)
    report.requests[#report.requests + 1] = row
    check("Kotlin Java-array " .. phase .. " keeps both current sources and owner stable for first response",
      delivered and row.stable, vim.inspect(row.after), row)
    if text then
      vim.api.nvim_buf_set_lines(file.buf, position.line, position.line + 1, false, { file.original[position.line + 1] })
      flush()
    end
    local result = response and response.result
    if result == vim.NIL then result = nil end
    return result, row
  end
  local ok, err = xpcall(function()
    for _, name in ipairs(names) do assert(vim.lsp.buf_attach_client(files[name].buf, id), "Kotlin array fixture did not attach") end
    flush()
    for _, case in ipairs({
      { phase = "matrix size get clone", anchor = "val total: Int = factory.matrix().size", token = "factory",
        text = "    factory.matrix().", expected = { "size", "get", "clone" }, excluded = { "length", "wait", "getClass", "string" } },
      { phase = "indexed row size get clone", anchor = "val rowSize: Int = row.size", token = "row",
        text = "    row.", expected = { "size", "get", "clone" }, excluded = { "length", "wait", "getClass", "string" } },
      { phase = "indexed element", anchor = "factory.matrix()[0][0].string()", token = "factory",
        text = "    factory.matrix()[0][0].", expected = { "string" }, excluded = { "length", "size", "get", "enqueue" } },
      { phase = "cloned get element", anchor = "factory.matrix().clone().get(0).get(0).string()", token = "factory",
        text = "    factory.matrix().clone().get(0).get(0).", expected = { "string" }, excluded = { "length", "size", "get", "enqueue" } },
    }) do
      local result, row = once(case.phase, "textDocument/completion", case.anchor, case.token, case.text)
      local matched = row.stable and row.attempted and row.attempts == 1 and row.availability == "responded"
      for _, name in ipairs(case.expected) do matched = matched and probe.completion_has(result, name) end
      for _, name in ipairs(case.excluded) do matched = matched and not probe.completion_has(result, name) end
      row.expected, row.excluded = case.expected, case.excluded
      check("Kotlin Java-array " .. case.phase .. " offers useful members without Java-only/scalar leakage",
        matched, vim.inspect(result), row)
    end
    local result, row = once("exact indexed Body member", "textDocument/definition", "factory.matrix()[0][0].string()", "string")
    local _, range = probe.find(vim.api.nvim_buf_get_lines(body_buf, 0, -1, false), "fun string", "string")
    row.expected_uri, row.expected_range = vim.uri_from_bufnr(body_buf), range
    check("Kotlin Java-array indexed element goes to exact authored Body.string",
      row.stable and row.attempted and row.attempts == 1 and row.availability == "responded"
        and probe.exact_definition(result, body_path, { range }), vim.inspect(result), row)
  end, debug.traceback)
  report.restored, report.wire_restored, report.disk_unchanged, report.modified_restored = true, true, true, true
  report.final, report.cleanup_errors = {}, {}
  for _, name in ipairs(names) do
    local file = files[name]
    local cleanup_ok, cleanup_error = pcall(function()
      if not vim.deep_equal(vim.api.nvim_buf_get_lines(file.buf, 0, -1, false), file.original) then
        vim.api.nvim_buf_set_lines(file.buf, 0, -1, false, file.original)
        flush()
      end
      vim.bo[file.buf].modified = file.modified
    end)
    if not cleanup_ok then report.cleanup_errors[name] = cleanup_error end
    local hash, latest = probe.buffer_sha256(file.buf), file.latest
    report.restored = report.restored and cleanup_ok and hash == file.hash
    report.wire_restored = report.wire_restored and latest ~= nil and latest.sent and not latest.error
      and latest.version == vim.lsp.util.buf_versions[file.buf] and latest.source_sha256 == file.hash
      and latest.wire_source_sha256 == file.hash and vim.fn.sha256(file.wire) == file.hash
    report.disk_unchanged = report.disk_unchanged
      and vim.fn.sha256(table.concat(vim.fn.readfile(file.path, "b"), "\n")) == file.disk
    report.modified_restored = report.modified_restored and vim.bo[file.buf].modified == file.modified
  end
  client.rpc.notify = notify
  report.final, report.owner_restored = sources(), vim.deep_equal(body_snapshot(), body_original)
  report.notify_restored, report.error = client.rpc.notify == notify, not ok and err or nil
  check("Kotlin Java-array buffers, current wire, owner, disks, flags and notify hook restore exactly",
    report.restored and report.wire_restored and report.disk_unchanged and report.modified_restored
      and report.owner_restored and report.notify_restored, vim.inspect(report))
  local each_once = #report.requests == report.expected_requests
  for _, row in ipairs(report.requests) do each_once = each_once and row.attempted and row.attempts == 1 end
  check("Kotlin Java-array five requests each run exactly once", each_once, vim.inspect(report.requests))
  if not ok then check("Kotlin Java-array probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-array-kotlin.json")
  end
end

-- Hints must use the same contextual types as completion, including unsaved Kotlin producers.
do
  local files = {
    java = { path = project .. "/hints/HintUse.java" },
    kotlin = { path = project .. "/hints/Hints.kt" },
  }
  local report = { requests = {}, outbound = {}, type_cases = 13, invalid_cases = 3, expected_requests = 6 }
  for _, file in pairs(files) do
    file.buf = vim.fn.bufadd(file.path)
    vim.fn.bufload(file.buf)
    file.original = vim.api.nvim_buf_get_lines(file.buf, 0, -1, false)
    file.modified = vim.bo[file.buf].modified
    file.uri = vim.uri_from_bufnr(file.buf)
    file.hash = probe.buffer_sha256(file.buf)
    file.disk = vim.fn.sha256(table.concat(vim.fn.readfile(file.path, "b"), "\n"))
    file.wire = probe.buffer_full_text(file.buf)
    vim.bo[file.buf].filetype = file == files.java and "java" or "kotlin"
  end
  local notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    for name, file in pairs(files) do
      if params.textDocument and params.textDocument.uri == file.uri
        and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
        local wire, err
        if method == "textDocument/didOpen" then wire = params.textDocument.text
        else wire, err = probe.apply_content_changes(file.wire, params.contentChanges, client.offset_encoding or "utf-16") end
        if sent and wire then file.wire = wire end
        file.latest = { sent = not not sent, version = params.textDocument.version,
          source_sha256 = probe.buffer_sha256(file.buf), wire_source_sha256 = wire and vim.fn.sha256(wire), error = err }
        report.outbound[#report.outbound + 1] = { file = name, delivery = vim.deepcopy(file.latest) }
      end
    end
    return sent
  end
  local function flush() require("vim.lsp._changetracking").flush(client) end
  local function query(phase, kotlin_type, only, invalid)
    local lines = vim.api.nvim_buf_get_lines(files.java.buf, 0, -1, false)
    local expected, count = {}, 0
    local range = { start = { line = 0, character = 0 }, ["end"] = { line = #lines, character = 0 } }
    if invalid then
      local position = probe.find(lines, "var missing = missingCall();", "missing")
      range.start = { line = position.line, character = 0 }
    else
      for _, pair in ipairs({
        { "longNumber", "long" }, { "floatNumber", "float" }, { "boxed", "Box<String>" },
        { "selected", "Body" }, { "overloaded", "Box<String>" },
        { "covariant", "Box<? extends Body>" }, { "contravariant", "Box<? super Body>" },
        { "wildcard", "Box<?>" }, { "alias", "Body" }, { "generic", "Body" },
        { "fromKotlin", kotlin_type }, { "resource", "Body" }, { "resourceAlias", "Body" },
      }) do
        if not only or pair[1] == only then
          local _, token = probe.find(lines, "var " .. pair[1] .. " =", pair[1])
          expected[token["end"].line .. ":" .. token["end"].character] = ": " .. pair[2]
          count = count + 1
          if only then range = { start = { line = token.start.line, character = 0 },
            ["end"] = { line = token.start.line + 1, character = 0 } } end
        end
      end
    end
    local delivered, delivery = true, {}
    for name, file in pairs(files) do
      local current = probe.buffer_sha256(file.buf)
      local latest = file.latest
      delivered = delivered and latest and latest.sent and not latest.error
        and latest.source_sha256 == current and latest.wire_source_sha256 == current
        and latest.version == vim.lsp.util.buf_versions[file.buf]
      delivery[name] = vim.deepcopy(latest)
    end
    local params = { textDocument = { uri = files.java.uri }, range = range }
    local response, row
    if delivered then response, row = request_rpc("textDocument/inlayHint", params, files.java.buf)
    else
      row = { method = "textDocument/inlayHint", params = params, attempted = false, attempts = 0,
        availability = "not-run", delivery_error = "current Java/Kotlin source was not observed outbound" }
      requests[#requests + 1] = row
    end
    local result = response and response.result
    local actual = {}
    if type(result) == "table" then
      for _, hint in ipairs(result) do actual[hint.position.line .. ":" .. hint.position.character] = hint.label end
    end
    row.hint_phase, row.delivery, row.expected, row.actual = phase, delivery, expected, actual
    report.requests[#report.requests + 1] = row
    local empty = result == nil or result == vim.NIL
    local matched = count == 0 and empty or type(result) == "table" and #result == count
      and vim.deep_equal(actual, expected)
    check("Java contextual hints " .. phase, matched, vim.inspect(result), row)
  end
  local ok, err = xpcall(function()
    for _, file in pairs(files) do assert(vim.lsp.buf_attach_client(file.buf, id), "hint fixture did not attach") end
    flush()
    query("initial exact 13 types", "Payload")
    local changed = vim.deepcopy(files.kotlin.original)
    for line, text in ipairs(changed) do changed[line] = text:gsub("supply%(%)%: Payload = Payload%(%)", "supply(): Updated = Updated()") end
    vim.api.nvim_buf_set_lines(files.kotlin.buf, 0, -1, false, changed)
    flush()
    query("unsaved Kotlin return Updated", "Updated")
    vim.api.nvim_buf_set_lines(files.kotlin.buf, 0, -1, false, files.kotlin.original)
    flush()
    query("restored Kotlin return Payload", "Payload")
    query("one visible line", "Payload", "floatNumber")
    local invalid = vim.deepcopy(files.java.original)
    for line, text in ipairs(invalid) do
      if text:find("Body explicit = body;", 1, true) then
        table.insert(invalid, line + 1, "        var missing = missingCall();")
        table.insert(invalid, line + 2, "        var self = self;")
        table.insert(invalid, line + 3, "        var rejected = factory.make(new Other());")
        break
      end
    end
    vim.api.nvim_buf_set_lines(files.java.buf, 0, -1, false, invalid)
    flush()
    query("invalid initializers stay unknown", "Payload", nil, true)
    vim.api.nvim_buf_set_lines(files.java.buf, 0, -1, false, files.java.original)
    flush()
    query("restored Java exact 13 types", "Payload")
  end, debug.traceback)
  report.restored, report.disk_unchanged = true, true
  for _, file in pairs(files) do
    if not vim.deep_equal(vim.api.nvim_buf_get_lines(file.buf, 0, -1, false), file.original) then
      vim.api.nvim_buf_set_lines(file.buf, 0, -1, false, file.original)
      flush()
    end
    report.restored = report.restored and probe.buffer_sha256(file.buf) == file.hash
    report.disk_unchanged = report.disk_unchanged
      and vim.fn.sha256(table.concat(vim.fn.readfile(file.path, "b"), "\n")) == file.disk
    vim.bo[file.buf].modified = file.modified
  end
  client.rpc.notify = notify
  report.error = not ok and err or nil
  check("Java contextual hint buffers restore without disk edits", report.restored and report.disk_unchanged, vim.inspect(report))
  if not ok then check("Java contextual hint probes execute", false, err) end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) }, vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/java-context-hints.json")
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

local function watch_changes(buffer)
  local notify, wire, delivery = client.rpc.notify, probe.buffer_full_text(buffer), nil
  client.rpc.notify = function(method, params)
    local sent = notify(method, params)
    if method == "textDocument/didChange" and params.textDocument.uri == vim.uri_from_bufnr(buffer) then
      local next_wire, err = probe.apply_content_changes(wire, params.contentChanges,
        client.offset_encoding or "utf-16")
      if sent and next_wire then wire = next_wire end
      delivery = { sent = not not sent, version = params.textDocument.version, error = err,
        text = wire, source_sha256 = probe.buffer_sha256(buffer), wire_source_sha256 = vim.fn.sha256(wire) }
    end
    return sent
  end
  return function() client.rpc.notify = notify; return delivery end
end

-- Annotation names must remain completable before their declarations are finished.
do
  local kotlin_buf = vim.fn.bufadd(project .. "/app/KotlinHelper.kt")
  vim.fn.bufload(kotlin_buf)
  vim.bo[kotlin_buf].filetype = "kotlin"
  assert(vim.lsp.buf_attach_client(kotlin_buf, id), "annotation Kotlin buffer did not attach")
  local snapshots = {}
  for _, case in ipairs({
    { "Java plain annotation", bufnr, "@Over", "Override" },
    { "Java qualified annotation", bufnr, "@java.lang.Over", "Override" },
    { "Java unknown annotation package", bufnr, "@missing.annotation.Over", false },
    { "Kotlin plain annotation", kotlin_buf, "@Depre", "Deprecated" },
    { "Kotlin qualified annotation", kotlin_buf, "@java.lang.Depre", "Deprecated" },
    { "Kotlin unknown annotation package", kotlin_buf, "@missing.annotation.Depre", false },
  }) do
    local name, buffer, partial, expected = unpack(case)
    local original = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
    local original_modified = vim.bo[buffer].modified
    local disk = vim.fn.readfile(vim.api.nvim_buf_get_name(buffer))
    local lines, line = vim.deepcopy(original), nil
    for i, text in ipairs(lines) do
      if buffer == bufnr and text:find("public void runAgain()", 1, true) then
        lines[i] = "    public String toString() { return \"annotation\"; }"
        table.insert(lines, i, "    " .. partial)
        line = i - 1
        break
      elseif buffer == kotlin_buf and text:find("class KotlinHelper", 1, true) then
        table.insert(lines, i, partial)
        line = i - 1
        break
      end
    end
    assert(line, "annotation declaration anchor not found")
    local finish_delivery = watch_changes(buffer)
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
    require("vim.lsp._changetracking").flush(client)
    local delivery = finish_delivery()
    local version = vim.lsp.util.buf_versions[buffer]
    check(name .. " current exact unsaved text is outbound", delivery and delivery.sent
      and not delivery.error and delivery.version == version
      and delivery.source_sha256 == delivery.wire_source_sha256)
    local params = { textDocument = { uri = vim.uri_from_bufnr(buffer) },
      position = { line = line, character = #lines[line + 1] } }
    local response, row = request_rpc("textDocument/completion", params, buffer)
    local result = response and response.result
    local items = result and result ~= vim.NIL and (result.items or result) or {}
    local matched = expected and probe.completion_has(result, expected)
      or (not expected and #items == 0)
    check(name .. (expected and " offers " .. expected or " excludes known annotations"),
      matched, vim.inspect(result), row)
    check(name .. " excludes ordinary classes", not probe.completion_has(result, "OverlayLayout")
      and not probe.completion_has(result, "DeprecatedTree"), vim.inspect(result), row)
    snapshots[#snapshots + 1] = { name = name, text = lines, expected = expected,
      version = version, delivery = delivery, request = row,
      unsaved = vim.bo[buffer].modified and vim.deep_equal(disk,
        vim.fn.readfile(vim.api.nvim_buf_get_name(buffer))) }
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, original)
    vim.bo[buffer].modified = original_modified
    require("vim.lsp._changetracking").flush(client)
    check(name .. " restores exact buffer and disk", vim.deep_equal(original,
      vim.api.nvim_buf_get_lines(buffer, 0, -1, false)) and snapshots[#snapshots].unsaved)
  end
  if arg[3] then
    vim.fn.writefile({ vim.json.encode(snapshots) }, arg[3] .. "/annotation-snapshots.json")
  end
  local applied_snapshots = {}
  vim.fn.mkdir(project .. "/annotationcase", "p")
  for _, case in ipairs({
    { name = "Java annotation auto-import", file = "Imported.java", label = "Documented", prefix = "Docu",
      original = { "package annotationcase;", "", "@java.lang.annotation.Documented", "public @interface Imported {}" },
      edited = { "package annotationcase;", "", "@Docu", "public @interface Imported {}" },
      expected = "package annotationcase;\nimport java.lang.annotation.Documented;\n\n@Documented\npublic @interface Imported {}\n" },
    { name = "Java ordinary auto-import", file = "Ordinary.java", label = "ArrayList", prefix = "ArrayLis",
      original = { "package annotationcase;", "", "class Ordinary { java.util.ArrayList values; }" },
      edited = { "package annotationcase;", "", "class Ordinary { ArrayLis values; }" },
      expected = "package annotationcase;\nimport java.util.ArrayList;\n\nclass Ordinary { ArrayList values; }\n" },
  }) do
    local path = project .. "/annotationcase/" .. case.file
    vim.fn.writefile(case.original, path)
    local buffer = vim.fn.bufadd(path)
    vim.fn.bufload(buffer)
    vim.bo[buffer].filetype = "java"
    assert(vim.lsp.buf_attach_client(buffer, id), "import buffer did not attach")
    local finish_delivery = watch_changes(buffer)
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, case.edited)
    require("vim.lsp._changetracking").flush(client)
    local delivery = finish_delivery()
    local version = vim.lsp.util.buf_versions[buffer]
    local start = assert(case.edited[3]:find(case.prefix, 1, true)) - 1
    local params = { textDocument = { uri = vim.uri_from_bufnr(buffer) },
      position = { line = 2, character = start + #case.prefix } }
    local response, row = request_rpc("textDocument/completion", params, buffer)
    local result = response and response.result
    local item
    for _, candidate in ipairs(result and result ~= vim.NIL and (result.items or result) or {}) do
      if candidate.label == case.label then item = candidate; break end
    end
    check(case.name .. " offers exact type on first response", item ~= nil, vim.inspect(result), row)
    local applied, message = false, nil
    if item then
      local edits = vim.deepcopy(item.additionalTextEdits or {})
      edits[#edits + 1] = item.textEdit or { newText = item.insertText or item.label,
        range = { start = { line = 2, character = start }, ["end"] = params.position } }
      applied, message = pcall(vim.lsp.util.apply_text_edits, edits, buffer, client.offset_encoding)
    end
    local actual = probe.buffer_full_text(buffer)
    local current_wire = delivery and delivery.sent and not delivery.error
      and delivery.version == version
      and delivery.source_sha256 == delivery.wire_source_sha256
      and delivery.text == table.concat(case.edited, "\n") .. "\n"
    check(case.name .. " exact unsaved text is outbound", current_wire)
    check(case.name .. " applies exact complete Java source", applied and actual == case.expected,
      message or actual, row)
    check(case.name .. " keeps disk source unchanged", vim.deep_equal(vim.fn.readfile(path), case.original))
    local artifact = arg[3] and arg[3] .. "/annotation-applied/" .. case.file
    if artifact then
      vim.fn.mkdir(vim.fs.dirname(artifact), "p")
      vim.fn.writefile(vim.split(actual, "\n", { plain = true }), artifact, "b")
    end
    applied_snapshots[#applied_snapshots + 1] = { name = case.name, request = row, delivery = delivery, version = version,
      applied = applied, text = actual, expected = case.expected, artifact = artifact,
      exact_current_wire = not not current_wire, unchanged_disk = vim.deep_equal(vim.fn.readfile(path), case.original) }
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, case.original)
    vim.bo[buffer].modified = false
    require("vim.lsp._changetracking").flush(client)
  end
  if arg[3] then
    vim.fn.writefile({ vim.json.encode(applied_snapshots) }, arg[3] .. "/annotation-import-snapshots.json")
  end
end

-- A valid default JVM extension must coexist with provable missing-member errors.
do
  local path = project .. "/app/KotlinHelper.kt"
  assert(vim.fn.filereadable(path) == 1, "universal-property probe requires the scanned Kotlin fixture")
  local buffer = vim.fn.bufadd(path)
  vim.fn.bufload(buffer)
  local original = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
  local original_disk = vim.fn.readfile(path, "b")
  local original_modified = vim.bo[buffer].modified
  vim.bo[buffer].filetype = "kotlin"
  vim.bo[buffer].eol = true
  assert(vim.lsp.buf_attach_client(buffer, id), "universal-property buffer did not attach")
  local valid = { "package app", "class UniversalPlain",
    "fun universalName(value: UniversalPlain) = value.javaClass.name" }
  local invalid = { valid[1], valid[2],
    "fun universalName(value: UniversalPlain) = value.missingUniversal" }
  local report = { phases = {}, completions = {} }
  for _, phase in ipairs({ { name = "valid", source = valid, missing = false },
      { name = "missing", source = invalid, missing = true },
      { name = "restored", source = valid, missing = false } }) do
    local observation, finish = probe.observe_diagnostics(client, buffer)
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, phase.source)
    require("vim.lsp._changetracking").flush(client)
    local result = finish(5000)
    local diagnostics = result and result.diagnostics or nil
    local expected = diagnostics ~= nil and #diagnostics == (phase.missing and 1 or 0)
    if phase.missing and expected then
      expected = diagnostics[1].code == "unresolved_reference"
        and diagnostics[1].message:find("missingUniversal", 1, true) ~= nil
    end
    check("Kotlin universal JVM property " .. phase.name .. " has exact diagnostics",
      expected, vim.inspect(result), observation)
    check("Kotlin universal JVM property " .. phase.name .. " observes exact unsaved source",
      observation.generation_certified and observation.wire_matches_snapshot
        and observation.source_sha256 == probe.buffer_sha256(buffer)
        and vim.bo[buffer].modified and vim.deep_equal(vim.fn.readfile(path, "b"), original_disk),
      vim.inspect(observation))
    report.phases[#report.phases + 1] = { name = phase.name,
      text = probe.buffer_full_text(buffer), observation = observation }
  end
  for _, case in ipairs({
    { name = "canonical blank", prefix = "", wanted = "javaClass" },
    { name = "canonical prefix", prefix = "javaC", wanted = "javaClass" },
    { name = "alias blank", alias = true, prefix = "", wanted = "runtimeClass" },
    { name = "alias prefix", alias = true, prefix = "runtimeC", wanted = "runtimeClass" },
    { name = "alias hides canonical", alias = true, prefix = "javaC" },
  }) do
    local lines = { "package app" }
    if case.alias then lines[#lines + 1] = "import kotlin.jvm.javaClass as runtimeClass" end
    lines[#lines + 1] = "class UniversalPlain"
    lines[#lines + 1] = "fun universalMembers(value: UniversalPlain) { value." .. case.prefix .. " }"
    local finish_delivery = watch_changes(buffer)
    vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines)
    require("vim.lsp._changetracking").flush(client)
    local delivery = finish_delivery()
    local position = { line = #lines - 1,
      character = assert(lines[#lines]:find("value.", 1, true)) - 1 + #"value." + #case.prefix }
    local response, row = request_rpc("textDocument/completion", {
      textDocument = { uri = vim.uri_from_bufnr(buffer) }, position = position }, buffer)
    local result = response and response.result
    local found, hidden = false, true
    for _, item in ipairs(result and result ~= vim.NIL and (result.items or result) or {}) do
      if item.label == case.wanted then found = true end
      if case.alias and item.label == "javaClass" then hidden = false end
    end
    local matched = row.availability == "responded" and hidden and (not case.wanted or found)
    check("Kotlin universal JVM property " .. case.name .. " first completion",
      matched, vim.inspect(result), row)
    local exact_wire = delivery and delivery.sent and not delivery.error
      and delivery.version == vim.lsp.util.buf_versions[buffer]
      and delivery.source_sha256 == probe.buffer_sha256(buffer)
      and delivery.wire_source_sha256 == delivery.source_sha256
      and delivery.text == table.concat(lines, "\n") .. "\n"
    check("Kotlin universal JVM property " .. case.name .. " keeps exact unsaved source",
      exact_wire and vim.bo[buffer].modified and vim.deep_equal(vim.fn.readfile(path, "b"), original_disk),
      vim.inspect(delivery))
    report.completions[#report.completions + 1] = { name = case.name, request = row,
      delivery = delivery, expected_label = case.wanted, canonical_hidden = hidden,
      exact_current_wire = not not exact_wire }
  end
  if vim.env.KTLSP_TRACE then
    vim.fn.writefile({ vim.json.encode(report) },
      vim.fs.dirname(vim.env.KTLSP_TRACE) .. "/kotlin-universal-property.json")
  end
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, original)
  require("vim.lsp._changetracking").flush(client)
  check("Kotlin universal JVM property cleanup restores the buffer and untouched disk",
    vim.deep_equal(vim.api.nvim_buf_get_lines(buffer, 0, -1, false), original)
      and vim.deep_equal(vim.fn.readfile(path, "b"), original_disk))
  vim.bo[buffer].modified = original_modified
end


-- Enum constants keep their implicit public/static role without guessing an unproved type name.
do
  local original = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  local original_modified = vim.bo[bufnr].modified
  local path = vim.api.nvim_buf_get_name(bufnr)
  local disk, snapshots = vim.fn.readfile(path), {}
  for _, case in ipairs({
    { name = "plain enum namespace", header = "class Main", params = "", receiver = "EnumProbeMode", expected = true },
    { name = "annotated enum namespace", header = "@UnknownProducer class Main", params = "", receiver = "EnumProbeMode", expected = false },
    { name = "shadowed enum namespace", header = "class Main", params = "Object EnumProbeMode", receiver = "EnumProbeMode", expected = false },
    { name = "malformed local enum shadow", header = "class Main", params = "", receiver = "EnumProbeMode", before = "Object EnumProbeMode = ; ", expected = false },
    { name = "annotated explicit enum value", header = "@UnknownProducer class Main", params = "EnumProbeMode value", receiver = "value", expected = true },
  }) do
    local lines = { "package app;", "enum EnumProbeMode { ENABLED, DISABLED }",
      case.header .. " { boolean enumProbe(" .. case.params .. ") { " .. (case.before or "")
        .. "return " .. case.receiver .. ".ENABLED == " .. case.receiver .. ".DISABLED; } }" }
    local stop = watch_changes(bufnr)
    vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, lines)
    require("vim.lsp._changetracking").flush(client)
    local delivery = stop()
    local position = probe.find(lines, "return ", "ENABLED")
    local _, target = probe.find(lines, "enum EnumProbeMode", "ENABLED")
    local response, observation = request_rpc("textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(bufnr) }, position = position,
    }, bufnr)
    local result = response and response.result
    local ok = case.expected and probe.exact_definition(result, path, { target })
      or not case.expected and #probe.locations(result) == 0
    check(case.name .. " exact enum navigation", ok, vim.inspect(result), observation)
    check(case.name .. " uses exact delivered unsaved source", delivery and delivery.sent
      and delivery.version == vim.lsp.util.buf_versions[bufnr]
      and delivery.text == probe.buffer_full_text(bufnr)
      and delivery.source_sha256 == delivery.wire_source_sha256
      and vim.deep_equal(vim.fn.readfile(path), disk), vim.inspect(delivery))
    snapshots[#snapshots + 1] = { name = case.name, source = lines, expected = case.expected,
      expected_range = target, delivery = delivery, observation = observation }
  end
  if arg[3] then
    vim.fn.writefile({ vim.json.encode(snapshots) }, arg[3] .. "/java-enum-namespace.json")
  end
  vim.api.nvim_buf_set_lines(bufnr, 0, -1, false, original)
  require("vim.lsp._changetracking").flush(client)
  vim.bo[bufnr].modified = original_modified
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

local versions_match = #diagnostic_probes == 4
for _, observation in ipairs(diagnostic_probes) do
  local result = observation.response and observation.response.result
  versions_match = versions_match and observation.generation_certified == true
    and observation.wire_matches_snapshot == true and result ~= nil
    and result.version == observation.expected_version
end
check("diagnostic notifications use exact edited document versions", versions_match,
  vim.inspect(diagnostic_probes))


probe.shutdown(client, shutdown, 10000)
check("graceful shutdown receives null then exit with zero process status", shutdown.clean,
  vim.inspect(shutdown))
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
