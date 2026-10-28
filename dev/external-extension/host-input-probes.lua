return function(env, proof, save)
  local probe=dofile(env.repo..'/dev/nvim_lsp_probe.lua')
  local client,check=env.client,env.check
  local barrier=assert(vim.env.KTLSP_IPC_BARRIER)
  local module=env.project..'/modules/editing'
  local root=module..'/src/main/resources'
  local buffers,paths,owned,expected={}, {}, {}, {}
  local cache={}
  proof.host_inputs={captures={},requests={}}
  local function raw(path)
    local file=assert(io.open(path,'rb'));local text=file:read('*a');file:close();return text
  end
  local function write(path,text)
    vim.fn.mkdir(vim.fs.dirname(path),'p');local file=assert(io.open(path,'wb'));assert(file:write(text));file:close()
  end
  local function create(path,text)
    assert(vim.fn.filereadable(path)==0,'host input sidecar must be disposable: '..path)
    owned[#owned+1]=path;write(path,text)
  end
  local function flush() require('vim.lsp._changetracking').flush(client) end
  local function notes(prefix,evaluation)
    local key=prefix..'-'..tostring(evaluation)
    if not cache[key] then
      local path=barrier..'/'..key..'.json'
      if vim.fn.filereadable(path)==0 then return nil end
      cache[key]=vim.json.decode(raw(path))
    end
    return cache[key]
  end
  local function snapshot(data,association,name)
    for _,value in ipairs(data.snapshots or {}) do
      if value.association==association and value.relativePath==name then return value end
    end
  end
  local function lookup(data,name)
    for _,value in ipairs(data.lookups or {}) do
      if value.association=='source' and value.relativePath==name then return value.fact end
    end
  end
  local function decode(value)
    local result,offset={},0
    for _,chunk in ipairs(value.chunks or {}) do
      assert(chunk.state=='found','resource snapshot is not found')
      local body=chunk.value;assert(body.offset==offset,'resource chunk offset mismatch')
      local bytes=vim.base64.decode(body.base64);result[#result+1]=bytes;offset=offset+#bytes
    end
    local bytes=table.concat(result);local first=value.chunks[1].value
    assert(#bytes==first.size,'resource payload truncated')
    return bytes,first
  end
  local function accepted(predicate, minimum)
    local selected
    assert(vim.wait(30000,function()
      for _,line in ipairs(vim.fn.filereadable(vim.env.KTLSP_TRACE)==1 and vim.fn.readfile(vim.env.KTLSP_TRACE) or {}) do
        if line:find('extension.ipc.accepted',1,true) then
          local ok,event=pcall(vim.json.decode,line)
          if ok and event.args and event.args.provider=='fixture.ipc-shadow'
            and (not minimum or tonumber(event.args.evaluation)>tonumber(minimum)) then
            local data=notes('resources',event.args.evaluation)
            if data and predicate(data) then selected={evaluation=tostring(event.args.evaluation),data=data};return true end
          end
        end
      end
      return false
    end,25),'current resource contribution did not arrive')
    return selected
  end
  local api={}
  function api.setup()
    paths.json=root..'/ktlsp/config.json';paths.aw=root..'/ktlsp/empty.aw';paths.at=root..'/META-INF/ktlsp.at'
    paths.large=root..'/ktlsp/large.json';paths.new=root..'/ktlsp/new.json';paths.props=module..'/gradle.properties'
    create(paths.json,'{"stage":"disk","雪":"😀"}\n');create(paths.aw,'');create(paths.at,'# disk AT\n')
    create(paths.large,'"'..string.rep('a',14*1024*1024-3)..'"\n')
    create(module..'/build/resources/main/ktlsp/processed.json','{"declaredOutput":true}\n')
    for _,kind in ipairs({'json','aw','at','props'}) do
      local buf=vim.fn.bufadd(paths[kind]);vim.fn.bufload(buf);buffers[kind]=buf
      vim.bo[buf].filetype=kind=='json' and 'json' or kind=='props' and 'jproperties' or 'text'
      if kind=='json' then vim.api.nvim_buf_set_lines(buf,0,-1,false,{'{"stage":"initial","雪":"😀 unsaved"}'}) end
      if kind=='at' then vim.api.nvim_buf_set_lines(buf,0,-1,false,{'# initial unsaved AT 😀'}) end
      if kind=='props' then vim.api.nvim_buf_set_lines(buf,0,-1,false,{'ktlsp.input=unsaved-initial'}) end
      vim.bo[buf].eol=kind~='aw';vim.lsp.buf_attach_client(buf,client.id)
      expected[kind]=probe.buffer_full_text(buf)
    end
    flush()
  end
  function api.capture(label,evaluation)
    local data=assert(notes('resources',evaluation),'resource evidence missing')
    local observations={evaluation=tostring(evaluation),coverage=data.inventory.coverage.state,resources=#data.inventory.resources,snapshots={}}
    check('host inputs '..label..' full resource inventory is measured',data.inventory.coverage.state=='found')
    for _,entry in ipairs({{'json','source','ktlsp/config.json'},{'aw','source','ktlsp/empty.aw'},{'at','source','META-INF/ktlsp.at'},{'props','configuration','gradle.properties'}}) do
      local value=assert(snapshot(data,entry[2],entry[3]),'expected owned resource snapshot missing: '..entry[3])
      local bytes,first=decode(value)
      check('host inputs '..label..' current '..entry[1]..' bytes',bytes==expected[entry[1]] and first.identity.sha256==vim.fn.sha256(bytes))
      check('host inputs '..label..' exact '..entry[1]..' document version',first.documentVersion==vim.lsp.util.buf_versions[buffers[entry[1]]])
      observations.snapshots[entry[1]]={bytes=#bytes,sha256=first.identity.sha256,version=first.documentVersion,chunks=#value.chunks}
    end
    local large=assert(snapshot(data,'source','ktlsp/large.json'));local bytes,first=decode(large)
    check('host inputs '..label..' complete 14MiB stream',#bytes==14*1024*1024 and #large.chunks>33 and first.identity.sha256==vim.fn.sha256(bytes))
    observations.large={bytes=#bytes,sha256=first.identity.sha256,chunks=#large.chunks}
    local output=assert(snapshot(data,'output','ktlsp/processed.json'));local out=decode(output)
    check('host inputs '..label..' declared output bytes',out=='{"declaredOutput":true}\n')
    local classes=assert(notes('runtime',evaluation));observations.classes={}
    local expected={['java/util/List']=true,['vendor/Versioned']=true,['vendor/Added']=true,['app/Main']=true}
    assert(#classes==4, 'expected four runtime class responses')
    for _,class in ipairs(classes) do
      local name=class.internalName;local descriptor=class.descriptor
      assert(expected[name], 'unexpected or duplicate runtime class: '..tostring(name));expected[name]=nil
      observations.classes[name]={state=descriptor.state,sha256=class.actualContentSha256,bytes=class.actualByteLength,hasCode=class.hasCode}
      if name=='vendor/Added' and label=='initial' then check('host inputs initial absent dependency is unknown',descriptor.state=='unknown')
      elseif descriptor.value and descriptor.value.authority=='sdk-api-header' then
        check('host inputs '..label..' SDK header never claims runtime Code',name=='java/util/List' and descriptor.value.codeAvailable==false and class.bytecodeUnavailable==true)
      else check('host inputs '..label..' independently decoded runtime '..name,descriptor.state=='found' and descriptor.value.runtimeOnly==true
          and descriptor.value.sourceCurrent.state=='unknown' and descriptor.value.contentSha256==class.actualContentSha256 and descriptor.value.byteLength==class.actualByteLength and class.hasCode==true) end
    end
    proof.host_inputs.captures[label]=observations;save()
  end
  function api.change_wire()
    vim.api.nvim_buf_set_lines(buffers.json,0,-1,false,{'{"stage":"fresh","雪":"😀 newest"}'})
    vim.api.nvim_buf_set_lines(buffers.at,0,-1,false,{'# fresh unsaved AT 😀'})
    for kind,buf in pairs(buffers) do expected[kind]=probe.buffer_full_text(buf) end
    flush()
  end
  function api.native_lifecycle()
    create(paths.new,'{"value":"one"}\n')
    local added=accepted(function(data)local fact=lookup(data,'ktlsp/new.json');return fact and fact.state=='found' end,proof.host_inputs.captures.fresh.evaluation)
    local bytes,first=decode(assert(snapshot(added.data,'source','ktlsp/new.json')))
    check('native resource addition invalidates previous negative read',bytes=='{"value":"one"}\n')
    write(paths.new,'{"value":"two"}\n')
    local replaced=accepted(function(data)local item=snapshot(data,'source','ktlsp/new.json');return item and item.chunks[1].state=='found' and item.chunks[1].value.identity.sha256~=first.identity.sha256 end,added.evaluation)
    local replacement=decode(assert(snapshot(replaced.data,'source','ktlsp/new.json')))
    check('native resource replacement observes equal-sized new bytes',replacement=='{"value":"two"}\n')
    assert(vim.fn.delete(paths.new)==0)
    local removed=accepted(function(data)local fact=lookup(data,'ktlsp/new.json');return fact and fact.state=='definitelyAbsent' end,replaced.evaluation)
    check('native resource deletion restores current definite absence',removed.evaluation~=added.evaluation)
    proof.host_inputs.native_lifecycle={added=added.evaluation,replaced=replaced.evaluation,removed=removed.evaluation};save()
  end
  function api.raw_commands()
    local provider='fixture.inputs-command'
    client:notify('workspace/didChangeConfiguration',{settings={analysisProviders={[provider]={enabled=true,executeCommands={'fixture.echo'},
      command={assert(vim.env.KTLSP_IPC_CHILD),'--barrier',barrier,'--provider-commands','--host-inputs'},configuration={runtimeClasses={'java/util/List','vendor/Versioned','vendor/Added','app/Main'}}}}}})
    for _,kind in ipairs({'json','aw','at'}) do
      flush();local buf=buffers[kind]
      local params={provider=provider,command='fixture.echo',arguments={{lookup='fixture.Target'}},context={workspaceRoot=env.project,
        documentUri=vim.uri_from_bufnr(buf),bufferText=probe.buffer_full_text(buf),documentVersion=vim.lsp.util.buf_versions[buf]}}
      local response,err=client:request_sync('workspace/executeCommand',{command='ktlsp.providerCommand',arguments={params}},30000,buf)
      local value=response and response.result;local ok=err==nil and response and not response.err and value and value.result
        and value.result.source==params.context.bufferText and value.result.context.documentVersion==params.context.documentVersion
        and value.result.hostInputs.sourceCount==9
      check('raw '..kind..' command uses current owned bytes and nine JVM sources',ok==true,ok and nil or vim.inspect(response and response.err or err))
      env.request_outcomes[#env.request_outcomes+1]={case='raw '..kind..' command',method='workspace/executeCommand',attempts=1,outcome=ok and 'matched' or response and 'mismatch' or 'unmeasured'}
      proof.host_inputs.requests[#proof.host_inputs.requests+1]={kind=kind,params=params,result=value,error=response and response.err or err,attempts=1}
      if ok then api.capture('raw-'..kind,value.result.hostInputs.evaluation) end
    end
    client:notify('workspace/didChangeConfiguration',{settings={analysisProviders={[provider]={enabled=false}}}});save()
  end
  function api.cleanup()
    for _,buf in pairs(buffers) do if vim.api.nvim_buf_is_valid(buf) then vim.api.nvim_buf_delete(buf,{force=true}) end end
    for _,path in ipairs(owned) do if vim.fn.filereadable(path)==1 then assert(vim.fn.delete(path)==0) end end
  end
  return api
end
