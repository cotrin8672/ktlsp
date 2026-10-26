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

fn analyze(
    input: &mut impl std::io::BufRead,
    output: &mut impl Write,
    next: &mut u64,
    params: &Value,
    barrier: &Option<PathBuf>,
    project_facts: bool,
    source_snapshots: bool,
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
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--barrier" if barrier.is_none() => {
                barrier = Some(PathBuf::from(args.next().ok_or("missing barrier")?))
            }
            "--project-facts" if !project_facts => project_facts = true,
            "--source-snapshots" if !source_snapshots => source_snapshots = true,
            _ => return Err("invalid child arguments".into()),
        }
    }
    if (project_facts || source_snapshots) && barrier.is_none() {
        return Err("project facts fixture needs its barrier".into());
    }
    let mut input = BufReader::new(std::io::stdin().lock());
    let mut output = std::io::stdout().lock();
    let mut next = 0;
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
                write_frame(
                    &mut output,
                    &json!({"jsonrpc":"2.0","id":message["id"],"result":result}),
                )?;
            }
            Some("analysis/evaluate") => {
                let result = analyze(
                    &mut input,
                    &mut output,
                    &mut next,
                    &message["params"],
                    &barrier,
                    project_facts,
                    source_snapshots,
                );
                let response = match result {
                    Ok(value) => json!({"jsonrpc":"2.0","id":message["id"],"result":value}),
                    Err(error) => {
                        json!({"jsonrpc":"2.0","id":message["id"],"result":{"sites":[],"diagnostics":[]},"fixtureError":error})
                    }
                };
                write_frame(&mut output, &response)?;
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
