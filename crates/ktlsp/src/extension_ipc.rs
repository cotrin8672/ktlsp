//! Versioned semantic extension transport. Framework rules belong in the child.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
const MAX_BATCH: usize = 256;
const MAX_HANDLES: usize = 16_384;
const EVALUATION_LIMIT: Duration = Duration::from_secs(30);
static NEXT_EVALUATION: AtomicU64 = AtomicU64::new(1);

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
    Type(TypeId),
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
    handles: BTreeMap<String, Handle>,
    batches: usize,
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
            handles: BTreeMap::new(),
            batches: 0,
        }
    }
    fn issue(&mut self, handle: Handle) -> String {
        let token = format!("{}:{}", self.id, self.handles.len());
        self.handles.insert(token.clone(), handle);
        token
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
    pub fn batch(
        &mut self,
        read: &mut ReadSession<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        if params["evaluation"].as_str() != Some(&self.id.to_string()) {
            return Err("foreign evaluation".into());
        }
        self.batches += 1;
        if self.batches > 64 || self.handles.len() > MAX_HANDLES {
            return Err("extension evaluation limit".into());
        }
        match method {
            "analysis/readFacts" => {
                let queries: Vec<ReadQuery> =
                    serde_json::from_value(params["queries"].clone()).map_err(|e| e.to_string())?;
                if queries.len() > MAX_BATCH {
                    return Err("extension batch too large".into());
                }
                let mut result = Vec::new();
                for query in queries {
                    result.push(match query {
                        ReadQuery::ResolveType {name} => self.type_fact(&read.resolve_type(&name)),
                        ReadQuery::ResolveTypeRef {reference} => {
                            let reference = match self.handles.get(&reference) { Some(Handle::TypeRef(value)) => value.clone(), _ => return Err("foreign type-reference token".into()) };
                            self.type_fact(&read.resolve_type_ref(&reference))
                        }
                        ReadQuery::DeclarationType {declaration} => self.type_fact(&read.declaration_type(&self.declaration(&json!(declaration))?).map_err(contract)?),
                        ReadQuery::DeclaredMembers {target} => {
                            let (members,coverage) = read.declared_members(&self.ty(&json!(target))?).map_err(contract)?;
                            json!({"members":members.iter().map(|member|self.member(member)).collect::<Vec<_>>(),"coverage":fact(&coverage, |_|Value::Null)})
                        }
                    });
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

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
enum ReadQuery {
    ResolveType { name: String },
    ResolveTypeRef { reference: String },
    DeclarationType { declaration: String },
    DeclaredMembers { target: String },
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
    transport: Mutex<Option<Transport>>,
}

impl Worker {
    pub fn new(command: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_EVALUATION.fetch_add(1, Ordering::Relaxed),
            command,
            stopped: AtomicBool::new(false),
            failed: Arc::new(AtomicBool::new(false)),
            transport: Mutex::new(None),
        })
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
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
        let mut transport = self.transport.lock().unwrap();
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
                transport.send(json!({"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{"protocol":{"major":PROTOCOL_MAJOR,"minor":0},"provider":work.provider,"capabilities":["semantic-v1"]}}))?;
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
                transport.initialized = true;
            }
            let id = work.evaluation.to_string();
            transport.send(
                json!({"jsonrpc":"2.0","id":id,"method":"analysis/evaluate","params":work.params}),
            )?;
            let deadline = Instant::now() + EVALUATION_LIMIT;
            let mut cancelled = false;
            loop {
                let message = transport.receive(deadline, &self.stopped, || {
                    if !cancelled && !workspace.lock().ipc_evaluation_current(work.evaluation) {
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
                    let result = workspace.lock().ipc_batch(
                        work.evaluation,
                        method,
                        message["params"].clone(),
                    );
                    after_batch();
                    let response = match result {
                        Ok(value) => json!({"jsonrpc":"2.0","id":request_id,"result":value}),
                        Err(error) => {
                            json!({"jsonrpc":"2.0","id":request_id,"error":{"code":-32602,"message":error}})
                        }
                    };
                    transport.send(response)?;
                } else if message["id"] == id {
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
                        .map(|_| ());
                } else {
                    return Err("unexpected extension response id".into());
                }
            }
        })();
        if result.as_ref().is_err_and(|error| error != "Stale") {
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
