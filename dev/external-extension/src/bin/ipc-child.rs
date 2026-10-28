//! Disposable process oracle. Production Minecraft policy belongs in mc-dev.
use std::io::{BufReader, Write};
use std::path::PathBuf;

use ktlsp::extension_ipc::{read_frame, write_frame, PROTOCOL_MAJOR};
use serde_json::{json, Value};

fn request(
    input: &mut impl std::io::BufRead,
    output: &mut impl Write,
    id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    *id += 1;
    write_frame(
        output,
        &json!({"jsonrpc":"2.0","id":*id,"method":method,"params":params}),
    )
    .map_err(|e| e.to_string())?;
    loop {
        let value = read_frame(input)
            .map_err(|e| e.to_string())?
            .ok_or("host closed")?;
        if value["id"] == *id {
            if value.get("error").is_some() {
                return Err(value["error"].to_string());
            }
            return Ok(value["result"].clone());
        }
    }
}

fn read_source_snapshots(
    input: &mut impl std::io::BufRead, output: &mut impl Write,
    next: &mut u64, evaluation: &Value,
) -> Result<(Value, Vec<Value>), String> {
    let mut after=Value::Null;
    let mut sources=Vec::new();let mut cursors=std::collections::BTreeSet::new();
    let mut ids=std::collections::BTreeSet::new();
    let coverage=loop {
        let query=if after.is_null() {json!({"op":"sourceInventory"})}
            else {json!({"op":"sourceInventory","after":after})};
        let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":[query]}))?;
        let page=&result["facts"][0];
        for source in page["sources"].as_array().ok_or("missing source inventory")? {
            if !ids.insert(source["id"].as_str().ok_or("source identity")?.to_string()) {
                return Err("duplicate source inventory identity".into());
            }
            sources.push(source.clone());
        }
        after=page["next"].clone();
        if after.is_null() {break page["coverage"].clone();}
        if !cursors.insert(after.as_str().ok_or("inventory continuation")?.to_string()) {
            return Err("repeated source inventory cursor".into());
        }
    };
    let mut snapshots=Vec::new();let mut offset=0;let mut limit=256usize;
    while offset<sources.len() {
        let count=limit.min(sources.len()-offset);
        let queries:Vec<_>=sources[offset..offset+count].iter()
            .map(|source|json!({"op":"sourceSnapshot","source":source["id"]})).collect();
        let result=match request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":queries})) {
            Ok(result)=>result,
            Err(error) if error.contains("split queries") && count>1 => {limit=count.div_ceil(2);continue;}
            Err(error)=>return Err(error),
        };
        let facts=result["facts"].as_array().ok_or("missing source snapshots")?;
        if facts.len()!=count {return Err("source snapshot count changed".into());}
        for (source,snapshot) in sources[offset..offset+count].iter().zip(facts) {
            if snapshot["state"]=="found" && (snapshot["value"]["id"]!=source["id"]
                || snapshot["value"]["identity"]!=source["identity"]) {
                return Err("source snapshot identity changed".into());
            }
            snapshots.push(snapshot.clone());
        }
        offset+=count;
        if serde_json::to_vec(&result).map_err(|error|error.to_string())?.len()<256*1024 {
            limit=(limit*2).min(256);
        }
    }
    Ok((json!({"sources":sources,"coverage":coverage,"next":null}),snapshots))
}

/// Test consumer: read only host-issued resource identities. The barrier directory
/// records RPC evidence; resource display paths are never opened by the child.
fn read_owned_resources(
    input:&mut impl std::io::BufRead,output:&mut impl std::io::Write,next:&mut u64,evaluation:&Value,
)->Result<Value,String> {
    let mut descriptors=Vec::new();let mut after=Value::Null;let mut pages=0usize;
    let coverage=loop {
        let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"resourceInventory","after":after}]}))?;
        let page=&result["facts"][0];pages+=1;
        descriptors.extend(page["resources"].as_array().ok_or("resource inventory descriptors")?.iter().cloned());
        after=page["next"].clone();
        if after.is_null() {break page["coverage"].clone();}
        if pages>=64 {return Err("resource inventory exceeded metadata batch contract".into());}
    };
    let mut lookups=Vec::new();let mut snapshots=Vec::new();
    for (association,name) in [
        ("source","ktlsp/config.json"),("source","ktlsp/empty.aw"),("source","META-INF/ktlsp.at"),
        ("source","ktlsp/large.json"),("source","ktlsp/new.json"),("output","ktlsp/processed.json"),
        ("configuration","gradle.properties"),("compileClasspath","META-INF/MANIFEST.MF"),
        ("runtimeClasspath","META-INF/MANIFEST.MF"),
    ] {
        let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"resourceLookup","association":association,"relativePath":name}]}))?;
        let lookup=result["facts"][0].clone();
        // The host preserves all classpath matches. One actual payload per role is
        // enough for this fixture; no match is never converted to a successful empty.
        if lookup["state"]=="found" {
            for descriptor in lookup["value"].as_array().ok_or("resource lookup descriptors")?.iter().take(1) {
                let id=descriptor["id"].clone();let mut after=Value::Null;let mut chunks=Vec::new();
                loop {
                    let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
                        "queries":[{"op":"resourceSnapshot","resource":id,"after":after}]}))?;
                    let chunk=result["facts"][0].clone();
                    let more=chunk["value"]["next"].clone();
                    chunks.push(chunk);
                    if more.is_null() {break;}
                    if chunks.len()>=256 {return Err("resource stream exceeded continuation contract".into());}
                    after=more;
                }
                snapshots.push(json!({"association":association,"relativePath":name,"descriptor":descriptor,"chunks":chunks}));
            }
        }
        lookups.push(json!({"association":association,"relativePath":name,"fact":lookup}));
    }
    Ok(json!({"inventory":{"resources":descriptors,"pages":pages,"coverage":coverage},"lookups":lookups,"snapshots":snapshots}))
}

fn read_runtime_class_bytes(
    input: &mut impl std::io::BufRead, output: &mut impl Write,
    next: &mut u64, params: &Value,
) -> Result<Vec<Value>, String> {
    use base64::Engine as _;
    let evaluation = &params["evaluation"];
    let names = params["configuration"]["runtimeClasses"].as_array()
        .ok_or("runtime bytes fixture requires exact internal names")?;
    if names.len() > 8 { return Err("runtime bytes fixture class limit".into()); }
    let mut observations = Vec::new();
    for name in names {
        let result = request(input, output, next, "analysis/readFacts", json!({
            "evaluation":evaluation,"queries":[{"op":"runtimeClass","internalName":name}]
        }))?;
        let descriptor = result["facts"][0].clone();
        if descriptor["state"] != "found" {
            observations.push(json!({"internalName":name,"descriptor":descriptor}));
            continue;
        }
        let value = &descriptor["value"];
        if value["authority"] == "sdk-api-header" {
            if value["codeAvailable"] != false || value["runtimeAbiCoverage"]["state"] != "unknown"
                || value["classHeader"]["state"] != "found" || value["hierarchy"]["state"] != "found" {
                return Err("SDK header authority/coverage was mixed with runtime Code".into());
            }
            let response = request(input,output,next,"analysis/readFacts",json!({
                "evaluation":evaluation,"queries":[{"op":"classBytes","classIdentity":value["id"]}]
            }))?;
            let unavailable = &response["facts"][0];
            if unavailable["state"] != "unknown" || !unavailable["reasons"].as_array().is_some_and(|reasons|
                reasons.iter().any(|reason|reason["code"]=="BYTECODE_UNAVAILABLE")) {
                return Err("API header granted executable class bytes".into());
            }
            observations.push(json!({"internalName":name,"descriptor":descriptor,"bytecodeUnavailable":true}));
            continue;
        }
        if value["runtimeOnly"] != true || value["sourceCurrent"]["state"] != "unknown"
            || value["internalName"] != *name { return Err("runtime/source authority was mixed".into()); }
        let identity = value["id"].clone();
        let length = value["byteLength"].as_u64().ok_or("class byte length")? as usize;
        if length > ktcore::classfile::MAX_CLASS_FILE_BYTES { return Err("class byte bound".into()); }
        let mut bytes = Vec::with_capacity(length);
        let mut after = Value::Null;
        let mut seen = std::collections::BTreeSet::new();
        loop {
            let mut query = json!({"op":"classBytes","classIdentity":identity});
            if !after.is_null() { query["after"] = after; }
            let page = request(input, output, next, "analysis/readFacts", json!({
                "evaluation":evaluation,"queries":[query]
            }))?;
            let chunk = &page["facts"][0];
            if chunk["state"] != "found" { return Err("selected runtime bytes unavailable".into()); }
            let chunk = &chunk["value"];
            if chunk["classIdentity"] != identity || chunk["offset"].as_u64() != Some(bytes.len() as u64)
                || chunk["byteLength"].as_u64() != Some(length as u64) { return Err("class chunk identity/offset changed".into()); }
            let decoded = base64::engine::general_purpose::STANDARD.decode(chunk["bytesBase64"].as_str().ok_or("class base64")?)
                .map_err(|error|error.to_string())?;
            if decoded.is_empty() || decoded.len() > length - bytes.len() { return Err("class chunk byte bound".into()); }
            bytes.extend(decoded);
            after = chunk["next"].clone();
            if after.is_null() { break; }
            if !seen.insert(after.as_str().ok_or("opaque class continuation")?.to_owned()) {
                return Err("repeated class continuation".into());
            }
        }
        if bytes.len() != length { return Err("class byte stream truncated".into()); }
        let class = ktcore::classfile::decode("independent-child-decode", &bytes).map_err(|error|error.to_string())?;
        let ktcore::jvm::InputIdentity::Binary { content_sha256 } = class.identity.input else {
            return Err("decoded runtime class has no byte digest".into());
        };
        let digest = content_sha256.iter().map(|byte|format!("{byte:02x}")).collect::<String>();
        if value["contentSha256"] != digest || class.identity.internal_name.as_str() != name.as_str().ok_or("internal name")? {
            return Err("host runtime identity differs from independently decoded bytes".into());
        }
        observations.push(json!({"internalName":name,"descriptor":descriptor,"actualByteLength":bytes.len(),
            "actualContentSha256":digest,"methods":class.methods.len(),"fields":class.fields.len(),
            "hasCode":class.methods.iter().any(|method|method.attribute_names.iter().any(|name|name.as_ref()=="Code")),
            "containsLocalVariableTableName":bytes.windows(b"LocalVariableTable".len()).any(|name|name==b"LocalVariableTable"),
            "chunks":seen.len()+1}));
    }
    Ok(observations)
}

fn record_host_inputs(
    input:&mut impl std::io::BufRead, output:&mut impl Write, next:&mut u64,
    params:&Value, barrier:&Option<PathBuf>, include_sources:bool,
) -> Result<Value,String> {
    let evaluation=&params["evaluation"];
    let directory=barrier.as_ref().ok_or("host inputs require configured evidence directory")?;
    std::fs::create_dir_all(directory).map_err(|error|error.to_string())?;
    let sources=if include_sources {
        let (inventory,snapshots)=read_source_snapshots(input,output,next,evaluation)?;
        let sources=json!({"evaluation":evaluation,"owner":params["owner"],"inventory":inventory,"snapshots":snapshots});
        std::fs::write(directory.join(format!("sources-{}.json",evaluation.as_str().ok_or("evaluation identity")?)),
            serde_json::to_vec(&sources).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
        Some(sources)
    } else {None};
    let resources=read_owned_resources(input,output,next,evaluation)?;
    let runtime=read_runtime_class_bytes(input,output,next,params)?;
    for (prefix,value) in [("resources",resources.clone()),("runtime",json!(runtime))] {
        std::fs::write(directory.join(format!("{prefix}-{}.json",evaluation.as_str().ok_or("evaluation identity")?)),
            serde_json::to_vec(&value).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
    }
    // Full chunk payloads stay in configured evidence files; command envelopes remain bounded.
    Ok(json!({"evaluation":evaluation,"sourceCount":sources.as_ref().and_then(|source|source["inventory"]["sources"].as_array()).map(Vec::len),
        "resourceCount":resources["inventory"]["resources"].as_array().map(Vec::len),"runtimeClassCount":runtime.len()}))
}

fn analyze(
    input: &mut impl std::io::BufRead,
    output: &mut impl Write,
    next: &mut u64,
    params: &Value,
    barrier: &Option<PathBuf>,
    project_facts: bool,
    source_snapshots: bool,
    host_inputs: bool,
) -> Result<Value, String> {
    let evaluation = params["evaluation"].clone();
    let declarations = params["declarations"]
        .as_array()
        .ok_or("missing declarations")?;
    let Some(shadow) = declarations.iter().find(|declaration| {
        declaration["name"]
            .as_str()
            .is_some_and(|name| name.contains("shadow$"))
    }) else {
        return Ok(json!({"sites":[],"diagnostics":[]}));
    };
    let target = params["configuration"]["target"]
        .as_str()
        .unwrap_or("fixture.Target");
    let read = request(
        input,
        output,
        next,
        "analysis/readFacts",
        json!({"evaluation":evaluation,"queries":[{"op":"resolveType","name":target},{"op":"declarationMember","declaration":shadow["id"]}]}),
    )?;
    let target = read["facts"][0].clone();
    if target["state"] != "found" || read["facts"][1]["state"] != "found" {
        return Ok(json!({"sites":[],"diagnostics":[]}));
    }
    let name = shadow["name"]
        .as_str()
        .ok_or("shadow name")?
        .trim_matches('`');
    let member = &read["facts"][1]["value"];
    if project_facts {
        let facts = request(
            input,
            output,
            next,
            "analysis/readFacts",
            json!({"evaluation":evaluation,"queries":[
                {"op":"projectContext"},
                {"op":"typeJvm","target":target["value"]["id"]}
            ]}),
        )?;
        let directory = barrier
            .as_ref()
            .ok_or("project facts fixture needs its barrier")?;
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        std::fs::write(
            directory.join(format!(
                "facts-{}.json",
                evaluation.as_str().ok_or("evaluation token")?
            )),
            serde_json::to_vec(
                &json!({"evaluation":evaluation,"owner":params["owner"],"facts":facts["facts"]}),
            )
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    if source_snapshots {
        let (inventory,snapshots)=read_source_snapshots(input,output,next,&evaluation)?;
        let directory = barrier.as_ref().ok_or("source snapshots need barrier")?;
        std::fs::create_dir_all(directory).map_err(|error|error.to_string())?;
        std::fs::write(directory.join(format!("sources-{}.json",evaluation.as_str().ok_or("evaluation token")?)),
            serde_json::to_vec(&json!({"evaluation":evaluation,"owner":params["owner"],
                "inventory":inventory,"snapshots":snapshots})).map_err(|error|error.to_string())?)
            .map_err(|error|error.to_string())?;
    }
    if host_inputs { record_host_inputs(input,output,next,params,barrier,false)?; }
    if let Some(barrier) = barrier {
        if !barrier.join("released-once").exists() {
            std::fs::create_dir_all(barrier).map_err(|e| e.to_string())?;
            std::fs::write(
                barrier.join("paused"),
                evaluation.as_str().ok_or("evaluation token")?,
            )
            .map_err(|e| e.to_string())?;
            let cancellation = read_frame(input)
                .map_err(|e| e.to_string())?
                .ok_or("host closed at barrier")?;
            if cancellation["method"] != "$/cancelRequest"
                || cancellation["params"]["id"] != evaluation
            {
                return Err("expected cancellation of the paused evaluation".into());
            }
            std::fs::write(
                barrier.join("cancelled"),
                evaluation.as_str().ok_or("evaluation token")?,
            )
            .map_err(|e| e.to_string())?;
            while !barrier.join("release").exists() {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::fs::write(barrier.join("released-once"), b"1").map_err(|e| e.to_string())?;
        }
    }
    let resolution = request(
        input,
        output,
        next,
        "analysis/require",
        json!({"evaluation":evaluation,"requirements":[{"target":target,"authored":shadow["id"],"names":{"kind":"exact","values":[name.strip_prefix("shadow$").ok_or("prefix")?]},"kind":member["kind"],"signature":member["signature"],"dispatch":member["dispatch"],"nameLink":{"kind":"prefixed","prefix":"shadow$"}}]}),
    )?;
    let range = shadow["range"].clone();
    let start = range["start"].as_u64().ok_or("range")?;
    let end = range["end"].as_u64().ok_or("range")?;
    let escaped = shadow["name"].as_str().unwrap().starts_with('`');
    let requirement = &resolution["requirements"][0];
    let bound = requirement["binding"]["state"] == "found";
    Ok(
        json!({"sites":[{"requirement":requirement["id"],"fullRange":range,"editableName":{"start":start+7+u64::from(escaped),"end":end-u64::from(escaped)},"renameSubject":"target"}],"diagnostics":[{"range":range,"severity":"hint","message":if bound {"ipc-fixture:bound"}else{"ipc-fixture:unknown"}}]}),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut barrier = None;
    let mut project_facts = false;
    let mut source_snapshots = false;
    let mut provider_commands = false;
    let mut host_inputs = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--barrier" if barrier.is_none() => {
                barrier = Some(PathBuf::from(args.next().ok_or("missing barrier")?))
            }
            "--project-facts" if !project_facts => project_facts = true,
            "--source-snapshots" if !source_snapshots => source_snapshots = true,
            "--provider-commands" if !provider_commands => provider_commands = true,
            "--host-inputs" if !host_inputs => host_inputs = true,
            _ => return Err("invalid child arguments".into()),
        }
    }
    if host_inputs { project_facts=true; source_snapshots=true; }
    if (project_facts || source_snapshots) && barrier.is_none() {
        return Err("project facts fixture needs its barrier".into());
    }
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    let mut next = 0;
    let mut command_count = 0u64;
    if let Some(directory) = barrier.clone() {
        std::thread::spawn(move || loop {
            if directory.join("crash").exists() {
                std::process::exit(23);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        });
    }
    while let Some(message) = read_frame(&mut input)? {
        match message["method"].as_str() {
            Some("initialize") => {
                let mut result = json!({"protocol":{"major":PROTOCOL_MAJOR,"minor":0},"capabilities":["semantic-v1"]});
                if project_facts {
                    let offered = &message["params"];
                    let supports = offered["capabilities"].as_array().is_some_and(|values| {
                        values.iter().any(|value| value == "project-class-facts-v1")
                    }) && ["projectContext", "typeJvm"].iter().all(|operation| {
                        offered["supportedOps"]["project-class-facts-v1"]
                            .as_array()
                            .is_some_and(|values| values.iter().any(|value| value == *operation))
                    });
                    if !supports {
                        return Err("host did not offer the two project facts operations".into());
                    }
                    result["capabilities"] = json!(["semantic-v1", "project-class-facts-v1"]);
                    result["supportedOps"] =
                        json!({"project-class-facts-v1":["projectContext","typeJvm"]});
                }
                if source_snapshots {
                    let offered = &message["params"];
                    if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="source-snapshots-v1"))
                        || !["sourceInventory","sourceSnapshot"].iter().all(|op|offered["supportedOps"]["source-snapshots-v1"]
                            .as_array().is_some_and(|values|values.iter().any(|value|value==*op))) {
                        return Err("host did not offer source snapshot operations".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("source-snapshots-v1"));
                    if result.get("supportedOps").is_none() { result["supportedOps"]=json!({}); }
                    result["supportedOps"]["source-snapshots-v1"]=json!(["sourceInventory","sourceSnapshot"]);
                }
                if host_inputs {
                    let offered=&message["params"];
                    for (capability,operations) in [("owned-resources-v1",&["resourceInventory","resourceLookup","resourceSnapshot"][..]),
                        ("runtime-class-bytes-v1",&["runtimeClass","classBytes"][..])] {
                        if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value==capability))
                            || !operations.iter().all(|operation|offered["supportedOps"][capability].as_array()
                                .is_some_and(|values|values.iter().any(|value|value==*operation))) {
                            return Err("host did not offer owned resource/runtime class operations".into());
                        }
                        result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!(capability));
                        if result.get("supportedOps").is_none() {result["supportedOps"]=json!({});}
                        result["supportedOps"][capability]=json!(operations);
                    }
                }
                if provider_commands {
                    if !message["params"]["capabilities"].as_array().is_some_and(|caps|caps.iter().any(|cap|cap=="provider-commands-v1")) {
                        return Err("host did not offer explicit provider commands".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("provider-commands-v1"));
                    // The fourth entry deliberately proves that host configuration still bounds acknowledgement.
                    result["commands"]=json!(["fixture.echo","fixture.pause","fixture.refresh","fixture.unapproved"]);
                }
                write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":message["id"],"result":result}),
                )?;
            }
            Some("analysis/evaluate") => {
                if provider_commands {
                    write_frame(&mut output,&json!({"jsonrpc":"2.0","id":message["id"],"result":{"sites":[],"diagnostics":[]}}))?;
                    continue;
                }
                let result = analyze(
                    &mut input,
                    &mut output,
                    &mut next,
                    &message["params"],
                    &barrier,
                    project_facts,
                    source_snapshots,
                    host_inputs,
                );
                let response = match result {
                    Ok(value) => json!({"jsonrpc":"2.0","id":message["id"],"result":value}),
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"sites":[],"diagnostics":[]},"fixtureError":error})
                    }
                };
                write_frame(&mut output, &response)?;
            }
            Some("analysis/executeCommand") if provider_commands => {
                command_count+=1;
                let params=&message["params"];
                let name=params["command"].as_str().ok_or("missing routed command")?;
                if !matches!(name,"fixture.echo"|"fixture.pause"|"fixture.refresh") { return Err("unapproved command dispatched".into()); }
                let lookup=params["arguments"][0]["lookup"].as_str().unwrap_or("fixture.Target");
                let facts=request(&mut input,&mut output,&mut next,"analysis/readFacts",json!({"evaluation":params["evaluation"],
                    "queries":[{"op":"resolveType","name":lookup}]}))?;
                if name=="fixture.pause" {
                    let case=params["arguments"][0]["pauseCase"].as_str().ok_or("missing command pause case")?;
                    if !matches!(case,"source"|"cancel"|"disable") { return Err("invalid fixture pause case".into()); }
                    let directory=barrier.as_ref().ok_or("command fixture needs barrier")?;
                    std::fs::create_dir_all(directory)?;
                    std::fs::write(directory.join(format!("command-{case}-paused")),message["id"].as_str().ok_or("command identity")?)?;
                    let cancellation=read_frame(&mut input)?.ok_or("host closed at command barrier")?;
                    if cancellation["method"]!="$/cancelRequest" || cancellation["params"]["id"]!=message["id"] {return Err("expected routed command cancellation".into());}
                    std::fs::write(directory.join(format!("command-{case}-cancelled")),b"1")?;
                    while !directory.join(format!("command-{case}-release")).exists() {std::thread::sleep(std::time::Duration::from_millis(10));}
                }
                let mut result=json!({
                    "protocolVersion":1,"serverVersion":"fixture-command-v1","capabilities":[name],
                    "result":{"arguments":params["arguments"],"owner":params["owner"],"source":params["source"],
                        "context":params["context"],"configuration":params["configuration"],"facts":facts["facts"],"commandCount":command_count}});
                if host_inputs { result["result"]["hostInputs"]=record_host_inputs(&mut input,&mut output,&mut next,params,&barrier,true)?; }
                if params["arguments"][0]["requestEdit"]==true {
                    if params["context"]["documentVersion"].as_i64().is_none() {return Err("fixture action needs a host document version".into());}
                    result["result"]["edit"]=json!({"documentChanges":[{"textDocument":{"uri":params["context"]["documentUri"],"version":params["context"]["documentVersion"]},
                        "edits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"// provider action\n"}]}]});
                }
                write_frame(&mut output,&json!({"jsonrpc":"2.0","id":message["id"],"result":result}))?;
            }
            Some("exit") => break,
            Some("shutdown") => write_frame(
                &mut output,
                &json!({"jsonrpc":"2.0","id":message["id"],"result":null}),
            )?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod source_inventory_client_tests {
    use super::*;

    #[test]
    fn follows_large_inventory_and_recovers_only_the_snapshot_byte_budget() {
        let sources:Vec<_>=(0..1257).map(|n|json!({"id":format!("source-{n}"),
            "displayPath":format!("/sources/Source{n}.java"),
            "identity":{"kind":"source","revision":n.to_string()}})).collect();
        let body="x".repeat(3*1024);
        let snapshot=|source:&Value| {let mut value=source.clone();value["text"]=json!(body);value["sha256"]=json!("checked-by-host");json!({"state":"found","value":value})};
        let too_large:Vec<_>=sources[..256].iter().map(snapshot).collect();
        assert!(serde_json::to_vec(&json!({"facts":too_large})).unwrap().len()>512*1024);
        let mut input=Vec::new();
        for (id,result) in [(1,json!({"facts":[{"sources":sources[..700],"coverage":{"state":"unknown"},"next":"opaque-next"}]})),
            (2,json!({"facts":[{"sources":sources[700..],"coverage":{"state":"found","value":null},"next":null}]}))] {
            write_frame(&mut input,&json!({"jsonrpc":"2.0","id":id,"result":result})).unwrap();
        }
        write_frame(&mut input,&json!({"jsonrpc":"2.0","id":3,"error":{"code":-32602,
            "message":"extension response batch too large; split queries"}})).unwrap();
        for (n,chunk) in sources.chunks(128).enumerate() {
            let facts:Vec<_>=chunk.iter().map(snapshot).collect();
            let result=json!({"facts":facts});assert!(serde_json::to_vec(&result).unwrap().len()<512*1024);
            write_frame(&mut input,&json!({"jsonrpc":"2.0","id":n+4,"result":result})).unwrap();
        }
        let mut input=std::io::Cursor::new(input);let mut output=Vec::new();let mut next=0;
        let (inventory,snapshots)=read_source_snapshots(&mut input,&mut output,&mut next,&json!("evaluation")).unwrap();
        assert_eq!(inventory["sources"],json!(sources));assert_eq!(inventory["coverage"]["state"],"found");
        assert!(inventory["next"].is_null());assert_eq!(snapshots.len(),1257);
        assert!(snapshots.iter().all(|fact|fact["value"]["text"]==body));
        let mut output=std::io::Cursor::new(output);let mut requests=Vec::new();
        while let Some(request)=read_frame(&mut output).unwrap() {requests.push(request);}
        assert_eq!(requests[1]["params"]["queries"][0]["after"],"opaque-next");
        assert!(requests.iter().all(|request|request["params"]["queries"].as_array().unwrap().len()<=256));
        assert!(requests.len()+3<64,"leave room for the existing semantic queries");
        assert_eq!(input.position() as usize,input.get_ref().len());
    }
}
