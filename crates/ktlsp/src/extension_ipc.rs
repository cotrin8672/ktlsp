//! Versioned semantic extension transport. Framework rules belong in the child.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use ktcore::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use ktcore::extensions::source::{AnnotationArgument, AnnotationValue, SourceAnnotation};
use ktcore::extensions::*;
use ktcore::knowledge::Knowledge;
use serde::Deserialize;
use serde_json::{json, Value};

pub const PROTOCOL_MAJOR: u64 = 1;
const MAX_FRAME: usize = 1024 * 1024;
pub(crate) const READ_FACTS_RESPONSE_BUDGET_EXCEEDED: &str =
    "extension response batch too large; split queries";
const MAX_BATCH: usize = 256;
const MAX_HANDLES: usize = 16_384;
const MAX_STREAM_CONTINUATIONS: usize = 256;
const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const EVALUATION_LIMIT: Duration = Duration::from_secs(30);
static NEXT_EVALUATION: AtomicU64 = AtomicU64::new(1);
const PROJECT_FACTS: &str = "project-class-facts-v1";
const PROJECT_CONTEXT: u16 = 1;
const TYPE_JVM: u16 = 2;
const SOURCE_SNAPSHOTS: &str = "source-snapshots-v1";
const SOURCE_INVENTORY: u16 = 4;
const SOURCE_SNAPSHOT: u16 = 8;
const RESOURCES: &str = "owned-resources-v1";
const RESOURCE_INVENTORY:u16=16;
const RESOURCE_LOOKUP:u16=32;
const RESOURCE_SNAPSHOT:u16=64;
const RUNTIME_CLASSES: &str = "runtime-class-bytes-v1";
const RUNTIME_CLASS: u16 = 128;
const CLASS_BYTES: u16 = 256;

fn project_operations(result: &Value) -> u16 {
    let mut selected = 0;
    for (capability, operations) in [
        (PROJECT_FACTS, &[("projectContext",PROJECT_CONTEXT),("typeJvm",TYPE_JVM)][..]),
        (SOURCE_SNAPSHOTS, &[("sourceInventory",SOURCE_INVENTORY),("sourceSnapshot",SOURCE_SNAPSHOT)][..]),
        (RESOURCES, &[("resourceInventory",RESOURCE_INVENTORY),("resourceLookup",RESOURCE_LOOKUP),("resourceSnapshot",RESOURCE_SNAPSHOT)][..]),
        (RUNTIME_CLASSES, &[("runtimeClass",RUNTIME_CLASS),("classBytes",CLASS_BYTES)][..]),
    ] {
        if !result["capabilities"].as_array().is_some_and(|values|
            values.iter().any(|value| value == capability)) { continue; }
        if let Some(offered) = result["supportedOps"][capability].as_array() {
            for &(operation, bit) in operations {
                if offered.iter().any(|value| value == operation) { selected |= bit; }
            }
        }
    }
    selected
}

fn initialize_message(provider:&str,commands:&std::collections::BTreeSet<String>)->Value {
    json!({"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{
        "protocol":{"major":PROTOCOL_MAJOR,"minor":0},"provider":provider,
        "capabilities":["semantic-v1",PROJECT_FACTS,SOURCE_SNAPSHOTS,RESOURCES,RUNTIME_CLASSES,"provider-commands-v1"],
        "commands":commands,"supportedOps":{
            PROJECT_FACTS:["projectContext","typeJvm"],SOURCE_SNAPSHOTS:["sourceInventory","sourceSnapshot"],
            RESOURCES:["resourceInventory","resourceLookup","resourceSnapshot"],RUNTIME_CLASSES:["runtimeClass","classBytes"]
        }}})
}

/// Shared with the disposable child fixture; no core Rust identities cross this boundary.
pub fn read_frame(reader: &mut impl BufRead) -> std::io::Result<Option<Value>> {
    let invalid =
        || std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid extension frame");
    let mut length = None;
    let mut header_bytes = 0;
    loop {
        let mut line = String::new();
        let count = (&mut *reader).take(4097).read_line(&mut line)?;
        if count == 0 && header_bytes == 0 {
            return Ok(None);
        }
        header_bytes += count;
        if count == 0 || header_bytes > 4096 || !line.ends_with('\n') {
            return Err(invalid());
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        let (name, value) = line.trim_end().split_once(':').ok_or_else(invalid)?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(invalid());
            }
            length = Some(value.trim().parse::<usize>().map_err(|_| invalid())?);
        }
    }
    let length = length
        .filter(|length| *length <= MAX_FRAME)
        .ok_or_else(invalid)?;
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid())
}

pub fn write_frame(writer: &mut impl Write, message: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "extension frame too large",
        ));
    }
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

fn reason(reason: &IncompleteReason) -> Value {
    match reason {
        IncompleteReason::UnsupportedType(detail) => {
            json!({"code":"unsupportedType","detail":detail})
        }
        other => json!({"code":format!("{other:?}")}),
    }
}

fn decode_reason(value: &Value) -> Result<IncompleteReason, String> {
    Ok(
        match value["code"].as_str().ok_or("missing knowledge reason")? {
            "MissingSource" => IncompleteReason::MissingSource,
            "MissingFacts" => IncompleteReason::MissingFacts,
            "IncompleteScope" => IncompleteReason::IncompleteScope,
            "IncompleteTarget" => IncompleteReason::IncompleteTarget,
            "IncompleteSelector" => IncompleteReason::IncompleteSelector,
            "AmbiguousType" => IncompleteReason::AmbiguousType,
            "AmbiguousMember" => IncompleteReason::AmbiguousMember,
            "UnsupportedSource" => IncompleteReason::UnsupportedSource,
            "unsupportedType" => IncompleteReason::UnsupportedType(
                value["detail"]
                    .as_str()
                    .ok_or("missing type reason")?
                    .into(),
            ),
            _ => return Err("unknown knowledge reason".into()),
        },
    )
}

fn fact<T>(value: &Fact<T>, mut found: impl FnMut(&T) -> Value) -> Value {
    match value {
        Knowledge::Found(value) => json!({"state":"found","value":found(value)}),
        Knowledge::Unknown(reasons) => {
            json!({"state":"unknown","reasons":reasons.iter().map(reason).collect::<Vec<_>>()})
        }
        Knowledge::DefinitelyAbsent => json!({"state":"definitelyAbsent"}),
    }
}

fn missing_fact() -> Value {
    fact(&Fact::<()>::Unknown(vec![IncompleteReason::MissingFacts]), |_| Value::Null)
}

fn decode_fact<T>(
    value: &Value,
    found: impl FnOnce(&Value) -> Result<T, String>,
) -> Result<Fact<T>, String> {
    match value["state"].as_str() {
        Some("found") => found(&value["value"]).map(Knowledge::Found),
        Some("unknown") => {
            let reasons = value["reasons"]
                .as_array()
                .ok_or("missing knowledge reasons")?;
            if reasons.is_empty() {
                return Err("empty knowledge reasons".into());
            }
            Ok(Knowledge::Unknown(
                reasons
                    .iter()
                    .map(decode_reason)
                    .collect::<Result<_, _>>()?,
            ))
        }
        Some("definitelyAbsent") => Ok(Knowledge::DefinitelyAbsent),
        _ => Err("invalid knowledge state".into()),
    }
}

fn kind(kind: MemberKind) -> &'static str {
    match kind {
        MemberKind::Field => "field",
        MemberKind::Method => "method",
    }
}
fn dispatch(dispatch: Dispatch) -> &'static str {
    match dispatch {
        Dispatch::Static => "static",
        Dispatch::Instance => "instance",
    }
}
fn token(value: &Value) -> Result<&str, String> {
    value
        .as_str()
        .or_else(|| value["id"].as_str())
        .ok_or_else(|| "missing opaque token".into())
}

enum Handle {
    ClassIdentity { identity: ktcore::jvm::ClassIdentity, byte_length: usize },
    ClassBytesCursor { identity: ktcore::jvm::ClassIdentity, next: usize },
    Type(TypeId),
    Source(SourceHandle),
    ResourceCatalog {revision:u64},
    ResourceInventory { revision:u64,next:usize },
    ResourceBytes { resource:String,bytes:Arc<crate::resource_inputs::ResourceBytes>,next:usize },
    SourceInventory { sources: Arc<Vec<SourceHandle>>, next: usize },
    Declaration(DeclarationId),
    TypeRef(ktcore::types::TypeRef),
    Signature(MemberSignature),
    #[allow(dead_code)]
    // Keep returned member tokens owned and typed even before a member query uses them.
    Member(MemberId),
    Requirement(RequirementId),
}

pub(crate) struct Evaluation {
    pub id: u64,
    pub provider: String,
    pub owner: String,
    pub instance: u64,
    pub pending: Option<PendingContributions>,
    pub failure: Option<String>,
    pub committed_revision: Option<u64>,
    pub project: Option<Arc<crate::classpath::ProjectSnapshot>>,
    pub project_operations: u16,
    pub command: bool,
    pub(crate) resource_catalog:Option<Arc<crate::resource_inputs::ResourceCatalog>>,
    resource_catalog_token:Option<String>,
    pub(crate) resource_results:BTreeMap<String,Result<Arc<crate::resource_inputs::ResourceBytes>,&'static str>>,
    class_scope: Option<Arc<crate::runtime_bytes::RuntimeClassScope>>,
    class_results: BTreeMap<String, crate::runtime_bytes::ClassReadResult>,
    handles: BTreeMap<String, Handle>,
    batches: usize,
    stream_continuations: usize,
    stream_bytes: usize,
    prepaid_input: Option<Value>,
    reply_is_stream: bool,
}

impl Evaluation {
    pub fn new(provider: String, owner: String, instance: u64) -> Self {
        Self {
            id: NEXT_EVALUATION.fetch_add(1, Ordering::Relaxed),
            provider,
            owner,
            instance,
            pending: None,
            failure: None,
            committed_revision: None,
            project: None,
            project_operations: 0,
            command: false,
            resource_catalog:None,resource_catalog_token:None,resource_results:BTreeMap::new(),
            class_scope: None,
            class_results: BTreeMap::new(),
            handles: BTreeMap::new(),
            batches: 0,
            stream_continuations: 0, stream_bytes: 0, prepaid_input: None, reply_is_stream: false,
        }
    }
    fn issue(&mut self, handle: Handle) -> String {
        let token = format!("{}:{}", self.id, self.handles.len());
        self.handles.insert(token.clone(), handle);
        token
    }
    pub(crate) fn begin_class_reads(&self, method: &str, params: &Value)
        -> Result<Option<crate::runtime_bytes::ClassReadPlan>, String>
    {
        if method != "analysis/readFacts" { return Ok(None); }
        if params["evaluation"].as_str() != Some(&self.id.to_string()) { return Err("foreign evaluation".into()); }
        let queries: Vec<ReadQuery> = serde_json::from_value(params["queries"].clone()).map_err(|error|error.to_string())?;
        if queries.len() > MAX_BATCH { return Err("extension batch too large".into()); }
        self.ensure_input_read_budget(params)?;
        let mut requests = Vec::new();
        for query in queries {
            match query {
                ReadQuery::RuntimeClass { internal_name } => {
                    if self.project_operations & RUNTIME_CLASS == 0 { return Err("runtimeClass was not negotiated".into()); }
                    if !crate::runtime_bytes::valid_internal_name(&internal_name) { return Err("invalid JVM internal name".into()); }
                    requests.push(crate::runtime_bytes::ClassReadRequest { internal_name, expected: None, bytes: false });
                }
                ReadQuery::ClassBytes { class_identity, after } => {
                    if self.project_operations & CLASS_BYTES == 0 { return Err("classBytes was not negotiated".into()); }
                    let (identity, length) = self.class_identity(&class_identity)?;
                    self.class_offset(&identity, after.as_deref())?;
                    if length == 0 { continue; }

                    if self.class_scope.as_ref().is_some_and(|scope|scope.guards().iter().all(|guard|guard.is_current()))
                        && self.class_results.get(&identity.internal_name).is_some_and(|result|
                            result.identity.as_ref()==Some(&identity) && result.bytes.is_some()) { continue; }
                    requests.push(crate::runtime_bytes::ClassReadRequest {
                        internal_name: identity.internal_name.clone(), expected: Some(identity), bytes: true });
                }
                _ => {}
            }
        }
        if requests.is_empty() { return Ok(None); }
        let mut merged: BTreeMap<String, crate::runtime_bytes::ClassReadRequest> = BTreeMap::new();
        for request in requests {
            if let Some(known) = merged.get_mut(&request.internal_name) {
                if known.expected.is_some() && request.expected.is_some() && known.expected != request.expected {
                    return Err("conflicting runtime class identities".into());
                }
                if request.expected.is_some() { known.expected = request.expected; }
                known.bytes |= request.bytes;
            } else { merged.insert(request.internal_name.clone(), request); }
        }
        let requests = merged.into_values().collect();
        Ok(Some(crate::runtime_bytes::ClassReadPlan { evaluation: self.id,
            model: self.project.clone().ok_or("missing selected project model")?,
            owner: self.owner.clone().into(), scope: self.class_scope.clone(), requests }))
    }

    pub(crate) fn finish_class_read(&mut self, read: &mut ReadSession<'_>,
        prepared: crate::runtime_bytes::PreparedClassRead) -> Result<(), String>
    {
        if prepared.evaluation != self.id || !self.project.as_ref().is_some_and(|model|
            Arc::ptr_eq(model, &prepared.model)) { return Err("Stale".into()); }
        if let Some(scope) = &prepared.scope {
            if !Arc::ptr_eq(&scope.model, &prepared.model) { return Err("Stale".into()); }
            for guard in scope.guards() { read.observe_host_input(guard).map_err(contract)?; }
        }
        let results = match prepared.results {
            Ok(results) => results,
            Err(error) => {
                return Err(error);
            }
        };
        if prepared.scope.is_none() && results.iter().any(|value|value.identity.is_some()) {
            return Err("Stale".into());
        }
        self.class_scope = prepared.scope;
        self.class_results = results.into_iter().map(|value|(value.internal_name.clone(), value)).collect();
        Ok(())
    }

    fn class_identity(&self, token: &str) -> Result<(ktcore::jvm::ClassIdentity, usize), String> {
        match self.handles.get(token) {
            Some(Handle::ClassIdentity { identity, byte_length }) => Ok((identity.clone(), *byte_length)),
            _ => Err("foreign runtime class identity".into()),
        }
    }

    fn class_offset(&self, identity: &ktcore::jvm::ClassIdentity, after: Option<&str>) -> Result<usize,String> {
        match after {
            None => Ok(0),
            Some(token) => match self.handles.get(token) {
                Some(Handle::ClassBytesCursor { identity: known, next }) if known == identity => Ok(*next),
                _ => Err("foreign class byte cursor".into()),
            }
        }
    }

    fn runtime_class(&mut self, internal_name: &str) -> Result<Value, String> {
        let result = self.class_results.get(internal_name).ok_or("class inputs were not captured")?;
        let Some(identity) = result.identity.clone() else {
            return Ok(json!({"state":"unknown","reasons":[{"code":"BYTECODE_UNAVAILABLE","detail":result.unavailable}]}));
        };
        let byte_length = result.byte_length;
        let api_header = result.api_header.clone();
        let header_fact = api_header.as_ref().and_then(|header|class_abi(header))
            .map(|header|json!({"state":"found","value":header})).unwrap_or_else(missing_fact);
        let hierarchy = api_header.as_ref().map(|header|json!({"state":"found","value":{
            "superName":header.super_name.as_deref(),"interfaces":header.interfaces.iter().map(|name|name.as_ref()).collect::<Vec<_>>(),
            "accessFlags":header.access_flags,"signature":header.signature.as_deref()
        }})).unwrap_or_else(missing_fact);
        let authority = if api_header.is_some() {"sdk-api-header"} else {"runtime-bytecode"};
        let runtime_coverage = if api_header.is_some() {missing_fact()} else {json!({"state":"found","value":null})};
        let digest = match &identity.input { ktcore::jvm::InputIdentity::Binary { content_sha256 } =>
            content_sha256.iter().map(|byte|format!("{byte:02x}")).collect::<String>(),
            _ => return Err("runtime identity is not a binary input".into()), };
        let id = self.issue(Handle::ClassIdentity { identity: identity.clone(), byte_length });
        Ok(json!({"state":"found","value":{"id":id,"internalName":identity.internal_name,
            "originId":identity.origin_id,"contentSha256":digest,"byteLength":byte_length,
            "authority":authority,"codeAvailable":byte_length>0,"runtimeOnly":api_header.is_none(),
            "classHeader":header_fact,"hierarchy":hierarchy,"runtimeAbiCoverage":runtime_coverage,
            "sourceCurrent":{"state":"unknown","reasons":[{"code":"notSourceAuthority"}]}}}))
    }

    fn class_bytes(&mut self, token: &str, after: Option<&str>) -> Result<Value,String> {
        use base64::Engine as _;
        let (identity, byte_length) = self.class_identity(token)?;
        let offset = self.class_offset(&identity, after)?;
        if byte_length == 0 {
            return Ok(json!({"state":"unknown","reasons":[{"code":"BYTECODE_UNAVAILABLE","detail":"sdk-api-header"}]}));
        }
        let result = self.class_results.get(&identity.internal_name).ok_or("class bytes were not captured")?;
        if result.identity.as_ref() != Some(&identity) { return Err("Stale".into()); }
        let bytes = result.bytes.as_ref().ok_or("class bytes were not captured")?;
        if bytes.len() != byte_length || offset >= byte_length { return Err("invalid class byte cursor".into()); }
        let end = offset.saturating_add(256 * 1024).min(bytes.len());
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes[offset..end]);
        let next = (end < byte_length).then(||self.issue(Handle::ClassBytesCursor { identity, next: end }));
        Ok(json!({"state":"found","value":{"classIdentity":token,"offset":offset,
            "byteLength":byte_length,"bytesBase64":encoded,"next":next}}))
    }

    fn read_charge(&self, params: &Value) -> Result<(bool, usize), String> {
        if params["evaluation"].as_str() != Some(&self.id.to_string()) { return Err("foreign evaluation".into()); }
        let queries: Vec<ReadQuery> = serde_json::from_value(params["queries"].clone()).map_err(|error|error.to_string())?;
        if queries.len() > MAX_BATCH { return Err("extension batch too large".into()); }
        let mut continuations = 0usize;
        for query in &queries {
            match query {
                ReadQuery::ClassBytes { class_identity, after: Some(after) } => {
                    if self.project_operations & CLASS_BYTES == 0 { return Err("classBytes was not negotiated".into()); }
                    let (identity, length) = self.class_identity(class_identity)?;
                    if self.class_offset(&identity, Some(after))? >= length { return Err("invalid class byte cursor".into()); }
                    continuations += 1;
                }
                ReadQuery::ResourceSnapshot { resource, after: Some(after) } => {
                    if self.project_operations & RESOURCE_SNAPSHOT == 0 { return Err("resourceSnapshot was not negotiated".into()); }
                    self.resource_stream_cursor(resource, after)?;
                    continuations += 1;
                }
                _ => {}
            }
        }
        Ok((continuations == 0 || continuations < queries.len(), continuations))
    }

    fn check_read_charge(&self, params: &Value) -> Result<(bool, usize), String> {
        let (metadata, continuations) = self.read_charge(params)?;
        if self.handles.len() > MAX_HANDLES || (metadata && self.batches >= 64)
            || continuations > MAX_STREAM_CONTINUATIONS.saturating_sub(self.stream_continuations)
            || (continuations > 0 && self.stream_bytes >= MAX_STREAM_BYTES)
        { return Err("extension evaluation limit".into()); }
        Ok((metadata, continuations))
    }

    fn ensure_input_read_budget(&self, params: &Value) -> Result<(), String> {
        if self.prepaid_input.as_ref() == Some(params) { return Ok(()); }
        self.check_read_charge(params).map(|_|())
    }

    pub(crate) fn prepay_input_read(&mut self, method: &str, params: &Value) -> Result<(), String> {
        if self.prepaid_input.is_some() { return Err("unfinished extension input read".into()); }
        self.reply_is_stream = false;
        if method != "analysis/readFacts" { return Ok(()); }
        let (metadata, continuations) = self.check_read_charge(params)?;
        self.batches += usize::from(metadata);
        self.stream_continuations += continuations;
        self.reply_is_stream = continuations > 0;
        self.prepaid_input = Some(params.clone());
        Ok(())
    }

    pub(crate) fn cancel_prepaid_input(&mut self) { self.prepaid_input = None; }

    pub(crate) fn account_stream_reply(&mut self, frame: &Value) -> Result<(), String> {
        if !std::mem::take(&mut self.reply_is_stream) { return Ok(()); }
        let length = serde_json::to_vec(frame).map_err(|error|error.to_string())?.len();
        let total = self.stream_bytes.checked_add(length).ok_or("extension stream byte limit")?;
        if total > MAX_STREAM_BYTES { return Err("extension stream byte limit".into()); }
        self.stream_bytes = total;
        Ok(())
    }

    fn source_descriptor(&self, source: &SourceHandle, id: &str) -> Value {
        json!({"id":id,"displayPath":source.file(),
            "documentUri":std::path::Path::new(source.file()).is_absolute()
                .then(|| tower_lsp_server::ls_types::Uri::from_file_path(source.file())).flatten()
                .map_or_else(missing_fact, |uri|json!({"state":"found","value":uri.to_string()})),
            "languageId":if source.file().ends_with(".java") {"java"} else {"kotlin"},
            "identity":{"kind":"source","revision":source.revision().to_string()}})
    }
    fn source_inventory(
        &mut self, read: &mut ReadSession<'_>, after: Option<&str>, budget: usize,
    ) -> Result<Value, String> {
        let continuation = match after {
            Some(token) => match self.handles.get(token) {
                Some(Handle::SourceInventory { sources, next }) => Some((sources.clone(), *next)),
                _ => return Err("foreign source inventory cursor".into()),
            },
            None => None,
        };
        let current = match read.source_inventory() {
            Ok(sources) => sources,
            Err(ContractError::ForeignFile | ContractError::MissingSource) if after.is_none() =>
                return Ok(json!({"sources":[],"coverage":missing_fact(),"next":null})),
            Err(error) => return Err(contract(error)),
        };
        let (sources, start) = match continuation {
            Some((sources, next)) => {
                if sources.as_ref() != &current || next == 0 || next >= sources.len() {
                    return Err("stale source inventory cursor".into());
                }
                (sources, next)
            }
            None => (Arc::new(current), 0),
        };
        let coverage = self.project.as_ref().and_then(|model|
            model.extension_project_context(std::path::Path::new(&self.owner)))
            .map_or_else(missing_fact, |context| {
                let accepted: std::collections::BTreeSet<_> = sources.iter().map(|source|source.file()).collect();
                let complete = source_inventory_complete(context.source, &accepted);
                if complete { json!({"state":"found","value":null}) } else { missing_fact() }
            });
        let limit = budget.min(MAX_FRAME / 2);
        let cursor_bound = format!("{}:{}", self.id, MAX_HANDLES);
        let size = |value: &Value| serde_json::to_vec(value).map(|bytes|bytes.len()).map_err(|error|error.to_string());
        // Reserve both final coverage and a continuation before issuing any identities.
        let mut used = size(&json!({"sources":[],"coverage":missing_fact(),"next":cursor_bound}))?
            .max(size(&json!({"sources":[],"coverage":coverage,"next":cursor_bound}))?);
        if used > limit { return Err(READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into()); }
        let mut entries = Vec::new();
        for (index, source) in sources.iter().enumerate().skip(start) {
            let cursor_slot = usize::from(index + 1 < sources.len());
            if self.handles.len() + entries.len() + 1 + cursor_slot > MAX_HANDLES {
                if entries.is_empty() { return Err("source inventory handle limit".into()); }
                break;
            }
            let id = format!("{}:{}", self.id, self.handles.len() + entries.len());
            let entry = self.source_descriptor(source, &id);
            let bytes = size(&entry)?.saturating_add(1);
            if used.saturating_add(bytes) > limit { break; }
            used += bytes;
            entries.push(entry);
        }
        if entries.is_empty() && start < sources.len() {
            return Err(READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into());
        }
        let end = start + entries.len();
        for source in &sources[start..end] { self.issue(Handle::Source(source.clone())); }
        let next = (end < sources.len()).then(||self.issue(Handle::SourceInventory { sources, next:end }));
        let value = json!({"sources":entries,
            "coverage":if next.is_some() { missing_fact() } else { coverage },"next":next});
        if size(&value)? > limit { return Err(READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into()); }
        Ok(value)
    }
    fn source_snapshot(&mut self, read: &mut ReadSession<'_>, token: &str) -> Result<Value, String> {
        use sha2::{Digest, Sha256};
        let source = match self.handles.get(token) {
            Some(Handle::Source(source)) => source.clone(),
            _ => return Err("foreign source token".into()),
        };
        let text = read.source_snapshot(&source).map_err(contract)?;
        if text.len() > MAX_FRAME / 8 {
            return Ok(fact(&Fact::<()>::Unknown(vec![IncompleteReason::UnsupportedSource]), |_|Value::Null));
        }
        let mut value = self.source_descriptor(&source, token);
        value["text"] = json!(text.as_ref());
        value["sha256"] = json!(format!("{:x}", Sha256::digest(text.as_bytes())));
        Ok(json!({"state":"found","value":value}))
    }
    fn project_context(&self) -> Value {
        let mut value = json!({"workspaceUri":missing_fact(),"scope":missing_fact(),
            "sourceRoots":missing_fact(),"outputRoots":missing_fact(),
            "classpath":missing_fact(),"sdk":missing_fact()});
        let Some(model) = self.project.as_ref() else { return value; };
        let Some(context) = model.extension_project_context(std::path::Path::new(&self.owner)) else { return value; };
        value["scope"] = json!({"state":"found","value":{"module":context.module,"sourceSet":context.scope.source_set}});
        value["sourceRoots"] = json!({"state":"found","value":context.source.source_roots.iter().map(|path|path.to_string_lossy()).collect::<Vec<_>>()});
        value["outputRoots"] = json!({"state":"found","value":context.source.output_roots.iter().map(|path|path.to_string_lossy()).collect::<Vec<_>>()});
        let entries: Option<Vec<_>> = context.source.entries.iter().map(|path| {
            Some(json!({"id":model.artifact_origin(path)?,"displayPath":path.to_string_lossy()}))
        }).collect();
        value["classpath"] = entries.map_or_else(missing_fact, |entries| json!({"state":"found","value":{
            "entries":entries,
            "coverage":if context.classpath_complete {json!({"state":"found","value":null})} else {fact(&Fact::<()>::Unknown(vec![IncompleteReason::IncompleteScope]), |_|Value::Null)}
        }}));
        if let Some(sdk) = context.sdk {
            value["sdk"] = json!({"state":"found","value":{"identity":sdk.identity(),
                "api":sdk.release.map_or_else(||json!({"kind":"full"}),|release|json!({"kind":"release","release":release}))}});
        }
        if serde_json::to_vec(&value).is_ok_and(|bytes| bytes.len() > MAX_FRAME / 2) {
            value["classpath"] = missing_fact();
        }
        value
    }
    fn ty(&self, value: &Value) -> Result<TypeId, String> {
        match self.handles.get(token(value)?) {
            Some(Handle::Type(value)) => Ok(value.clone()),
            _ => Err("foreign type token".into()),
        }
    }
    fn declaration(&self, value: &Value) -> Result<DeclarationId, String> {
        match self.handles.get(token(value)?) {
            Some(Handle::Declaration(value)) => Ok(value.clone()),
            _ => Err("foreign declaration token".into()),
        }
    }
    fn signature(&self, value: &Value) -> Result<MemberSignature, String> {
        match self.handles.get(token(value)?) {
            Some(Handle::Signature(value)) => Ok(value.clone()),
            _ => Err("foreign signature token".into()),
        }
    }
    fn type_fact(&mut self, value: &Fact<TypeId>) -> Value {
        fact(
            value,
            |ty| json!({"id":self.issue(Handle::Type(ty.clone())),"qualifiedName":ty.qualified_name(),"package":ty.package()}),
        )
    }
    fn signature_fact(&mut self, value: &Fact<MemberSignature>) -> Value {
        fact(
            value,
            |signature| json!({"id":self.issue(Handle::Signature(signature.clone())),"kind":kind(signature.kind()),"descriptor":signature.descriptor()}),
        )
    }
    fn annotation(&mut self, annotation: &SourceAnnotation) -> Value {
        let reference = self.issue(Handle::TypeRef(annotation.ty.clone()));
        json!({"typeRef":reference,"useSiteTarget":annotation.use_site_target,"range":range(&annotation.range),"arguments":annotation.arguments.iter().map(|argument|self.argument(argument)).collect::<Vec<_>>()})
    }
    fn argument(&mut self, argument: &AnnotationArgument) -> Value {
        let value = fact(&argument.value, |value| match value {
            AnnotationValue::Class(ty) => {
                json!({"kind":"class","typeRef":self.issue(Handle::TypeRef(ty.clone()))})
            }
            AnnotationValue::String {
                value,
                content_range,
            } => json!({"kind":"string","value":value,"contentRange":range(content_range)}),
            AnnotationValue::Boolean(value) => json!({"kind":"boolean","value":value}),
            AnnotationValue::Array(values) => {
                json!({"kind":"array","values":values.iter().map(|value|self.argument(value)).collect::<Vec<_>>()})
            }
        });
        json!({"name":argument.name,"range":range(&argument.range),"value":value})
    }
    pub fn source(&mut self, read: &mut ReadSession<'_>) -> Result<Value, String> {
        let declarations = read.declarations().to_vec();
        let mut ids = Vec::new();
        for declaration in &declarations {
            let id = read
                .declaration(declaration.definition.start_byte..declaration.definition.end_byte)
                .map_err(contract)?;
            ids.push(self.issue(Handle::Declaration(id)));
        }
        let mut values = Vec::new();
        for (position, declaration) in declarations.iter().enumerate() {
            let owner = declaration
                .owner
                .as_ref()
                .and_then(|owner| {
                    declarations
                        .iter()
                        .position(|candidate| &candidate.definition == owner)
                })
                .map(|index| ids[index].clone());
            values.push(json!({"id":ids[position],"owner":owner,"range":{"start":declaration.definition.start_byte,"end":declaration.definition.end_byte},"name":read.source().get(declaration.definition.start_byte..declaration.definition.end_byte),"annotations":declaration.annotations.iter().map(|annotation|self.annotation(annotation)).collect::<Vec<_>>()}));
        }
        if self.handles.len() > MAX_HANDLES {
            return Err("too many extension handles".into());
        }
        Ok(
            json!({"evaluation":self.id.to_string(),"owner":self.owner,"source":read.source(),"configuration":serde_json::from_str::<Value>(read.configuration()).map_err(|e|e.to_string())?,"declarations":values}),
        )
    }
    fn member(&mut self, candidate: &MemberCandidate) -> Value {
        let id = self.issue(Handle::Member(candidate.id.clone()));
        json!({"id":id,"name":candidate.name,"kind":kind(candidate.kind),"signature":self.signature_fact(&candidate.signature),"dispatch":fact(&candidate.dispatch,|value|json!(dispatch(*value))),"visibility":fact(&candidate.visibility,|value|json!(format!("{value:?}"))),"synthetic":fact(&candidate.synthetic,|value|json!(value)),"mismatches":candidate.mismatches.iter().map(|value|format!("{value:?}")).collect::<Vec<_>>()})
    }

    fn type_jvm(&mut self, facts: &TypeJvmFacts) -> Value {
        let abi = match &facts.abi {
            Knowledge::Found(class) => class_abi(class)
                .map(|value| json!({"state":"found","value":value}))
                .unwrap_or_else(missing_fact),
            other => fact(other, |_| Value::Null),
        };
        json!({
            "binaryName":fact(&facts.binary_name, |name| json!({"internalName":name.internal_name,"descriptor":name.descriptor})),
            "classFlags":fact(&facts.class_flags, |flags| json!(flags)),
            "declaredMembers":{
                "members":facts.declared_members.iter().map(|member|self.member(member)).collect::<Vec<_>>(),
                "coverage":fact(&facts.declared_members_coverage, |_|Value::Null)
            },
            "abi":abi,
            "jvmAbiCoverage":fact(&facts.jvm_abi_coverage, |_|Value::Null)
        })
    }
    fn resource_identity(&mut self,catalog:&crate::resource_inputs::ResourceCatalog,index:usize)->String {
        let prefix=if let Some(prefix)=&self.resource_catalog_token {prefix.clone()} else {
            let prefix=self.issue(Handle::ResourceCatalog{revision:catalog.revision});
            self.resource_catalog_token=Some(prefix.clone());prefix
        };
        format!("{prefix}#resource:{index:x}")
    }

    fn resource_index(&self,catalog:&crate::resource_inputs::ResourceCatalog,token:&str)->Result<usize,String> {
        let (prefix,slot)=token.split_once("#resource:").ok_or("invalid resource identity")?;
        if self.resource_catalog_token.as_deref()!=Some(prefix)
            || !matches!(self.handles.get(prefix),Some(Handle::ResourceCatalog{revision}) if *revision==catalog.revision)
            || slot.is_empty() || slot.len()>16 || !slot.bytes().all(|byte|byte.is_ascii_hexdigit()) {return Err("foreign resource catalog".into());}
        let index=usize::from_str_radix(slot,16).map_err(|_|"invalid resource index")?;
        if index>=catalog.entries.len() || format!("{index:x}")!=slot {return Err("foreign resource index".into());}
        Ok(index)
    }

    fn resource_stream_cursor(&self,resource:&str,after:&str)->Result<(),String> {
        let catalog=self.resource_catalog.as_ref().ok_or("resource inputs were not captured")?;
        self.resource_index(catalog,resource)?;
        match self.handles.get(after) {
            Some(Handle::ResourceBytes{resource:expected,..}) if expected==resource=>Ok(()),
            _=>Err("invalid resource snapshot cursor".into()),
        }
    }

    fn resource_descriptor(&mut self, catalog:&crate::resource_inputs::ResourceCatalog, index:usize)->Value {
        let id=self.resource_identity(catalog,index);
        Self::resource_descriptor_value(catalog,index,&id)
    }

    fn resource_descriptor_value(catalog:&crate::resource_inputs::ResourceCatalog,index:usize,id:&str)->Value {
        use crate::resource_inputs::ResourceLocation;
        let entry=&catalog.entries[index];
        let uri=match &entry.location {
            ResourceLocation::File(path) if path.is_absolute()=>tower_lsp_server::ls_types::Uri::from_file_path(path)
                .map_or_else(missing_fact,|uri|json!({"state":"found","value":uri.to_string()})),
            ResourceLocation::Archive{..}|ResourceLocation::File(_)=>missing_fact(),
        };
        json!({"id":id,"relativePath":entry.relative_path,"association":entry.association.name(),
            "classpathOrder":entry.order,"editable":entry.editable,"documentUri":uri,
            "documentVersion":entry.document.as_ref().and_then(|document|document.document_version),
            "identity":{"kind":"resource","revision":catalog.revision.to_string()}})
    }

    fn resource_inventory(&mut self,read:&mut ReadSession<'_>,after:Option<&str>)->Result<Value,String> {
        let Some(catalog)=self.resource_catalog.clone() else {return Ok(json!({"resources":[],"next":null,"coverage":missing_fact()}));};
        read.observe_host_input(catalog.guard.clone()).map_err(|error|format!("{error:?}"))?;
        let start=match after {
            None=>0,
            Some(token)=>match self.handles.get(token) {
                Some(Handle::ResourceInventory{revision,next}) if *revision==catalog.revision=>*next,
                _=>return Err("invalid resource inventory cursor".into()),
            }
        };
        let mut values=Vec::new();let mut bytes=512;let mut end=start;
        while end<catalog.entries.len() {
            let id=self.resource_identity(&catalog,end);
            let descriptor=Self::resource_descriptor_value(&catalog,end,&id);
            let size=serde_json::to_vec(&descriptor).map_err(|error|error.to_string())?.len()+1;
            if bytes+size>MAX_FRAME/2-4096 {break;}
            values.push(descriptor);bytes+=size;end+=1;
        }
        if start==end && end<catalog.entries.len() {return Err(READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into());}
        let next=(end<catalog.entries.len()).then(||self.issue(Handle::ResourceInventory{revision:catalog.revision,next:end}));
        Ok(json!({"resources":values,"next":next,"coverage":if catalog.complete && end==catalog.entries.len() {
            json!({"state":"found","value":true})
        } else {missing_fact()}}))
    }

    fn resource_lookup(&mut self,read:&mut ReadSession<'_>,relative:&str,
        association:Option<crate::resource_inputs::ResourceAssociation>)->Result<Value,String> {
        if !crate::resource_model::safe_relative(relative) {return Err("invalid resource relative path".into());}
        let Some(catalog)=self.resource_catalog.clone() else {return Ok(missing_fact());};
        read.observe_host_input(catalog.guard.clone()).map_err(|error|format!("{error:?}"))?;
        let indices:Vec<_>=catalog.entries.iter().enumerate().filter(|(_,entry)|
            entry.relative_path==relative && association.is_none_or(|kind|kind==entry.association))
            .map(|(index,_)|index).collect();
        if indices.is_empty() {
            return Ok(if catalog.complete {json!({"state":"definitelyAbsent"})} else {missing_fact()});
        }
        let values:Vec<_>=indices.into_iter().map(|index|self.resource_descriptor(&catalog,index)).collect();
        Ok(json!({"state":"found","value":values}))
    }

    pub(crate) fn begin_resource_reads(&self,method:&str,params:&Value)->Result<Vec<crate::resource_inputs::ResourceReadPlan>,String> {
        if method!="analysis/readFacts" {return Ok(Vec::new());}
        if params["evaluation"].as_str()!=Some(self.id.to_string().as_str()) {return Err("foreign evaluation".into());}
        self.ensure_input_read_budget(params)?;
        let queries=params["queries"].as_array().filter(|queries|queries.len()<=MAX_BATCH).ok_or("invalid resource read batch")?;
        let catalog=self.resource_catalog.as_ref();
        if catalog.is_some_and(|catalog|!catalog.guard.is_current()) {return Err("Stale".into());}
        let mut seen=std::collections::BTreeSet::new();
        let used:usize=self.resource_results.values().filter_map(|result|result.as_ref().ok())
            .filter(|bytes|seen.insert(Arc::as_ptr(bytes) as usize)).map(|bytes|bytes.bytes.len()).sum();
        let mut reads=BTreeMap::new();
        for value in queries {
            let query:ReadQuery=serde_json::from_value(value.clone()).map_err(|error|error.to_string())?;
            match query {
                ReadQuery::ResourceInventory{after}=>{
                    if self.project_operations&RESOURCE_INVENTORY==0 {return Err("resourceInventory was not negotiated".into());}
                    if let Some(after)=after {
                        if !matches!(self.handles.get(&after),Some(Handle::ResourceInventory{revision,..}) if catalog.is_some_and(|catalog|catalog.revision==*revision)) {
                            return Err("invalid resource inventory cursor".into());
                        }
                    }
                }
                ReadQuery::ResourceLookup{relative_path,..}=>{
                    if self.project_operations&RESOURCE_LOOKUP==0 {return Err("resourceLookup was not negotiated".into());}
                    if !crate::resource_model::safe_relative(&relative_path) {return Err("invalid resource relative path".into());}
                }
                ReadQuery::ResourceSnapshot{resource,after}=>{
                    if self.project_operations&RESOURCE_SNAPSHOT==0 {return Err("resourceSnapshot was not negotiated".into());}
                    let catalog=catalog.ok_or("resource inputs were not captured")?;
                    let index=self.resource_index(catalog,&resource)?;
                    if let Some(after)=after {
                        self.resource_stream_cursor(&resource,&after)?;
                        continue;
                    }
                    if self.resource_results.contains_key(&resource) {continue;}
                    let entry=catalog.entries.get(index).ok_or("invalid resource identity")?.clone();
                    reads.entry(resource.clone()).or_insert(crate::resource_inputs::ResourceReadPlan {
                        evaluation:self.id,model:catalog.model.clone(),token:resource,entry,guard:catalog.guard.clone(),catalog:catalog.clone(),index,
                        byte_budget:(64usize*1024*1024).saturating_sub(used),
                    });
                }
                _=>{}
            }
        }
        Ok(reads.into_values().collect())
    }

    fn resource_snapshot(&mut self,read:&mut ReadSession<'_>,resource:&str,after:Option<&str>)->Result<Value,String> {
        use base64::Engine;
        let catalog=self.resource_catalog.clone().ok_or("resource inputs were not captured")?;
        self.resource_index(&catalog,resource)?;
        read.observe_host_input(catalog.guard.clone()).map_err(|error|format!("{error:?}"))?;
        let (body,start)=match after {
            Some(cursor)=>match self.handles.get(cursor) {
                Some(Handle::ResourceBytes{resource:expected,bytes,next}) if expected==resource=>(bytes.clone(),*next),
                _=>return Err("invalid resource snapshot cursor".into()),
            },
            None=>match self.resource_results.get(resource) {
                Some(Ok(body))=>(body.clone(),0),
                Some(Err(_))|None=>return Ok(missing_fact()),
            },
        };
        read.observe_host_input(body.guard.clone()).map_err(|error|format!("{error:?}"))?;
        const CHUNK:usize=(MAX_FRAME/2-4096)/4*3;
        let end=(start+CHUNK).min(body.bytes.len());
        if start>end {return Err("invalid resource byte cursor".into());}
        let next=(end<body.bytes.len()).then(||self.issue(Handle::ResourceBytes{resource:resource.into(),bytes:body.clone(),next:end}));
        let text=if !body.utf8 {json!({"state":"definitelyAbsent"})}
            else if body.bytes.len()<=MAX_FRAME/32 {
                json!({"state":"found","value":std::str::from_utf8(&body.bytes).map_err(|error|error.to_string())?})
            } else {missing_fact()};
        Ok(json!({"state":"found","value":{"identity":{"kind":"resource","revision":catalog.revision.to_string(),"sha256":body.sha256},
            "documentVersion":body.document_version,"size":body.bytes.len(),"offset":start,
            "base64":base64::engine::general_purpose::STANDARD.encode(&body.bytes[start..end]),
            "utf8":body.utf8,"text":text,"next":next}}))
    }

    pub fn batch(
        &mut self,
        read: &mut ReadSession<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if params["evaluation"].as_str() != Some(&self.id.to_string()) {
            return Err("foreign evaluation".into());
        }
        if self.command && !matches!(method, "analysis/readFacts" | "analysis/require") {
            return Err("command reads cannot publish semantic contributions".into());
        }
        if method == "analysis/readFacts" {
            if let Some(prepaid) = self.prepaid_input.take() {
                if prepaid != params { return Err("extension input reservation mismatch".into()); }
            } else {
                self.prepay_input_read(method, &params)?;
                self.prepaid_input = None;
            }
        } else {
            if self.prepaid_input.is_some() { return Err("unfinished extension input read".into()); }
            self.reply_is_stream = false;
            self.batches += 1;
            if self.batches > 64 || self.handles.len() > MAX_HANDLES { return Err("extension evaluation limit".into()); }
        }
        match method {
            "analysis/readFacts" => {
                let queries: Vec<ReadQuery> =
                    serde_json::from_value(params["queries"].clone()).map_err(|e| e.to_string())?;
                if queries.len() > MAX_BATCH {
                    return Err("extension batch too large".into());
                }
                let mut result = Vec::new();
                // Leave room for the JSON-RPC envelope. Count escaped UTF-8 bytes before
                // retaining another fact, so a child can split a batch without dying.
                let mut remaining = MAX_FRAME / 2;
                for query in queries {
                    let value = match query {
                        ReadQuery::ResourceInventory{after}=>{
                            if self.project_operations&RESOURCE_INVENTORY==0 {return Err("resourceInventory was not negotiated".into());}
                            self.resource_inventory(read,after.as_deref())?
                        }
                        ReadQuery::ResourceLookup{relative_path,association}=>{
                            if self.project_operations&RESOURCE_LOOKUP==0 {return Err("resourceLookup was not negotiated".into());}
                            self.resource_lookup(read,&relative_path,association)?
                        }
                        ReadQuery::ResourceSnapshot{resource,after}=>{
                            if self.project_operations&RESOURCE_SNAPSHOT==0 {return Err("resourceSnapshot was not negotiated".into());}
                            self.resource_snapshot(read,&resource,after.as_deref())?
                        }
                        ReadQuery::RuntimeClass { internal_name } => {
                            if self.project_operations & RUNTIME_CLASS == 0 { return Err("runtimeClass was not negotiated".into()); }
                            self.runtime_class(&internal_name)?
                        }
                        ReadQuery::ClassBytes { class_identity, after } => {
                            if self.project_operations & CLASS_BYTES == 0 { return Err("classBytes was not negotiated".into()); }
                            self.class_bytes(&class_identity, after.as_deref())?
                        }
                        ReadQuery::SourceInventory { after } => {
                            if self.project_operations & SOURCE_INVENTORY == 0 {
                                return Err("sourceInventory was not negotiated".into());
                            }
                            self.source_inventory(read, after.as_deref(), remaining.saturating_sub(1))?
                        }
                        ReadQuery::SourceSnapshot { source } => {
                            if self.project_operations & SOURCE_SNAPSHOT == 0 {
                                return Err("sourceSnapshot was not negotiated".into());
                            }
                            self.source_snapshot(read, &source)?
                        }
                        ReadQuery::ProjectContext => {
                            if self.project_operations & PROJECT_CONTEXT == 0 {
                                return Err("projectContext was not negotiated".into());
                            }
                            self.project_context()
                        }
                        ReadQuery::TypeJvm {target} => {
                            if self.project_operations & TYPE_JVM == 0 {
                                return Err("typeJvm was not negotiated".into());
                            }
                            let facts = read.type_jvm(&self.ty(&json!(target))?).map_err(contract)?;
                            self.type_jvm(&facts)
                        }
                        ReadQuery::ResolveType {name} => self.type_fact(&read.resolve_type(&name)),
                        ReadQuery::ResolveTypeRef {reference} => {
                            let reference = match self.handles.get(&reference) { Some(Handle::TypeRef(value)) => value.clone(), _ => return Err("foreign type-reference token".into()) };
                            self.type_fact(&read.resolve_type_ref(&reference))
                        }
                        ReadQuery::DeclarationType {declaration} => self.type_fact(&read.declaration_type(&self.declaration(&json!(declaration))?).map_err(contract)?),
                        ReadQuery::DeclarationMember {declaration} => {
                            let member = read.declaration_member(&self.declaration(&json!(declaration))?).map_err(contract)?;
                            fact(&member, |member| self.member(member))
                        }
                        ReadQuery::DeclaredMembers {target} => {
                            let (members,coverage) = read.declared_members(&self.ty(&json!(target))?).map_err(contract)?;
                            json!({"members":members.iter().map(|member|self.member(member)).collect::<Vec<_>>(),"coverage":fact(&coverage, |_|Value::Null)})
                        }
                    };
                    let size = serde_json::to_vec(&value).map_err(|error|error.to_string())?.len();
                    remaining = remaining.checked_sub(size.saturating_add(1))
                        .ok_or_else(|| READ_FACTS_RESPONSE_BUDGET_EXCEEDED.to_string())?;
                    result.push(value);
                }
                Ok(json!({"facts":result}))
            }
            "analysis/require" => {
                let proposals: Vec<Requirement> =
                    serde_json::from_value(params["requirements"].clone())
                        .map_err(|e| e.to_string())?;
                if proposals.len() > MAX_BATCH {
                    return Err("extension batch too large".into());
                }
                let mut results = Vec::new();
                for proposal in proposals {
                    let requirement = MemberRequirement {
                        target: decode_fact(&proposal.target, |value| self.ty(value))?,
                        authored: proposal
                            .authored
                            .as_ref()
                            .map(|value| self.declaration(&json!(value)))
                            .transpose()?,
                        names: match proposal.names {
                            Names::Exact { values } => MemberNames::Exact(values),
                            Names::Prefix { value } => MemberNames::Prefix(value),
                        },
                        kind: match proposal.kind.as_str() {
                            "method" => MemberKind::Method,
                            "field" => MemberKind::Field,
                            _ => return Err("invalid member kind".into()),
                        },
                        signature: decode_fact(&proposal.signature, |value| self.signature(value))?,
                        dispatch: decode_fact(&proposal.dispatch, |value| match value.as_str() {
                            Some("instance") => Ok(Dispatch::Instance),
                            Some("static") => Ok(Dispatch::Static),
                            _ => Err("invalid dispatch".into()),
                        })?,
                        name_link: match proposal.name_link {
                            Link::Direct => NameLink::Direct,
                            Link::Prefixed { prefix } => NameLink::Prefixed(prefix),
                            Link::IndependentAlias => NameLink::IndependentAlias,
                        },
                    };
                    let id = read.require(requirement).map_err(contract)?;
                    let resolved = read.resolution(id).map_err(contract)?.clone();
                    let id = self.issue(Handle::Requirement(id));
                    let binding = fact(
                        &resolved.binding,
                        |binding| json!({"target":self.issue(Handle::Member(binding.target.clone()))}),
                    );
                    results.push(json!({"id":id,"binding":binding,"candidates":resolved.candidates.iter().map(|candidate|self.member(candidate)).collect::<Vec<_>>()}));
                }
                Ok(json!({"requirements":results}))
            }
            "analysis/finish" => {
                let result: Contributions = serde_json::from_value(params["contributions"].clone())
                    .map_err(|e| e.to_string())?;
                if result.sites.len() + result.diagnostics.len() > MAX_HANDLES {
                    return Err("too many extension contributions".into());
                }
                for site in result.sites {
                    let id = match self.handles.get(&site.requirement) {
                        Some(Handle::Requirement(id)) => *id,
                        _ => return Err("foreign requirement token".into()),
                    };
                    read.bind_site(
                        id,
                        site.full_range.start..site.full_range.end,
                        site.editable_name.start..site.editable_name.end,
                        match site.rename_subject.as_str() {
                            "target" => RenameSubject::Target,
                            "authored" => RenameSubject::Authored,
                            _ => return Err("invalid rename subject".into()),
                        },
                    )
                    .map_err(contract)?;
                }
                for diagnostic in result.diagnostics {
                    let code = diagnostic
                        .code
                        .map(|code| match code.as_str() {
                            "syntax_error" => Ok(DiagnosticCode::SyntaxError),
                            "unused_import" => Ok(DiagnosticCode::UnusedImport),
                            "unresolved_reference" => Ok(DiagnosticCode::UnresolvedReference),
                            "call_shape_mismatch" => Ok(DiagnosticCode::CallShapeMismatch),
                            _ => Err("unsupported diagnostic code"),
                        })
                        .transpose()?;
                    read.diagnostic(Diagnostic {
                        start_byte: diagnostic.range.start,
                        end_byte: diagnostic.range.end,
                        severity: match diagnostic.severity.as_str() {
                            "error" => Severity::Error,
                            "warning" => Severity::Warning,
                            "hint" => Severity::Hint,
                            _ => return Err("invalid diagnostic severity".into()),
                        },
                        code,
                        message: diagnostic.message,
                    })
                    .map_err(contract)?;
                }
                Ok(Value::Null)
            }
            _ => Err("unsupported extension method".into()),
        }
    }
}

fn contract(error: ContractError) -> String {
    format!("{error:?}")
}
fn range(range: &std::ops::Range<usize>) -> Value {
    json!({"start":range.start,"end":range.end})
}

fn class_member(member: &ktcore::classfile::MemberFacts) -> Value {
    json!({
        "name":member.name.as_ref(), "descriptor":member.descriptor.as_ref(),
        "accessFlags":member.access_flags, "signature":member.signature.as_deref(),
        "attributeNames":member.attribute_names.iter().map(|name|name.as_ref()).collect::<Vec<_>>()
    })
}

fn class_abi(class: &ktcore::classfile::ClassFileFacts) -> Option<Value> {
    let ktcore::jvm::InputIdentity::Binary { content_sha256 } = &class.identity.input else {
        return None;
    };
    let mut digest = String::with_capacity(64);
    for byte in content_sha256 {
        write!(&mut digest, "{byte:02x}").expect("formatting into a String");
    }
    let value = json!({
        "identity":{
            "originId":class.identity.origin_id, "internalName":class.identity.internal_name,
            "contentSha256":digest
        },
        "majorVersion":class.major_version, "minorVersion":class.minor_version,
        "accessFlags":class.access_flags, "superName":class.super_name.as_deref(),
        "interfaces":class.interfaces.iter().map(|name|name.as_ref()).collect::<Vec<_>>(),
        "signature":class.signature.as_deref(), "sourceFile":class.source_file.as_deref(),
        "attributeNames":class.attribute_names.iter().map(|name|name.as_ref()).collect::<Vec<_>>(),
        "innerClasses":class.inner_classes.iter().map(|inner|json!({
            "inner":inner.inner.as_ref(),"outer":inner.outer.as_deref(),
            "name":inner.name.as_deref(),"accessFlags":inner.access_flags
        })).collect::<Vec<_>>(),
        "fields":class.fields.iter().map(class_member).collect::<Vec<_>>(),
        "methods":class.methods.iter().map(class_member).collect::<Vec<_>>()
    });
    (serde_json::to_vec(&value).ok()?.len() <= MAX_FRAME / 2).then_some(value)
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum ReadQuery {
    ResourceInventory { #[serde(default)] after:Option<String> },
    ResourceLookup { #[serde(rename="relativePath")] relative_path:String, #[serde(default)] association:Option<crate::resource_inputs::ResourceAssociation> },
    ResourceSnapshot { resource:String, #[serde(default)] after:Option<String> },
    RuntimeClass { #[serde(rename="internalName")] internal_name: String },
    ClassBytes { #[serde(rename="classIdentity")] class_identity: String, #[serde(default)] after: Option<String> },
    SourceInventory { #[serde(default)] after: Option<String> },
    SourceSnapshot { source: String },
    ProjectContext,
    TypeJvm { target: String },
    ResolveType { name: String },
    ResolveTypeRef { reference: String },
    DeclarationType { declaration: String },
    DeclarationMember { declaration: String },
    DeclaredMembers { target: String },
}

fn source_inventory_complete(
    source: &crate::classpath::SourceSetClasspath,
    accepted: &std::collections::BTreeSet<&str>,
) -> bool {
    let mut normal = false;
    !source.compiler_metadata_incomplete
        && !source.compiler_tasks.is_empty()
        && source.compiler_tasks.iter().all(|task| {
            let known = match task.task_type.strip_suffix("_Decorated").unwrap_or(&task.task_type) {
                "org.gradle.api.tasks.compile.JavaCompile"
                | "org.jetbrains.kotlin.gradle.tasks.KotlinCompile" => {
                    normal = true;
                    crate::classpath::sdk_producer_roots(task).is_some_and(|(_, complete)| complete)
                }
                // These describe nonexecuted inputs; their claimed files must still be present.
                "org.jetbrains.kotlin.gradle.internal.KaptWithoutKotlincTask"
                | "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask"
                | "com.google.devtools.ksp.gradle.KspTaskJvm" => true,
                _ => false,
            };
            known && task.complete && task.problem.is_none()
                && task.source_files.iter()
                    .filter(|path| matches!(path.extension().and_then(|ext|ext.to_str()),Some("java"|"kt")))
                    .all(|path|accepted.contains(crate::paths::file_key(path).as_str()))
        })
        && normal
}

#[cfg(test)]
mod source_snapshot_tests {
    use super::*;
    use ktcore::index::{Index, Tier};
    use ktcore::project_model::{FileOrigin, ProjectScope, SourceRepresentation};

    fn inventory_page_fixture(count: usize) -> (Index, ExtensionStore, Evaluation, Vec<String>) {
        use crate::classpath::{CompilerTask, ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
        let root = std::path::PathBuf::from(if cfg!(windows) {"C:/source-inventory-fixture"} else {"/source-inventory-fixture"})
            .join("descriptor-path-padding".repeat(8));
        let files: Vec<_> = (0..count).map(|n|crate::paths::file_key(&root.join(format!("Source{n:04}.java")))).collect();
        let task: CompilerTask = serde_json::from_value(json!({
            "task":":inventory:compileJava","language":"java",
            "task_type":"org.gradle.api.tasks.compile.JavaCompile",
            "arguments":[],"annotation_processor_path":[],"source_files":files,
            "options":{"sdkProducerClaims":{"adapter":"gradle-producing-source-trees-v1",
                "language":"java","task":":inventory:compileJava","roots":[root],"complete":true}},
            "output_roots":[],"complete":true,"problem":null
        })).unwrap();
        let project = Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
            module:":inventory".into(),build:":".into(),project_dir:root.clone(),
            source_sets:vec![SourceSetClasspath { name:"main".into(),source_roots:vec![root],
                compiler_tasks:vec![task], ..Default::default() }],entries:Vec::new(),unresolved:Vec::new(),
        }]));
        let mut index=Index::new();let mut store=ExtensionStore::default();
        store.configure_provider("pages",true,Arc::from("{}"));
        for file in &files {
            index.replace_file(file,Vec::new(),Tier::Volatile);
            index.set_file_origin(file,project.file_origin(std::path::Path::new(file)).unwrap());
            store.replace_source(&index,file,Arc::from("// accepted empty source"),Knowledge::Found(()),Vec::new()).unwrap();
        }
        let mut evaluation=Evaluation::new("pages".into(),files[0].clone(),1);
        evaluation.project=Some(project);evaluation.project_operations=SOURCE_INVENTORY|SOURCE_SNAPSHOT;
        (index,store,evaluation,files)
    }

    #[test]
    fn large_source_inventory_pages_preserve_all_sources_and_snapshot_batch_limits() {
        // Real Gradle :dependency-management/main has 1257 files and >=808251 descriptor bytes.
        let (index,mut store,mut evaluation,files)=inventory_page_fixture(1257);
        let mut read=store.begin(&index,"pages",&files[0]).unwrap();
        let sources=read.source_inventory().unwrap();
        let whole:Vec<_>=sources.iter().enumerate().map(|(n,source)|evaluation.source_descriptor(source,&format!("{}:{n}",evaluation.id))).collect();
        assert!(serde_json::to_vec(&json!({"sources":whole,"coverage":missing_fact(),"next":null})).unwrap().len()>MAX_FRAME/2);
        let mut after=Value::Null;let mut ids=Vec::new();let mut paths=Vec::new();let mut pages=0;
        loop {
            let value=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),
                "queries":[{"op":"sourceInventory","after":after}]})).unwrap();
            let page=&value["facts"][0];pages+=1;
            assert!(serde_json::to_vec(page).unwrap().len()+1<=MAX_FRAME/2);
            for entry in page["sources"].as_array().unwrap() {
                ids.push(entry["id"].as_str().unwrap().to_string());paths.push(entry["displayPath"].as_str().unwrap().to_string());
            }
            after=page["next"].clone();
            if after.is_null() { assert_eq!(page["coverage"]["state"],"found");break; }
            assert_eq!(page["coverage"]["state"],"unknown");
            let pending=read.finish();read=store.resume(&index,&files[0],pending).unwrap();
        }
        assert_eq!(pages,2);assert_eq!(paths,files);
        assert_eq!(ids.iter().collect::<std::collections::BTreeSet<_>>().len(),files.len());
        assert_eq!(evaluation.handles.len(),files.len()+pages-1);
        for chunk in ids.chunks(MAX_BATCH) {
            let queries:Vec<_>=chunk.iter().map(|id|json!({"op":"sourceSnapshot","source":id})).collect();
            let value=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":queries})).unwrap();
            assert!(value["facts"].as_array().unwrap().iter().all(|fact|fact["value"]["text"]=="// accepted empty source"));
        }
        assert_eq!(evaluation.batches,pages+files.len().div_ceil(MAX_BATCH));assert!(evaluation.batches<64);
    }

    #[test]
    fn inventory_cursors_reject_foreign_sessions_and_membership_changes_without_false_absence() {
        for mutation in ["add-empty","remove-empty","body","foreign-scope"] {
            let (mut index,mut store,mut evaluation,files)=inventory_page_fixture(1257);
            let mut read=store.begin(&index,"pages",&files[0]).unwrap();
            let first=evaluation.source_inventory(&mut read,None,MAX_FRAME/2-1).unwrap();
            let cursor=first["next"].as_str().unwrap().to_string();
            assert_eq!(first["coverage"]["state"],"unknown");
            assert!(evaluation.source_inventory(&mut read,Some("foreign-cursor"),MAX_FRAME/2-1).is_err());
            let source=first["sources"][0]["id"].as_str().unwrap();
            assert!(evaluation.source_inventory(&mut read,Some(source),MAX_FRAME/2-1).is_err());
            let pending=read.finish();
            match mutation {
                "add-empty" => {
                    let extra=files[0].replace("Source0000.java","NewEmpty.java");
                    index.replace_file(&extra,Vec::new(),Tier::Volatile);
                    index.set_file_origin(&extra,index.file_origin(&files[0]).unwrap().clone());
                    store.replace_source(&index,&extra,Arc::from(""),Knowledge::Found(()),Vec::new()).unwrap();
                }
                "remove-empty" => store.remove_source(&files[1]),
                "body" => { store.replace_source(&index,&files[1],Arc::from("// edited"),Knowledge::Found(()),Vec::new()).unwrap(); }
                "foreign-scope" => {
                    let mut origin=index.file_origin(&files[0]).unwrap().clone();origin.project_scope.as_mut().unwrap().module="foreign".into();
                    index.set_file_origin(&files[0],origin);
                    store.replace_source(&index,&files[0],Arc::from("// moved"),Knowledge::Found(()),Vec::new()).unwrap();
                }
                _ => unreachable!(),
            }
            assert!(store.resume(&index,&files[0],pending).is_err(),"{mutation}");
            let mut fresh=store.begin(&index,"pages",&files[0]).unwrap();
            assert!(evaluation.source_inventory(&mut fresh,Some(&cursor),MAX_FRAME/2-1).is_err(),"{mutation}");
        }
    }

    #[test]
    fn inventory_final_page_preserves_unknown_coverage_and_handle_limits() {
        let (mut index,mut store,mut evaluation,files)=inventory_page_fixture(1);
        evaluation.project=None;
        let mut read=store.begin(&index,"pages",&files[0]).unwrap();
        let response=evaluation.source_inventory(&mut read,None,MAX_FRAME/2-1).unwrap();
        assert!(response["next"].is_null());assert_eq!(response["sources"].as_array().unwrap().len(),1);
        assert_eq!(response["coverage"]["state"],"unknown");
        assert!(evaluation.source_inventory(&mut read,None,8).is_err());
        while evaluation.handles.len()<MAX_HANDLES { evaluation.issue(Handle::Source(read.source_inventory().unwrap()[0].clone())); }
        assert!(evaluation.source_inventory(&mut read,None,MAX_FRAME/2-1).unwrap_err().contains("handle limit"));
        drop(read);
        let empty_owner=files[0].replace(".java",".txt");
        index.replace_file(&empty_owner,Vec::new(),Tier::Volatile);
        index.set_file_origin(&empty_owner,index.file_origin(&files[0]).unwrap().clone());
        store.replace_source(&index,&empty_owner,Arc::from(""),Knowledge::Found(()),Vec::new()).unwrap();
        store.remove_source(&files[0]);
        let mut empty=store.begin(&index,"pages",&empty_owner).unwrap();
        let mut evaluation=Evaluation::new("pages".into(),empty_owner.clone(),1);
        let response=evaluation.source_inventory(&mut empty,None,MAX_FRAME/2-1).unwrap();
        assert_eq!(response["sources"],json!([]));assert!(response["next"].is_null());
        assert_eq!(response["coverage"]["state"],"unknown");
    }

    #[test]
    fn source_snapshot_host_batch_budget_allows_split_retry_and_publication() {
        let mut index = Index::new();
        let mut host = crate::analysis_extensions::AnalysisExtensions::new(Vec::new());
        host.register_ipc(&index,"source-test",vec!["fixture-child".into()]).unwrap();
        let file="/ipc/Owner.kt";
        let text:Arc<str>=Arc::from(format!("// {}\nclass Owner {{ fun value(): Int = 1 }}", "x".repeat(96*1024)));
        let tree=ktcore::parser::KotlinParser::new().parse(&text);
        index.replace_file(file,ktcore::indexer::extract_symbols(&tree,&text,""),Tier::Volatile);
        index.set_file_origin(file,FileOrigin { origin_id:"module".into(),representation:SourceRepresentation::Source,
            project_scope:Some(ProjectScope { module:"module".into(),source_set:"main".into() }) });
        host.replace_sources(&index,vec![(file.into(),text.clone(),tree)]);
        let work=host.take_ipc_work(&index,None).remove(0);
        work.worker.project_operations.store(SOURCE_INVENTORY|SOURCE_SNAPSHOT,Ordering::Release);
        let id=work.evaluation.to_string();
        let inventory=host.ipc_batch(&index,work.evaluation,"analysis/readFacts",
            json!({"evaluation":id,"queries":[{"op":"sourceInventory"}]})).unwrap();
        let query=json!({"op":"sourceSnapshot","source":inventory["facts"][0]["sources"][0]["id"]});
        let error=host.ipc_batch(&index,work.evaluation,"analysis/readFacts",
            json!({"evaluation":id,"queries":vec![query.clone();8]})).unwrap_err();
        assert_eq!(error,READ_FACTS_RESPONSE_BUDGET_EXCEEDED);
        assert!(host.ipc_current(work.evaluation));
        assert!(host.ipc_completion_pending(file));
        let smaller=host.ipc_batch(&index,work.evaluation,"analysis/readFacts",
            json!({"evaluation":id,"queries":[query]})).unwrap();
        assert_eq!(smaller["facts"][0]["value"]["text"],text.as_ref());
        host.ipc_batch(&index,work.evaluation,"analysis/finish",
            json!({"evaluation":id,"contributions":{"sites":[],"diagnostics":[]}})).unwrap();
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_some());
    }

    #[test]
    fn source_snapshot_fixed_frames_match_the_actual_serializer() {
        let mut index = Index::new();
        let mut store = ExtensionStore::default();
        store.configure_provider("source-test",true,Arc::from("{}"));
        for (file,text) in [("source-fixture/Owner.kt","// Kotlin current\n"),
            ("source-fixture/Empty.java","// 未保存 Java 😀\n")] {
            index.replace_file(file,Vec::new(),Tier::Volatile);
            index.set_file_origin(file,FileOrigin { origin_id:"module".into(),representation:SourceRepresentation::Source,
                project_scope:Some(ProjectScope { module:"module".into(),source_set:"main".into() }) });
            store.replace_source(&index,file,Arc::from(text),Knowledge::Found(()),Vec::new()).unwrap();
        }
        let mut read = store.begin(&index,"source-test","source-fixture/Owner.kt").unwrap();
        let mut evaluation = Evaluation::new("source-test".into(),"source-fixture/Owner.kt".into(),1);
        evaluation.id=42;
        evaluation.project_operations=SOURCE_INVENTORY|SOURCE_SNAPSHOT;
        let inventory=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":"42","queries":[{"op":"sourceInventory"}]})).unwrap();
        let token=inventory["facts"][0]["sources"][0]["id"].clone();
        let snapshot=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":"42","queries":[{"op":"sourceSnapshot","source":token}]})).unwrap();
        let mut actual=Vec::new();
        write_frame(&mut actual,&json!({"jsonrpc":"2.0","id":7,"result":inventory})).unwrap();
        write_frame(&mut actual,&json!({"jsonrpc":"2.0","id":8,"result":snapshot})).unwrap();
        let fixture=include_bytes!("../../../dev/external-extension/source-snapshots-v1.frames");
        assert_eq!(actual.as_slice(),fixture.as_slice());
        let mut input=std::io::Cursor::new(fixture);
        assert_eq!(read_frame(&mut input).unwrap().unwrap()["result"]["facts"][0]["sources"][0]["identity"]["revision"],"3");
        assert_eq!(read_frame(&mut input).unwrap().unwrap()["result"]["facts"][0]["value"]["text"],"// 未保存 Java 😀\n");
        assert!(read_frame(&mut input).unwrap().is_none());
    }

    #[test]
    fn source_inventory_coverage_rejects_unknown_roles_and_missing_generator_inputs() {
        use crate::classpath::{CompilerLanguage, CompilerTask, SourceSetClasspath};
        let root = std::env::current_dir().unwrap().join("source-fixture");
        let owner = crate::paths::file_key(&root.join("Owner.kt"));
        let java = crate::paths::file_key(&root.join("Empty.java"));
        let task: CompilerTask = serde_json::from_value(json!({
            "task":":compileJava","language":"java",
            "task_type":"org.gradle.api.tasks.compile.JavaCompile",
            "arguments":[],"annotation_processor_path":[],"source_files":[owner,java],
            "options":{"sdkProducerClaims":{"adapter":"gradle-producing-source-trees-v1",
                "language":"java","task":":compileJava","roots":[root],"complete":true}},
            "output_roots":[],"complete":true,"problem":null
        })).unwrap();
        let mut source = SourceSetClasspath { compiler_tasks:vec![task.clone()], ..Default::default() };
        let accepted = [owner.as_str(),java.as_str()].into_iter().collect();
        assert!(source_inventory_complete(&source,&accepted));
        let mut unknown = task;
        unknown.task = ":customKotlin".into();
        unknown.language = CompilerLanguage::Kotlin;
        unknown.task_type = "custom.KotlinCompile".into();
        unknown.source_files = vec![root.join("Missing.kt")];
        source.compiler_tasks.push(unknown);
        assert!(!source_inventory_complete(&source,&accepted));
        source.compiler_tasks[1].task_type = "com.google.devtools.ksp.gradle.KspTaskJvm".into();
        assert!(!source_inventory_complete(&source,&accepted),"nonexecuted generator inputs cannot disappear from coverage");
        source.compiler_tasks[1].source_files = vec![owner.clone().into()];
        assert!(source_inventory_complete(&source,&accepted));
        source.compiler_metadata_incomplete = true;
        assert!(!source_inventory_complete(&source,&accepted));
    }

    #[test]
    fn source_snapshot_batch_budget_counts_escaped_bytes_and_allows_split_retry() {
        let mut index = Index::new();
        let mut store = ExtensionStore::default();
        store.configure_provider("source-test",true,Arc::from("{}"));
        let file = "/sources/Large.kt";
        index.replace_file(file,Vec::new(),Tier::Volatile);
        index.set_file_origin(file,FileOrigin { origin_id:"module".into(),representation:SourceRepresentation::Source,
            project_scope:Some(ProjectScope { module:"module".into(),source_set:"main".into() }) });
        store.replace_source(&index,file,Arc::from("\u{1}".repeat(24*1024)),Knowledge::Found(()),Vec::new()).unwrap();
        let mut read = store.begin(&index,"source-test",file).unwrap();
        let mut evaluation = Evaluation::new("source-test".into(),file.into(),1);
        evaluation.project_operations = SOURCE_INVENTORY | SOURCE_SNAPSHOT;
        let inventory = evaluation.source_inventory(&mut read, None, MAX_FRAME / 2 - 1).unwrap();
        let token = inventory["sources"][0]["id"].clone();
        let query = json!({"op":"sourceSnapshot","source":token});
        let params = |queries:Vec<Value>| json!({"evaluation":evaluation.id.to_string(),"queries":queries});
        let oversized = params(vec![query.clone();8]);
        assert!(evaluation.batch(&mut read,"analysis/readFacts",oversized).unwrap_err().contains("split queries"));
        let smaller = json!({"evaluation":evaluation.id.to_string(),"queries":[query]});
        let response = evaluation.batch(&mut read,"analysis/readFacts",smaller).unwrap();
        assert_eq!(response["facts"][0]["state"],"found");
        let mut bytes = Vec::new();
        write_frame(&mut bytes,&json!({"jsonrpc":"2.0","id":2,"result":response})).unwrap();
        assert!(bytes.len()<MAX_FRAME);
    }

    #[test]
    fn source_snapshot_operations_preserve_scoped_unsaved_text_and_negative_reads() {
        let mut index = Index::new();
        let mut store = ExtensionStore::default();
        store.configure_provider("source-test", true, Arc::from("{}"));
        let scope = ProjectScope { module:"source-module".into(), source_set:"main".into() };
        for (file,text,module) in [
            ("/sources/Owner.kt","// Kotlin current\n","source-module"),
            ("/sources/Empty.java","// 未保存 Java 😀\n","source-module"),
            ("/foreign/Hidden.kt","// must not leak\n","other-module"),
        ] {
            index.replace_file(file, Vec::new(), Tier::Volatile);
            index.set_file_origin(file, FileOrigin { origin_id:module.into(), representation:SourceRepresentation::Source,
                project_scope:Some(ProjectScope { module:module.into(), source_set:"main".into() }) });
            store.replace_source(&index,file,Arc::from(text),Knowledge::Found(()),Vec::new()).unwrap();
        }
        let mut read = store.begin(&index,"source-test","/sources/Owner.kt").unwrap();
        let mut evaluation = Evaluation::new("source-test".into(),"/sources/Owner.kt".into(),1);
        evaluation.id = 42;
        evaluation.project_operations = SOURCE_INVENTORY | SOURCE_SNAPSHOT;
        let inventory = evaluation.batch(&mut read,"analysis/readFacts",
            json!({"evaluation":"42","queries":[{"op":"sourceInventory"}]})).unwrap();
        let sources = inventory["facts"][0]["sources"].as_array().unwrap();
        assert_eq!(sources.len(),2);
        assert_eq!(sources[0]["displayPath"],"/sources/Empty.java");
        assert_eq!(sources[0]["languageId"],"java");
        assert_eq!(sources[1]["languageId"],"kotlin");
        assert_eq!(inventory["facts"][0]["coverage"]["state"],"unknown",
            "no complete project inventory is inferred from a partial index");
        let token = sources[0]["id"].as_str().unwrap();
        let snapshot = evaluation.batch(&mut read,"analysis/readFacts",
            json!({"evaluation":"42","queries":[{"op":"sourceSnapshot","source":token}]})).unwrap();
        assert_eq!(snapshot["facts"][0]["state"],"found");
        assert_eq!(snapshot["facts"][0]["value"]["text"],"// 未保存 Java 😀\n");
        assert_eq!(snapshot["facts"][0]["value"]["id"],token);
        assert_eq!(snapshot["facts"][0]["value"]["identity"],sources[0]["identity"]);
        assert!(evaluation.batch(&mut read,"analysis/readFacts",
            json!({"evaluation":"42","queries":[{"op":"sourceSnapshot","source":"/foreign/Hidden.kt"}]})).is_err(),
            "a display path cannot become a filesystem or foreign-scope grant");
        let pending = read.finish();
        index.replace_file("/sources/Added.java",Vec::new(),Tier::Volatile);
        index.set_file_origin("/sources/Added.java",FileOrigin { origin_id:scope.module.clone(),
            representation:SourceRepresentation::Source,project_scope:Some(scope) });
        store.replace_source(&index,"/sources/Added.java",Arc::from("// no declarations"),Knowledge::Found(()),Vec::new()).unwrap();
        assert!(matches!(store.validate_pending(&pending),Err(ContractError::Stale)));
        assert_eq!(project_operations(&json!({"capabilities":[SOURCE_SNAPSHOTS],
            "supportedOps":{SOURCE_SNAPSHOTS:["sourceInventory","sourceSnapshot"],RESOURCES:["resourceInventory","resourceLookup","resourceSnapshot"],RUNTIME_CLASSES:["runtimeClass","classBytes"]}})),SOURCE_INVENTORY | SOURCE_SNAPSHOT);
        assert_eq!(project_operations(&json!({"capabilities":["semantic-v1"],
            "supportedOps":{SOURCE_SNAPSHOTS:["sourceInventory"]}})),0);
    }
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Names {
    Exact { values: Vec<String> },
    Prefix { value: String },
}
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Link {
    Direct,
    Prefixed { prefix: String },
    IndependentAlias,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Requirement {
    target: Value,
    authored: Option<String>,
    names: Names,
    kind: String,
    signature: Value,
    dispatch: Value,
    name_link: Link,
}
#[derive(Deserialize)]
struct ByteRange {
    start: usize,
    end: usize,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Site {
    requirement: String,
    full_range: ByteRange,
    editable_name: ByteRange,
    rename_subject: String,
}
#[derive(Deserialize)]
struct WireDiagnostic {
    range: ByteRange,
    severity: String,
    code: Option<String>,
    message: String,
}
#[derive(Deserialize)]
struct Contributions {
    #[serde(default)]
    sites: Vec<Site>,
    #[serde(default)]
    diagnostics: Vec<WireDiagnostic>,
}

pub(crate) struct Work {
    pub command: Option<String>,
    pub evaluation: u64,
    pub provider: String,
    pub worker: Arc<Worker>,
    pub params: Value,
}

pub(crate) struct Worker {
    pub id: u64,
    command: Vec<String>,
    stopped: AtomicBool,
    failed: Arc<AtomicBool>,
    project_operations: AtomicU16,
    commands: std::collections::BTreeSet<String>,
    negotiated_commands: Mutex<std::collections::BTreeSet<String>>,
    transport: Mutex<Option<Transport>>,
}

impl Worker {
    pub fn new(command: Vec<String>) -> Arc<Self> {
        Self::with_commands(command, std::collections::BTreeSet::new())
    }
    pub(crate) fn with_commands(command: Vec<String>, commands: std::collections::BTreeSet<String>) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_EVALUATION.fetch_add(1, Ordering::Relaxed),
            command,
            stopped: AtomicBool::new(false),
            failed: Arc::new(AtomicBool::new(false)),
            project_operations: AtomicU16::new(0),
            commands,
            negotiated_commands: Mutex::new(std::collections::BTreeSet::new()),
            transport: Mutex::new(None),
        })
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub(crate) fn project_operations(&self) -> u16 {
        self.project_operations.load(Ordering::Acquire)
    }
    pub(crate) fn command_acknowledged(&self, command: &str) -> bool {
        self.negotiated_commands.lock().unwrap().contains(command)
    }
    pub fn close(&self) {
        self.stop();
        drop(self.transport.lock().unwrap().take());
    }
    pub fn run(
        &self,
        work: &Work,
        workspace: &Arc<parking_lot::Mutex<crate::workspace::Workspace>>,
        after_batch: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), String> {
        self.run_request(work, workspace, after_batch, &AtomicBool::new(false), true).map(|_| ())
    }
    pub(crate) fn run_command(
        &self, work: &Work,
        workspace: &Arc<parking_lot::Mutex<crate::workspace::Workspace>>,
        after_batch: Arc<dyn Fn() + Send + Sync>, cancelled: &AtomicBool, dispatch: bool,
    ) -> Result<Value, String> {
        self.run_request(work, workspace, after_batch, cancelled, dispatch)
    }
    fn run_request(
        &self, work: &Work,
        workspace: &Arc<parking_lot::Mutex<crate::workspace::Workspace>>,
        after_batch: Arc<dyn Fn() + Send + Sync>, cancellation: &AtomicBool, dispatch: bool,
    ) -> Result<Value, String> {
        let queue_deadline = Instant::now() + EVALUATION_LIMIT;
        let mut transport = if work.command.is_some() {
            loop {
                if cancellation.load(Ordering::Acquire) { return Err("Cancelled".into()); }
                if self.stopped.load(Ordering::Acquire) || !workspace.lock().ipc_evaluation_current(work.evaluation) { return Err("Stale".into()); }
                match self.transport.try_lock() {
                    Ok(guard) => break guard,
                    Err(std::sync::TryLockError::WouldBlock) if Instant::now() < queue_deadline => std::thread::sleep(Duration::from_millis(10)),
                    Err(std::sync::TryLockError::WouldBlock) => return Err("CommandUnavailable".into()),
                    Err(_) => return Err("extension transport poisoned".into()),
                }
            }
        } else { self.transport.lock().unwrap() };
        if self.stopped.load(Ordering::Acquire) {
            return Err("extension stopped".into());
        }
        let result = (|| {
            if transport.is_none() {
                *transport = Some(Transport::spawn(
                    &self.command,
                    self.failed.clone(),
                    after_batch.clone(),
                )?);
            }
            let transport = transport.as_mut().unwrap();
            if !transport.initialized {
                transport.send(initialize_message(&work.provider,&self.commands))?;
                let response =
                    transport
                        .receive(Instant::now() + EVALUATION_LIMIT, &self.stopped, || Ok(()))?;
                if response["id"] != "initialize"
                    || response["result"]["protocol"]["major"] != PROTOCOL_MAJOR
                    || !response["result"]["capabilities"]
                        .as_array()
                        .is_some_and(|values| values.iter().any(|value| value == "semantic-v1"))
                {
                    return Err("incompatible extension protocol".into());
                }
                self.project_operations.store(project_operations(&response["result"]), Ordering::Release);
                let mut negotiated = self.negotiated_commands.lock().unwrap();
                negotiated.clear();
                if response["result"]["capabilities"].as_array().is_some_and(|values|values.iter().any(|value|value=="provider-commands-v1")) {
                    let commands = response["result"]["commands"].as_array().ok_or("missing provider command acknowledgement")?;
                    if commands.len()>64 { return Err("provider command acknowledgement limit".into()); }
                    for command in commands {
                        let command=command.as_str().filter(|id|valid_command_id(id)).ok_or("invalid provider command acknowledgement")?;
                        if self.commands.contains(command) { negotiated.insert(command.to_owned()); }
                    }
                }
                transport.initialized = true;
            }
            if let Some(command) = &work.command {
                if !self.command_acknowledged(command) { return Err("CommandUnavailable".into()); }
                if cancellation.load(Ordering::Acquire) { return Err("Cancelled".into()); }
                if !workspace.lock().ipc_evaluation_current(work.evaluation) { return Err("Stale".into()); }
                if !dispatch { return Ok(Value::Null); }
            }
            if work.command.is_none() && self.project_operations() & (SOURCE_INVENTORY | SOURCE_SNAPSHOT) != 0 {
                let request = workspace.lock().begin_ipc_source_capture(
                    work.evaluation, work.params["owner"].as_str().ok_or("missing source owner")?,
                );
                if let Some(request) = request {
                    let prepared = crate::workspace::prepare_ipc_source_capture(request);
                    if workspace.lock().finish_ipc_source_capture(prepared) > 0 {
                        after_batch();
                        return Err("Stale".into());
                    }
                }
            }
            if self.project_operations()&(RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT)!=0 {
                let plan=workspace.lock().begin_ipc_resource_capture(work.evaluation);
                if let Some(plan)=plan {
                    let prepared=crate::resource_inputs::prepare_resource_catalog(plan,after_batch.clone());
                    workspace.lock().finish_ipc_resource_capture(prepared)?;
                }
            }
            let id = work.evaluation.to_string();
            transport.send(
                work_message(work),
            )?;
            let deadline = Instant::now() + EVALUATION_LIMIT;
            let mut cancelled = false;
            loop {
                let message = transport.receive(deadline, &self.stopped, || {
                    if !cancelled && (cancellation.load(Ordering::Acquire) || !workspace.lock().ipc_evaluation_current(work.evaluation)) {
                        cancelled = true;
                        transport.send(
                            json!({"jsonrpc":"2.0","method":"$/cancelRequest","params":{"id":id}}),
                        )?;
                    }
                    Ok(())
                })?;
                if message["jsonrpc"] != "2.0" {
                    return Err("invalid extension JSON-RPC version".into());
                }
                if let Some(method) = message["method"].as_str() {
                    let Some(request_id) = message.get("id") else {
                        continue;
                    };
                    let prepared_inputs=(||->Result<(),String> {
                        let (reads,class)={
                            let mut workspace=workspace.lock();
                            workspace.prepay_ipc_input_read(work.evaluation,method,&message["params"])?;
                            let reads=workspace.begin_ipc_resource_reads(work.evaluation,method,&message["params"])?;
                            let class=workspace.begin_ipc_class_reads(work.evaluation,method,&message["params"])?;
                            (reads,class)
                        };
                        let mut budget=64usize*1024*1024;
                        for mut read in reads {
                            read.byte_budget=read.byte_budget.min(budget);
                            let prepared=crate::resource_inputs::prepare_resource_read(read);
                            if let Ok(bytes)=&prepared.content {budget=budget.saturating_sub(bytes.bytes.len());}
                            workspace.lock().finish_ipc_resource_read(prepared)?;
                        }
                        if let Some(class)=class {
                            let prepared=crate::runtime_bytes::prepare_class_read(class,after_batch.clone());
                            workspace.lock().finish_ipc_class_read(prepared)?;
                        }
                        Ok(())
                    })();
                    if let Err(error)=&prepared_inputs {workspace.lock().fail_ipc_input_read(work.evaluation,error);}
                    let result = prepared_inputs.and_then(|_|workspace.lock().ipc_batch(
                        work.evaluation,
                        method,
                        message["params"].clone(),
                    ));
                    after_batch();
                    let response = match result {
                        Ok(value) => json!({"jsonrpc":"2.0","id":request_id,"result":value}),
                        Err(error) => {
                            json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32602,"message":error}})
                        }
                    };
                    workspace.lock().account_ipc_stream_reply(work.evaluation,&response)?;
                    transport.send(response)?;
                } else if message["id"] == id {
                    if work.command.is_some() {
                        if cancellation.load(Ordering::Acquire) { return Err("Cancelled".into()); }
                        if cancelled || !workspace.lock().ipc_evaluation_current(work.evaluation) { return Err("Stale".into()); }
                        if let Some(error) = message.get("error") {
                            return Err(match error["code"].as_i64() {Some(-32800)=>"Cancelled",Some(-32801)=>"Stale",_=>"CommandFailed"}.into());
                        }
                        let value=message.get("result").cloned().ok_or("missing provider command result")?;
                        if serde_json::to_vec(&value).map_err(|error|error.to_string())?.len()>MAX_FRAME/2 { return Err("provider command result limit".into()); }
                        return Ok(value);
                    }
                    if message.get("error").is_some() {
                        return Err(format!("extension evaluation failed: {}", message["error"]));
                    }
                    return workspace
                        .lock()
                        .ipc_batch(
                            work.evaluation,
                            "analysis/finish",
                            json!({"evaluation":id,"contributions":message["result"]}),
                        )
                        ;
                } else {
                    return Err("unexpected extension response id".into());
                }
            }
        })();
        if result.as_ref().is_err_and(|error| !matches!(error.as_str(), "Stale" | "Cancelled" | "CommandUnavailable" | "CommandFailed")) {
            drop(transport.take());
        }
        result
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        // The final registration can be dropped under Workspace access; process cleanup cannot.
        if let Some(transport) = self.transport.get_mut().unwrap().take() {
            std::thread::spawn(move || drop(transport));
        }
    }
}

struct Transport {
    #[cfg(windows)]
    tree: crate::compile::process_tree::ProcessTree,
    #[cfg(not(windows))]
    child: std::process::Child,
    sender: Option<mpsc::SyncSender<Value>>,
    receiver: mpsc::Receiver<Result<Value, String>>,
    initialized: bool,
}

impl Transport {
    fn spawn(
        argv: &[String],
        failed: Arc<AtomicBool>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, String> {
        let executable = argv
            .first()
            .filter(|value| !value.is_empty())
            .ok_or("missing extension executable")?;
        let mut command = Command::new(executable);
        command
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        let mut tree = crate::compile::process_tree::ProcessTree::spawn(&mut command)
            .map_err(|e| e.to_string())?;
        #[cfg(windows)]
        let child = tree.child_mut();
        #[cfg(not(windows))]
        let mut child = command.spawn().map_err(|e| e.to_string())?;
        let input = child.stdin.take().ok_or("extension stdin unavailable")?;
        let output = child.stdout.take().ok_or("extension stdout unavailable")?;
        let error = child.stderr.take().ok_or("extension stderr unavailable")?;
        let (sender, writes) = mpsc::sync_channel::<Value>(8);
        std::thread::spawn(move || write_messages(input, writes));
        let (messages, receiver) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let result = match read_frame(&mut output) {
                    Ok(Some(value)) => Ok(value),
                    Ok(None) => Err("extension stdout closed".into()),
                    Err(error) => Err(error.to_string()),
                };
                let terminal = result.is_err();
                let queue_failed = messages.try_send(result).is_err();
                if terminal || queue_failed {
                    failed.store(true, Ordering::Release);
                    wake();
                    break;
                }
            }
        });
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut BufReader::new(error), &mut std::io::sink());
        });
        Ok(Self {
            #[cfg(windows)]
            tree,
            #[cfg(not(windows))]
            child,
            sender: Some(sender),
            receiver,
            initialized: false,
        })
    }
    fn send(&self, message: Value) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or("extension writer closed")?
            .try_send(message)
            .map_err(|e| e.to_string())
    }
    fn receive(
        &self,
        deadline: Instant,
        stopped: &AtomicBool,
        mut idle: impl FnMut() -> Result<(), String>,
    ) -> Result<Value, String> {
        loop {
            if stopped.load(Ordering::Acquire) {
                return Err("extension stopped".into());
            }
            idle()?;
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or("extension evaluation timed out")?;
            match self
                .receiver
                .recv_timeout(remaining.min(Duration::from_millis(50)))
            {
                Ok(value) => return value,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

fn write_messages(mut input: ChildStdin, messages: mpsc::Receiver<Value>) {
    for message in messages {
        if write_frame(&mut input, &message).is_err() {
            break;
        }
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.sender.take();
        #[cfg(windows)]
        let _ = self.tree.cleanup(None);
        #[cfg(not(windows))]
        {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_project_operations_require_both_capability_and_selected_operations() {
        assert_eq!(project_operations(&json!({"capabilities":["semantic-v1"]})), 0);
        assert_eq!(project_operations(&json!({"capabilities":[PROJECT_FACTS]})), 0);
        assert_eq!(project_operations(&json!({"supportedOps":{PROJECT_FACTS:["typeJvm"]}})), 0);
        let selected = json!({"capabilities":["semantic-v1",PROJECT_FACTS],
            "supportedOps":{PROJECT_FACTS:["projectContext","typeJvm","unsupported"]}});
        assert_eq!(project_operations(&selected), PROJECT_CONTEXT | TYPE_JVM);
        let first = Worker::new(vec!["first".into()]);
        let second = Worker::new(vec!["second".into()]);
        first.project_operations.store(project_operations(&selected), Ordering::Release);
        assert_eq!(first.project_operations(), PROJECT_CONTEXT | TYPE_JVM);
        assert_eq!(second.project_operations(), 0);
    }

    #[test]
    fn type_jvm_wire_keeps_generated_metadata_and_encodes_only_class_content_digest() {
        use sha2::{Digest, Sha256};
        const BYTES: &[u8] = include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Model$$serializer.class");
        let class = ktcore::classfile::decode("selected", BYTES).unwrap();
        let value = class_abi(&class).unwrap();
        assert_eq!(value["identity"]["contentSha256"], format!("{:x}", Sha256::digest(BYTES)));
        assert_eq!(value["methods"].as_array().unwrap().len(), class.methods.len());
        assert_eq!(value["fields"].as_array().unwrap().len(), class.fields.len());
        for (actual, expected) in value["methods"].as_array().unwrap().iter().zip(&class.methods) {
            assert_eq!(actual, &class_member(expected));
            assert!(actual.get("id").is_none(), "raw methods have no invented editing identity");
        }
        assert!(value["methods"].as_array().unwrap().iter().any(|method|
            method["name"] == "deserialize" && method["accessFlags"] == 0x1041));
        assert!(value["methods"].as_array().unwrap().iter().any(|method|
            method["name"] == "deserialize" && method["accessFlags"] == 0x0001));
        let mut oversized = class.clone();
        oversized.attribute_names.push(Arc::from("x".repeat(MAX_FRAME)));
        assert!(class_abi(&oversized).is_none());
        let mut source = class;
        source.identity.input = ktcore::jvm::InputIdentity::Source { file: "Model.kt".into(), revision: 1 };
        assert!(class_abi(&source).is_none());
    }

    #[test]
    fn declaration_member_reads_exact_overload_and_rejects_foreign_tokens() {
        use ktcore::index::{Index, Tier};
        let file = "/wire/Owner.kt";
        let text = "package wire\nclass Owner { fun same(arg: Int): Int = arg; fun same(arg: String): String = arg; private var count: Int = 0 }";
        let tree = ktcore::parser::KotlinParser::new().parse(text);
        assert!(!tree.root_node().has_error());
        let mut index = Index::new();
        index.replace_file(
            file,
            ktcore::indexer::extract_symbols(&tree, text, "wire"),
            Tier::Volatile,
        );
        let scope = ktcore::indexer::TypeScope::with_default_imports(
            "wire",
            Vec::new(),
            &["kotlin", "java.lang"],
        );
        let declarations = ktcore::extensions::source::extract(&index, file, &tree, text, &scope);
        let mut store = ExtensionStore::default();
        store.configure_provider("wire", true, Arc::from("{}"));
        store
            .replace_source_declarations(
                &index,
                file,
                Arc::from(text),
                Knowledge::Found(()),
                declarations,
            )
            .unwrap();
        let mut read = store.begin(&index, "wire", file).unwrap();
        let mut evaluation = Evaluation::new("wire".into(), file.into(), 1);
        let source = evaluation.source(&mut read).unwrap();
        let declarations = source["declarations"].as_array().unwrap();
        let queries: Vec<_> = declarations
            .iter()
            .map(|declaration| json!({"op":"declarationMember","declaration":declaration["id"]}))
            .collect();
        let response = evaluation
            .batch(
                &mut read,
                "analysis/readFacts",
                json!({"evaluation":evaluation.id.to_string(),"queries":queries}),
            )
            .unwrap();
        let facts = response["facts"].as_array().unwrap();
        let mut signatures = std::collections::BTreeSet::new();
        for (declaration, fact) in declarations.iter().zip(facts) {
            if declaration["name"] == "Owner" {
                assert_eq!(fact["state"], "unknown");
                continue;
            }
            assert_eq!(fact["state"], "found");
            let candidate = &fact["value"];
            assert_eq!(candidate["name"], declaration["name"]);
            let Handle::Member(member) = &evaluation.handles[candidate["id"].as_str().unwrap()]
            else {
                panic!("host member token")
            };
            let identity = evaluation.declaration(&declaration["id"]).unwrap();
            assert_eq!(member.definition(), identity.definition());
            signatures.insert(
                candidate["signature"]["value"]["descriptor"]
                    .as_str()
                    .unwrap()
                    .to_string(),
            );
        }
        assert_eq!(
            signatures,
            ["(I)I", "(Ljava/lang/String;)Ljava/lang/String;", "I"]
                .into_iter()
                .map(str::to_string)
                .collect()
        );
        assert!(evaluation.batch(&mut read, "analysis/readFacts", json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"declarationMember","declaration":"foreign:0"}]})).is_err());
    }

    #[test]
    fn framing_rejects_duplicate_lengths_truncation_and_oversize() {
        for bytes in [
            b"Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}".as_slice(),
            b"Content-Length: 3\r\n\r\n{}",
            b"Content-Length: 1048577\r\n\r\n",
        ] {
            assert!(read_frame(&mut std::io::Cursor::new(bytes)).is_err());
        }
        let message = json!({"jsonrpc":"2.0","id":1,"result":"日本語"});
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &message).unwrap();
        assert_eq!(
            read_frame(&mut std::io::Cursor::new(bytes)).unwrap(),
            Some(message)
        );
    }
}

pub(crate) fn valid_command_id(value: &str) -> bool {
    !value.is_empty() && value.len()<=128 && value.bytes().all(|c|c.is_ascii_alphanumeric() || matches!(c,b'.'|b'_'|b'-'))
}

#[derive(Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub(crate) struct ProviderCommandRequest {
    pub provider: String,
    pub command: String,
    pub context: ProviderCommandContext,
    pub arguments: Vec<Value>,
    #[serde(default)]
    pub refresh_project: bool,
}
#[derive(Deserialize)]
#[serde(rename_all="camelCase")]
pub(crate) struct ProviderCommandContext {
    pub workspace_root: String,
    pub document_uri: tower_lsp_server::ls_types::Uri,
    pub buffer_text: String,
    pub document_version: Option<i64>,
}
impl ProviderCommandRequest {
    pub(crate) fn decode(arguments: &[Value]) -> Result<Self,String> {
        if arguments.len()!=1 || serde_json::to_vec(arguments).map_err(|e|e.to_string())?.len()>MAX_FRAME/8 {
            return Err("provider command request limit".into());
        }
        let request:Self=serde_json::from_value(arguments[0].clone()).map_err(|e|e.to_string())?;
        if !valid_command_id(&request.provider) || !valid_command_id(&request.command) || request.arguments.len()>MAX_BATCH {
            return Err("invalid provider command envelope".into());
        }
        Ok(request)
    }
}

#[cfg(test)]
mod provider_command_wire_tests {
    use super::*;
    #[test]
    fn provider_envelope_preserves_opaque_arguments_and_bounds_transport() {
        let original=json!(["{\"LegacyCase\":\"KeepCase\"}",{"nested":[{"CallbackName":"RunMe"}]}]);
        let envelope=json!({"provider":"test.commands","command":"fixture.echo",
            "context":{"workspaceRoot":"/workspace","documentUri":"file:///workspace/Owner.kt","bufferText":"class Owner","documentVersion":7},
            "arguments":original,"refreshProject":true});
        let decoded=ProviderCommandRequest::decode(&[envelope.clone()]).unwrap();
        assert_eq!(Value::Array(decoded.arguments),original);
        assert!(decoded.refresh_project);
        assert_eq!(decoded.context.document_version,Some(7));
        for key in ["provider","command"] {
            let mut invalid=envelope.clone();invalid[key]=json!("not/a/registered/command");
            assert!(ProviderCommandRequest::decode(&[invalid]).is_err());
        }
        assert!(ProviderCommandRequest::decode(&[envelope.clone(),envelope.clone()]).is_err());
        let mut oversized=envelope;oversized["arguments"]=json!(["x".repeat(MAX_FRAME/8)]);
        assert!(ProviderCommandRequest::decode(&[oversized]).is_err());
    }
}

fn work_message(work:&Work)->Value {
    json!({"jsonrpc":"2.0","id":work.evaluation.to_string(),
        "method":if work.command.is_some(){"analysis/executeCommand"}else{"analysis/evaluate"},"params":work.params})
}

#[cfg(test)]
mod provider_command_frame_tests {
    use super::*;
    #[test]
    fn provider_command_frame_matches_the_production_serializer() {
        let work=Work {command:Some("example.complete".into()),evaluation:7,provider:"example.provider".into(),
            worker:Worker::new(vec!["not-spawned".into()]),params:json!({"evaluation":"7","owner":"/workspace/Owner.kt",
                "source":"class Owner","configuration":{},"declarations":[],"command":"example.complete",
                "context":{"workspaceRoot":"/workspace","documentUri":"file:///workspace/Owner.kt","documentVersion":3},
                "arguments":[{"CallbackName":"KeepCase","text":"😀"}]})};
        let mut actual=Vec::new();write_frame(&mut actual,&work_message(&work)).unwrap();
        let expected=include_bytes!("../../../dev/external-extension/provider-commands-v1.frames");
        assert_eq!(actual.as_slice(),expected);
        let value=read_frame(&mut std::io::Cursor::new(expected)).unwrap().unwrap();
        assert_eq!(value["params"]["arguments"][0]["CallbackName"],"KeepCase");
        assert_eq!(value["method"],"analysis/executeCommand");
    }
}

#[cfg(test)]
mod runtime_class_tests {
    use super::*;

    #[test]
    fn runtime_class_wire_keeps_bytes_opaque_identity_and_source_authority_separate() {
        use base64::Engine as _;
        let bytes = include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Model$$serializer.class");
        let class = ktcore::classfile::decode("runtime-selected", bytes).unwrap();
        assert!(class.attribute_names.iter().any(|name|name.as_ref()=="RuntimeVisibleAnnotations"));
        assert!(class.methods.iter().any(|method|method.attribute_names.iter().any(|name|name.as_ref()=="Code")));
        assert!(bytes.windows(b"LocalVariableTable".len()).any(|name|name==b"LocalVariableTable"));
        let identity = class.identity.clone();
        let mut evaluation = Evaluation::new("runtime".into(), "/Owner.kt".into(), 1);
        evaluation.class_results.insert(identity.internal_name.clone(), crate::runtime_bytes::ClassReadResult {
            internal_name: identity.internal_name.clone(), identity: Some(identity.clone()),
            byte_length: bytes.len(), bytes: Some(Arc::from(&bytes[..])), unavailable: None,api_header:None,
        });
        let descriptor = evaluation.runtime_class(&identity.internal_name).unwrap();
        assert_eq!(descriptor["value"]["runtimeOnly"], true);
        assert_eq!(descriptor["value"]["sourceCurrent"]["state"], "unknown");
        let token = descriptor["value"]["id"].as_str().unwrap().to_owned();
        let value = evaluation.class_bytes(&token, None).unwrap();
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(value["value"]["bytesBase64"].as_str().unwrap()).unwrap(), bytes);
        assert_eq!(value["value"]["next"], Value::Null);
        let cursor = evaluation.issue(Handle::ClassBytesCursor { identity: identity.clone(), next: 1 });
        let value = evaluation.class_bytes(&token, Some(&cursor)).unwrap();
        assert_eq!(value["value"]["offset"], 1);
        assert_eq!(base64::engine::general_purpose::STANDARD.decode(value["value"]["bytesBase64"].as_str().unwrap()).unwrap(), &bytes[1..]);
        let other = Evaluation::new("runtime".into(), "/Owner.kt".into(), 1);
        assert!(other.class_identity(&token).is_err());
        let mut changed = identity.clone();
        changed.internal_name.push_str("Other");
        let foreign = evaluation.issue(Handle::ClassBytesCursor { identity: changed, next: 1 });
        assert!(evaluation.class_bytes(&token, Some(&foreign)).is_err());
        evaluation.class_results.get_mut(&identity.internal_name).unwrap().identity = None;
        assert_eq!(evaluation.class_bytes(&token, None), Err("Stale".into()));
        assert_eq!(evaluation.runtime_class(&identity.internal_name).unwrap()["state"], "unknown");
    }

    #[test]
    fn runtime_class_negotiation_requires_each_operation_and_keeps_the_ninth_bit() {
        assert_eq!(project_operations(&json!({"supportedOps":{RUNTIME_CLASSES:["runtimeClass","classBytes"]}})), 0);
        let selected = project_operations(&json!({"capabilities":[RUNTIME_CLASSES],
            "supportedOps":{RUNTIME_CLASSES:["classBytes"]}}));
        assert_eq!(selected, CLASS_BYTES);
        assert_eq!(selected & RUNTIME_CLASS, 0);
        let worker = Worker::new(vec!["not-started".into()]);
        worker.project_operations.store(selected, Ordering::Release);
        assert_eq!(worker.project_operations(), 256);
        let evaluation = Evaluation::new("runtime".into(), "/Owner.kt".into(), 1);
        let params = json!({"evaluation":evaluation.id.to_string(), "queries":[{"op":"runtimeClass","internalName":"java/util/List"}]});
        assert!(evaluation.begin_class_reads("analysis/readFacts", &params).is_err());
    }

    #[test]
    fn runtime_class_finisher_cannot_mint_positive_bytes_without_a_host_guard() {
        let index = ktcore::index::Index::new();
        let mut store = ExtensionStore::default();
        store.replace_source(&index, "/Owner.kt", Arc::from(""), Knowledge::Found(()), Vec::new()).unwrap();
        store.configure_provider("runtime", true, Arc::from("{}"));
        let mut read = store.begin(&index, "runtime", "/Owner.kt").unwrap();
        let model = Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new()));
        let mut evaluation = Evaluation::new("runtime".into(), "/Owner.kt".into(), 1);
        evaluation.project = Some(model.clone());
        let bytes = include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let class = ktcore::classfile::decode("selected", bytes).unwrap();
        let prepared = crate::runtime_bytes::PreparedClassRead { evaluation: evaluation.id, model,
            scope: None, results: Ok(vec![crate::runtime_bytes::ClassReadResult {
                internal_name: class.identity.internal_name.clone(), identity: Some(class.identity),
                byte_length: bytes.len(), bytes: Some(Arc::from(&bytes[..])), unavailable: None,api_header:None,
            }]),
        };
        assert_eq!(evaluation.finish_class_read(&mut read, prepared), Err("Stale".into()));
        assert!(evaluation.class_results.is_empty());
    }
}

#[cfg(test)]
mod resource_tests {
    #[test]
    fn resource_fourteen_mib_snapshot_after_metadata_prefix_finishes_without_raising_metadata_limit() {
        use base64::Engine;
        let scope=ProjectScope{module:"large-resource".into(),source_set:"main".into()};let owner="large-resource/Owner.kt";
        let mut index=Index::new();index.replace_file(owner,Vec::new(),Tier::Volatile);
        index.set_file_origin(owner,FileOrigin{origin_id:"large-resource".into(),representation:SourceRepresentation::Source,project_scope:Some(scope.clone())});
        let mut store=ExtensionStore::default();store.configure_provider("resources",true,Arc::from("{}"));
        store.replace_source(&index,owner,Arc::from("// owner"),Knowledge::Found(()),Vec::new()).unwrap();
        let model=Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new()));
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let mut catalog=ResourceCatalog::prepare(ResourceSelection{
            model:model.clone(),scope,claims:Some(crate::resource_model::ResourceClaims{adapter:"gradle-source-set-resources-v1".into(),roots:vec![],trees:vec![],files:vec![],
                output_roots:vec![],configuration_inputs:vec![],default_excludes:vec![],complete:true,problem:None}),compile_classpath:vec![],runtime_classpath:Some(vec![]),
            runtime_complete:true,classpath_complete:true,directory_inputs:std::collections::BTreeSet::new(),configuration_base:"large-resource".into(),overlays:BTreeMap::new(),
            guard:flag.clone(),read_coverage:crate::compiled_watch::WatchPlan::read_inputs(&[]).unwrap(),
        });
        let text:Arc<str>=Arc::from(format!("\"{}\"\n","a".repeat(14*1024*1024-3)));
        catalog.entries.push(ResourceEntry{association:ResourceAssociation::Source,order:0,relative_path:"mapping.json".into(),
            location:ResourceLocation::File("large-resource/mapping.json".into()),editable:true,
            document:Some(crate::workspace::OwnedDocumentSnapshot{text:text.clone(),document_version:Some(4),language_id:"json".into(),revision:7})});
        let mut evaluation=Evaluation::new("resources".into(),owner.into(),1);evaluation.project=Some(model);evaluation.resource_catalog=Some(Arc::new(catalog));
        evaluation.project_operations=RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT;
        let mut read=store.begin(&index,"resources",owner).unwrap();
        for _ in 0..31 {evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[]})).unwrap();}
        let inventory=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceInventory"}]})).unwrap();
        let id=inventory["facts"][0]["resources"][0]["id"].clone();let mut after=Value::Null;let mut decoded=Vec::with_capacity(text.len());let mut chunks=0;
        let expected_sha=format!("{:x}",sha2::Sha256::digest(text.as_bytes()));
        loop {
            let params=json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceSnapshot","resource":id,"after":after}]});
            evaluation.prepay_input_read("analysis/readFacts",&params).unwrap();
            for plan in evaluation.begin_resource_reads("analysis/readFacts",&params).unwrap() {
                let prepared=crate::resource_inputs::prepare_resource_read(plan);evaluation.resource_results.insert(prepared.token,prepared.content);
            }
            let response=evaluation.batch(&mut read,"analysis/readFacts",params).unwrap();
            let value=&response["facts"][0]["value"];assert_eq!(response["facts"][0]["state"],"found");assert_eq!(value["offset"],decoded.len());
            assert_eq!(value["size"],text.len());assert_eq!(value["identity"]["sha256"],expected_sha.as_str());
            decoded.extend(base64::engine::general_purpose::STANDARD.decode(value["base64"].as_str().unwrap()).unwrap());
            after=value["next"].clone();chunks+=1;
            evaluation.account_stream_reply(&json!({"jsonrpc":"2.0","id":chunks,"result":response})).unwrap();
            if after.is_null() {break;}
        }
        assert_eq!(&decoded,text.as_bytes());assert!(chunks>33,"the old metadata counter would reject this valid stream");
        assert_eq!(evaluation.batches,33);assert_eq!(evaluation.stream_continuations,chunks-1);assert!(evaluation.stream_bytes>14*1024*1024);
        let pending=read.finish();flag.0.store(true,Ordering::Release);assert!(store.validate_pending(&pending).is_err());
    }

    #[test]
    fn resource_catalog_uses_one_handle_for_large_inventory_and_owned_selectors() {
        let scope=ProjectScope{module:"resource-catalog".into(),source_set:"main".into()};
        let owner=crate::paths::file_key(&std::env::current_dir().unwrap().join("Owner.kt"));
        let mut index=Index::new();index.replace_file(&owner,Vec::new(),Tier::Volatile);
        index.set_file_origin(&owner,FileOrigin{origin_id:"resource-catalog".into(),representation:SourceRepresentation::Source,project_scope:Some(scope.clone())});
        let mut store=ExtensionStore::default();store.configure_provider("resources",true,Arc::from("{}"));
        store.replace_source(&index,&owner,Arc::from("// owner"),Knowledge::Found(()),Vec::new()).unwrap();
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let mut catalog=ResourceCatalog::prepare(ResourceSelection {
            model:Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new())),scope,
            claims:Some(crate::resource_model::ResourceClaims{adapter:"gradle-source-set-resources-v1".into(),roots:vec![],trees:vec![],files:vec![],
                output_roots:vec![],configuration_inputs:vec![],default_excludes:vec![],complete:true,problem:None}),
            compile_classpath:vec![],runtime_classpath:Some(vec![]),runtime_complete:true,classpath_complete:true,
            directory_inputs:std::collections::BTreeSet::new(),configuration_base:std::env::current_dir().unwrap(),overlays:BTreeMap::new(),
            guard:flag,read_coverage:crate::compiled_watch::WatchPlan::read_inputs(&[]).unwrap(),
        });
        // Captured CEM compile classpath alone contains 14,469 resources. This
        // Include runtime-resource associations in a larger catalog without
        // raising the existing handle or serialized-page bounds.
        for number in 0..20_000 {
            catalog.entries.push(ResourceEntry{association:ResourceAssociation::CompileClasspath,order:0,
                relative_path:format!("assets/entry{number}.json"),location:ResourceLocation::File(format!("resource-fixture/entry{number}.json").into()),document:None,editable:false});
        }
        let catalog=Arc::new(catalog);
        let mut evaluation=Evaluation::new("resources".into(),owner.clone(),1);
        evaluation.resource_catalog=Some(catalog.clone());evaluation.project_operations=RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT;
        let mut read=store.begin(&index,"resources",&owner).unwrap();
        let mut after=Value::Null;let mut count=0;let mut pages=0;let mut first=None;
        loop {
            let response=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceInventory","after":after}]})).unwrap();
            let page=&response["facts"][0];pages+=1;
            for resource in page["resources"].as_array().unwrap() {
                let token=resource["id"].as_str().unwrap();
                if first.is_none() {first=Some(token.to_owned());}
                assert_eq!(evaluation.resource_index(&catalog,token).unwrap(),count);count+=1;
            }
            after=page["next"].clone();
            if after.is_null() {assert_eq!(page["coverage"]["state"],"found");break;}
            assert_eq!(page["coverage"]["state"],"unknown");
        }
        assert_eq!(count,20_000);assert!(pages>1);
        assert_eq!(evaluation.handles.len(),pages,"one catalog plus page cursors, never one handle per entry");
        let before=evaluation.handles.len();
        let found=evaluation.resource_lookup(&mut read,"assets/entry19999.json",Some(ResourceAssociation::CompileClasspath)).unwrap();
        assert_eq!(found["state"],"found");
        assert_eq!(evaluation.resource_index(&catalog,found["value"][0]["id"].as_str().unwrap()).unwrap(),19_999);
        assert_eq!(evaluation.handles.len(),before,"exact lookup uses the same catalog authority");
        let mut foreign=Evaluation::new("resources".into(),owner,2);foreign.resource_catalog=Some(catalog.clone());
        assert!(foreign.resource_index(&catalog,first.as_deref().unwrap()).is_err());
        assert!(evaluation.resource_index(&catalog,&format!("{}#resource:4e20",evaluation.resource_catalog_token.as_ref().unwrap())).is_err());
        assert!(evaluation.class_identity(first.as_deref().unwrap()).is_err(),"resource catalog authority never becomes a class-byte identity");
    }

    #[test]
    fn owned_resource_fixed_frames_match_actual_inventory_and_snapshot_serializer() {
        let scope=ProjectScope{module:"resource-fixture".into(),source_set:"main".into()};
        let owner="resource-fixture/Owner.kt";
        let mut index=Index::new();index.replace_file(owner,Vec::new(),Tier::Volatile);
        index.set_file_origin(owner,FileOrigin{origin_id:"resource-fixture".into(),representation:SourceRepresentation::Source,project_scope:Some(scope.clone())});
        let mut store=ExtensionStore::default();store.configure_provider("resources",true,Arc::from("{}"));
        store.replace_source(&index,owner,Arc::from("// owner"),Knowledge::Found(()),Vec::new()).unwrap();
        let model=Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new()));
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let mut catalog=ResourceCatalog::prepare(ResourceSelection {
            model:model.clone(),scope,claims:Some(crate::resource_model::ResourceClaims{adapter:"gradle-source-set-resources-v1".into(),roots:vec![],trees:vec![],files:vec![],
                output_roots:vec![],configuration_inputs:vec![],default_excludes:vec![],complete:true,problem:None}),compile_classpath:vec![],
            runtime_classpath:Some(vec![]),runtime_complete:true,classpath_complete:true,directory_inputs:std::collections::BTreeSet::new(),configuration_base:"resource-fixture".into(),
            overlays:BTreeMap::new(),guard:flag,read_coverage:crate::compiled_watch::WatchPlan::read_inputs(&[]).unwrap(),
        });
        catalog.revision=7;
        catalog.entries.push(ResourceEntry{association:ResourceAssociation::Source,order:0,relative_path:"config.json".into(),
            location:ResourceLocation::File("resource-fixture/config.json".into()),editable:true,
            document:Some(crate::workspace::OwnedDocumentSnapshot{text:Arc::from("{\"雪\":\"😀\"}\n"),document_version:Some(7),language_id:"json".into(),revision:9})});
        let mut evaluation=Evaluation::new("resources".into(),owner.into(),1);evaluation.id=42;evaluation.project=Some(model);
        evaluation.resource_catalog=Some(Arc::new(catalog));evaluation.project_operations=RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT;
        let mut read=store.begin(&index,"resources",owner).unwrap();
        let inventory=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":"42","queries":[{"op":"resourceInventory"}]})).unwrap();
        let id=inventory["facts"][0]["resources"][0]["id"].clone();
        let params=json!({"evaluation":"42","queries":[{"op":"resourceSnapshot","resource":id}]});
        let plans=evaluation.begin_resource_reads("analysis/readFacts",&params).unwrap();
        for plan in plans {let prepared=crate::resource_inputs::prepare_resource_read(plan);evaluation.resource_results.insert(prepared.token,prepared.content);}
        let snapshot=evaluation.batch(&mut read,"analysis/readFacts",params).unwrap();
        let mut actual=Vec::new();
        write_frame(&mut actual,&json!({"jsonrpc":"2.0","id":7,"result":inventory})).unwrap();
        write_frame(&mut actual,&json!({"jsonrpc":"2.0","id":8,"result":snapshot})).unwrap();
        assert_eq!(actual,include_bytes!("../../../dev/external-extension/owned-resources-v1.frames"));
    }

    use super::*;
    use sha2::Digest;
    use crate::resource_inputs::{ResourceAssociation,ResourceCatalog,ResourceEntry,ResourceLocation,ResourceSelection};
    use ktcore::{extensions::ExtensionStore,index::{Index,Tier},project_model::{FileOrigin,ProjectScope,SourceRepresentation}};
    #[derive(Debug)] struct Flag(AtomicBool);
    impl ktcore::extensions::HostInputGuard for Flag {fn is_current(&self)->bool {!self.0.load(Ordering::Acquire)}}

    #[test]
    fn resource_operations_preserve_unsaved_bytes_opaque_chunks_and_negative_lifetime() {
        use base64::Engine;
        let scope=ProjectScope{module:"resource-wire".into(),source_set:"main".into()};
        let owner=std::env::current_dir().unwrap().join("Owner.kt");let owner=crate::paths::file_key(&owner);
        let mut index=Index::new();index.replace_file(&owner,Vec::new(),Tier::Volatile);
        index.set_file_origin(&owner,FileOrigin{origin_id:"resource-wire".into(),representation:SourceRepresentation::Source,project_scope:Some(scope.clone())});
        let mut store=ExtensionStore::default();store.configure_provider("resources",true,Arc::from("{}"));
        store.replace_source(&index,&owner,Arc::from("// owner"),Knowledge::Found(()),Vec::new()).unwrap();
        let model=Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new()));
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let mut catalog=ResourceCatalog::prepare(ResourceSelection {
            model:model.clone(),scope,claims:Some(crate::resource_model::ResourceClaims {
                adapter:"gradle-source-set-resources-v1".into(),roots:vec![],trees:vec![],files:vec![],output_roots:vec![],configuration_inputs:vec![],
                default_excludes:vec![],complete:true,problem:None,
            }),compile_classpath:vec![],runtime_classpath:Some(vec![]),runtime_complete:true,classpath_complete:true,
            directory_inputs:std::collections::BTreeSet::new(),configuration_base:std::env::current_dir().unwrap(),
            overlays:BTreeMap::new(),guard:flag.clone(),read_coverage:crate::compiled_watch::WatchPlan::read_inputs(&[]).unwrap(),
        });
        for(name,text,version) in [("config.json","{\"雪\":\"😀\"}\n".to_owned(),4),("empty.aw",String::new(),5),("large.at","😀é".repeat(90000),6)] {
            catalog.entries.push(ResourceEntry{association:ResourceAssociation::Source,order:0,relative_path:name.into(),
                location:ResourceLocation::File(std::env::current_dir().unwrap().join(name)),
                document:Some(crate::workspace::OwnedDocumentSnapshot{text:Arc::from(text),document_version:Some(version),language_id:"text".into(),revision:version as u64}),editable:true});
        }
        let mut evaluation=Evaluation::new("resources".into(),owner.clone(),1);evaluation.project=Some(model);evaluation.resource_catalog=Some(Arc::new(catalog));
        evaluation.project_operations=RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT;
        let mut read=store.begin(&index,"resources",&owner).unwrap();
        let inventory=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceInventory"}]})).unwrap();
        assert_eq!(inventory["facts"][0]["coverage"]["state"],"found");
        let resources=inventory["facts"][0]["resources"].as_array().unwrap();assert_eq!(resources.len(),3);
        let ids:Vec<_>=resources.iter().map(|resource|resource["id"].as_str().unwrap().to_owned()).collect();
        for (n,id) in ids.iter().enumerate() {
            let params=json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceSnapshot","resource":id}]});
            let plans=evaluation.begin_resource_reads("analysis/readFacts",&params).unwrap();assert_eq!(plans.len(),1);
            for plan in plans {let prepared=crate::resource_inputs::prepare_resource_read(plan);evaluation.resource_results.insert(prepared.token,prepared.content);}
            let response=evaluation.batch(&mut read,"analysis/readFacts",params).unwrap();
            let value=&response["facts"][0]["value"];assert_eq!(value["documentVersion"],4+n);
            assert_eq!(value["identity"]["revision"],resources[n]["identity"]["revision"]);
            let mut bytes=base64::engine::general_purpose::STANDARD.decode(value["base64"].as_str().unwrap()).unwrap();
            let mut after=value["next"].clone();
            while !after.is_null() {
                let page=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceSnapshot","resource":id,"after":after}]})).unwrap();
                let page=&page["facts"][0]["value"];assert_eq!(page["offset"],bytes.len());
                bytes.extend(base64::engine::general_purpose::STANDARD.decode(page["base64"].as_str().unwrap()).unwrap());after=page["next"].clone();
            }
            let text=evaluation.resource_catalog.as_ref().unwrap().entries[n].document.as_ref().unwrap().text.clone();
            assert_eq!(bytes,text.as_bytes());assert_eq!(value["identity"]["sha256"],format!("{:x}",sha2::Sha256::digest(&bytes)));
            if n==1 {assert_eq!(value["text"],json!({"state":"found","value":""}));assert_eq!(value["base64"],"");}
        }
        let missing=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceLookup","relativePath":"new.json","association":"source"}]})).unwrap();
        assert_eq!(missing["facts"][0]["state"],"definitelyAbsent");
        assert!(evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"resourceSnapshot","resource":owner}]})).is_err());
        let pending=read.finish();flag.0.store(true,Ordering::Release);
        assert!(store.validate_pending(&pending).is_err(),"a changed overlay or negative resource inventory cannot commit late");
        assert_eq!(project_operations(&json!({"capabilities":[RESOURCES],"supportedOps":{RESOURCES:["resourceInventory","resourceLookup","resourceSnapshot"]}})),RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT);
    }
}

#[cfg(test)]
mod initialize_stream_contract_tests {
    use super::*;
    #[test]
    fn initialize_frame_advertises_each_owned_resource_operation_to_the_real_child() {
        let message=initialize_message("fixture.inputs",&std::collections::BTreeSet::new());
        let mut bytes=Vec::new();write_frame(&mut bytes,&message).unwrap();
        let actual=read_frame(&mut std::io::Cursor::new(bytes)).unwrap().unwrap();
        assert_eq!(actual["method"],"initialize");
        let offered=&actual["params"];
        assert!(offered["capabilities"].as_array().unwrap().iter().any(|capability|capability==RESOURCES));
        assert_eq!(offered["supportedOps"][RESOURCES],json!(["resourceInventory","resourceLookup","resourceSnapshot"]));
        assert_eq!(project_operations(offered),PROJECT_CONTEXT|TYPE_JVM|SOURCE_INVENTORY|SOURCE_SNAPSHOT
            |RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT|RUNTIME_CLASS|CLASS_BYTES);
        let mut acknowledgement=offered.clone();acknowledgement["supportedOps"][RESOURCES]=json!(["resourceSnapshot"]);
        assert_eq!(project_operations(&acknowledgement)&(RESOURCE_INVENTORY|RESOURCE_LOOKUP|RESOURCE_SNAPSHOT),RESOURCE_SNAPSHOT);
    }

}

#[cfg(test)]
mod stream_quota_tests {
    use super::*;

    fn call(evaluation: &mut Evaluation, read: &mut ReadSession<'_>, queries: Value)
        -> Result<(Value, usize), String>
    {
        let params = json!({"evaluation":evaluation.id.to_string(),"queries":queries});
        evaluation.prepay_input_read("analysis/readFacts", &params)?;
        let result = evaluation.batch(read, "analysis/readFacts", params);
        if result.is_err() { evaluation.cancel_prepaid_input(); }
        let frame = match &result {
            Ok(value) => json!({"jsonrpc":"2.0","id":"wire","result":value}),
            Err(error) => json!({"jsonrpc":"2.0","id":"wire","error":{"code":-32602,"message":error}}),
        };
        let length = serde_json::to_vec(&frame).unwrap().len();
        evaluation.account_stream_reply(&frame)?;
        result.map(|value|(value,length))
    }

    #[test]
    fn opaque_blob_stream_quotas_preserve_metadata_and_validate_before_io() {
        for length in [300*1024, ktcore::classfile::MAX_CLASS_FILE_BYTES] {
            let index = ktcore::index::Index::new();
            let mut store = ExtensionStore::default();
            store.replace_source(&index,"/Owner.kt",Arc::from(""),Knowledge::Found(()),Vec::new()).unwrap();
            store.configure_provider("stream",true,Arc::from("{}"));
            let mut read = store.begin(&index,"stream","/Owner.kt").unwrap();
            let mut evaluation = Evaluation::new("stream".into(),"/Owner.kt".into(),1);
            evaluation.project_operations = PROJECT_CONTEXT | RUNTIME_CLASS | CLASS_BYTES;
            // Pure wire boundary data; this injection does not establish class-file authority.
            // The separate reader and serializer fixtures prove real Code/LVT/annotation bytes.
            let bytes: Arc<[u8]> = vec![0x53;length].into();
            let identity = ktcore::jvm::ClassIdentity { origin_id:"wire-boundary-only".into(),
                internal_name:"boundary/TransportOnly".into(),input:ktcore::jvm::InputIdentity::Binary{content_sha256:[0;32]} };
            evaluation.class_results.insert(identity.internal_name.clone(),crate::runtime_bytes::ClassReadResult {
                internal_name:identity.internal_name.clone(), identity:Some(identity.clone()),
                byte_length:length,bytes:Some(bytes),unavailable:None,api_header:None,
            });
            for _ in 0..31 { call(&mut evaluation,&mut read,json!([{"op":"projectContext"}])).unwrap(); }
            let (descriptor,_) = call(&mut evaluation,&mut read,json!([{"op":"runtimeClass","internalName":identity.internal_name}])).unwrap();
            let token = descriptor["facts"][0]["value"]["id"].clone();
            let (initial,_) = call(&mut evaluation,&mut read,json!([{"op":"classBytes","classIdentity":token}])).unwrap();
            assert_eq!(evaluation.batches,33);
            let first_cursor = initial["facts"][0]["value"]["next"].clone();
            assert!(first_cursor.is_string());
            if length == 300*1024 {
                for _ in 33..64 { call(&mut evaluation,&mut read,json!([{"op":"projectContext"}])).unwrap(); }
                assert!(call(&mut evaluation,&mut read,json!([{"op":"projectContext"}])).is_err());
                assert_eq!(evaluation.batches,64,"the ordinary limit is unchanged");
                let mut wire_bytes = 0;
                for _ in 0..MAX_STREAM_CONTINUATIONS {
                    let (_,size) = call(&mut evaluation,&mut read,json!([{"op":"classBytes","classIdentity":token,"after":first_cursor}])).unwrap();
                    wire_bytes += size;
                }
                assert_eq!(evaluation.stream_continuations,256);
                assert_eq!(evaluation.stream_bytes,wire_bytes,"count actual serialized JSON-RPC replies");
                assert_eq!(evaluation.batches,64,"continuations do not replenish ordinary metadata");
                assert!(call(&mut evaluation,&mut read,json!([{"op":"classBytes","classIdentity":token,"after":first_cursor}])).is_err());
            } else {
                let mut after = first_cursor;
                let mut chunks = 1;
                let mut wire_bytes = 0;
                while !after.is_null() {
                    let (result,size) = call(&mut evaluation,&mut read,json!([{"op":"classBytes","classIdentity":token,"after":after}])).unwrap();
                    after = result["facts"][0]["value"]["next"].clone();
                    chunks += 1;wire_bytes += size;
                }
                assert_eq!(chunks,64,"the existing declared 16MiB body bound is streamable");
                assert_eq!(evaluation.stream_continuations,63);
                assert_eq!(evaluation.batches,33);
                assert_eq!(evaluation.stream_bytes,wire_bytes);
                assert!(wire_bytes < MAX_STREAM_BYTES);
            }
            let before = evaluation.stream_continuations;
            assert!(call(&mut evaluation,&mut read,json!([{"op":"classBytes","classIdentity":token,"after":"foreign"}])).is_err());
            assert_eq!(evaluation.stream_continuations,before,"invalid cursors never prepay IO");
            evaluation.stream_bytes = MAX_STREAM_BYTES - 1;
            evaluation.reply_is_stream = true;
            assert!(evaluation.account_stream_reply(&json!({"jsonrpc":"2.0","id":1,"result":{}})).is_err());
        }
    }

    #[test]
    fn prepaid_input_reads_cannot_change_queries_or_refund_failed_io() {
        let index = ktcore::index::Index::new();
        let mut store = ExtensionStore::default();
        store.replace_source(&index,"/Owner.kt",Arc::from(""),Knowledge::Found(()),Vec::new()).unwrap();
        store.configure_provider("stream",true,Arc::from("{}"));
        let mut read = store.begin(&index,"stream","/Owner.kt").unwrap();
        let mut evaluation = Evaluation::new("stream".into(),"/Owner.kt".into(),1);
        evaluation.project_operations=PROJECT_CONTEXT;
        let params=json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"projectContext"}]});
        evaluation.prepay_input_read("analysis/readFacts",&params).unwrap();
        assert!(evaluation.prepay_input_read("analysis/readFacts",&params).is_err());
        let other=json!({"evaluation":evaluation.id.to_string(),"queries":[]});
        assert!(evaluation.batch(&mut read,"analysis/readFacts",other).is_err());
        assert_eq!(evaluation.batches,1);
        evaluation.prepay_input_read("analysis/readFacts",&params).unwrap();
        evaluation.cancel_prepaid_input();
        assert_eq!(evaluation.batches,2,"failed preparation consumes its attempt");
    }
}

#[cfg(test)]
mod sdk_api_header_wire_tests {
    use super::*;

    #[test]
    fn sdk_api_header_exposes_hierarchy_but_never_mints_executable_class_bytes() {
        let bytes=include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let header=Arc::new(ktcore::classfile::decode("selected-api-header",bytes).unwrap());
        // Authority, rather than a convenient Code attribute, decides byte access.
        assert!(header.methods.iter().any(|method|method.attribute_names.iter().any(|name|name.as_ref()=="Code")));
        let mut evaluation=Evaluation::new("headers".into(),"/Owner.kt".into(),1);
        evaluation.project_operations=RUNTIME_CLASS|CLASS_BYTES;
        evaluation.class_results.insert(header.identity.internal_name.clone(),crate::runtime_bytes::ClassReadResult{
            internal_name:header.identity.internal_name.clone(),identity:Some(header.identity.clone()),
            byte_length:0,bytes:None,unavailable:None,api_header:Some(header.clone()),
        });
        let fact=evaluation.runtime_class(&header.identity.internal_name).unwrap();
        assert_eq!(fact["value"]["authority"],"sdk-api-header");
        assert_eq!(fact["value"]["codeAvailable"],false);
        assert_eq!(fact["value"]["runtimeOnly"],false);
        assert_eq!(fact["value"]["runtimeAbiCoverage"]["state"],"unknown");
        assert_eq!(fact["value"]["classHeader"]["state"],"found");
        assert_eq!(fact["value"]["hierarchy"]["state"],"found");
        assert_eq!(fact["value"]["hierarchy"]["value"]["superName"],"java/lang/Object");
        let id=fact["value"]["id"].as_str().unwrap();
        let unavailable=evaluation.class_bytes(id,None).unwrap();
        assert_eq!(unavailable["state"],"unknown");
        assert_eq!(unavailable["reasons"][0]["code"],"BYTECODE_UNAVAILABLE");
        let params=json!({"evaluation":evaluation.id.to_string(),"queries":[{"op":"classBytes","classIdentity":id}]});
        assert!(evaluation.begin_class_reads("analysis/readFacts",&params).unwrap().is_none(),
            "a header token must not schedule a raw byte read");
    }
}
