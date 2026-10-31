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
    read_scoped_source_snapshots(input,output,next,evaluation,None)
}
fn read_scoped_source_snapshots(
    input: &mut impl std::io::BufRead, output: &mut impl Write,
    next: &mut u64, evaluation: &Value, scope:Option<&Value>,
) -> Result<(Value, Vec<Value>), String> {
    let mut after=Value::Null;
    let mut sources=Vec::new();let mut cursors=std::collections::BTreeSet::new();
    let mut ids=std::collections::BTreeSet::new();
    let coverage=loop {
        let mut query=if after.is_null() {json!({"op":"sourceInventory"})}
            else {json!({"op":"sourceInventory","after":after})};
        if let Some(scope)=scope {query["scope"]=scope.clone();}
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
    read_scoped_owned_resources(input,output,next,evaluation,None)
}
fn read_scoped_owned_resources(
    input:&mut impl std::io::BufRead,output:&mut impl std::io::Write,next:&mut u64,evaluation:&Value,scope:Option<&Value>,
)->Result<Value,String> {
    let mut descriptors=Vec::new();let mut after=Value::Null;let mut pages=0usize;
    let coverage=loop {
        let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"resourceInventory","after":after,"scope":scope}]}))?;
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
            "queries":[{"op":"resourceLookup","association":association,"relativePath":name,"scope":scope}]}))?;
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

fn read_project_inputs(
    input:&mut impl std::io::BufRead,output:&mut impl Write,next:&mut u64,evaluation:&Value,source_facts_v2:bool,
)->Result<Value,String> {
    let mut records=Vec::new();let mut after=Value::Null;let mut cursors=std::collections::BTreeSet::new();
    let coverage=loop {
        let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"projectInputs","after":after}]}))?;
        let page=&result["facts"][0];
        records.extend(page["records"].as_array().ok_or("missing project input records")?.iter().cloned());
        if records.len()>131072 {return Err("project input record limit".into());}
        after=page["next"].clone();if after.is_null() {break page["coverage"].clone();}
        if !cursors.insert(after.as_str().ok_or("project input cursor")?.to_owned()) || cursors.len()>256 {
            return Err("invalid project input continuation".into());
        }
    };
    let mut scopes=Vec::new();let mut seen=std::collections::BTreeSet::new();
    for record in records.iter().filter(|record|record["kind"]=="sourceSet") {
        let scope=record["id"].as_str().ok_or("project source-set handle")?;
        if !seen.insert(scope.to_owned()) {return Err("duplicate project scope handle".into());}
        let (inventory,snapshots)=read_scoped_source_snapshots(input,output,next,evaluation,Some(&record["id"]))?;
        let resources=read_scoped_owned_resources(input,output,next,evaluation,Some(&record["id"]))?;
        let mut normalized=Vec::new();
        if source_facts_v2 {
            for chunk in inventory["sources"].as_array().ok_or("source inventory")?.chunks(8) {
                let queries:Vec<_>=chunk.iter().map(|source|json!({"op":"sourceFacts","source":source["id"]})).collect();
                let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":queries}))?;
                let mut facts=result["facts"].as_array().ok_or("normalized source facts")?.clone();
                if facts.len()!=chunk.len() {return Err("normalized source count changed".into());}
                let mut declarations=Vec::new();
                for (position,fact) in facts.iter().enumerate() {
                    for declaration in fact["declarations"].as_array().ok_or("normalized declarations")? {
                        declarations.push((position,declaration["id"].clone()));
                    }
                }
                for fact in &mut facts {fact["declarationFacts"]=json!([]);}
                for chunk in declarations.chunks(32) {
                    let queries:Vec<_>=chunk.iter().flat_map(|(_,declaration)|[
                        json!({"op":"declarationType","declaration":declaration}),
                        json!({"op":"declarationMember","declaration":declaration}),
                    ]).collect();
                    let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":queries}))?;
                    let values=result["facts"].as_array().ok_or("snapshot declaration facts")?;
                    if values.len()!=chunk.len()*2 {return Err("snapshot declaration fact count changed".into());}
                    for ((position,declaration),pair) in chunk.iter().zip(values.chunks_exact(2)) {
                        facts[*position]["declarationFacts"].as_array_mut().ok_or("declaration fact list")?.push(
                            json!({"declaration":declaration,"type":pair[0],"member":pair[1]}));
                    }
                }
                let mut locations=Vec::new();let mut positions=Vec::new();
                for (position,fact) in facts.iter().enumerate() {
                    if let Some(declaration)=fact["declarations"].as_array().and_then(|values|values.first()) {
                        positions.push(position);locations.push(json!({"op":"sourceLocation","target":declaration["id"]}));
                    }
                }
                if !locations.is_empty() {
                    let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":locations}))?;
                    let values=result["facts"].as_array().ok_or("snapshot declaration locations")?;
                    if values.len()!=positions.len() {return Err("snapshot location count changed".into());}
                    for (position,location) in positions.into_iter().zip(values) {facts[position]["sampleLocation"]=location.clone();}
                }
                normalized.extend(facts);
            }
        }
        scopes.push(json!({"scope":scope,"name":record["name"],"inventory":inventory,"snapshots":snapshots,
            "sourceFacts":normalized,"resources":resources}));
    }
    Ok(json!({"records":records,"coverage":coverage,"scopes":scopes}))
}

fn read_binary_annotation_fixture(
    input:&mut impl std::io::BufRead,output:&mut impl Write,next:&mut u64,evaluation:&Value,
)->Result<Value,String> {
    let catalog=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
        "queries":[{"op":"jvmCatalog","prefix":"annotationfixture/CustomPoint"}]}))?;
    let page=&catalog["facts"][0];
    let classes=page["classes"].as_array().ok_or("annotation fixture catalog")?;
    if classes.len()!=1 || !page["next"].is_null() || page["coverage"]["state"]!="found"
        || classes[0]["internalName"]!="annotationfixture/CustomPoint"
        || classes[0]["association"]!="compileClasspath" {
        return Err("expected one selected compile-only annotation fixture".into());
    }
    let response=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,"queries":[
        {"op":"jvmClass","entry":classes[0]["entry"]},
        {"op":"runtimeClass","internalName":"annotationfixture/CustomPoint"},
    ]}))?;
    let facts=response["facts"].as_array().ok_or("annotation fixture facts")?;
    if facts.len()!=2 {return Err("annotation fixture fact count".into());}
    Ok(json!({"descriptor":classes[0],"class":facts[0],"runtime":facts[1]}))
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

fn record_jvm_catalog(
    input: &mut impl std::io::BufRead, output: &mut impl Write, next: &mut u64,
    evaluation: &Value, barrier: &Option<PathBuf>,
) -> Result<bool, String> {
    let directory=barrier.as_ref().ok_or("catalog requires evidence directory")?;
    std::fs::create_dir_all(directory).map_err(|error|error.to_string())?;
    let denial_path=directory.join("catalog-denial.json");
    if !denial_path.exists() {
        let response=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"jvmCatalog","prefix":"vendor/"}]}))?;
        let selected=response["facts"][0]["classes"].as_array().and_then(|classes|classes.first())
            .ok_or("catalog denial fixture entry")?;
        let entry=selected["entry"].as_str().ok_or("catalog denial fixture identity")?;
        std::fs::write(directory.join("catalog-denial-before.json"),serde_json::to_vec(&json!({"evaluation":evaluation,"descriptor":selected}))
            .map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
        let error=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
            "queries":[{"op":"classBytes","classIdentity":entry}]})).err().ok_or("catalog entry granted runtime bytes")?;
        let error:Value=serde_json::from_str(&error).map_err(|error|error.to_string())?;
        if error["code"] != -32602 || error["message"] != "foreign runtime class identity" {
            return Err("unexpected catalog/runtime handle rejection".into());
        }
        std::fs::write(&denial_path,serde_json::to_vec(&json!({"evaluation":evaluation,"error":error}))
            .map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
        return Ok(false);
    }
    let mut catalogs = Vec::new();
    for prefix in ["vendor/", "ktlsp_catalog_absent/", "java/util/List"] {
        let mut after = Value::Null;
        let mut cursors = std::collections::BTreeSet::new();
        let mut classes = Vec::new();
        let (coverage, problems) = loop {
            let mut query = json!({"op":"jvmCatalog","prefix":prefix});
            if !after.is_null() { query["after"] = after; }
            let response = request(input, output, next, "analysis/readFacts", json!({"evaluation":evaluation,"queries":[query]}))?;
            let page = &response["facts"][0];
            if page["view"] != "selectedPhysicalJvmInputs" || page["sourceCurrent"]["state"] != "unknown" {
                return Err("catalog authority changed".into());
            }
            classes.extend(page["classes"].as_array().ok_or("catalog classes")?.iter().cloned());
            after = page["next"].clone();
            if after.is_null() { break (page["coverage"].clone(), page["problems"].clone()); }
            if !cursors.insert(after.as_str().ok_or("catalog cursor")?.to_owned()) { return Err("repeated catalog cursor".into()); }
        };
        let mut decoded = Vec::new();
        for class in &classes {
            let entry = &class["entry"];
            let response = request(input, output, next, "analysis/readFacts", json!({"evaluation":evaluation,"queries":[{"op":"jvmClass","entry":entry}]}))?;
            let fact = response["facts"][0].clone();
            decoded.push(json!({"descriptor":class,"fact":fact}));
        }
        catalogs.push(json!({"prefix":prefix,"coverage":coverage,"problems":problems,"classes":decoded,"pages":cursors.len()+1}));
    }
    std::fs::write(directory.join(format!("catalog-{}.json",evaluation.as_str().ok_or("evaluation identity")?)),
        serde_json::to_vec(&catalogs).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
    Ok(true)
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

fn record_source_locations(
    input:&mut impl std::io::BufRead, output:&mut impl Write, next:&mut u64,
    evaluation:&Value, target:&Value, barrier:&Option<PathBuf>,
) -> Result<(),String> {
    let members=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
        "queries":[{"op":"declaredMembers","target":target["value"]["id"]}]}))?;
    let member=members["facts"][0]["members"].as_array().ok_or("location target members")?.iter()
        .find(|member|member["name"]=="twice" && member["signature"]["value"]["descriptor"]=="(I)I")
        .ok_or("location fixture requires exact twice(I)I")?;
    let locations=request(input,output,next,"analysis/readFacts",json!({"evaluation":evaluation,
        "queries":[{"op":"sourceLocation","target":target["value"]["id"]},
            {"op":"sourceLocation","target":member["id"]}]}))?;
    let directory=barrier.as_ref().ok_or("source locations require barrier")?;
    std::fs::create_dir_all(directory).map_err(|error|error.to_string())?;
    std::fs::write(directory.join(format!("locations-{}.json",evaluation.as_str().ok_or("evaluation identity")?)),
        serde_json::to_vec(&json!({"evaluation":evaluation,"type":target,"member":member,"facts":locations["facts"]}))
            .map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
    Ok(())
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
    jvm_catalog: bool,
    source_locations: bool,
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
    if source_locations {record_source_locations(input,output,next,&evaluation,&target,barrier)?;}
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
    if jvm_catalog && !record_jvm_catalog(input,output,next,&evaluation,barrier)? {
        return Ok(json!({"sites":[],"diagnostics":[]}));
    }
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
    let mut jvm_catalog = false;
    let mut source_locations = false;
    let mut project_inputs = false;
    let mut source_facts_v2 = false;
    let mut binary_annotations = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--barrier" if barrier.is_none() => {
                barrier = Some(PathBuf::from(args.next().ok_or("missing barrier")?))
            }
            "--project-facts" if !project_facts => project_facts = true,
            "--source-snapshots" if !source_snapshots => source_snapshots = true,
            "--provider-commands" if !provider_commands => provider_commands = true,
            "--host-inputs" if !host_inputs => host_inputs = true,
            "--jvm-catalog" if !jvm_catalog => jvm_catalog = true,
            "--source-locations" if !source_locations => source_locations = true,
            "--project-inputs" if !project_inputs => project_inputs = true,
            "--source-facts-v2" if !source_facts_v2 => source_facts_v2 = true,
            "--binary-annotations" if !binary_annotations => binary_annotations = true,
            _ => return Err("invalid child arguments".into()),
        }
    }
    if host_inputs { project_facts=true; source_snapshots=true; }
    if project_inputs {source_snapshots=true;source_facts_v2=true;source_locations=true;}
    if source_locations { project_facts=true; }
    if (project_facts || source_snapshots) && barrier.is_none() {
        return Err("project facts fixture needs its barrier".into());
    }
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    let mut next = 0;
    let mut command_count = 0u64;
    let mut source_evaluations = std::collections::BTreeMap::<String, u64>::new();
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
                if source_facts_v2 {
                    let offered=&message["params"];
                    if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="source-facts-v2"))
                        || !offered["supportedOps"]["source-facts-v2"].as_array()
                            .is_some_and(|values|values.iter().any(|value|value=="sourceFacts")) {
                        return Err("host did not offer source facts v2".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("source-facts-v2"));
                    if result.get("supportedOps").is_none() {result["supportedOps"]=json!({});}
                    result["supportedOps"]["source-facts-v2"]=json!(["sourceFacts"]);
                }
                if binary_annotations {
                    if !message["params"]["capabilities"].as_array().is_some_and(|values|
                        values.iter().any(|value|value=="binary-annotations-v1")) {
                        return Err("host did not offer binary declaration annotations".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("binary-annotations-v1"));
                }
                if project_inputs {
                    let offered=&message["params"];
                    if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="project-inputs-v1"))
                        || !offered["supportedOps"]["project-inputs-v1"].as_array().is_some_and(|values|values.iter().any(|value|value=="projectInputs")) {
                        return Err("host did not offer selected-project inputs".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("project-inputs-v1"));
                    if result.get("supportedOps").is_none() {result["supportedOps"]=json!({});}
                    result["supportedOps"]["project-inputs-v1"]=json!(["projectInputs"]);
                }
                if source_locations {
                    let offered=&message["params"];
                    if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="source-locations-v1"))
                        || !offered["supportedOps"]["source-locations-v1"].as_array()
                            .is_some_and(|values|values.iter().any(|value|value=="sourceLocation")) {
                        return Err("host did not offer source locations".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("source-locations-v1"));
                    result["supportedOps"]["source-locations-v1"]=json!(["sourceLocation"]);
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
                if host_inputs || project_inputs {
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
                if jvm_catalog {
                    let offered=&message["params"];
                    if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="jvm-catalog-v1"))
                        || !["jvmCatalog","jvmClass"].iter().all(|op|offered["supportedOps"]["jvm-catalog-v1"].as_array()
                            .is_some_and(|values|values.iter().any(|value|value==*op))) {
                        return Err("host did not offer JVM catalog operations".into());
                    }
                    result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("jvm-catalog-v1"));
                    if result.get("supportedOps").is_none() { result["supportedOps"]=json!({}); }
                    result["supportedOps"]["jvm-catalog-v1"]=json!(["jvmCatalog","jvmClass"]);
                    if !host_inputs {
                        if !offered["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="runtime-class-bytes-v1"))
                            || !["runtimeClass","classBytes"].iter().all(|op|offered["supportedOps"]["runtime-class-bytes-v1"].as_array()
                                .is_some_and(|values|values.iter().any(|value|value==*op))) {
                            return Err("host did not offer runtime class operations".into());
                        }
                        result["capabilities"].as_array_mut().ok_or("capabilities")?.push(json!("runtime-class-bytes-v1"));
                        result["supportedOps"]["runtime-class-bytes-v1"]=json!(["runtimeClass","classBytes"]);
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
                if source_facts_v2 {
                    let owner_evaluation=source_evaluations.entry(message["params"]["owner"].to_string()).or_default();
                    *owner_evaluation+=1;
                    record_source_facts_v2(&mut input,&mut output,&mut next,&message["params"],&barrier,*owner_evaluation)?;
                    if !project_facts && !source_snapshots {
                        write_frame(&mut output,&json!({"jsonrpc":"2.0","id":message["id"],"result":{"sites":[],"diagnostics":[]}}))?;
                        continue;
                    }
                }
                if project_inputs {
                    let capture=read_project_inputs(&mut input,&mut output,&mut next,&message["params"]["evaluation"],source_facts_v2)?;
                    let directory=barrier.as_ref().ok_or("project inputs need barrier")?;
                    std::fs::create_dir_all(directory)?;
                    std::fs::write(directory.join(format!("project-inputs-{}.json",message["params"]["evaluation"].as_str().ok_or("evaluation")?)),serde_json::to_vec(&capture)?)?;
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
                    jvm_catalog,
                    source_locations,
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
                if project_inputs {result["result"]["projectInputs"]=read_project_inputs(&mut input,&mut output,&mut next,&params["evaluation"],source_facts_v2)?;}
                if params["configuration"]["binaryAnnotationsProbe"]==true {
                    result["result"]["binaryAnnotations"]=read_binary_annotation_fixture(&mut input,&mut output,&mut next,&params["evaluation"])?;
                }
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

fn record_source_facts_v2(input:&mut impl std::io::BufRead,output:&mut impl Write,next:&mut u64,
    params:&Value,barrier:&Option<PathBuf>,owner_evaluation:u64)->Result<(),String> {
    let Some(legacy)=params["declarations"].as_array().ok_or("initial declarations")?.iter()
        .find(|declaration|declaration["name"]=="sourceFactsProbe") else {return Ok(());};
    if legacy.get("fullRange").is_some() || legacy.get("modifiers").is_some() {
        return Err("v2 metadata leaked into the legacy initial payload".into());
    }
    let legacy_annotation=&legacy["annotations"][0];
    if legacy_annotation.get("nameRange").is_some()
        || !legacy_annotation["arguments"].as_array().ok_or("legacy arguments")?.iter()
            .all(|argument|argument["value"]["state"]=="unknown") {
        return Err("new annotation values must remain honest Unknown in the legacy payload".into());
    }
    let result=request(input,output,next,"analysis/readFacts",json!({"evaluation":params["evaluation"],
        "queries":[{"op":"sourceFacts"}]}))?;
    let rich=result["facts"][0]["declarations"].as_array().ok_or("source facts declarations")?.iter()
        .find(|declaration|declaration["name"]=="sourceFactsProbe").ok_or("v2 probe declaration")?;
    if rich["nameRange"]!=legacy["range"] || rich["modifiers"]["static"]!=json!({"state":"found","value":true})
        || rich["modifiers"]["final"]!=json!({"state":"found","value":true}) {
        return Err("source facts declaration ranges/modifiers were not normalized".into());
    }
    let arguments=&rich["annotations"][0]["arguments"];
    let string=&arguments[0]["value"]["value"];
    let nested=&arguments[1]["value"]["value"];
    if string["kind"]!="mappedString" || string["value"]!="tick()V" || string.get("contentRange").is_some()
        || nested["kind"]!="annotation" || nested["annotation"]["arguments"][2]["value"]["value"]
            !=json!({"kind":"integer","type":"int","value":1})
        || arguments[2]["value"]["value"]!=json!({"kind":"integer","type":"long","value":2}) {
        return Err("source facts nested annotation/string/integer facts were not normalized".into());
    }
    let types=request(input,output,next,"analysis/readFacts",json!({"evaluation":params["evaluation"],"queries":[
        {"op":"resolveTypeRef","reference":rich["annotations"][0]["typeRef"]},
        {"op":"resolveTypeRef","reference":nested["annotation"]["typeRef"]},
    ]}))?;
    let directory=barrier.as_ref().ok_or("source facts fixture barrier")?;
    std::fs::create_dir_all(directory).map_err(|error|error.to_string())?;
    std::fs::write(directory.join(format!("source-facts-{}.json",params["evaluation"].as_str().ok_or("evaluation")?)),
        serde_json::to_vec(&json!({"evaluation":params["evaluation"],"ownerEvaluation":owner_evaluation,"legacy":legacy,"rich":rich,"types":types["facts"]})).map_err(|error|error.to_string())?)
        .map_err(|error|error.to_string())?;
    Ok(())
}
