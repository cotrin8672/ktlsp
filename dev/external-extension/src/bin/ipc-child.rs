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

fn analyze(
    input: &mut impl std::io::BufRead,
    output: &mut impl Write,
    next: &mut u64,
    params: &Value,
    barrier: &Option<PathBuf>,
    project_facts: bool,
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
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--barrier" if barrier.is_none() => {
                barrier = Some(PathBuf::from(args.next().ok_or("missing barrier")?))
            }
            "--project-facts" if !project_facts => project_facts = true,
            _ => return Err("invalid child arguments".into()),
        }
    }
    if project_facts && barrier.is_none() {
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
