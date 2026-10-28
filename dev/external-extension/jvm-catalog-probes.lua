return function(env, proof, save)
  local barrier = assert(vim.env.KTLSP_IPC_BARRIER)
  local check = env.check
  proof.jvm_catalog = {}
  local function methods(abi)
    local result = {}
    for _, method in ipairs(abi.methods or {}) do result[method.name] = method.descriptor end
    return result
  end
  local function capture(label, evaluation)
    if label == 'initial' then
      local denial = assert(proof.jvm_catalog.denial, 'missing catalog handle denial evidence')
      check('catalog positive evaluation differs from rejected evaluation', tostring(denial.evaluation) ~= tostring(evaluation))
    end
    local file = barrier .. '/catalog-' .. evaluation .. '.json'
    assert(vim.fn.filereadable(file) == 1, 'missing JVM catalog capture')
    local catalogs = vim.json.decode(table.concat(vim.fn.readfile(file), '\n'))
    local by_prefix = {}
    for _, catalog in ipairs(catalogs) do by_prefix[catalog.prefix] = catalog end
    local vendor = assert(by_prefix['vendor/'])
    check(label .. ' vendor physical catalog is complete', vendor.coverage.state == 'found'
      and vendor.coverage.value == vim.NIL)
    local versions, added, seen_entries = {}, {}, {}
    for _, row in ipairs(vendor.classes) do
      local descriptor, fact = row.descriptor, row.fact
      assert(type(descriptor.entry) == 'string' and not seen_entries[descriptor.entry], 'duplicate catalog entry identity')
      seen_entries[descriptor.entry] = true
      if descriptor.internalName == 'vendor/Versioned' or descriptor.internalName == 'vendor/Added' then
        check(label .. ' catalog class has physical ABI', fact.state == 'found'
          and fact.value.entry == descriptor.entry and fact.value.authority == 'compiled-input'
          and fact.value.sourceCurrent.state == 'unknown'
          and fact.value.abi.identity.internalName == descriptor.internalName
          and fact.value.abi.identity.originId == descriptor.originId)
        if fact.state == 'found' then
          local item = { descriptor = descriptor, methods = methods(fact.value.abi) }
          if descriptor.internalName == 'vendor/Versioned' then versions[#versions + 1] = item else added[#added + 1] = item end
        end
      end
    end
    local old, new = false, false
    for _, item in ipairs(versions) do
      if item.descriptor.originId:find('versioned/1.0/', 1, true) then old = item.methods.oldApi == '()I' and item.methods.newApi == nil and item.descriptor.association == 'compileClasspath' end
      if item.descriptor.originId:find('versioned/2.0/', 1, true) then new = item.methods.oldApi == '()I' and item.methods.newApi == '()I' and item.descriptor.association == 'compileClasspath' end
    end
    check(label .. ' duplicate physical class origins preserve distinct ABIs', #versions == 2 and old and new
      and versions[1].descriptor.originId ~= versions[2].descriptor.originId
      and versions[1].descriptor.classpathOrder ~= versions[2].descriptor.classpathOrder)
    if label == 'initial' then
      check('initial selected inputs do not contain Added', #added == 0)
    else
      check('fresh selected inputs contain Added ABI', #added == 1 and added[1].methods.addedApi == '()I')
    end
    local absent = assert(by_prefix['ktlsp_catalog_absent/'])
    check(label .. ' absent physical prefix is complete', #absent.classes == 0
      and absent.coverage.state == 'found' and absent.coverage.value == vim.NIL)
    local sdk = assert(by_prefix['java/util/List'])
    local selected = assert(proof.project_facts[label]).selected
    local expected_sdk_origin = 'jdk:' .. selected.java_home:gsub('\\', '/') .. '/lib/src.zip'
    local sdk_list = false
    for _, row in ipairs(sdk.classes) do
      if row.descriptor.internalName == 'java/util/List' and row.fact.state == 'found' then
        sdk_list = row.descriptor.association == 'sdk'
          and (row.descriptor.authority == 'sdk-api-header' or row.descriptor.authority == 'sdk-runtime-input')
          and row.descriptor.originId == expected_sdk_origin
          and row.fact.value.authority == row.descriptor.authority
          and row.fact.value.abi.identity.originId == row.descriptor.originId
          and row.fact.value.abi.identity.internalName == 'java/util/List'
          and row.fact.value.sourceCurrent.state == 'unknown'
      end
    end
    check(label .. ' selected SDK List has a proved ABI and authority', sdk_list)
    proof.jvm_catalog[label] = { evaluation = evaluation, vendorCoverage = vendor.coverage,
      absentCoverage = absent.coverage, absentProblems = absent.problems,
      selected = selected, sdk = sdk, versions = versions, added = added }
    save()
  end
  return { capture = capture }
end
