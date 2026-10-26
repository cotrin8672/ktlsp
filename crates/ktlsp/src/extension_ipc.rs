//! Versioned semantic extension transport. Framework rules belong in the child.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
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
const EVALUATION_LIMIT: Duration = Duration::from_secs(30);
static NEXT_EVALUATION: AtomicU64 = AtomicU64::new(1);
const PROJECT_FACTS: &str = "project-class-facts-v1";
const PROJECT_CONTEXT: u8 = 1;
const TYPE_JVM: u8 = 2;
const SOURCE_SNAPSHOTS: &str = "source-snapshots-v1";
const SOURCE_INVENTORY: u8 = 4;
const SOURCE_SNAPSHOT: u8 = 8;

fn project_operations(result: &Value) -> u8 {
    let mut selected = 0;
    for (capability, operations) in [
        (PROJECT_FACTS, [("projectContext", PROJECT_CONTEXT), ("typeJvm", TYPE_JVM)]),
        (SOURCE_SNAPSHOTS, [("sourceInventory", SOURCE_INVENTORY), ("sourceSnapshot", SOURCE_SNAPSHOT)]),
    ] {
        if !result["capabilities"].as_array().is_some_and(|values|
            values.iter().any(|value| value == capability)) { continue; }
        if let Some(offered) = result["supportedOps"][capability].as_array() {
            for (operation, bit) in operations {
                if offered.iter().any(|value| value == operation) { selected |= bit; }
            }
        }
    }
    selected
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
    Type(TypeId),
    Source(SourceHandle),
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
    pub project_operations: u8,
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
            committed_revision: None,
            project: None,
            project_operations: 0,
            handles: BTreeMap::new(),
            batches: 0,
        }
    }
    fn issue(&mut self, handle: Handle) -> String {
        let token = format!("{}:{}", self.id, self.handles.len());
        self.handles.insert(token.clone(), handle);
        token
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
                // Leave room for the JSON-RPC envelope. Count escaped UTF-8 bytes before
                // retaining another fact, so a child can split a batch without dying.
                let mut remaining = MAX_FRAME / 2;
                for query in queries {
                    let value = match query {
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
            "supportedOps":{SOURCE_SNAPSHOTS:["sourceInventory","sourceSnapshot"]}})),SOURCE_INVENTORY | SOURCE_SNAPSHOT);
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
    project_operations: AtomicU8,
    transport: Mutex<Option<Transport>>,
}

impl Worker {
    pub fn new(command: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_EVALUATION.fetch_add(1, Ordering::Relaxed),
            command,
            stopped: AtomicBool::new(false),
            failed: Arc::new(AtomicBool::new(false)),
            project_operations: AtomicU8::new(0),
            transport: Mutex::new(None),
        })
    }
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub(crate) fn project_operations(&self) -> u8 {
        self.project_operations.load(Ordering::Acquire)
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
                transport.send(json!({"jsonrpc":"2.0","id":"initialize","method":"initialize","params":{"protocol":{"major":PROTOCOL_MAJOR,"minor":0},"provider":work.provider,"capabilities":["semantic-v1",PROJECT_FACTS,SOURCE_SNAPSHOTS],"supportedOps":{PROJECT_FACTS:["projectContext","typeJvm"],SOURCE_SNAPSHOTS:["sourceInventory","sourceSnapshot"]}}}))?;
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
                transport.initialized = true;
            }
            if self.project_operations() & (SOURCE_INVENTORY | SOURCE_SNAPSHOT) != 0 {
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
