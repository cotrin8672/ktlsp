---@diagnostic disable: undefined-global
-- Headless Neovim probe for representative Kotlin semantic shapes.
--
-- Checks:
-- - Result/onFailure/getOrThrow chain completion
-- - `this` receiver typing inside `apply`
-- - KMP source-set narrowing for goto-definition
-- - Constructor-property goto when the class header and `(` are split across lines
-- - Alias-backed nested implicit receivers for definition, hover, and completion
-- - `super` navigation and return completion exclude private vararg overloads
-- - Imported extension aliases through unsaved rebind, references, and applied rename

local script_dir = vim.fs.dirname(debug.getinfo(1, "S").source:sub(2))
local probe = dofile(script_dir .. "/nvim_lsp_probe.lua")
local root = arg[1]
assert(root and root ~= "", "usage: nvim -l dev/nvim_semantic_fixture.lua <project-dir> [ktlsp-bin]")

local bin = arg[2] or os.getenv("KTLSP_BIN")
if not bin or bin == "" then
  bin = vim.fn.getcwd() .. "/target/release/ktlsp"
  if vim.fn.filereadable(bin) == 0 then
    bin = vim.fn.getcwd() .. "/target/debug/ktlsp"
  end
end
assert(vim.fn.filereadable(bin) == 1, "ktlsp binary not found — run `cargo build` first")

local files = {
  accounts = root .. "/probe/accounts/AccountsProbe.kt",
  cleaner = root .. "/probe/cleaner/CleanerProbe.kt",
  invitations = root .. "/probe/invitations/InvitationsApi.kt",
  kmp = root .. "/feature/src/jvmMain/kotlin/probe/kmp/KmpProbe.kt",
}

local failures = {}
local function check(name, ok, detail)
  if ok then
    print(("PASS  %s"):format(name))
  else
    print(("FAIL  %s%s"):format(name, detail and ("  (" .. tostring(detail) .. ")") or ""))
    table.insert(failures, name)
  end
end

local id
local config = { name = "ktlsp", cmd = { bin }, root_dir = root }
local shutdown_state = probe.observe_shutdown(config)

local function open_file(path)
  vim.cmd("edit " .. vim.fn.fnameescape(path))
  local bufnr = vim.api.nvim_get_current_buf()
  vim.bo[bufnr].filetype = path:match("%.java$") and "java" or "kotlin"
  id = vim.lsp.start(config)
  return bufnr
end

local bufs = {}
for key, path in pairs(files) do
  bufs[key] = open_file(path)
end

assert(id, "vim.lsp.start returned nil")
vim.wait(10000, function()
  local c = vim.lsp.get_client_by_id(id)
  return c ~= nil and (c.initialized == true or (c.server_capabilities and c.server_capabilities.definitionProvider))
end, 50)

local client = vim.lsp.get_client_by_id(id)
check("advertises definitionProvider", client and client.server_capabilities.definitionProvider ~= nil)
check("advertises completionProvider", client and client.server_capabilities.completionProvider ~= nil)

local function find(bufnr, needle, token, use_last)
  local lines = vim.api.nvim_buf_get_lines(bufnr, 0, -1, false)
  for i, line in ipairs(lines) do
    if line:find(needle, 1, true) then
      local s
      if use_last then
        local from = 1
        while true do
          local hit = line:find(token, from, true)
          if not hit then
            break
          end
          s = hit
          from = hit + 1
        end
      else
        s = line:find(token, 1, true)
      end
      if s then
        return i - 1, s - 1
      end
    end
  end
  return nil
end

local function request(bufnr, method, params)
  local res
  vim.wait(4000, function()
    local r = vim.lsp.buf_request_sync(bufnr, method, params, 1000) or {}
    for _, v in pairs(r) do
      if v.result ~= nil then
        res = v.result
        return true
      end
    end
    return false
  end, 100)
  return res
end

local function completion_labels(bufnr, line, character)
  local res = request(bufnr, "textDocument/completion", {
    textDocument = { uri = vim.uri_from_bufnr(bufnr) },
    position = { line = line, character = character },
    context = { triggerKind = 1 },
  })
  local items = res and (res.items or res) or {}
  local labels = {}
  for _, item in ipairs(items) do
    labels[item.label] = true
  end
  return labels
end

local ready = vim.wait(10000, function()
  local b = open_file(files.accounts)
  local l, c = find(b, ".ema", "ema")
  if not l then
    return false
  end
  local labels = completion_labels(b, l, c + #"ema")
  return labels.email == true
end, 200)
check("semantic probe warmed", ready)

do
  local b = open_file(files.invitations)
  local l, c = find(b, "createInvitationUseCase.execute", "createInvitationUseCase")
  check("found split-header constructor property goto anchor", l ~= nil)
  if l then
    local res = request(b, "textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c },
    })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check(
      "split-header constructor property resolves to declaration",
      loc ~= nil
        and loc.uri ~= nil
        and loc.uri:match("probe/invitations/InvitationsApi%.kt$") ~= nil
        and loc.range.start.line == 15,
      vim.inspect(loc)
    )
  end
end

do
  local b = open_file(files.invitations)
  local l, c = find(b, "val resolvedCall = call", "call", true)
  check("found alias-backed implicit receiver call anchor", l ~= nil)
  if l then
    local params = {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c },
    }
    local res = request(b, "textDocument/definition", params)
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check(
      "alias-backed outer receiver call resolves to Routing.kt",
      loc ~= nil and loc.uri ~= nil and loc.uri:match("probe/invitations/Routing%.kt$") ~= nil,
      vim.inspect(loc)
    )

    local hover = request(b, "textDocument/hover", params)
    check(
      "alias-backed outer receiver call has hover",
      hover ~= nil and vim.inspect(hover):match("call") ~= nil,
      vim.inspect(hover)
    )
  end
end

do
  local b = open_file(files.invitations)
  local l, c = find(b, "                ca", "ca")
  check("found alias-backed implicit receiver completion anchor", l ~= nil)
  if l then
    local labels = completion_labels(b, l, c + #"ca")
    check("alias-backed outer receiver completion offers call", labels.call == true, vim.inspect(labels))
  end
end

do
  local b = open_file(files.accounts)
  local l, c = find(b, ".ema", "ema")
  check("found AccountsProbe completion anchor", l ~= nil)
  if l then
    local labels = completion_labels(b, l, c + #"ema")
    check("Result chain completion offers email", labels.email == true, vim.inspect(labels))
  end
end

do
  local b = open_file(files.cleaner)
  local l, c = find(b, "this.s", "s", true)
  check("found CleanerProbe completion anchor", l ~= nil)
  if l then
    local labels = completion_labels(b, l, c + #"s")
    check("apply-this completion offers s", labels.s == true, vim.inspect(labels))
  end
end

do
  local b = open_file(files.accounts)
  local l, c = find(b, ".onFailure", "onFailure")
  check("found AccountsProbe goto anchor", l ~= nil)
  if l then
    local res = request(b, "textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c },
    })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check("goto onFailure -> Stdlib.kt", loc ~= nil and loc.uri ~= nil and loc.uri:match("Stdlib%.kt$") ~= nil, loc and loc.uri)
  end
end

do
  local b = open_file(files.kmp)
  local l, c = find(b, "Thing = Thing()", "Thing", true)
  check("found KmpProbe goto anchor", l ~= nil)
  if l then
    local res = request(b, "textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c },
    })
    local loc = res
    if type(res) == "table" and res[1] then
      loc = res[1]
    end
    check(
      "KMP goto prefers jvmMain",
      loc ~= nil and loc.uri ~= nil and loc.uri:match("feature/src/jvmMain/kotlin/lib/Thing%.kt$") ~= nil,
      loc and loc.uri
    )
  end
end

do
  local b = open_file(files.accounts)
  local l, c = find(b, "super.pick()", "pick")
  local declaration_line, declaration_character = find(b, "open fun pick", "pick")
  check("found super overload goto anchor", l ~= nil and declaration_line ~= nil)
  if l then
    local reply = client and client:request_sync("textDocument/definition", {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c },
    }, 4000, b)
    local raw = reply and reply.result
    local locations = type(raw) == "table" and (raw.uri and { raw } or raw) or {}
    check("first super goto selects only public overload",
      reply ~= nil and reply.err == nil and #locations == 1
        and locations[1].uri and locations[1].uri:match("probe/accounts/AccountsProbe%.kt$")
        and locations[1].range.start.line == declaration_line
        and locations[1].range.start.character == declaration_character, vim.inspect(reply))
  end
  l, c = find(b, "super.pick().publicO", "publicO")
  check("found super return completion anchor", l ~= nil)
  if l then
    local reply = client and client:request_sync("textDocument/completion", {
      textDocument = { uri = vim.uri_from_bufnr(b) },
      position = { line = l, character = c + #"publicO" },
      context = { triggerKind = 1 },
    }, 4000, b)
    local raw = reply and reply.result
    local items = type(raw) == "table" and (raw.items or raw) or {}
    local found = false
    for _, item in ipairs(items) do
      found = found or item.label == "publicOnly"
    end
    check("first super return completion preserves public type",
      reply ~= nil and reply.err == nil and found, vim.inspect(reply))
  end
end

-- Imported extension aliases use one existing client; every new request consumes its first response.
if client then
  local paths = {
    versioned = root .. "/probe/extensions/vendor/Versioned.java",
    payload = root .. "/probe/extensions/compat/Payload.kt",
    compatibility = root .. "/probe/extensions/compat/Compatibility.kt",
    use = root .. "/probe/extensions/app/AliasUse.kt",
  }
  local order = { "versioned", "payload", "compatibility", "use" }
  local buffers, original, disk, sent_versions = {}, {}, {}, {}
  local alias = { checks = {}, failures = {}, first_responses = {}, deliveries = {}, notifications = {}, disk = {},
    phase = "initial", request_timeout_ms = 4000, timeout_policy = "execution bound, not latency acceptance" }
  local function save_alias()
    if arg[3] then
      vim.fn.writefile({ vim.json.encode(alias) }, arg[3] .. "/semantic-extension-alias.json")
    end
  end
  local function alias_check(name, ok, detail)
    alias.checks[#alias.checks + 1] = { name = name, passed = ok == true, detail = detail }
    if ok ~= true then alias.failures[#alias.failures + 1] = name end
    check("extension alias " .. name, ok, detail and vim.inspect(detail))
    save_alias()
  end
  local function lines(key)
    return vim.api.nvim_buf_get_lines(buffers[key], 0, -1, false)
  end
  local rpc_notify = client.rpc.notify
  client.rpc.notify = function(method, params)
    local sent = rpc_notify(method, params)
    if sent and (method == "textDocument/didOpen" or method == "textDocument/didChange") then
      local document = params.textDocument
      sent_versions[document.uri] = document.version
      alias.notifications[#alias.notifications + 1] = {
        method = method, uri = document.uri, version = document.version,
      }
    end
    return sent
  end
  local same_client = true
  for _, key in ipairs(order) do
    buffers[key] = open_file(paths[key])
    same_client = same_client and id == client.id
    original[key] = lines(key)
    disk[key] = vim.fn.readfile(paths[key], "b")
    alias.disk[key] = { path = paths[key], before_sha256 = vim.fn.sha256(table.concat(disk[key], "\n")) }
  end
  alias_check("four source buffers share the existing client", same_client)

  local function deliver(phase, keys)
    alias.phase = phase
    local expected = {}
    for _, key in ipairs(keys) do expected[key] = vim.lsp.util.buf_versions[buffers[key]] end
    require("vim.lsp._changetracking").flush(client)
    local delivered = { phase = phase, expected = expected, sent = {} }
    for _, key in ipairs(keys) do
      delivered.sent[key] = sent_versions[vim.uri_from_bufnr(buffers[key])]
      alias_check(phase .. " current " .. key .. " text is outbound before the first query",
        expected[key] ~= nil and delivered.sent[key] == expected[key],
        { expected = expected[key], sent = delivered.sent[key] })
    end
    alias.deliveries[#alias.deliveries + 1] = delivered
    save_alias()
  end
  local function ask(name, key, method, anchor, token, extra)
    local located, position, range = pcall(probe.find, lines(key), anchor, token)
    if not located then
      alias.first_responses[#alias.first_responses + 1] = { name = name, phase = alias.phase, attempts = 0,
        method = method, response_received = false, error = position, anchor = anchor, token = token }
      save_alias()
      return nil, false
    end
    if method == "textDocument/completion" then position = range["end"] end
    local params = vim.tbl_extend("force", {
      textDocument = { uri = vim.uri_from_bufnr(buffers[key]) }, position = position,
    }, extra or {})
    local reply, err = client:request_sync(method, params, 4000, buffers[key])
    alias.first_responses[#alias.first_responses + 1] = { name = name, phase = alias.phase, attempts = 1,
      method = method, params = params, response_received = reply ~= nil, response = reply,
      error = err, protocol_error = reply and reply.err,
      sent_version = sent_versions[params.textDocument.uri] }
    save_alias()
    return reply and reply.result, reply ~= nil and reply.err == nil
  end
  local function definition(name, key, anchor, token, target, declaration)
    local _, expected = probe.find(original[target], declaration, token)
    local raw, ok = ask(name, key, "textDocument/definition", anchor, token)
    alias_check(name, ok and probe.exact_definition(raw, paths[target], { expected }), raw)
  end
  -- Alias calls use the written name, while their authored declaration has a different name.
  local function alias_definition(label, declaration, token, target_lines)
    local _, expected = probe.find(target_lines or original.compatibility, declaration, token)
    local raw, ok = ask(label, "use", "textDocument/definition", "v.oldApi()", "oldApi")
    alias_check(label, ok and probe.exact_definition(raw, paths.compatibility, { expected }), raw)
  end
  local function set_use(contents)
    vim.api.nvim_buf_set_lines(buffers.use, 0, -1, false, contents)
  end
  local function replaced(contents, before_text, after_text)
    local result, matches = vim.deepcopy(contents), 0
    for index, line in ipairs(result) do
      local first, last = line:find(before_text, 1, true)
      if first then
        result[index] = line:sub(1, first - 1) .. after_text .. line:sub(last + 1)
        matches = matches + 1
      end
    end
    assert(matches == 1, "unique fixture edit required: " .. before_text)
    return result
  end
  local function restore(phase, delivery_keys)
    for _, key in ipairs(order) do
      if not vim.deep_equal(lines(key), original[key]) then
        vim.api.nvim_buf_set_lines(buffers[key], 0, -1, false, original[key])
      end
    end
    deliver(phase, delivery_keys)
    local exact = true
    for _, key in ipairs(order) do exact = exact and vim.deep_equal(lines(key), original[key]) end
    alias_check(phase .. " restores every complete buffer", exact)
  end

  deliver("initial", order)
  alias_definition("initial alias goto selects authored compat", "fun Versioned.compat()", "compat")
  definition("initial typed return chain goto selects payloadOnly", "use", "v.oldApi().payloadOnly()", "payloadOnly",
    "payload", "fun payloadOnly()")
  definition("initial newApi paired control selects Java source", "use", "v.newApi()", "newApi",
    "versioned", "public int newApi()")
  local references, reference_ok = ask("references include the actual alias call", "compatibility", "textDocument/references",
    "fun Versioned.compat()", "compat", { context = { includeDeclaration = true } })
  local use_uri = vim.uri_from_bufnr(buffers.use)
  local _, use_range = probe.find(lines("use"), "v.oldApi()", "oldApi")
  local _, declaration_range = probe.find(lines("compatibility"), "fun Versioned.compat()", "compat")
  local found_use, found_declaration = false, false
  for _, location in ipairs(probe.locations(references)) do
    local key = probe.location_key(location.uri, location.range)
    found_use = found_use or key == probe.location_key(use_uri, use_range)
    found_declaration = found_declaration or key == probe.location_key(vim.uri_from_bufnr(buffers.compatibility), declaration_range)
  end
  alias_check("references include authored declaration and actual oldApi usage", reference_ok and found_use and found_declaration,
    references)

  local partial = replaced(original.use, "v.oldApi().payloadOnly()", "v.old")
  partial = replaced(partial, "v.newApi()", "v.new")
  set_use(partial)
  deliver("incomplete", { "use" })
  local completion, completion_ok = ask("incomplete v.old offers oldApi without another import", "use", "textDocument/completion",
    "= v.old", "old", { context = { triggerKind = 1 } })
  local found_alias, auto_import = false, false
  for _, item in ipairs(type(completion) == "table" and (completion.items or completion) or {}) do
    if item.label == "oldApi" then
      found_alias = true
      auto_import = auto_import or (item.additionalTextEdits ~= nil
        and (type(item.additionalTextEdits) ~= "table" or #item.additionalTextEdits > 0))
    end
  end
  alias_check("incomplete v.old offers oldApi without another import", completion_ok and found_alias and not auto_import, completion)
  completion, completion_ok = ask("incomplete newApi paired control", "use", "textDocument/completion",
    "= v.new", "new", { context = { triggerKind = 1 } })
  alias_check("incomplete newApi paired control remains useful", completion_ok and probe.completion_has(completion, "newApi"), completion)

  set_use(replaced(original.use, "import compat.compat as oldApi", "// alias import removed in this buffer"))
  deliver("removed", { "use" })
  local raw, ok = ask("removed alias has no stale definition", "use", "textDocument/definition", "v.oldApi()", "oldApi")
  alias_check("removed alias has no stale definition", ok and #probe.locations(raw) == 0, raw)
  raw, ok = ask("removed alias has no stale completion", "use", "textDocument/completion", "v.oldApi()", "oldApi",
    { context = { triggerKind = 1 } })
  alias_check("removed alias has no stale completion", ok and not probe.completion_has(raw, "oldApi"), raw)
  definition("removed alias preserves newApi paired control", "use", "v.newApi()", "newApi",
    "versioned", "public int newApi()")

  local rebound = replaced(original.use, "import compat.compat as oldApi", "import compat.reboundCompat as oldApi")
  rebound = replaced(rebound, "v.oldApi().payloadOnly()", "v.oldApi().newApi()")
  set_use(rebound)
  deliver("rebound", { "use" })
  alias_definition("rebound alias goto selects reboundCompat", "fun Versioned.reboundCompat()", "reboundCompat")
  definition("rebound alias changes its typed return to Versioned", "use", "v.oldApi().newApi()", "newApi",
    "versioned", "public int newApi()")
  restore("restored-before-rename", { "use" })
  alias_definition("restored alias goto selects compat again", "fun Versioned.compat()", "compat")
  definition("restored alias typed return selects payloadOnly again", "use", "v.oldApi().payloadOnly()", "payloadOnly",
    "payload", "fun payloadOnly()")

  local edit, rename_ok = ask("rename authored compat", "compatibility", "textDocument/rename", "fun Versioned.compat()", "compat",
    { newName = "renamedCompat" })
  local expected = vim.deepcopy(original)
  expected.compatibility = replaced(expected.compatibility, "fun Versioned.compat()", "fun Versioned.renamedCompat()")
  expected.use = replaced(expected.use, "import compat.compat as oldApi", "import compat.renamedCompat as oldApi")
  local allowed, actual, seen = {}, {}, {}
  for _, key in ipairs(order) do allowed[probe.path_key(paths[key])] = key end
  local safe = rename_ok and type(edit) == "table"
  local function collect(uri, edits)
    local key = type(uri) == "string" and allowed[probe.path_key(vim.uri_to_fname(uri))] or nil
    safe = safe and key ~= nil and type(edits) == "table"
    for _, item in ipairs(type(edits) == "table" and edits or {}) do
      if not key or not probe.range_in_source(item.range, original[key]) then
        safe = false
      else
        local identity = probe.location_key(uri, item.range)
        safe = safe and item.newText == "renamedCompat" and not seen[identity]
        seen[identity] = true
        actual[#actual + 1] = identity
      end
    end
  end
  if safe then
    for uri, edits in pairs(edit.changes or {}) do collect(uri, edits) end
    for _, change in ipairs(edit.documentChanges or {}) do
      if change.textDocument then collect(change.textDocument.uri, change.edits) else safe = false end
    end
  end
  local _, import_range = probe.find(original.use, "compat as oldApi", "compat")
  local wanted = { probe.location_key(vim.uri_from_bufnr(buffers.compatibility), declaration_range),
    probe.location_key(use_uri, import_range) }
  table.sort(wanted)
  table.sort(actual)
  alias_check("rename edits only authored name and import target", safe and vim.deep_equal(actual, wanted), edit)
  local applied, apply_error, normalized = false, "no safe workspace edit", nil
  if safe then
    normalized = vim.deepcopy(edit)
    if normalized.changes then
      local changes = {}
      for uri, edits in pairs(normalized.changes) do
        local key = allowed[probe.path_key(vim.uri_to_fname(uri))]
        changes[vim.uri_from_bufnr(buffers[key])] = edits
      end
      normalized.changes = changes
    end
    for _, change in ipairs(normalized.documentChanges or {}) do
      local key = allowed[probe.path_key(vim.uri_to_fname(change.textDocument.uri))]
      change.textDocument.uri = vim.uri_from_bufnr(buffers[key])
    end
    applied, apply_error = pcall(vim.lsp.util.apply_workspace_edit, normalized, client.offset_encoding)
  end
  alias_check("rename workspace edit applies in Neovim", applied, apply_error)
  for _, key in ipairs(order) do
    alias_check("applied rename preserves exact whole " .. key .. " buffer", vim.deep_equal(lines(key), expected[key]),
      { expected = expected[key], actual = lines(key) })
  end
  alias.applied_rename = { applied = applied, error = apply_error, edit = normalized,
    expected = expected, actual = {}, equal = {} }
  for _, key in ipairs(order) do
    alias.applied_rename.actual[key] = lines(key)
    alias.applied_rename.equal[key] = vim.deep_equal(lines(key), expected[key])
  end
  deliver("renamed", { "compatibility", "use" })
  alias_definition("renamed import keeps oldApi call bound to renamedCompat", "fun Versioned.renamedCompat()", "renamedCompat",
    expected.compatibility)
  definition("renamed alias preserves the payload return chain", "use", "v.oldApi().payloadOnly()", "payloadOnly",
    "payload", "fun payloadOnly()")
  restore("final-restored", { "compatibility", "use" })
  alias_definition("final restored alias goto selects compat", "fun Versioned.compat()", "compat")
  definition("final restored typed return selects payloadOnly", "use", "v.oldApi().payloadOnly()", "payloadOnly",
    "payload", "fun payloadOnly()")
  for _, key in ipairs(order) do
    local current = vim.fn.readfile(paths[key], "b")
    alias.disk[key].after_sha256 = vim.fn.sha256(table.concat(current, "\n"))
    alias_check("disk source remains byte unchanged: " .. key, vim.deep_equal(current, disk[key]), alias.disk[key])
  end
  client.rpc.notify = rpc_notify
  alias.completed = true
  alias.counts = { checks = #alias.checks, failures = #alias.failures, first_responses = #alias.first_responses }
  save_alias()
else
  check("extension alias uses the existing initialized client", false)
end

if client then
  local stopped = probe.shutdown(client, shutdown_state, 15000)
  check("client shuts down gracefully", stopped.clean, vim.inspect(stopped))
end
if arg[3] then
  local path = arg[3] .. "/semantic-shutdown.json"
  assert(vim.fn.filereadable(path) == 0, "fresh shutdown artifact required")
  vim.fn.writefile({ vim.json.encode(shutdown_state) }, path)
end

if #failures == 0 then
  print("\nALL PASS")
  os.exit(0)
else
  print("\nFAILED: " .. table.concat(failures, ", "))
  print("(LSP log under: " .. vim.fn.stdpath("log") .. "/lsp.log)")
  os.exit(1)
end
