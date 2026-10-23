//! The LSP layer: a thin `tower-lsp-server` backend that translates between LSP types and the
//! pure core. This is the ONLY module that depends on `tower-lsp-server` / `ls-types`.
//!
//! Identity: we key the workspace by the file's *path string* (`uri.to_file_path()`), converting
//! URI <-> path exactly once at this boundary and never re-deriving identity from the filesystem
//! mid-request. Byte ranges from the core are converted to LSP positions via `LineIndex` here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use tower_lsp_server::jsonrpc::Result;
use tower_lsp_server::ls_types::request::{
    GotoImplementationParams, GotoImplementationResponse, GotoTypeDefinitionParams,
    GotoTypeDefinitionResponse,
};
use tower_lsp_server::ls_types::*;
use tower_lsp_server::{Client, LanguageServer};

use crate::classpath;
use crate::complete::ShapedItem;
use crate::deps::file_content_digest as configuration_content;
use crate::diagnostics::Severity;
use crate::format::FormatterConfig;
use crate::hierarchy::HierarchyItem;
use crate::symbol::{Def, SymbolKind};
use crate::text::LineIndex;
use crate::workspace::Workspace;

/// Debounce window for diagnostics: the server is FULL-sync (a keystroke is a whole-doc change), so
/// we coalesce rapid edits before recomputing.
const DIAGNOSTIC_DEBOUNCE: Duration = Duration::from_millis(300);
const PROJECT_REFRESH_DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Default)]
struct RefreshState {
    requested: u64,
    running: bool,
    force: bool,
    stopped: bool,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
}

#[derive(Default)]
struct WatchState {
    dynamic: bool,
    relative: bool,
    registration: Option<String>,
    registered_options: Option<serde_json::Value>,
    sequence: u64,
    configuration_contents: HashMap<String, [u8; 32]>,
    archive_contents: HashMap<String, [u8; 32]>,
    configuration_baselined: bool,
    model_loading: bool,
    pending_model_jar: bool,
}

impl WatchState {
    fn unchanged_configuration(&mut self, key: &str, kind: FileChangeType) -> bool {
        if !is_project_configuration(Path::new(key)) {
            return false;
        }
        let contents = &mut self.configuration_contents;
        let Some(current) = configuration_content(Path::new(key)) else {
            contents.remove(key);
            return false;
        };
        let previous = contents.insert(key.to_string(), current);
        kind == FileChangeType::CHANGED && previous == Some(current)
    }

    fn acknowledges(&self, expected_options: &serde_json::Value) -> bool {
        self.dynamic
            && self.registration.is_some()
            && self.registered_options.as_ref() == Some(expected_options)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiagnosticScope {
    OpenFilesOnly,
    Workspace,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct DiagnosticRevision {
    generation: u64,
    document_version: Option<i32>,
}

pub struct Backend {
    client: Client,
    ws: Arc<Mutex<Workspace>>,
    root: Mutex<Option<PathBuf>>,
    gradle_project: Mutex<bool>,
    /// Whether the client advertised snippet support in `initialize`. Set once; gates whether
    /// completion items insert `name($0)` snippets or plain bare names.
    snippets_supported: Mutex<bool>,
    /// Client document versions stay separate from debounce generations used for invalidation.
    doc_versions: Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    /// Whether the client advertised `window.workDoneProgress` in `initialize`. Gates server-initiated
    /// progress (the indexing spinner); when false we fall back to log messages.
    progress_supported: Arc<Mutex<bool>>,
    diagnostic_scope: Mutex<DiagnosticScope>,
    formatter: Mutex<Option<FormatterConfig>>,
    refresh: Arc<Mutex<RefreshState>>,
    watches: Arc<Mutex<WatchState>>,
    compile_workers: Arc<tokio::sync::Semaphore>,
    compile_watch: CompilerWatchSlot,
    kotlin_diagnostics: Arc<Mutex<KotlinDiagnosticState>>,
    jdk_source_seeds: Arc<Mutex<HashMap<String, (u64, tokio::sync::watch::Receiver<()>)>>>,
}

impl Backend {
    pub fn new(client: Client) -> Self {
        Self::with_analysis_providers(client, Vec::new())
    }

    pub fn with_analysis_providers(
        client: Client,
        providers: Vec<Arc<dyn crate::analysis_extensions::AnalysisProvider>>,
    ) -> Self {
        Backend {
            client,
            ws: Arc::new(Mutex::new(Workspace::with_analysis_providers(providers))),
            root: Mutex::new(None),
            gradle_project: Mutex::new(false),
            snippets_supported: Mutex::new(false),
            doc_versions: Arc::new(Mutex::new(HashMap::new())),
            progress_supported: Arc::new(Mutex::new(false)),
            diagnostic_scope: Mutex::new(DiagnosticScope::OpenFilesOnly),
            formatter: Mutex::new(None),
            refresh: Arc::new(Mutex::new(RefreshState::default())),
            watches: Arc::new(Mutex::new(WatchState::default())),
            compile_workers: Arc::new(tokio::sync::Semaphore::new(1)),
            compile_watch: Arc::new(Mutex::new(None)),
            kotlin_diagnostics: Arc::new(Mutex::new(KotlinDiagnosticState::default())),
            jdk_source_seeds: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn await_extension_sources(&self, key: &str) -> Result<()> {
        let (generation, version, input) = {
            let ws = self.ws.lock();
            (
                ws.analysis_generation(),
                self.doc_versions
                    .lock()
                    .get(key)
                    .and_then(|r| r.document_version),
                ws.extension_source_input_identity(key),
            )
        };
        loop {
            let pending = {
                let mut ws = self.ws.lock();
                let current_input = ws.extension_source_input_identity(key);
                let same_input = match (&input, &current_input) {
                    (Some(before), Some(current)) => Arc::ptr_eq(before, current),
                    (None, None) => true,
                    _ => false,
                };
                if !same_input
                    || !ws.is_current_generation(generation)
                    || self
                        .doc_versions
                        .lock()
                        .get(key)
                        .and_then(|r| r.document_version)
                        != version
                {
                    return Err(tower_lsp_server::jsonrpc::Error {
                        code: tower_lsp_server::jsonrpc::ErrorCode::ServerError(-32801),
                        message: "document or project changed during source preparation".into(),
                        data: None,
                    });
                }
                ws.extension_source_wait(key)
            };
            let Some(mut ready) = pending else {
                return Ok(());
            };
            schedule_extension_sources(&self.ws, &self.client, &self.doc_versions);
            // Only exact selected producers advance this dependency frontier; no semantic polling.
            let _ = ready.changed().await;
        }
    }

    fn schedule_project_refresh(&self, force: bool, initial_delay: Duration) {
        self.request_project_refresh(force, initial_delay, false, &[]);
    }

    fn request_project_refresh(
        &self,
        force: bool,
        initial_delay: Duration,
        jar_only: bool,
        retired_origins: &[String],
    ) {
        let Some(root) = self.root.lock().clone() else {
            return;
        };
        let mut state = self.refresh.lock();
        if state.stopped {
            return;
        }
        let (defer, coalesced) = {
            let mut watches = self.watches.lock();
            if jar_only && watches.model_loading {
                let coalesced = watches.pending_model_jar;
                watches.pending_model_jar = true;
                (true, coalesced)
            } else {
                (false, false)
            }
        };
        // Gradle may recreate build-logic JARs while importing its own model. Retain
        // every such change, but let this owned dump close its tasks before retrying.
        // Advancing the generation prevents its unsettled model from being published.
        if !defer {
            state
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            state.cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        }
        let changed = {
            let mut ws = self.ws.lock();
            if !coalesced {
                state.requested = ws.begin_analysis_generation();
                crate::trace::span(
                    "workspace.refresh_requested",
                    "workspace",
                    Instant::now(),
                    serde_json::json!({"root": root.to_string_lossy(), "generation": state.requested,
                        "deferredJar": defer}),
                );
                self.jdk_source_seeds.lock().clear();
            }
            // Retire under the generation lock so an old worker cannot restore completion.
            for origin in retired_origins {
                ws.retire_dependency_origin(origin);
            }
            ws.take_extension_diagnostic_changes()
        };
        for key in changed {
            self.schedule_one_diagnostic(key);
        }
        if coalesced {
            return;
        }
        state.force |= force;
        if state.running {
            return;
        }
        state.running = true;
        let state = self.refresh.clone();
        let ws = self.ws.clone();
        let client = self.client.clone();
        let watches = self.watches.clone();
        let progress = *self.progress_supported.lock();
        let gradle = *self.gradle_project.lock();
        let compiler_state = self.kotlin_diagnostics.clone();
        let compiler_workers = self.compile_workers.clone();
        let diagnostic_versions = self.doc_versions.clone();
        tokio::spawn(async move {
            let mut delay = initial_delay;
            loop {
                let requested = state.lock().requested;
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                delay = PROJECT_REFRESH_DEBOUNCE;
                let (generation, force, cancelled) = {
                    let mut state = state.lock();
                    if state.stopped {
                        state.running = false;
                        break;
                    }
                    if state.requested != requested {
                        continue;
                    }
                    (
                        state.requested,
                        std::mem::take(&mut state.force),
                        state.cancelled.clone(),
                    )
                };
                index_workspace(
                    client.clone(),
                    ws.clone(),
                    diagnostic_versions.clone(),
                    root.clone(),
                    progress,
                    gradle,
                    generation,
                    force,
                    watches.clone(),
                    cancelled,
                    compiler_workers.clone(),
                )
                .await;
                schedule_selected_kotlin_diagnostics(
                    &ws,
                    &client,
                    &diagnostic_versions,
                    &compiler_state,
                    &compiler_workers,
                    &watches,
                    &root,
                );
                let mut state = state.lock();
                if state.stopped || state.requested == generation {
                    state.running = false;
                    break;
                }
            }
        });
    }

    /// Bump `key`'s version and schedule a debounced diagnostics recompute. Off the request path; the
    /// task discards itself if a newer edit lands during the debounce window.
    fn schedule_diagnostics(&self, key: String) {
        schedule_extension_sources(&self.ws, &self.client, &self.doc_versions);
        let mut keys = self.ws.lock().take_extension_diagnostic_changes();
        keys.push(key);
        keys.sort();
        keys.dedup();
        for key in keys {
            self.schedule_one_diagnostic(key);
        }
        self.schedule_kotlin_diagnostics();
    }

    fn schedule_one_diagnostic(&self, key: String) {
        let invalid = self
            .compile_watch
            .lock()
            .as_ref()
            .filter(|watch| watch.owner.check_live().is_err())
            .map(|watch| watch.owner.clone());
        if let Some(owner) = invalid {
            close_owned_compile_watch(&self.compile_watch, &owner);
        }
        schedule_owned_diagnostic(&self.ws, &self.client, &self.doc_versions, key);
    }

    fn seed_jdk_imports_for_open_java(&self, key: String, workspace: &Workspace) {
        let Some((text, tree)) = workspace.open_java_source(&key) else {
            return;
        };
        let ws = self.ws.clone();
        let root = self.root.lock().clone();
        let generation = workspace.analysis_generation();
        let model = workspace.project_snapshot();
        let jdk_source = selected_jdk_source_for_file(
            model.as_deref(),
            Path::new(&key),
            *self.gradle_project.lock(),
        );
        let pending = self.jdk_source_seeds.clone();
        // Closing this channel wakes every first request, including failed or panicking workers.
        let (published, ready) = tokio::sync::watch::channel(());
        pending
            .lock()
            .insert(key.clone(), (generation, ready.clone()));
        tokio::spawn(async move {
            let start = Instant::now();
            let seed_ws = ws.clone();
            let seed_pending = pending.clone();
            let seed_ready = ready.clone();
            let trace_key = key.clone();
            let result = tokio::task::spawn_blocking(move || {
                let mut java = crate::java::JavaParser::new();
                let imports = crate::java::explicit_import_fqns(&tree, &text);
                let mut jdk_imports = imports
                    .iter()
                    .filter(|fqn| fqn.starts_with("java.") || fqn.starts_with("javax."))
                    .cloned()
                    .collect::<Vec<_>>();
                jdk_imports.extend(crate::java::referenced_java_lang_fqns(&tree, &text));
                jdk_imports.sort();
                jdk_imports.dedup();
                let batches = jdk_source
                    .as_deref()
                    .map(|source| {
                        crate::deps::resolve_jdk_imports_from_zip(
                            source,
                            &crate::deps::extract_root(),
                            &jdk_imports,
                            &mut java,
                        )
                    })
                    .unwrap_or_default();
                let files = batches.len();
                let symbols = batches
                    .iter()
                    .map(|batch| batch.symbols.len())
                    .sum::<usize>();
                let current = {
                    let mut guard = seed_ws.lock();
                    if !guard.is_current_generation(generation) {
                        false
                    } else {
                        if !batches.is_empty() {
                            for batch in batches {
                                guard.index_dependency_batch_for_generation(generation, batch);
                            }
                            guard.bump_index_revision();
                        }
                        true
                    }
                };
                {
                    let mut seeds = seed_pending.lock();
                    if seeds
                        .get(&key)
                        .is_some_and(|(_, seed)| seed.same_channel(&seed_ready))
                    {
                        seeds.remove(&key);
                    }
                }
                // Publish JDK facts before unrelated local-jar import resolution can delay completion.
                drop(published);
                if !current {
                    return None;
                }
                let mut batches = Vec::new();
                if let Some(root) = root {
                    if crate::deps::gradle_root(&root).is_some() {
                        let local_jars = model
                            .as_ref()
                            .and_then(|model| model.local_jars_for_file(Path::new(&key)))
                            .unwrap_or_default();
                        if !local_jars.is_empty() {
                            let extract_root = crate::deps::extract_root();
                            let mut kotlin = crate::parser::KotlinParser::new();
                            for fqn in imports.iter().filter(|fqn| {
                                !fqn.starts_with("java.") && !fqn.starts_with("javax.")
                            }) {
                                batches.extend(crate::deps::resolve_import_fqn_from_local_jars(
                                    fqn,
                                    local_jars.clone(),
                                    model.as_deref(),
                                    &extract_root,
                                    &mut kotlin,
                                    &mut java,
                                ));
                            }
                        }
                    }
                }
                Some((imports.len(), files, symbols, batches))
            })
            .await;
            {
                let mut seeds = pending.lock();
                if seeds
                    .get(&trace_key)
                    .is_some_and(|(_, seed)| seed.same_channel(&ready))
                {
                    seeds.remove(&trace_key);
                }
            }
            let Ok(Some((imports, mut files, mut symbols, batches))) = result else {
                return;
            };
            files += batches.len();
            symbols += batches
                .iter()
                .map(|batch| batch.symbols.len())
                .sum::<usize>();
            if !batches.is_empty() {
                let mut guard = ws.lock();
                if !guard.is_current_generation(generation) {
                    return;
                }
                for batch in batches {
                    guard.index_dependency_batch_for_generation(generation, batch);
                }
                guard.bump_index_revision();
            }
            crate::trace::span(
                "deps.jdk_seed_open_file",
                "deps",
                start,
                serde_json::json!({
                    "file": trace_key,
                    "imports": imports,
                    "files": files,
                    "symbols": symbols,
                }),
            );
        });
    }
}

#[derive(Default)]
struct DepStats {
    coordinates: usize,
    local_jars: usize,
    reused_coordinates: usize,
    reused_local_jars: usize,
    files: usize,
    symbols: usize,
    failed: usize,
    missing_sources: usize,
    shadowed: usize,
    skipped: usize,
    jdk_files: usize,
    jdk_symbols: usize,
    local_stub_files: usize,
    local_stub_symbols: usize,
    local_source_files: usize,
    local_source_symbols: usize,
    incomplete_classpath: bool,
    jdk_reused: bool,
    jdk_complete: bool,
}

#[derive(Clone)]
struct IndexedDependencyFile {
    path: String,
    symbols: usize,
}

struct DependencyResult {
    jdk_parse_plan: Option<crate::deps::JdkParsePlan>,
    coord: Option<crate::coords::Coordinate>,
    label: String,
    batches: Vec<crate::deps::FileSymbols>,
    discovered: Vec<crate::coords::Coordinate>,
    failed: bool,
    skipped: bool,
    missing_source: bool,
    jdk: bool,
    local_jar: bool,
    local_origin: Option<String>,
}

impl DependencyResult {
    // A failed source archive stays failed/incomplete. Only the existing exact selected-module
    // binary decoder may still contribute positive members; never publish partial source text.
    fn source_error_binary_fallback(&self) -> bool {
        let Some(coord) = &self.coord else {
            return false;
        };
        let owner = classpath::maven_origin_id(coord);
        self.failed
            && self.missing_source
            && !self.jdk
            && !self.local_jar
            && !self.batches.is_empty()
            && self.batches.iter().all(|batch| {
                batch.origin.as_ref().is_some_and(|origin| {
                    origin.origin_id == owner
                        && origin.representation == crate::project_model::SourceRepresentation::Stub
                }) && batch.symbols.iter().any(|symbol| {
                    matches!(
                        symbol.kind,
                        ktcore::symbol::SymbolKind::Function | ktcore::symbol::SymbolKind::Property
                    ) || symbol.kind.is_type_like() && symbol.unavailable_members.is_some()
                })
            })
    }
}

fn dependency_index_threads() -> usize {
    std::env::var("KTLSP_INDEX_THREADS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|threads| *threads > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|threads| threads.get())
                .unwrap_or(1)
                .min(8)
        })
}

/// Spawn one worker per JDK shard. Sharding the ~15k-file `src.zip` across the dependency pool
/// removes the single-thread tail that previously dominated cold library indexing.
/// Spawn one worker per JDK shard. Sharding the ~15k-file `src.zip` across the dependency pool
/// removes the single-thread tail that previously dominated cold library indexing. Returns the
/// number of workers actually spawned: a shortfall means entries whose index maps to an
/// unspawned shard are skipped this run (never cached), rather than hanging the scheduler.
fn spawn_jdk_shard_workers(
    plan: crate::deps::JdkParsePlan,
    origin: String,
    shards: usize,
    generation: u64,
    tx: std::sync::mpsc::Sender<DependencyResult>,
) -> usize {
    let mut spawned = 0;
    for shard in 0..shards {
        let plan = plan.clone();
        let origin = origin.clone();
        let tx = tx.clone();
        let ok = std::thread::Builder::new()
            .name(format!("ktlsp-jdk-index-{shard}"))
            .stack_size(32 * 1024 * 1024)
            .spawn(move || {
                use crate::deps;
                use crate::java::JavaParser;
                use crate::parser::KotlinParser;

                let resolve_started = crate::trace::enabled().then(Instant::now);
                let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut kotlin = KotlinParser::new();
                    let mut java = JavaParser::new();
                    deps::parse_jdk_shard(&plan, shard, shards, &mut kotlin, &mut java)
                }))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("JDK index worker panicked")));
                let result = match resolved {
                    Ok(batches) => DependencyResult {
                        jdk_parse_plan: None,
                        coord: None,
                        label: format!("JDK sources (shard {shard}/{shards})"),
                        batches,
                        discovered: Vec::new(),
                        failed: false,
                        skipped: false,
                        missing_source: false,
                        jdk: true,
                        local_jar: false,
                        local_origin: Some(origin.clone()),
                    },
                    Err(error) => {
                        tracing::warn!(%error, shard, "JDK source indexing failed");
                        DependencyResult {
                            jdk_parse_plan: None,
                            coord: None,
                            label: format!("JDK sources (shard {shard}/{shards})"),
                            batches: Vec::new(),
                            discovered: Vec::new(),
                            failed: true,
                            skipped: false,
                            missing_source: false,
                            jdk: true,
                            local_jar: false,
                            local_origin: Some(origin.clone()),
                        }
                    }
                };
                if let Some(started) = resolve_started {
                    crate::trace::span(
                        "deps.jdk_parse_shard",
                        "deps",
                        started,
                        serde_json::json!({"generation":generation, "origin":result.local_origin.as_deref(), "shard":shard, "shards":shards, "files":result.batches.len(), "failed":result.failed}),
                    );
                }
                let _ = tx.send(result);
            })
            .is_ok();
        spawned += ok as usize;
    }
    spawned
}

fn spawn_sdk_api_worker(
    selection: classpath::SdkApiSelection,
    extract_root: PathBuf,
    tx: std::sync::mpsc::Sender<DependencyResult>,
) -> bool {
    std::thread::Builder::new()
        .name("ktlsp-sdk-api".into())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            let origin = selection.origin();
            let indexed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                crate::sdk_api::index(&selection, &extract_root)
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("SDK API index worker panicked")));
            let (batches, failed, incomplete) = match indexed {
                Ok(indexed) => (indexed.batches, false, !indexed.complete),
                Err(error) => {
                    tracing::warn!(%error, "selected SDK API indexing failed");
                    (Vec::new(), true, true)
                }
            };
            let _ = tx.send(DependencyResult {
                jdk_parse_plan: None,
                coord: None,
                label: "selected SDK API signatures".into(),
                batches,
                discovered: Vec::new(),
                failed,
                skipped: false,
                missing_source: incomplete,
                jdk: true,
                local_jar: false,
                local_origin: Some(origin),
            });
        })
        .is_ok()
}

type JdkPlanner =
    dyn Fn(&std::path::Path, &std::path::Path) -> Option<crate::deps::JdkIndexPlan> + Send + Sync;

fn spawn_jdk_plan_worker<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    source: PathBuf,
    extract_root: PathBuf,
    origin: String,
    generation: u64,
    planner: Arc<JdkPlanner>,
    tx: std::sync::mpsc::Sender<DependencyResult>,
) -> bool {
    std::thread::Builder::new()
        .name("ktlsp-jdk-plan".to_string())
        .stack_size(32 * 1024 * 1024)
        .spawn_scoped(scope, move || {
            let started = crate::trace::enabled().then(Instant::now);
            let plan = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                planner(&source, &extract_root)
            }))
            .unwrap_or_else(|_| {
                tracing::warn!(%origin, "JDK index planning worker panicked");
                None
            });
            if let Some(started) = started {
                crate::trace::span(
                    "deps.jdk_plan",
                    "deps",
                    started,
                    serde_json::json!({"generation":generation, "origin":origin,
                        "available":plan.is_some(),
                        "cached":matches!(&plan, Some(crate::deps::JdkIndexPlan::Cached(_)))}),
                );
            }
            let failed = plan.is_none();
            let (jdk_parse_plan, batches) = match plan {
                Some(crate::deps::JdkIndexPlan::Cached(batches)) => (None, batches),
                Some(crate::deps::JdkIndexPlan::Parse(plan)) => (Some(plan), Vec::new()),
                None => (None, Vec::new()),
            };
            let _ = tx.send(DependencyResult {
                jdk_parse_plan,
                coord: None,
                label: "JDK sources".to_string(),
                batches,
                discovered: Vec::new(),
                failed,
                skipped: false,
                missing_source: false,
                jdk: true,
                local_jar: false,
                local_origin: Some(origin),
            });
        })
        .is_ok()
}

fn spawn_local_jar_index_worker(
    jar_path: std::path::PathBuf,
    origin_id: String,
    extract_root: std::path::PathBuf,
    generation: u64,
    tx: std::sync::mpsc::Sender<DependencyResult>,
) {
    let label = jar_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("local jar")
        .to_string();
    let thread_name = format!("ktlsp-local-jar-index-{label}");
    let _ = std::thread::Builder::new()
        .name(thread_name)
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            use crate::deps;
            use crate::java::JavaParser;
            use crate::parser::KotlinParser;

            let resolve_started = crate::trace::enabled().then(Instant::now);
            let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut kotlin = KotlinParser::new();
                let mut java = JavaParser::new();
                let batches = deps::resolve_local_jar_stubs_for_origin(
                    &jar_path,
                    &origin_id,
                    &extract_root,
                    &mut kotlin,
                    &mut java,
                )?;
                let missing_source = batches.is_empty()
                    && !verified_empty_binaries(&[(jar_path.clone(), origin_id.clone())])?;
                Ok((batches, missing_source))
            }))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("local artifact index worker panicked")));
            let result = match resolved {
                Ok((batches, missing_source)) => DependencyResult {
                    jdk_parse_plan: None,
                    coord: None,
                    label,
                    failed: false,
                    batches,
                    discovered: Vec::new(),
                    skipped: false,
                    missing_source,
                    jdk: false,
                    local_jar: true,
                    local_origin: Some(origin_id),
                },
                Err(error) => {
                    tracing::warn!("index {label} failed: {error:#}");
                    DependencyResult {
                        jdk_parse_plan: None,
                        coord: None,
                        label,
                        batches: Vec::new(),
                        discovered: Vec::new(),
                        failed: true,
                        skipped: false,
                        missing_source: false,
                        jdk: false,
                        local_jar: true,
                        local_origin: None,
                    }
                }
            };
            if let Some(started) = resolve_started {
                crate::trace::span(
                    "deps.jar_resolve",
                    "deps",
                    started,
                    serde_json::json!({"generation":generation, "label":&result.label, "origin":result.local_origin.as_deref(), "local":true, "binary_only":true, "files":result.batches.len(), "failed":result.failed}),
                );
            }
            let _ = tx.send(result);
        });
}

fn verified_empty_binaries(binaries: &[(std::path::PathBuf, String)]) -> anyhow::Result<bool> {
    let mut empty = !binaries.is_empty();
    for (binary, expected_origin) in binaries {
        anyhow::ensure!(
            binary.is_file() && classpath::file_origin_id(binary) == *expected_origin,
            "binary artifact changed since project discovery: {}",
            binary.display()
        );
        empty &= !crate::jar::may_contain_definitions(binary)?;
        anyhow::ensure!(
            binary.is_file() && classpath::file_origin_id(binary) == *expected_origin,
            "binary artifact changed while inspecting {}",
            binary.display()
        );
    }
    Ok(empty)
}

fn spawn_coordinate_index_worker(
    coord: crate::coords::Coordinate,
    repos: crate::artifacts::Repos,
    extract_root: std::path::PathBuf,
    indexed_sources: std::sync::Arc<parking_lot::Mutex<std::collections::BTreeSet<String>>>,
    authoritative: bool,
    selected_binaries: Vec<(std::path::PathBuf, String)>,
    model: Option<Arc<classpath::ProjectSnapshot>>,
    binary_only: bool,
    generation: u64,
    workspace: Arc<Mutex<Workspace>>,
    tx: std::sync::mpsc::Sender<DependencyResult>,
) {
    let _ = std::thread::Builder::new()
        .name(format!("ktlsp-dep-index-{}", coord.artifact))
        .stack_size(32 * 1024 * 1024)
        .spawn(move || {
            use crate::artifacts;
            use crate::deps;
            use crate::java::JavaParser;
            use crate::parser::KotlinParser;

            let label = coord.label();
            let resolve_started = crate::trace::enabled().then(Instant::now);
            let resolved = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let current = || workspace.lock().is_current_generation(generation);
                if !current() {
                    return (Vec::new(), false, Vec::new(), true, false);
                }
                let (source, source_failed) = if binary_only {
                    (None, false)
                } else {
                    match deps::coordinate_source_for_model(
                        &coord,
                        &repos,
                        &extract_root,
                        model.as_deref(),
                    ) {
                        Ok(source) => (source, false),
                        Err(_) => {
                            tracing::warn!("source selection for {label} is unavailable");
                            (None, true)
                        }
                    }
                };
                if !current() {
                    return (Vec::new(), false, Vec::new(), true, false);
                }
                let mut source_missing = source.is_none();
                let skipped = source
                    .as_ref()
                    .map(|source| {
                        let mut guard = indexed_sources.lock();
                        let identity = if authoritative {
                            format!("{}|{}", source.identity(), coord.label())
                        } else {
                            source.identity()
                        };
                        !guard.insert(identity)
                    })
                    .unwrap_or(false);
                let (batches, mut failed) = if skipped {
                    (Vec::new(), false)
                } else if let Some(source) = source {
                    let mut kotlin = KotlinParser::new();
                    let mut java = JavaParser::new();
                    match deps::resolve_library_source_while_current(
                        &source, &mut kotlin, &mut java, &current,
                    ) {
                        Ok(batches) => (batches, false),
                        Err(error) => {
                            if !current() {
                                return (Vec::new(), false, Vec::new(), true, false);
                            }
                            tracing::warn!("index {label} source failed: {error:#}");
                            source_missing = true;
                            let batches = if authoritative {
                                match deps::resolve_module_binary_stubs(
                                    &coord,
                                    &selected_binaries,
                                    &extract_root,
                                    &mut kotlin,
                                    &mut java,
                                ) {
                                    Ok(batches) => batches,
                                    Err(binary_error) => {
                                        tracing::warn!(
                                            "index {label} binary fallback failed: {binary_error:#}"
                                        );
                                        Vec::new()
                                    }
                                }
                            } else {
                                Vec::new()
                            };
                            (batches, true)
                        }
                    }
                } else if authoritative && current() {
                    let mut kotlin = KotlinParser::new();
                    let mut java = JavaParser::new();
                    match deps::resolve_module_binary_stubs(
                        &coord,
                        &selected_binaries,
                        &extract_root,
                        &mut kotlin,
                        &mut java,
                    ) {
                        Ok(batches) => (batches, false),
                        Err(error) => {
                            tracing::warn!("index {label} binary failed: {error:#}");
                            (Vec::new(), true)
                        }
                    }
                } else {
                    (Vec::new(), false)
                };
                failed |= source_failed;
                let empty_input = source_missing || (!skipped && batches.is_empty());
                let verified_empty_binary = if empty_input && authoritative && !failed {
                    match verified_empty_binaries(&selected_binaries) {
                        Ok(empty) => empty,
                        Err(error) => {
                            tracing::warn!("inspect {label} binary failed: {error:#}");
                            failed = true;
                            false
                        }
                    }
                } else {
                    false
                };
                let missing_source = empty_input && !verified_empty_binary;
                let discovered = if authoritative || !current() {
                    Vec::new()
                } else {
                    artifacts::dependency_coordinates(&repos, &coord)
                };
                (batches, failed, discovered, skipped, missing_source)
            }));
            let result = match resolved {
                Ok((batches, failed, discovered, skipped, missing_source)) => DependencyResult {
                    jdk_parse_plan: None,
                    coord: Some(coord),
                    label,
                    failed,
                    batches,
                    discovered,
                    skipped,
                    missing_source,
                    jdk: false,
                    local_jar: false,
                    local_origin: None,
                },
                Err(_) => DependencyResult {
                    jdk_parse_plan: None,
                    coord: Some(coord),
                    label,
                    batches: Vec::new(),
                    discovered: Vec::new(),
                    failed: true,
                    skipped: false,
                    missing_source: false,
                    jdk: false,
                    local_jar: false,
                    local_origin: None,
                },
            };
            if let Some(started) = resolve_started {
                crate::trace::span(
                    "deps.jar_resolve",
                    "deps",
                    started,
                    serde_json::json!({"generation":generation, "label":&result.label, "local":false, "binary_only":binary_only, "files":result.batches.len(), "failed":result.failed, "skipped":result.skipped}),
                );
            }
            let _ = tx.send(result);
        });
}

/// Index version-catalog dependencies and locally discoverable transitive source dependencies into
/// the shared index. Runs on a blocking thread; IO/parsing is lock-free and results are inserted
/// per-coordinate under brief locks so `goto_definition` can interleave while indexing proceeds.
fn index_dependencies(
    ws: &Arc<Mutex<Workspace>>,
    root: &std::path::Path,
    model: Option<Arc<classpath::ProjectSnapshot>>,
    gradle: bool,
    generation: u64,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<(usize, usize, String)>>,
    cancelled: &std::sync::atomic::AtomicBool,
) -> DepStats {
    index_dependencies_with_source_query(
        ws,
        root,
        model,
        gradle,
        generation,
        progress,
        cancelled,
        classpath::load_source_snapshot,
    )
}

fn index_dependencies_with_source_query(
    ws: &Arc<Mutex<Workspace>>,
    root: &std::path::Path,
    model: Option<Arc<classpath::ProjectSnapshot>>,
    gradle: bool,
    generation: u64,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<(usize, usize, String)>>,
    cancelled: &std::sync::atomic::AtomicBool,
    load_sources: impl FnOnce(
            &std::path::Path,
            &Arc<classpath::ProjectSnapshot>,
            &std::sync::atomic::AtomicBool,
        ) -> anyhow::Result<Arc<classpath::ProjectSnapshot>>
        + Send,
) -> DepStats {
    let pending = model
        .as_ref()
        .filter(|model| gradle && model.source_enrichment_needed())
        .cloned();
    let Some(accepted) = pending else {
        return index_dependency_model(ws, root, model, gradle, generation, progress, false);
    };
    // Source selection reads only the immutable accepted model. Keep its worker owned by
    // this blocking index job, overlap repository IO with binary preview, and publish neither
    // its result nor source-derived facts until both phases join and acceptance checks pass.
    let (binary, enriched) = std::thread::scope(|scope| {
        let query_model = accepted.clone();
        let query = std::thread::Builder::new()
            .name("ktlsp-source-query".to_string())
            .spawn_scoped(scope, move || load_sources(root, &query_model, cancelled));
        let started = Instant::now();
        let binary = index_dependency_model(
            ws,
            root,
            Some(accepted.clone()),
            gradle,
            generation,
            progress,
            true,
        );
        crate::trace::span(
            "deps.binary_preview",
            "deps",
            started,
            serde_json::json!({"generation":generation, "files":binary.files, "symbols":binary.symbols}),
        );
        let enriched = match query {
            Ok(query) => query.join().unwrap_or_else(|_| {
                Err(anyhow::anyhow!("Gradle source selection worker panicked"))
            }),
            Err(error) => Err(error.into()),
        };
        (binary, enriched)
    });
    if cancelled.load(std::sync::atomic::Ordering::Acquire)
        || !ws.lock().is_current_generation(generation)
    {
        return binary;
    }
    let enriched = match enriched {
        Ok(enriched) => enriched,
        Err(_) => {
            tracing::warn!("Gradle source selection enrichment is unavailable");
            return binary;
        }
    };
    {
        let mut guard = ws.lock();
        if cancelled.load(std::sync::atomic::Ordering::Acquire)
            || !guard.is_current_generation(generation)
        {
            return binary;
        }
        if Arc::ptr_eq(&accepted, &enriched) {
            return binary;
        }
        guard.set_project_model(enriched.clone());
        classpath::publish_snapshot(root, enriched.clone());
    }
    if ws.lock().is_current_generation(generation) {
        classpath::store_snapshot(root, &enriched);
    }
    let mut source = index_dependency_model(
        ws,
        root,
        Some(enriched),
        gradle,
        generation,
        progress,
        false,
    );
    source.files += binary.files;
    source.symbols += binary.symbols;
    source.jdk_files += binary.jdk_files;
    source.jdk_symbols += binary.jdk_symbols;
    source.local_stub_files += binary.local_stub_files;
    source.local_stub_symbols += binary.local_stub_symbols;
    source.local_source_files += binary.local_source_files;
    source.local_source_symbols += binary.local_source_symbols;
    source.jdk_reused &= binary.jdk_reused;
    source
}

fn index_dependency_model(
    ws: &Arc<Mutex<Workspace>>,
    root: &std::path::Path,
    model: Option<Arc<classpath::ProjectSnapshot>>,
    gradle: bool,
    generation: u64,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<(usize, usize, String)>>,
    binary_only: bool,
) -> DepStats {
    std::thread::scope(|scope| {
        index_dependency_model_with_jdk_plan(
            scope,
            ws,
            root,
            model,
            gradle,
            generation,
            progress,
            binary_only,
            dependency_index_threads(),
            Arc::new(crate::deps::plan_jdk_index),
        )
    })
}

fn index_dependency_model_with_jdk_plan<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    ws: &Arc<Mutex<Workspace>>,
    root: &std::path::Path,
    model: Option<Arc<classpath::ProjectSnapshot>>,
    gradle: bool,
    generation: u64,
    progress: Option<&tokio::sync::mpsc::UnboundedSender<(usize, usize, String)>>,
    binary_only: bool,
    max_workers: usize,
    planner: Arc<JdkPlanner>,
) -> DepStats {
    use std::collections::{BTreeMap, BTreeSet, VecDeque};

    use crate::artifacts::Repos;
    use crate::coords::Coordinate;
    use crate::deps::{self, CoordinateDecision, CoordinateSelector};

    let repos = Repos::defaults();
    let mut coords = Vec::new();
    if !gradle {
        coords = deps::coordinates_for_root(root);
        coords.extend(deps::cached_catalog_coordinates(root, &repos));
        coords.extend(deps::coordinates_from_build_files(root, &repos));
        // Standalone editing uses the documented product default; Gradle owns its classpath.
        deps::inject_stdlib(&mut coords, true);
    }
    let mut local_jars = VecDeque::new();
    let authoritative = gradle
        && model
            .as_ref()
            .is_some_and(|m| m.modules.iter().any(|m| !m.source_sets.is_empty()));
    if gradle && !authoritative {
        return DepStats {
            incomplete_classpath: true,
            ..Default::default()
        };
    }
    if authoritative {
        let model = model.as_ref().unwrap();
        coords = model.coordinates();
        local_jars.extend(model.local_jars());
    }
    let mut queue: VecDeque<_> = coords.into();
    let mut seen = BTreeSet::new();
    let mut selected = CoordinateSelector::new();
    let mut indexed_files: BTreeMap<Coordinate, Vec<IndexedDependencyFile>> = BTreeMap::new();
    let extract_root = deps::extract_root();
    let jdk_sources = if authoritative {
        model.as_ref().unwrap().selected_jdk_sources()
    } else {
        deps::jdk_src_zip().into_iter().collect()
    };
    let mut stats = DepStats {
        incomplete_classpath: gradle
            && (!authoritative
                || model.as_ref().is_some_and(|m| {
                    m.modules.iter().any(|module| {
                        !module.unresolved.is_empty()
                            || module.source_sets.iter().any(|source| {
                                source.classpath_incomplete || !source.unresolved.is_empty()
                            })
                    })
                })),
        ..Default::default()
    };
    const MAX_DEPENDENCY_COORDINATES: usize = 1024;
    let indexed_sources = std::sync::Arc::new(parking_lot::Mutex::new(BTreeSet::new()));
    let (tx, rx) = std::sync::mpsc::channel::<DependencyResult>();
    let mut active = 0usize;
    let mut completed = 0usize;
    let mut suppressed = BTreeSet::new();

    struct JdkShardState {
        pending: usize,
        failed: bool,
        plan: deps::JdkParsePlan,
        batches: Vec<deps::FileSymbols>,
    }
    let mut jdk_states: BTreeMap<String, JdkShardState> = BTreeMap::new();
    let mut jdk_pending: VecDeque<(String, deps::JdkParsePlan)> = VecDeque::new();
    let mut jdk_plans = VecDeque::new();
    let mut jdk_origins = jdk_sources
        .iter()
        .map(|source| classpath::jdk_origin_id(source))
        .collect::<Vec<_>>();
    let api_selections = model
        .as_ref()
        .map(|model| model.selected_release_apis())
        .unwrap_or_default();
    jdk_origins.extend(
        api_selections
            .iter()
            .map(classpath::SdkApiSelection::origin),
    );
    stats.jdk_reused = !jdk_origins.is_empty()
        && jdk_origins
            .iter()
            .all(|origin| ws.lock().dependency_origin_complete(origin));
    let jdk_shards = (max_workers / jdk_sources.len().max(1)).max(1);
    let mut sdk_api_pending: VecDeque<_> = api_selections
        .into_iter()
        .filter(|selection| !ws.lock().dependency_origin_complete(&selection.origin()))
        .collect();
    for (source, origin) in jdk_sources.into_iter().zip(&jdk_origins) {
        crate::trace::span(
            "deps.jdk_selection",
            "deps",
            Instant::now(),
            serde_json::json!({"generation":generation, "archive":source, "origin":origin, "authoritative":authoritative}),
        );
        if !ws.lock().dependency_origin_complete(origin) {
            jdk_plans.push_back((source, origin.clone()));
        }
    }

    while active > 0
        || !jdk_plans.is_empty()
        || !jdk_pending.is_empty()
        || !sdk_api_pending.is_empty()
        || !local_jars.is_empty()
        || !queue.is_empty()
    {
        if !ws.lock().is_current_generation(generation) {
            queue.clear();
            local_jars.clear();
            jdk_plans.clear();
            jdk_pending.clear();
            sdk_api_pending.clear();
        }
        while active < max_workers {
            if let Some(selection) = sdk_api_pending.pop_front() {
                if spawn_sdk_api_worker(selection, extract_root.clone(), tx.clone()) {
                    active += 1;
                } else {
                    stats.failed += 1;
                }
                continue;
            }
            if let Some((source, origin)) = jdk_plans.pop_front() {
                if spawn_jdk_plan_worker(
                    scope,
                    source,
                    extract_root.clone(),
                    origin,
                    generation,
                    planner.clone(),
                    tx.clone(),
                ) {
                    active += 1;
                } else {
                    stats.failed += 1;
                }
                continue;
            }
            let jdk_slots = (!jdk_pending.is_empty()).then_some(jdk_shards);
            if jdk_slots.is_some_and(|slots| slots <= max_workers - active) {
                let (origin, plan) = jdk_pending.pop_front().unwrap();
                if let Some(progress) = progress {
                    let _ = progress.send((
                        completed + active + 1,
                        completed + active + queue.len() + jdk_pending.len() + 1,
                        "JDK sources".to_string(),
                    ));
                }
                let spawned = spawn_jdk_shard_workers(
                    plan.clone(),
                    origin.clone(),
                    jdk_shards,
                    generation,
                    tx.clone(),
                );
                if spawned < jdk_shards {
                    stats.failed += 1;
                }
                if spawned > 0 {
                    jdk_states.insert(
                        origin,
                        JdkShardState {
                            pending: spawned,
                            failed: spawned < jdk_shards,
                            plan,
                            batches: Vec::new(),
                        },
                    );
                    active += spawned;
                }
                continue;
            }
            if let Some(jar_path) = local_jars.pop_front() {
                stats.local_jars += 1;
                let progress_total =
                    (seen.len() + queue.len() + local_jars.len()).max(completed + active + 1);
                if let Some(tx) = progress {
                    let label = jar_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("local jar")
                        .to_string();
                    let _ = tx.send((completed + active + 1, progress_total, label));
                }
                let origin_id = model
                    .as_ref()
                    .and_then(|m| m.artifact_origin(&jar_path))
                    .map(str::to_string)
                    .unwrap_or_else(|| classpath::file_origin_id(&jar_path));
                if ws.lock().dependency_origin_complete(&origin_id) {
                    stats.reused_local_jars += 1;
                    continue;
                }
                spawn_local_jar_index_worker(
                    jar_path,
                    origin_id,
                    extract_root.clone(),
                    generation,
                    tx.clone(),
                );
                active += 1;
                continue;
            }
            let Some(coord) = queue.pop_front() else {
                break;
            };
            if !seen.insert(coord.clone()) {
                continue;
            }
            stats.coordinates = seen.len();
            if !binary_only
                && ws
                    .lock()
                    .dependency_origin_complete(&classpath::maven_origin_id(&coord))
            {
                stats.reused_coordinates += 1;
                continue;
            }
            match if authoritative {
                CoordinateDecision::Selected
            } else {
                selected.consider(coord.clone())
            } {
                CoordinateDecision::Selected => {}
                CoordinateDecision::Replaces(previous) => {
                    stats.shadowed += 1;
                    suppressed.insert(previous.clone());
                    if let Some(files) = indexed_files.remove(&previous) {
                        let mut guard = ws.lock();
                        if !guard.is_current_generation(generation) {
                            continue;
                        }
                        for file in files {
                            guard.remove_dependency_file(&file.path);
                            stats.files = stats.files.saturating_sub(1);
                            stats.symbols = stats.symbols.saturating_sub(file.symbols);
                        }
                        guard.bump_index_revision();
                    }
                }
                CoordinateDecision::ShadowedBy(_) => {
                    stats.shadowed += 1;
                    continue;
                }
            }
            stats.coordinates = seen.len();
            let progress_total =
                (seen.len() + queue.len() + local_jars.len()).max(completed + active + 1);
            if let Some(tx) = progress {
                let _ = tx.send((completed + active + 1, progress_total, coord.label()));
            }
            spawn_coordinate_index_worker(
                coord.clone(),
                repos.clone(),
                extract_root.clone(),
                indexed_sources.clone(),
                authoritative,
                model
                    .as_ref()
                    .and_then(|model| model.coordinate_binaries(&coord))
                    .unwrap_or_default(),
                model.clone(),
                binary_only,
                generation,
                ws.clone(),
                tx.clone(),
            );
            active += 1;
        }

        if active == 0 {
            continue;
        }

        let Ok(mut result) = rx.recv() else {
            break;
        };
        active = active.saturating_sub(1);
        if let Some(plan) = result.jdk_parse_plan.take() {
            if ws.lock().is_current_generation(generation) {
                jdk_pending.push_back((result.local_origin.unwrap(), plan));
            }
            continue;
        }
        completed += 1;

        if result.jdk
            && result
                .local_origin
                .as_ref()
                .is_some_and(|origin| jdk_states.contains_key(origin))
        {
            let origin = result.local_origin.as_ref().unwrap();
            let state = jdk_states.get_mut(origin).unwrap();
            state.pending = state.pending.saturating_sub(1);
            if result.failed {
                stats.failed += 1;
                state.failed = true;
            } else {
                state.batches.extend(result.batches);
            }
            if state.pending == 0 {
                let mut state = jdk_states.remove(origin).unwrap();
                if !state.failed {
                    let store_started = crate::trace::enabled().then(Instant::now);
                    deps::store_jdk_symcache(&state.plan, &state.batches);
                    if let Some(started) = store_started {
                        crate::trace::span(
                            "deps.jdk_symcache_store",
                            "deps",
                            started,
                            serde_json::json!({"generation":generation, "origin":origin,
                                "files":state.batches.len(), "attempted":true}),
                        );
                    }
                }
                let publish_started = crate::trace::enabled().then(Instant::now);
                let expected_files = state.batches.len();
                let previous_jdk_files = stats.jdk_files;
                for batch in state.batches.drain(..) {
                    let mut guard = ws.lock();
                    if !guard.is_current_generation(generation) {
                        continue;
                    }
                    let symbol_count = batch.symbols.len();
                    stats.symbols += symbol_count;
                    stats.jdk_symbols += symbol_count;
                    stats.jdk_files += 1;
                    guard.index_dependency_batch_for_generation(generation, batch);
                    stats.files += 1;
                }
                {
                    let mut guard = ws.lock();
                    if guard.is_current_generation(generation) {
                        guard.bump_index_revision();
                        if !state.failed {
                            guard.complete_dependency_origin(origin.clone());
                        }
                    }
                }
                if let Some(started) = publish_started {
                    crate::trace::span(
                        "deps.jdk_publish",
                        "deps",
                        started,
                        serde_json::json!({"generation":generation, "origin":origin,
                            "expected_files":expected_files, "attempted_files":stats.jdk_files - previous_jdk_files,
                            "skipped_generation_files":expected_files - (stats.jdk_files - previous_jdk_files),
                            "parse_failed":state.failed}),
                    );
                }
            }
            continue;
        }

        if result.failed {
            stats.failed += 1;
            if !result.source_error_binary_fallback() {
                if result.jdk {
                    tracing::warn!("indexing JDK sources failed; skipping");
                } else {
                    tracing::warn!("indexing failed for {}; skipping", result.label);
                }
                continue;
            }
            tracing::warn!(coordinate = %result.label,
                "source indexing failed; retaining selected binary members with incomplete source coverage");
        }
        if result.skipped {
            stats.skipped += 1;
        }
        if result.missing_source {
            tracing::warn!(coordinate = %result.label, "dependency source unavailable");
            stats.missing_sources += 1;
        }
        if result
            .coord
            .as_ref()
            .is_some_and(|coord| suppressed.contains(coord))
        {
            continue;
        }
        let install_start = crate::trace::enabled().then(Instant::now);
        let previous_files = stats.files;
        let mut files_for_coord = Vec::new();
        for batch in result.batches {
            let mut guard = ws.lock();
            if !guard.is_current_generation(generation) {
                continue;
            }
            let symbol_count = batch.symbols.len();
            stats.symbols += symbol_count;
            if result.jdk {
                stats.jdk_symbols += symbol_count;
                stats.jdk_files += 1;
            } else if result.local_jar {
                if batch.origin.as_ref().is_some_and(|origin| {
                    origin.representation == crate::project_model::SourceRepresentation::Source
                }) {
                    stats.local_source_symbols += symbol_count;
                    stats.local_source_files += 1;
                } else {
                    stats.local_stub_symbols += symbol_count;
                    stats.local_stub_files += 1;
                }
            } else {
                files_for_coord.push(IndexedDependencyFile {
                    path: batch.file.clone(),
                    symbols: symbol_count,
                });
            }
            guard.index_dependency_batch_for_generation(generation, batch);
            guard.bump_index_revision();
            stats.files += 1;
        }
        if let Some(start) = install_start {
            crate::trace::span(
                "deps.install_batch",
                "deps",
                start,
                serde_json::json!({ "label": result.label, "files": stats.files - previous_files }),
            );
        }
        if let Some(coord) = result.coord.clone() {
            indexed_files.insert(coord, files_for_coord);
        }
        if !result.missing_source {
            let origin = result
                .coord
                .as_ref()
                .map(classpath::maven_origin_id)
                .or(result.local_origin);
            let mut guard = ws.lock();
            if guard.is_current_generation(generation) {
                if let Some(origin) = origin {
                    guard.complete_dependency_origin(origin);
                }
            }
        }
        for dep in result.discovered {
            if seen.len() + queue.len() + active >= MAX_DEPENDENCY_COORDINATES {
                break;
            }
            if !seen.contains(&dep) && !queue.iter().any(|queued| queued == &dep) {
                queue.push_back(dep);
            }
        }
    }
    stats.jdk_complete = !jdk_origins.is_empty()
        && jdk_origins
            .iter()
            .all(|origin| ws.lock().dependency_origin_complete(origin));
    stats
}

/// Warm the index off the request path, reporting progress to the client when it supports work-done
/// progress (rust-analyzer-style status: "scanning project", then "indexing <coordinate> (n/total)").
/// Falls back to log messages otherwise. Summaries are always logged.
async fn prime_selected_compiler_api(
    ws: &Arc<Mutex<Workspace>>,
    workers: &Arc<tokio::sync::Semaphore>,
) {
    let targets = ws.lock().compiler_api_targets();
    for key in targets {
        let Ok(permit) = workers.clone().acquire_owned().await else {
            return;
        };
        let request = match ws.lock().begin_kotlin_compiler_api(&key) {
            Ok(request) => request,
            Err(_) => continue,
        };
        let ws = ws.clone();
        let _ = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let start = Instant::now();
            let (wake, _events) = tokio::sync::mpsc::channel(1);
            let environment = match crate::compile::KotlinDiagnosticEnvironment::prepare(&request, wake) {
                Ok(environment) => environment,
                Err(error) => { tracing::debug!(%error, "initial compiler API environment unavailable"); return; }
            };
            let outcome = crate::compile::run_kotlin_editor_compile(&request, &environment);
            let compiler_error = match &outcome {
                crate::compile::KotlinDiagnosticOutcome::Unverified(reason) => Some(reason.clone()),
                _ => None,
            };
            let snapshot = environment.compiler_api.lock().ok().and_then(|mut snapshot| snapshot.take());
            let mut ws = ws.lock();
            let accepted = snapshot.map(|snapshot| ws.accept_kotlin_compiler_api(&request, environment.clone(), snapshot));
            ws.finish_kotlin_diagnostics(&request, outcome, environment.invalid.clone());
            crate::trace::span("compiler.api.initialize", "compiler", start,
                serde_json::json!({"generation": request.generation, "accepted": accepted.as_ref().is_some_and(|result| result.is_ok()), "error": accepted.and_then(|result| result.err()), "compiler_error": compiler_error, "processor_plan": request.processor_plan.is_some()}));
        }).await;
    }
}

async fn index_workspace(
    client: Client,
    ws: Arc<Mutex<Workspace>>,
    versions: Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    root: PathBuf,
    progress: bool,
    gradle: bool,
    generation: u64,
    force: bool,
    watches: Arc<Mutex<WatchState>>,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
    compiler_workers: Arc<tokio::sync::Semaphore>,
) {
    // 1. Project sources (fast).
    let scan_ws = ws.clone();
    let scan_root = root.clone();

    let ongoing = if progress {
        let token = ProgressToken::String("ktlsp/index".to_string());
        // Server-initiated progress requires creating the token first (per the LSP spec).
        let _ = client.create_work_done_progress(token.clone()).await;
        Some(
            client
                .progress(token, "ktlsp: indexing")
                .with_message("scanning project")
                .with_percentage(0)
                .begin()
                .await,
        )
    } else {
        None
    };

    let scan_start = Instant::now();
    if gradle {
        if !watches.lock().configuration_baselined {
            let configuration_root = root.clone();
            let contents =
                tokio::task::spawn_blocking(move || configuration_contents(&configuration_root))
                    .await
                    .unwrap_or_default();
            let mut state = watches.lock();
            for (key, content) in contents {
                state.configuration_contents.entry(key).or_insert(content);
            }
            state.configuration_baselined = true;
        }
        let registered = watches.lock().registration.is_some();
        if !registered {
            let watch_start = Instant::now();
            register_project_watches(&client, &root, None, &watches).await;
            crate::trace::span(
                "workspace.watch_initial",
                "workspace",
                watch_start,
                serde_json::json!({"root": root.to_string_lossy(), "generation": generation}),
            );
        }
    }
    if !ws.lock().is_current_generation(generation) {
        if let Some(p) = ongoing {
            p.finish_with_message("superseded by project changes").await;
        }
        return;
    }
    let model = if gradle {
        watches.lock().model_loading = true;
        let model_start = Instant::now();
        let load_root = root.clone();
        let model_cancelled = cancelled.clone();
        let result = tokio::task::spawn_blocking(move || {
            classpath::load_snapshot_cancellable(&load_root, force, Some(&model_cancelled))
        })
        .await;
        {
            let mut watches = watches.lock();
            watches.model_loading = false;
            watches.pending_model_jar = false;
        }
        crate::trace::span(
            "workspace.model_load",
            "workspace",
            model_start,
            serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
                "forcedModel": force, "ok": matches!(&result, Ok(Ok(_)))}),
        );
        if !ws.lock().is_current_generation(generation) {
            if let Some(p) = ongoing {
                p.finish_with_message("superseded by project changes").await;
            }
            return;
        }
        match result {
            Ok(Ok(model)) => Some(model),
            _ => {
                tracing::warn!(
                    generation,
                    "Gradle model refresh failed; scope remains incomplete"
                );
                Some(Arc::new(classpath::ProjectSnapshot::new(Vec::new())))
            }
        }
    } else {
        None
    };
    let count = if let Some(model) = &model {
        let prepare_start = Instant::now();
        let known = ws.lock().source_stamps();
        let prepared_model = model.clone();
        let update = tokio::task::spawn_blocking(move || {
            crate::workspace::prepare_model_sources(&prepared_model, &known)
        })
        .await;
        crate::trace::span(
            "workspace.source_prepare",
            "workspace",
            prepare_start,
            serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
                "joined": update.is_ok()}),
        );
        let Ok(update) = update else {
            if let Some(p) = ongoing {
                p.finish_with_message("project source refresh failed").await;
            }
            return;
        };
        let install_start = Instant::now();
        let count = {
            let mut guard = ws.lock();
            let count = guard.install_project_update(generation, model.clone(), update);
            if count.is_some() {
                classpath::publish_snapshot(&root, model.clone());
            }
            count
        };
        crate::trace::span(
            "workspace.source_install",
            "workspace",
            install_start,
            serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
                "accepted": count.is_some(), "files": count}),
        );
        let Some(count) = count else {
            if let Some(p) = ongoing {
                p.finish_with_message("superseded by project changes").await;
            }
            return;
        };
        if model
            .modules
            .iter()
            .any(|module| !module.source_sets.is_empty())
        {
            let store_start = Instant::now();
            let cache_model = model.clone();
            let cache_root = root.clone();
            let cache_ws = ws.clone();
            let stored = tokio::task::spawn_blocking(move || {
                let current = cache_ws.lock().is_current_generation(generation);
                if current {
                    classpath::store_snapshot(&cache_root, &cache_model);
                }
            })
            .await;
            crate::trace::span(
                "workspace.model_store",
                "workspace",
                store_start,
                serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
                    "joined": stored.is_ok()}),
            );
        }
        let watch_start = Instant::now();
        register_project_watches(&client, &root, Some(model), &watches).await;
        crate::trace::span(
            "workspace.watch_selected",
            "workspace",
            watch_start,
            serde_json::json!({"root": root.to_string_lossy(), "generation": generation}),
        );
        count
    } else {
        tokio::task::spawn_blocking(move || {
            let mut guard = scan_ws.lock();
            if guard.is_current_generation(generation) {
                guard.scan(&scan_root)
            } else {
                0
            }
        })
        .await
        .unwrap_or(0)
    };
    if !ws.lock().is_current_generation(generation) {
        if let Some(p) = ongoing {
            p.finish_with_message("superseded by project changes").await;
        }
        return;
    }
    crate::trace::span(
        "workspace.project_scan",
        "workspace",
        scan_start,
        serde_json::json!({
            "root": root.to_string_lossy(),
            "files": count,
            "generation": generation,
            "forcedModel": force,
        }),
    );
    client
        .log_message(
            MessageType::INFO,
            format!("ktlsp indexed {count} project files"),
        )
        .await;
    if let Some(p) = &ongoing {
        p.report_with_message(
            format!("indexed {count} project files; resolving dependencies"),
            5,
        )
        .await;
    }

    // 2. Library sources from the version catalog. Stream per-coordinate progress over a channel so
    //    the blocking indexer can report into the async progress without blocking on it.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(usize, usize, String)>();
    let index_ws = ws.clone();
    let index_root = root.clone();
    let deps_start = Instant::now();
    let handle = tokio::task::spawn_blocking(move || {
        // Fingerprint selected mutable archives and the tracked inventory before indexing.
        // A post-index fingerprint alone could accept a replacement
        // whose new bytes were never read by this generation.
        let archive_contents = project_archive_contents(&index_root, model.as_deref());
        let stats = index_dependencies(
            &index_ws,
            &index_root,
            model,
            gradle,
            generation,
            Some(&tx),
            &cancelled,
        );
        let archive_contents = archive_contents
            .into_iter()
            .filter(|(key, before)| configuration_content(Path::new(key)).as_ref() == Some(before))
            .collect();
        (stats, archive_contents)
    });
    while let Some((done, total, label)) = rx.recv().await {
        if let Some(p) = &ongoing {
            let pct = if total == 0 {
                100
            } else {
                (5 + 95 * done / total).min(100) as u32
            };
            p.report_with_message(format!("indexing {label} ({done}/{total})"), pct)
                .await;
        }
    }
    let (stats, archive_contents) = handle.await.unwrap_or_default();
    if !ws.lock().is_current_generation(generation) {
        return;
    }
    if gradle {
        let enriched = classpath::snapshot(&root);
        register_project_watches(&client, &root, enriched.as_deref(), &watches).await;
    }
    if !ws.lock().is_current_generation(generation) {
        if let Some(p) = ongoing {
            p.finish_with_message("superseded by project changes").await;
        }
        return;
    }

    let jdk_summary = if stats.jdk_files > 0 {
        format!(
            ", including {} JDK files ({} symbols)",
            stats.jdk_files, stats.jdk_symbols
        )
    } else if stats.jdk_reused {
        ", reusing indexed JDK sources".to_string()
    } else {
        String::new()
    };
    let local_jar_summary = if stats.local_stub_files + stats.local_source_files > 0 {
        format!(
            ", plus {} local-jar source files ({} symbols) and {} stub files ({} symbols)",
            stats.local_source_files,
            stats.local_source_symbols,
            stats.local_stub_files,
            stats.local_stub_symbols
        )
    } else {
        String::new()
    };
    let summary = format!(
        "ktlsp indexed {} library files ({} symbols) from {} dependencies{}{} ({} failed, {} missing sources, {} shadowed, {} duplicate skipped, {} dependencies and {} local jars reused)",
        stats.files,
        stats.symbols,
        stats.coordinates,
        jdk_summary,
        local_jar_summary,
        stats.failed,
        stats.missing_sources,
        stats.shadowed,
        stats.skipped,
        stats.reused_coordinates,
        stats.reused_local_jars,
    );
    let accepted = {
        let mut guard = ws.lock();
        let accepted = guard.finish_analysis_generation(
            generation,
            stats.failed == 0 && stats.missing_sources == 0 && !stats.incomplete_classpath,
            stats.jdk_complete,
        );
        if accepted {
            watches.lock().archive_contents = archive_contents;
            crate::trace::span(
                "workspace.accepted_finish",
                "workspace",
                Instant::now(),
                serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
                    "failed": stats.failed, "missingSources": stats.missing_sources,
                    "classpathComplete": !stats.incomplete_classpath}),
            );
        }
        accepted
    };
    if !accepted {
        if let Some(p) = ongoing {
            p.finish_with_message("superseded by project changes").await;
        }
        return;
    }
    prime_selected_compiler_api(&ws, &compiler_workers).await;
    crate::trace::span(
        "deps.index_workspace",
        "deps",
        deps_start,
        serde_json::json!({"root": root.to_string_lossy(), "generation": generation,
            "coordinates": stats.coordinates, "localJars": stats.local_jars,
            "reusedCoordinates": stats.reused_coordinates, "reusedLocalJars": stats.reused_local_jars,
            "files": stats.files, "symbols": stats.symbols, "failed": stats.failed,
            "missingSources": stats.missing_sources, "jdkFiles": stats.jdk_files, "jdkReused": stats.jdk_reused,
            "jdkComplete": stats.jdk_complete, "jdkSymbols": stats.jdk_symbols,
            "localStubFiles": stats.local_stub_files, "localStubSymbols": stats.local_stub_symbols,
            "localSourceFiles": stats.local_source_files, "localSourceSymbols": stats.local_source_symbols,
            "classpathComplete": !stats.incomplete_classpath}),
    );
    // Open buffers may have received conservative diagnostics before dependency/JDK indexing
    // completed. Republish them now that the library completeness facts and durable symbols are in.
    let open_keys = {
        let mut guard = ws.lock();
        let mut keys = guard.open_doc_keys();
        keys.extend(guard.take_extension_diagnostic_changes());
        keys.sort();
        keys.dedup();
        keys
    };
    for key in open_keys {
        if !ws.lock().is_current_generation(generation) {
            if let Some(p) = ongoing {
                p.finish_with_message("superseded by project changes").await;
            }
            return;
        }
        publish_diagnostics(&ws, &client, &versions, &key).await;
    }
    if !ws.lock().is_current_generation(generation) {
        if let Some(p) = ongoing {
            p.finish_with_message("superseded by project changes").await;
        }
        return;
    }
    client.log_message(MessageType::INFO, summary.clone()).await;
    if let Some(p) = ongoing {
        p.finish_with_message(format!(
            "indexed {} dependencies, {} files",
            stats.coordinates, stats.files
        ))
        .await;
    }
}

fn project_watchers(
    root: &Path,
    model: Option<&classpath::ProjectSnapshot>,
    relative: bool,
) -> Vec<FileSystemWatcher> {
    use std::collections::BTreeSet;
    let root_key = crate::paths::file_key(root);
    let mut patterns = BTreeSet::from([
        (
            root_key.clone(),
            "**/*.{gradle,gradle.kts,toml,jar}".to_string(),
        ),
        (
            root_key.clone(),
            "**/{gradle.properties,gradle-wrapper.properties}".to_string(),
        ),
    ]);
    if let Some(model) = model {
        let mut workspace_files = false;
        for module in &model.modules {
            for source in &module.source_sets {
                for directory in &source.source_roots {
                    let directory = crate::paths::file_key(directory);
                    if path_is_under(&directory, &root_key) {
                        workspace_files = true;
                    } else {
                        patterns.insert((directory, "**/*.{kt,kts,java}".to_string()));
                    }
                }
                for file in source
                    .compiler_tasks
                    .iter()
                    .filter(|task| {
                        task.language == classpath::CompilerLanguage::Kotlin
                            && source.kotlin_compile_task.as_deref() == Some(task.task.as_str())
                    })
                    .flat_map(|task| &task.source_files)
                {
                    let key = crate::paths::file_key(file);
                    if path_is_under(&key, &root_key) {
                        workspace_files = true;
                        continue;
                    }
                    if source
                        .source_roots
                        .iter()
                        .any(|directory| path_is_under(&key, &crate::paths::file_key(directory)))
                    {
                        continue;
                    }
                    if let (Some(parent), Some(name)) = (
                        file.parent(),
                        file.file_name().and_then(|name| name.to_str()),
                    ) {
                        patterns.insert((crate::paths::file_key(parent), glob_literal(name)));
                    }
                }
                for directory in &source.output_roots {
                    let directory = crate::paths::file_key(directory);
                    if path_is_under(&directory, &root_key) {
                        workspace_files = true;
                    } else {
                        patterns.insert((directory, "**/*".to_string()));
                    }
                }
            }
        }
        let archives = model
            .local_jars()
            .into_iter()
            .flat_map(|jar| {
                std::iter::once(jar.clone()).chain(classpath::adjacent_sources_jar(&jar))
            })
            .chain(
                model
                    .modules
                    .iter()
                    .flat_map(|module| &module.source_sets)
                    .flat_map(|source| {
                        source
                            .source_artifacts
                            .iter()
                            .flatten()
                            .chain(source.source_poms.iter().flatten())
                    })
                    .map(|artifact| artifact.path.clone()),
            );
        for artifact in archives {
            if path_is_under(&crate::paths::file_key(&artifact), &root_key)
                && artifact.extension().and_then(|value| value.to_str()) != Some("pom")
            {
                continue;
            }
            if let (Some(parent), Some(name)) = (
                artifact.parent(),
                artifact.file_name().and_then(|name| name.to_str()),
            ) {
                patterns.insert((crate::paths::file_key(parent), glob_literal(name)));
            }
        }
        if workspace_files {
            // Thousands of same-base globs exhaust some clients; project_change filters events.
            patterns.retain(|(directory, _)| directory != &root_key);
            patterns.insert((root_key, "**/*".to_string()));
        }
    }
    patterns
        .into_iter()
        .filter_map(|(directory, pattern)| {
            let requested = Path::new(&directory);
            let mut base = requested;
            while !base.is_dir() {
                base = base.parent()?;
                // A not-yet-created model directory must not turn into a drive-wide watch.
                if base.parent().is_none() {
                    return None;
                }
            }
            let prefix = crate::paths::file_key(requested.strip_prefix(base).ok()?);
            let pattern = if prefix.is_empty() {
                pattern
            } else {
                format!("{}/{pattern}", glob_literal(&prefix))
            };
            let directory = crate::paths::file_key(base);
            let glob_pattern = if relative {
                GlobPattern::Relative(RelativePattern {
                    base_uri: OneOf::Right(key_to_uri(&directory)?),
                    pattern,
                })
            } else {
                GlobPattern::String(format!(
                    "{}/{}",
                    glob_literal(directory.trim_end_matches('/')),
                    pattern
                ))
            };
            Some(FileSystemWatcher {
                glob_pattern,
                kind: None,
            })
        })
        .collect()
}

fn glob_literal(text: &str) -> String {
    let mut literal = String::new();
    for ch in text.chars() {
        match ch {
            '*' | '?' | '[' | ']' | '{' | '}' | '\\' => {
                literal.push('\\');
                literal.push(ch);
            }
            _ => literal.push(ch),
        }
    }
    literal
}

async fn register_project_watches(
    client: &Client,
    root: &Path,
    model: Option<&classpath::ProjectSnapshot>,
    state: &Arc<Mutex<WatchState>>,
) {
    let (registration, relative) = {
        let mut state = state.lock();
        if !state.dynamic {
            return;
        }
        state.sequence += 1;
        (
            format!("ktlsp/project-files/{}", state.sequence),
            state.relative,
        )
    };
    let options = DidChangeWatchedFilesRegistrationOptions {
        watchers: project_watchers(root, model, relative),
    };
    let options = serde_json::to_value(options).unwrap();
    let result = client
        .register_capability(vec![Registration {
            id: registration.clone(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: Some(options.clone()),
        }])
        .await;
    if let Err(error) = result {
        tracing::warn!(%error, "client rejected Gradle file watches; explicit watched-file notifications remain supported");
        return;
    }
    let previous = {
        let mut state = state.lock();
        state.registered_options = Some(options);
        state.registration.replace(registration)
    };
    if let Some(previous) = previous {
        let _ = client
            .unregister_capability(vec![Unregistration {
                id: previous,
                method: "workspace/didChangeWatchedFiles".to_string(),
            }])
            .await;
    }
}

fn is_gradle_wrapper(file: &Path) -> bool {
    if !file.ends_with("gradle/wrapper/gradle-wrapper.jar") {
        return false;
    }
    let Some(root) = file.ancestors().nth(3) else {
        return false;
    };
    ["gradlew", "gradlew.bat"]
        .iter()
        .any(|name| root.join(name).is_file())
        && [
            "settings.gradle",
            "settings.gradle.kts",
            "build.gradle",
            "build.gradle.kts",
        ]
        .iter()
        .any(|name| root.join(name).is_file())
}

fn is_project_configuration(file: &Path) -> bool {
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    (name.ends_with(".gradle") && name != ".gradle")
        || name.ends_with(".gradle.kts")
        || name.ends_with(".toml")
        || matches!(name, "gradle.properties" | "gradle-wrapper.properties")
        || is_gradle_wrapper(file)
}

fn model_archive_origins(model: &classpath::ProjectSnapshot) -> HashMap<String, String> {
    let local_jars = model.local_jars();
    let project_jars = model
        .modules
        .iter()
        .flat_map(|module| &module.source_sets)
        .flat_map(|source| &source.artifacts)
        .filter(|artifact| matches!(artifact.component, classpath::ComponentId::Project { .. }))
        .map(|artifact| &artifact.path);
    local_jars
        .iter()
        .chain(project_jars)
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("jar"))
        .map(|path| crate::paths::file_key(path))
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .filter_map(|key| {
            let origin = model.artifact_origin(Path::new(&key))?.to_string();
            Some((key, origin))
        })
        .collect()
}

fn project_archive_contents(
    root: &Path,
    model: Option<&classpath::ProjectSnapshot>,
) -> HashMap<String, [u8; 32]> {
    let mut keys: std::collections::BTreeSet<_> = model
        .map(model_archive_origins)
        .unwrap_or_default()
        .into_keys()
        .collect();
    keys.extend(
        crate::workspace::git_tracked_project_files(root, &["*.jar"])
            .unwrap_or_default()
            .into_iter()
            .map(|path| crate::paths::file_key(&path)),
    );
    keys.into_iter()
        .filter(|key| !is_project_configuration(Path::new(key)))
        .filter_map(|key| {
            let content = configuration_content(Path::new(&key))?;
            Some((key, content))
        })
        .collect()
}

fn unchanged_archive(
    contents: &HashMap<String, [u8; 32]>,
    key: &str,
    kind: FileChangeType,
) -> bool {
    kind == FileChangeType::CHANGED
        && !is_project_configuration(Path::new(key))
        && contents.get(key).is_some_and(|accepted| {
            configuration_content(Path::new(key)).as_ref() == Some(accepted)
        })
}

fn configuration_contents(root: &Path) -> HashMap<String, [u8; 32]> {
    let paths = crate::workspace::git_project_files(
        root,
        &[
            "*.gradle",
            "*.gradle.kts",
            "*.toml",
            "*gradle.properties",
            "*gradle-wrapper.properties",
            "*gradle-wrapper.jar",
        ],
    )
    .unwrap_or_else(|| {
        walkdir::WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| !crate::workspace::is_excluded(entry))
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_type().is_file() && is_project_configuration(entry.path()))
            .map(|entry| entry.into_path())
            .collect()
    });
    paths
        .into_iter()
        .filter(|path| is_project_configuration(path))
        .filter_map(|path| Some((crate::paths::file_key(&path), configuration_content(&path)?)))
        .collect()
}

/// `Some(force)` for relevant events; source-only saves reuse the published classpath.
fn project_change(
    root: &Path,
    model: Option<&classpath::ProjectSnapshot>,
    key: &str,
) -> Option<bool> {
    let file = Path::new(key);
    let name = file
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if is_project_configuration(file) {
        return Some(true);
    }
    if let Some(model) = model {
        if (name.ends_with("-sources.jar") || name.ends_with(".pom")) && file.is_file() {
            let current = classpath::file_origin_id(file);
            if model.artifact_origin(file) == Some(current.as_str()) {
                return None;
            }
            for binary in model.local_jars() {
                if binary.is_file()
                    && classpath::adjacent_sources_jar(&binary)
                        .is_some_and(|sources| crate::paths::file_key(&sources) == key)
                {
                    // shortcut: mtime/size identity, use content origins for metadata-preserving edits.
                    let current = classpath::file_origin_id(&binary);
                    if model.artifact_origin(&binary) == Some(current.as_str()) {
                        tracing::debug!(path = key, "unchanged source archive watch ignored");
                        return None;
                    }
                }
            }
        }
        if model.is_output_key(key) || model.has_dependency_key(key) {
            return Some(true);
        }
        if crate::language::SourceLanguage::for_project_path(file).is_some()
            && (model
                .owner_for_key(key)
                .is_some_and(|(_, source)| source.is_some())
                || model.has_selected_kotlin_source_key(key))
        {
            return Some(false);
        }
    }
    if name.ends_with(".jar") && path_is_under(key, &crate::paths::file_key(root)) {
        return Some(true);
    }
    None
}

fn project_changes(changes: impl Iterator<Item = Option<bool>>) -> Option<bool> {
    let mut relevant = None;
    for force in changes.flatten() {
        relevant = Some(force);
        if force {
            break;
        }
    }
    relevant
}

fn path_is_under(key: &str, root: &str) -> bool {
    key == root
        || key
            .strip_prefix(root.trim_end_matches('/'))
            .is_some_and(|suffix| suffix.starts_with('/'))
}

/// `file://` URI -> canonical key (the file path string). `None` for non-file URIs.
fn uri_to_key(uri: &Uri) -> Option<String> {
    uri.to_file_path().map(|path| crate::paths::file_key(&path))
}

/// Canonical key (path string) -> `file://` URI.
fn key_to_uri(key: &str) -> Option<Uri> {
    Uri::from_file_path(key)
}

fn selected_jdk_source_for_file(
    model: Option<&classpath::ProjectSnapshot>,
    file: &Path,
    gradle_project: bool,
) -> Option<PathBuf> {
    match model {
        Some(model) => model.selected_jdk_source_for_file(file),
        None if gradle_project => None,
        None => crate::deps::jdk_src_zip(),
    }
}

fn resolve_import_source_batches(
    root: &Path,
    file: &Path,
    text: &str,
    offset: usize,
    gradle_project: bool,
    model: Option<&classpath::ProjectSnapshot>,
) -> (Option<String>, Vec<crate::deps::FileSymbols>) {
    let mut java = crate::java::JavaParser::new();
    let import_fqn = if file.extension().and_then(|ext| ext.to_str()) == Some("java") {
        java_import_fqn_for_identifier(text, offset, &mut java)
    } else {
        kotlin_import_fqn_for_identifier(text, offset)
    };
    let Some(import_fqn) = import_fqn else {
        return (None, Vec::new());
    };
    // Editor requests use the published model; Gradle resolution belongs to background indexing.
    if gradle_project
        && model
            .and_then(|model| model.owner_for(file))
            .and_then(|(_, source)| source)
            .is_none()
    {
        return (Some(import_fqn), Vec::new());
    }
    let extract_root = crate::deps::extract_root();
    if import_fqn.starts_with("android.") {
        let batches = crate::deps::resolve_android_imports(
            root,
            std::slice::from_ref(&import_fqn),
            &extract_root,
            &mut java,
        );
        return (Some(import_fqn), batches);
    }
    if import_fqn.starts_with("java.") || import_fqn.starts_with("javax.") {
        let batches = selected_jdk_source_for_file(model, file, gradle_project)
            .as_deref()
            .map(|source| {
                crate::deps::resolve_jdk_imports_from_zip(
                    source,
                    &extract_root,
                    std::slice::from_ref(&import_fqn),
                    &mut java,
                )
            })
            .unwrap_or_default();
        if !batches.is_empty() {
            return (Some(import_fqn), batches);
        }
    }

    let repos = crate::artifacts::Repos::defaults();
    let coords = if gradle_project {
        model
            .and_then(|m| m.coordinates_for_file(file))
            .unwrap_or_default()
    } else {
        let mut coords = crate::deps::coordinates_for_root(root);
        coords.extend(crate::deps::cached_catalog_coordinates(root, &repos));
        coords.extend(crate::deps::coordinates_from_build_files(root, &repos));
        coords
    };
    let local_jars = if gradle_project {
        model
            .and_then(|m| m.local_jars_for_file(file))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut kotlin = crate::parser::KotlinParser::new();
    if !local_jars.is_empty() {
        let batches = crate::deps::resolve_import_fqn_from_local_jars(
            &import_fqn,
            local_jars.clone(),
            model,
            &extract_root,
            &mut kotlin,
            &mut java,
        );
        if !batches.is_empty() {
            return (Some(import_fqn), batches);
        }
    }
    let mut batches = if coords.is_empty() {
        Vec::new()
    } else {
        crate::deps::resolve_explicit_import_fqn_from_coordinates(
            &import_fqn,
            coords,
            &repos,
            model,
            &extract_root,
            &mut kotlin,
            &mut java,
        )
    };
    if batches.is_empty() && !local_jars.is_empty() {
        batches = crate::deps::resolve_import_fqn_from_local_jars(
            &import_fqn,
            local_jars,
            model,
            &extract_root,
            &mut kotlin,
            &mut java,
        );
    }
    (Some(import_fqn), batches)
}

fn java_import_fqn_for_identifier(
    text: &str,
    offset: usize,
    java: &mut crate::java::JavaParser,
) -> Option<String> {
    let tree = java.parse(text);
    crate::java::explicit_import_fqn_at(&tree, text, offset).or_else(|| {
        let identifier = crate::trace::ident_at(text, offset)?;
        crate::java::explicit_import_fqns(&tree, text)
            .into_iter()
            .find(|fqn| fqn.rsplit('.').next() == Some(identifier.as_str()))
    })
}

fn kotlin_import_fqn_for_identifier(text: &str, offset: usize) -> Option<String> {
    let identifier = crate::trace::ident_at(text, offset)?;
    let mut parser = crate::parser::KotlinParser::new();
    let tree = parser.parse(text);
    crate::parser::imports_of(&tree, text)
        .into_iter()
        .find(|import| !import.wildcard && import.local_name() == Some(identifier.as_str()))
        .map(|import| import.path)
}

fn configure_analysis_providers(ws: &mut Workspace, options: &Option<serde_json::Value>) {
    let Some(providers) = options
        .as_ref()
        .and_then(|options| options.get("analysisProviders"))
        .and_then(serde_json::Value::as_object)
    else {
        return;
    };
    for (id, settings) in providers {
        let Some(settings) = settings.as_object() else {
            continue;
        };
        let enabled = settings
            .get("enabled")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true);
        let configuration = settings
            .get("configuration")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        ws.configure_analysis_provider(id, enabled, Arc::from(configuration.to_string()));
    }
}

fn diagnostic_scope_from(opts: &Option<serde_json::Value>) -> DiagnosticScope {
    match opts
        .as_ref()
        .and_then(|v| v.get("diagnostics"))
        .and_then(|d| d.get("scope"))
        .and_then(|s| s.as_str())
    {
        Some("workspace") => DiagnosticScope::Workspace,
        _ => DiagnosticScope::OpenFilesOnly,
    }
}

fn formatter_from(opts: &Option<serde_json::Value>) -> Option<FormatterConfig> {
    let formatting = opts.as_ref()?.get("formatting")?;
    let command = formatting.get("command")?.as_str()?.to_string();
    if command.trim().is_empty() {
        return None;
    }
    let args = formatting
        .get("args")
        .and_then(|args| args.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some(FormatterConfig { command, args })
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        // Remember the workspace root so `initialized` can index it off the request path.
        #[allow(deprecated)]
        let root = params
            .root_uri
            .as_ref()
            .and_then(uri_to_key)
            .or_else(|| {
                params
                    .workspace_folders
                    .as_ref()
                    .and_then(|folders| folders.first())
                    .and_then(|folder| uri_to_key(&folder.uri))
            })
            .map(PathBuf::from);
        if let Some(root) = root {
            let gradle_project = crate::deps::gradle_root(&root).is_some();
            *self.gradle_project.lock() = gradle_project;
            if gradle_project {
                // Pending or failed Gradle resolution cannot expose an unscoped checkout.
                self.ws
                    .lock()
                    .set_project_model(Arc::new(classpath::ProjectSnapshot::new(Vec::new())));
            }
            *self.root.lock() = Some(root);
        }

        // Whether the client supports snippet insertion (`name($0)`). Option chain; default false.
        let snippets = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|td| td.completion.as_ref())
            .and_then(|c| c.completion_item.as_ref())
            .and_then(|ci| ci.snippet_support)
            .unwrap_or(false);
        *self.snippets_supported.lock() = snippets;

        // Whether the client supports server-initiated work-done progress (the indexing
        // spinner). Default false -> fall back to log messages.
        let progress = params
            .capabilities
            .window
            .as_ref()
            .and_then(|w| w.work_done_progress)
            .unwrap_or(false);
        *self.progress_supported.lock() = progress;
        let watches = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref());
        {
            let mut state = self.watches.lock();
            state.dynamic = watches
                .and_then(|watches| watches.dynamic_registration)
                .unwrap_or(false);
            state.relative = watches
                .and_then(|watches| watches.relative_pattern_support)
                .unwrap_or(false)
                // Neovim on Windows compares decoded backslashes with normalized event paths.
                // shortcut: string watches cover workspace folders; external roots need a client fix.
                && !(cfg!(windows)
                    && params
                        .client_info
                        .as_ref()
                        .is_some_and(|client| client.name == "Neovim"));
        }

        *self.diagnostic_scope.lock() = diagnostic_scope_from(&params.initialization_options);
        configure_analysis_providers(&mut self.ws.lock(), &params.initialization_options);
        let formatter = formatter_from(&params.initialization_options);
        *self.formatter.lock() = formatter.clone();

        Ok(InitializeResult {
            server_info: Some(ServerInfo {
                name: "ktlsp".into(),
                version: Some(env!("CARGO_PKG_VERSION").into()),
            }),
            capabilities: ServerCapabilities {
                // FULL sync (each change carries the whole document) plus open/close.
                text_document_sync: Some(TextDocumentSyncCapability::Options(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::FULL),
                        save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                        ..Default::default()
                    },
                )),
                diagnostic_provider: None,
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                definition_provider: Some(OneOf::Left(true)),
                type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
                implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
                references_provider: Some(OneOf::Left(true)),
                code_action_provider: Some(CodeActionProviderCapability::Options(
                    CodeActionOptions {
                        code_action_kinds: Some(vec![
                            CodeActionKind::QUICKFIX,
                            CodeActionKind::REFACTOR_REWRITE,
                            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                            CodeActionKind::SOURCE_FIX_ALL,
                            CodeActionKind::new("source.fixAll.ktlsp"),
                        ]),
                        work_done_progress_options: Default::default(),
                        resolve_provider: Some(false),
                    },
                )),
                document_highlight_provider: Some(OneOf::Left(true)),
                document_symbol_provider: Some(OneOf::Left(true)),
                workspace_symbol_provider: Some(OneOf::Left(true)),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
                    retrigger_characters: Some(vec![",".to_string()]),
                    work_done_progress_options: Default::default(),
                }),
                folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
                selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
                rename_provider: Some(OneOf::Right(RenameOptions {
                    prepare_provider: Some(true),
                    work_done_progress_options: Default::default(),
                })),
                document_formatting_provider: formatter.as_ref().map(|_| OneOf::Left(true)),
                execute_command_provider: Some(ExecuteCommandOptions {
                    commands: crate::commands::all(),
                    work_done_progress_options: Default::default(),
                }),
                call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
                type_hierarchy_provider: Some(OneOf::Left(true)),
                semantic_tokens_provider: Some(
                    SemanticTokensServerCapabilities::SemanticTokensOptions(
                        SemanticTokensOptions {
                            work_done_progress_options: Default::default(),
                            legend: semantic_tokens_legend(),
                            range: None,
                            full: Some(SemanticTokensFullOptions::Bool(true)),
                        },
                    ),
                ),
                inlay_hint_provider: Some(OneOf::Right(InlayHintServerCapabilities::Options(
                    InlayHintOptions {
                        work_done_progress_options: Default::default(),
                        resolve_provider: Some(false),
                    },
                ))),
                // `.` is registered now so the capability is correct for Stage B; the after-dot
                // branch returns nothing in Stage A.
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(vec![".".to_string()]),
                    resolve_provider: Some(true),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        self.client
            .log_message(MessageType::INFO, "ktlsp initialized")
            .await;

        // Index the workspace once, off the request path. Early goto requests fall back to
        // local-scope resolution until the index warms up.
        self.schedule_project_refresh(false, Duration::ZERO);
    }

    async fn shutdown(&self) -> Result<()> {
        self.close_project_refresh();
        self.close_compiler_jobs();
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let doc = params.text_document;
        if let Some(key) = uri_to_key(&doc.uri) {
            let text = doc.text;
            {
                let mut ws = self.ws.lock();
                ws.open(key.clone(), text);
                self.doc_versions
                    .lock()
                    .entry(key.clone())
                    .or_default()
                    .document_version = Some(doc.version);
                self.seed_jdk_imports_for_open_java(key.clone(), &ws);
            }
            self.schedule_diagnostics(key);
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        // FULL sync: the last (only) change holds the entire new document.
        if let Some(key) = uri_to_key(&params.text_document.uri) {
            if let Some(change) = params.content_changes.into_iter().last() {
                {
                    let mut ws = self.ws.lock();
                    ws.change(&key, change.text);
                    self.doc_versions
                        .lock()
                        .entry(key.clone())
                        .or_default()
                        .document_version = Some(params.text_document.version);
                }
                self.schedule_diagnostics(key);
            }
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Some(key) = uri_to_key(&params.text_document.uri) {
            {
                let mut ws = self.ws.lock();
                ws.close(&key);
                self.doc_versions.lock().remove(&key);
                self.jdk_source_seeds.lock().remove(&key);
            }
            self.schedule_diagnostics(key.clone());
            publish_diagnostics(&self.ws, &self.client, &self.doc_versions, &key).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        if !*self.gradle_project.lock() {
            return;
        }
        let Some(key) = uri_to_key(&params.text_document.uri) else {
            return;
        };
        let Some(root) = self.root.lock().clone() else {
            return;
        };
        let model = self.ws.lock().project_snapshot();
        if self
            .watches
            .lock()
            .unchanged_configuration(&key, FileChangeType::CHANGED)
        {
            return;
        }
        if let Some(force) = project_change(&root, model.as_deref(), &key) {
            self.schedule_project_refresh(force, PROJECT_REFRESH_DEBOUNCE);
        }
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        if !*self.gradle_project.lock() {
            return;
        }
        let Some(root) = self.root.lock().clone() else {
            return;
        };
        let model = self.ws.lock().project_snapshot();
        // Archive reads may be large. Hash only selected event paths, outside the
        // workspace/watch locks so completion can proceed while IO is pending.
        let archives = self.watches.lock().archive_contents.clone();
        let archive_keys = params
            .changes
            .iter()
            .filter(|event| event.typ == FileChangeType::CHANGED)
            .filter_map(|event| uri_to_key(&event.uri))
            .filter(|key| archives.contains_key(key))
            .collect::<std::collections::BTreeSet<_>>();
        let unchanged_archives = if archive_keys.is_empty() {
            HashMap::new()
        } else {
            tokio::task::spawn_blocking(move || {
                archive_keys
                    .into_iter()
                    .filter(|key| unchanged_archive(&archives, key, FileChangeType::CHANGED))
                    .map(|key| {
                        let digest = archives[&key];
                        (key, digest)
                    })
                    .collect::<HashMap<_, _>>()
            })
            .await
            .unwrap_or_default()
        };
        let archive_origins = model
            .as_deref()
            .map(model_archive_origins)
            .unwrap_or_default();
        let mut retired_origins = std::collections::BTreeSet::new();
        let mut jar_only = true;
        let changes = {
            let mut ws = self.ws.lock();
            params
                .changes
                .iter()
                .filter_map(|change| {
                    let key = uri_to_key(&change.uri)?;
                    let mut watches = self.watches.lock();
                    (!watches.unchanged_configuration(&key, change.typ)
                        && !(change.typ == FileChangeType::CHANGED
                            && unchanged_archives.get(&key).is_some_and(|accepted| {
                                watches.archive_contents.get(&key) == Some(accepted)
                            })))
                    .then_some(key)
                })
                .filter(|key| !ws.observe_compiled_resource(key))
                .map(|key| {
                    let change = project_change(&root, model.as_deref(), &key);
                    if change.is_some() {
                        if let Some(origin) = archive_origins.get(&key) {
                            retired_origins.insert(origin.clone());
                        }
                        jar_only &= key.ends_with(".jar")
                            && !is_project_configuration(Path::new(&key));
                        tracing::debug!(path = key, force = ?change, "project refresh requested by client watch");
                    }
                    change
                })
                .collect::<Vec<_>>()
        };
        let changed = self.ws.lock().take_extension_diagnostic_changes();
        for key in changed {
            self.schedule_one_diagnostic(key);
        }
        let force = project_changes(changes.into_iter());
        if let Some(force) = force {
            self.request_project_refresh(
                force,
                PROJECT_REFRESH_DEBOUNCE,
                jar_only,
                &retired_origins.into_iter().collect::<Vec<_>>(),
            );
        }
    }

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        self.ws.lock().invalidate_observed_compile();
        self.ws.lock().invalidate_kotlin_diagnostics();
        self.schedule_kotlin_diagnostics();
        let options = Some(params.settings);
        configure_analysis_providers(&mut self.ws.lock(), &options);
        schedule_extension_sources(&self.ws, &self.client, &self.doc_versions);
        let changed = self.ws.lock().take_extension_diagnostic_changes();
        for key in changed {
            self.schedule_one_diagnostic(key);
        }
        *self.diagnostic_scope.lock() = diagnostic_scope_from(&options);
        if let Some(formatter) = formatter_from(&options) {
            *self.formatter.lock() = Some(formatter);
        }
        let provider_only = options
            .as_ref()
            .and_then(serde_json::Value::as_object)
            .is_some_and(|settings| {
                settings.len() == 1 && settings.contains_key("analysisProviders")
            });
        if *self.gradle_project.lock() && !provider_only {
            self.schedule_project_refresh(true, PROJECT_REFRESH_DEBOUNCE);
        }
    }

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult> {
        let Some(key) = uri_to_key(&params.text_document.uri) else {
            return Ok(
                DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                    related_documents: None,
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id: None,
                        items: Vec::new(),
                    },
                })
                .into(),
            );
        };
        self.await_extension_sources(&key).await?;
        let start = Instant::now();
        let (items, result_id, _) = lsp_diagnostics_with_result(&self.ws, &self.doc_versions, &key);
        crate::trace::span(
            "diagnostics_pull",
            "lsp",
            start,
            serde_json::json!({
                "path": key, "result_id": result_id, "count": items.len(),
            }),
        );
        Ok(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport { result_id, items },
            })
            .into(),
        )
    }

    async fn workspace_diagnostic(
        &self,
        _params: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReportResult> {
        let scope = *self.diagnostic_scope.lock();
        let keys = {
            let ws = self.ws.lock();
            match scope {
                DiagnosticScope::OpenFilesOnly => ws.open_doc_keys(),
                DiagnosticScope::Workspace => ws.project_doc_keys(),
            }
        };
        let mut items = Vec::new();
        for key in keys {
            let Some(uri) = key_to_uri(&key) else {
                continue;
            };
            self.await_extension_sources(&key).await?;
            let (diagnostics, result_id, version) =
                lsp_diagnostics_with_result(&self.ws, &self.doc_versions, &key);
            items.push(WorkspaceDocumentDiagnosticReport::Full(
                WorkspaceFullDocumentDiagnosticReport {
                    uri,
                    version: version.map(i64::from),
                    full_document_diagnostic_report: FullDocumentDiagnosticReport {
                        result_id,
                        items: diagnostics,
                    },
                },
            ));
        }
        Ok(WorkspaceDiagnosticReport { items }.into())
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        // All work is synchronous; the lock is never held across an `.await`.
        let (mut locations, symbol, text, offset) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            let offset = line_index.offset(&text, pos.line, pos.character);
            let symbol = crate::trace::ident_at(&text, offset);
            let defs = ws.goto_definition(&key, offset);
            let locs = defs
                .iter()
                .filter_map(|d| def_to_location(&ws, d))
                .collect::<Vec<_>>();
            (locs, symbol, text, offset)
        };

        if locations.is_empty()
            && (key.ends_with(".java") || key.ends_with(".kt") || key.ends_with(".kts"))
        {
            let root = { self.root.lock().clone() };
            if let Some(root) = root {
                let seed_start = Instant::now();
                let seed_text = text.clone();
                let seed_key = key.clone();
                let gradle_project = *self.gradle_project.lock();
                let (generation, model) = {
                    let ws = self.ws.lock();
                    (ws.analysis_generation(), ws.project_snapshot())
                };
                let seed = tokio::task::spawn_blocking(move || {
                    resolve_import_source_batches(
                        &root,
                        Path::new(&seed_key),
                        &seed_text,
                        offset,
                        gradle_project,
                        model.as_deref(),
                    )
                })
                .await
                .unwrap_or_else(|_| (None, Vec::new()));
                let (import_fqn, batches) = seed;
                let files = batches.len();
                let symbols = batches
                    .iter()
                    .map(|batch| batch.symbols.len())
                    .sum::<usize>();
                // The initial project/dependency scan can hold the workspace lock for a while in
                // large Android monorepos.  A source seed already contains the exact imported
                // declaration, so turn it into a location before trying to merge it into the
                // shared index.  This keeps an explicit-import goto responsive on cold start;
                // the normal index is still populated whenever it is immediately available.
                let seeded_locations = import_fqn
                    .as_deref()
                    .map(|fqn| import_seed_locations(&batches, fqn))
                    .unwrap_or_default();
                if files > 0 {
                    if let Some(mut ws) = self.ws.try_lock() {
                        if ws.is_current_generation(generation) {
                            for batch in batches {
                                ws.index_dependency_batch_for_generation(generation, batch);
                            }
                            ws.bump_index_revision();
                            let defs = ws.goto_definition(&key, offset);
                            locations = defs
                                .iter()
                                .filter_map(|d| def_to_location(&ws, d))
                                .collect::<Vec<_>>();
                        }
                    }
                    if locations.is_empty() && self.ws.lock().is_current_generation(generation) {
                        locations = seeded_locations;
                    }
                }
                crate::trace::span(
                    "deps.import_seed_definition",
                    "deps",
                    seed_start,
                    serde_json::json!({
                        "file": key,
                        "import": import_fqn,
                        "files": files,
                        "symbols": symbols,
                        "resolved": !locations.is_empty(),
                    }),
                );
            }
        }

        let count = locations.len();
        crate::trace::request(
            "goto_definition",
            start,
            &key,
            pos.line,
            pos.character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok(match count {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(
                locations.into_iter().next().unwrap(),
            )),
            _ => Some(GotoDefinitionResponse::Array(locations)),
        })
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let locations = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.type_definition(&key, offset)
                .iter()
                .filter_map(|d| def_to_location(&ws, d))
                .collect::<Vec<_>>()
        };
        Ok(goto_type_response(locations))
    }

    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> Result<Option<GotoImplementationResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let locations = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.implementation(&key, offset)
                .iter()
                .filter_map(|d| def_to_location(&ws, d))
                .collect::<Vec<_>>()
        };
        Ok(goto_implementation_response(locations))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let include_declaration = params.context.include_declaration;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        let (locations, symbol) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            let symbol = crate::trace::ident_at(&text, offset);
            let sites = ws.references(&key, offset, include_declaration);
            let locs = sites
                .iter()
                .filter_map(|d| def_to_location(&ws, d))
                .collect::<Vec<_>>();
            (locs, symbol)
        };

        let count = locations.len();
        crate::trace::request(
            "references",
            start,
            &key,
            pos.line,
            pos.character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((!locations.is_empty()).then_some(locations))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let help = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.signature_help(&key, offset).map(to_lsp_signature_help)
        };
        Ok(help)
    }

    async fn code_action(&self, params: CodeActionParams) -> Result<Option<CodeActionResponse>> {
        let start = std::time::Instant::now();
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let requested = params.context.only;

        let (actions, symbol, line, character) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            let range_start =
                line_index.offset(&text, params.range.start.line, params.range.start.character);
            let range_end =
                line_index.offset(&text, params.range.end.line, params.range.end.character);
            let symbol = crate::trace::ident_at(&text, range_start);
            let actions = ws
                .code_actions(&key, range_start, range_end, range_start)
                .into_iter()
                .filter(|action| action_kind_allowed(action.kind, requested.as_ref()))
                .filter_map(|action| to_lsp_code_action(&ws, action))
                .collect::<Vec<_>>();
            (
                actions,
                symbol,
                params.range.start.line,
                params.range.start.character,
            )
        };

        let count = actions.len();
        crate::trace::request(
            "code_action",
            start,
            &key,
            line,
            character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((count > 0).then_some(actions))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        let Some(config) = self.formatter.lock().clone() else {
            return Ok(None);
        };
        let text = {
            let ws = self.ws.lock();
            match ws.doc_text(&key) {
                Some(text) => text,
                None => return Ok(None),
            }
        };
        let edits = crate::format::format_document(&key, &text, &config)
            .and_then(|edits| to_lsp_text_edits_for_text(&text, edits));
        Ok(edits.filter(|edits| !edits.is_empty()))
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let edit = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.rename(&key, offset, &params.new_name)
                .and_then(|edits| to_lsp_workspace_edit(&ws, edits))
        };
        Ok(edit)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let pos = params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let prepared = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            let offset = line_index.offset(&text, pos.line, pos.character);
            ws.prepare_rename(&key, offset).and_then(|prepared| {
                let target_text = ws.doc_text(&prepared.range.file)?;
                let target_index = LineIndex::new(&target_text);
                Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: byte_range_to_lsp(
                        &target_index,
                        &target_text,
                        prepared.range.start_byte,
                        prepared.range.end_byte,
                    ),
                    placeholder: prepared.placeholder,
                })
            })
        };
        Ok(prepared)
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;
        let items = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.hierarchy_item_at(&key, offset)
                .filter(|item| matches!(item.kind, SymbolKind::Function | SymbolKind::Property))
                .and_then(|item| to_call_hierarchy_item(&ws, &item))
                .map(|item| vec![item])
        };
        Ok(items)
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        let Some(item) = from_call_hierarchy_item(&params.item) else {
            return Ok(None);
        };
        self.await_extension_sources(&item.file).await?;
        let calls = {
            let mut ws = self.ws.lock();
            ws.incoming_calls(&item)
                .into_iter()
                .filter_map(|call| to_lsp_incoming_call(&ws, call))
                .collect::<Vec<_>>()
        };
        Ok((!calls.is_empty()).then_some(calls))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        let Some(item) = from_call_hierarchy_item(&params.item) else {
            return Ok(None);
        };
        self.await_extension_sources(&item.file).await?;
        let calls = {
            let mut ws = self.ws.lock();
            ws.outgoing_calls(&item)
                .into_iter()
                .filter_map(|call| to_lsp_outgoing_call(&ws, call))
                .collect::<Vec<_>>()
        };
        Ok((!calls.is_empty()).then_some(calls))
    }

    async fn prepare_type_hierarchy(
        &self,
        params: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        let items = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            ws.hierarchy_item_at(&key, offset)
                .filter(|item| item.kind.is_type_like())
                .and_then(|item| to_type_hierarchy_item(&ws, &item))
                .map(|item| vec![item])
        };
        Ok(items)
    }

    async fn supertypes(
        &self,
        params: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let Some(item) = from_type_hierarchy_item(&params.item) else {
            return Ok(None);
        };
        let items = {
            let ws = self.ws.lock();
            ws.type_supertypes(&item)
                .iter()
                .filter_map(|item| to_type_hierarchy_item(&ws, item))
                .collect::<Vec<_>>()
        };
        Ok((!items.is_empty()).then_some(items))
    }

    async fn subtypes(
        &self,
        params: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        let Some(item) = from_type_hierarchy_item(&params.item) else {
            return Ok(None);
        };
        let items = {
            let ws = self.ws.lock();
            ws.type_subtypes(&item)
                .iter()
                .filter_map(|item| to_type_hierarchy_item(&ws, item))
                .collect::<Vec<_>>()
        };
        Ok((!items.is_empty()).then_some(items))
    }

    async fn execute_command(&self, params: ExecuteCommandParams) -> Result<Option<LSPAny>> {
        match params.command.as_str() {
            crate::commands::COMPILE => {
                Ok(Some(self.execute_observed_compile(&params.arguments).await))
            }
            crate::commands::TRACE_PATH => Ok(crate::trace::log_path()
                .map(|path| serde_json::Value::String(path.to_string_lossy().into_owned()))),
            crate::commands::REINDEX => {
                let Some(root) = self.root.lock().clone() else {
                    return Ok(Some(serde_json::json!({ "status": "no-root" })));
                };
                let count = self.ws.lock().scan(&root);
                Ok(Some(
                    serde_json::json!({ "status": "ok", "indexedFiles": count }),
                ))
            }
            crate::commands::EXPLAIN_RESOLUTION
            | crate::commands::EXPLAIN_COMPLETION
            | crate::commands::DUMP_SYMBOL => {
                let Some((uri, position)) = command_uri_position(&params.arguments) else {
                    return Ok(Some(serde_json::json!({ "status": "invalid-arguments" })));
                };
                let Some(key) = uri_to_key(&uri) else {
                    return Ok(Some(serde_json::json!({ "status": "invalid-uri" })));
                };
                self.await_extension_sources(&key).await?;
                let result = {
                    let mut ws = self.ws.lock();
                    let text = match ws.doc_text(&key) {
                        Some(t) => t,
                        None => {
                            return Ok(Some(serde_json::json!({ "status": "missing-document" })));
                        }
                    };
                    let offset =
                        LineIndex::new(&text).offset(&text, position.line, position.character);
                    match params.command.as_str() {
                        crate::commands::EXPLAIN_COMPLETION => {
                            serde_json::to_value(ws.explain_completion(&key, offset).unwrap_or(
                                crate::commands::CompletionExplanation {
                                    status: "unknown",
                                    context: "none",
                                    prefix: String::new(),
                                    candidate_count: 0,
                                    reasons: vec!["non-completable-position".to_string()],
                                    candidates: Vec::new(),
                                },
                            ))
                            .unwrap_or_else(|_| serde_json::json!({ "status": "error" }))
                        }
                        _ => {
                            let explanation = ws.explain_resolution(&key, offset).unwrap_or(
                                crate::commands::ResolutionExplanation {
                                    status: "no-identifier",
                                    kind: "unknown",
                                    symbol: crate::trace::ident_at(&text, offset),
                                    targets: Vec::new(),
                                    reasons: Vec::new(),
                                },
                            );
                            serde_json::to_value(explanation)
                                .unwrap_or_else(|_| serde_json::json!({ "status": "error" }))
                        }
                    }
                };
                Ok(Some(result))
            }
            _ => Ok(Some(serde_json::json!({ "status": "unknown-command" }))),
        }
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        let (mut hover, symbol, mut semantic, text, offset) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            let symbol = crate::trace::ident_at(&text, offset);
            let query = ws.resolved_symbol_query(&key, offset);
            let semantic = query.as_ref().map(|query| {
                (
                    query.reference().kind_label(),
                    query.reference().status_label(),
                    query.reference().reason_labels(),
                )
            });
            let hover = query
                .and_then(|query| query.hover_markdown())
                .map(|value| Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value,
                    }),
                    range: None,
                });
            (hover, symbol, semantic, text, offset)
        };

        // Dependency symbols are normally indexed in the background.  Hover must not depend on
        // a prior go-to-definition request having seeded that index, though: Eglot commonly asks
        // for hover first.  Mirror the explicit-import seed used by definition and retry the
        // semantic query once the matching source batch is available.
        if hover.is_none()
            && (key.ends_with(".java") || key.ends_with(".kt") || key.ends_with(".kts"))
        {
            let root = { self.root.lock().clone() };
            if let Some(root) = root {
                let seed_key = key.clone();
                let seed_text = text.clone();
                let gradle_project = *self.gradle_project.lock();
                let (generation, model) = {
                    let ws = self.ws.lock();
                    (ws.analysis_generation(), ws.project_snapshot())
                };
                let (_, batches) = tokio::task::spawn_blocking(move || {
                    resolve_import_source_batches(
                        &root,
                        Path::new(&seed_key),
                        &seed_text,
                        offset,
                        gradle_project,
                        model.as_deref(),
                    )
                })
                .await
                .unwrap_or_else(|_| (None, Vec::new()));
                if !batches.is_empty() {
                    let mut ws = self.ws.lock();
                    if !ws.is_current_generation(generation) {
                        return Ok(None);
                    }
                    for batch in batches {
                        ws.index_dependency_batch_for_generation(generation, batch);
                    }
                    ws.bump_index_revision();
                    if let Some(query) = ws.resolved_symbol_query(&key, offset) {
                        semantic = Some((
                            query.reference().kind_label(),
                            query.reference().status_label(),
                            query.reference().reason_labels(),
                        ));
                        hover = query.hover_markdown().map(|value| Hover {
                            contents: HoverContents::Markup(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value,
                            }),
                            range: None,
                        });
                    }
                }
            }
        }

        let count = usize::from(hover.is_some());
        if let Some((kind, status, reasons)) = semantic {
            crate::trace::semantic(
                "hover",
                &key,
                pos.line,
                pos.character,
                symbol.as_deref(),
                kind,
                status,
                &reasons,
            );
        }
        crate::trace::request(
            "hover",
            start,
            &key,
            pos.line,
            pos.character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok(hover)
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        let (highlights, symbol) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let offset = LineIndex::new(&text).offset(&text, pos.line, pos.character);
            let symbol = crate::trace::ident_at(&text, offset);
            let defs = ws.document_highlights(&key, offset);
            let highlights = defs
                .iter()
                .filter_map(|d| def_to_range(&ws, d))
                .map(|range| DocumentHighlight {
                    range,
                    kind: Some(DocumentHighlightKind::TEXT),
                })
                .collect::<Vec<_>>();
            (highlights, symbol)
        };

        let count = highlights.len();
        crate::trace::request(
            "document_highlight",
            start,
            &key,
            pos.line,
            pos.character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((!highlights.is_empty()).then_some(highlights))
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document.uri;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };

        let (symbols, text) = {
            let ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            (ws.document_symbols(&key), text)
        };

        let line_index = LineIndex::new(&text);
        let items = symbols
            .into_iter()
            .map(|s| to_document_symbol(&line_index, &text, s))
            .collect::<Vec<_>>();
        let count = items.len();
        crate::trace::request(
            "document_symbol",
            start,
            &key,
            0,
            0,
            None,
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((count > 0).then_some(DocumentSymbolResponse::Nested(items)))
    }

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let start = std::time::Instant::now();
        let items = {
            let ws = self.ws.lock();
            ws.workspace_symbols(&params.query)
                .into_iter()
                .filter_map(|s| to_symbol_information(&ws, s))
                .collect::<Vec<_>>()
        };
        let count = items.len();
        crate::trace::request(
            "workspace_symbol",
            start,
            "",
            0,
            0,
            Some(&params.query),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((count > 0).then_some(WorkspaceSymbolResponse::Flat(items)))
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        let start = std::time::Instant::now();
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };

        let ranges = {
            let mut ws = self.ws.lock();
            ws.folding_ranges(&key)
                .into_iter()
                .map(to_lsp_folding_range)
                .collect::<Vec<_>>()
        };

        let count = ranges.len();
        crate::trace::request(
            "folding_range",
            start,
            &key,
            0,
            0,
            None,
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((count > 0).then_some(ranges))
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        let start = std::time::Instant::now();
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        let positions = params.positions;

        let (ranges, first_symbol, first_line, first_character, requested) = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            let offsets = positions
                .iter()
                .map(|pos| line_index.offset(&text, pos.line, pos.character))
                .collect::<Vec<_>>();
            let first_symbol = offsets
                .first()
                .and_then(|offset| crate::trace::ident_at(&text, *offset));
            let ranges = ws
                .selection_ranges(&key, &offsets)
                .into_iter()
                .filter_map(|range| range.map(|r| to_lsp_selection_range(&line_index, &text, r)))
                .collect::<Vec<_>>();
            let first = positions.first().copied().unwrap_or_default();
            (
                ranges,
                first_symbol,
                first.line,
                first.character,
                positions.len(),
            )
        };

        let count = ranges.len();
        crate::trace::request(
            "selection_range",
            start,
            &key,
            first_line,
            first_character,
            first_symbol.as_deref(),
            if count == requested && count > 0 {
                "ok"
            } else {
                "empty"
            },
            count,
        );
        Ok((count == requested && count > 0).then_some(ranges))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        let start = std::time::Instant::now();
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };

        let data = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            to_lsp_semantic_tokens(&line_index, &text, ws.semantic_tokens(&key))
        };

        let count = data.len();
        crate::trace::request(
            "semantic_tokens_full",
            start,
            &key,
            0,
            0,
            None,
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    async fn inlay_hint(&self, params: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        let start = std::time::Instant::now();
        let key = match uri_to_key(&params.text_document.uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        let hints = {
            let mut ws = self.ws.lock();
            let text = match ws.doc_text(&key) {
                Some(t) => t,
                None => return Ok(None),
            };
            let line_index = LineIndex::new(&text);
            let start_byte =
                line_index.offset(&text, params.range.start.line, params.range.start.character);
            let end_byte =
                line_index.offset(&text, params.range.end.line, params.range.end.character);
            ws.inlay_hints(&key, start_byte, end_byte)
                .into_iter()
                .map(|hint| to_lsp_inlay_hint(&line_index, &text, hint))
                .collect::<Vec<_>>()
        };

        let count = hints.len();
        crate::trace::request(
            "inlay_hint",
            start,
            &key,
            params.range.start.line,
            params.range.start.character,
            None,
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((count > 0).then_some(hints))
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let start = std::time::Instant::now();
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let key = match uri_to_key(&uri) {
            Some(k) => k,
            None => return Ok(None),
        };
        self.await_extension_sources(&key).await?;

        let snippets = *self.snippets_supported.lock();
        let query = |ws: &mut Workspace| {
            let text = ws.doc_text(&key)?;
            let line_index = LineIndex::new(&text);
            let offset = line_index.offset(&text, pos.line, pos.character);
            let symbol = crate::trace::ident_at(&text, offset);
            let (shaped, semantic) = ws.complete_with_semantic(&key, offset, snippets);
            let (items, is_incomplete, symbol, semantic) = match shaped {
                Some(shaped) => {
                    let incomplete = shaped.is_incomplete;
                    let items = shaped
                        .items
                        .into_iter()
                        .map(|item| to_completion_item(item, &line_index, &text))
                        .collect::<Vec<_>>();
                    (items, incomplete, symbol, semantic)
                }
                None => (Vec::new(), false, symbol, semantic),
            };
            Some((items, is_incomplete, symbol, semantic, text))
        };
        let (first, generation, version, pending) = {
            let mut ws = self.ws.lock();
            let first = query(&mut ws);
            let pending = if first.as_ref().is_some_and(|(items, _, _, semantic, _)| {
                items.is_empty()
                    && semantic.as_ref().is_some_and(|(kind, status, reasons)| {
                        *kind == "member"
                            && *status == "unknown"
                            && reasons
                                .iter()
                                .any(|reason| reason == "unknown-receiver-type")
                    })
            }) {
                self.jdk_source_seeds.lock().get(&key).cloned()
            } else {
                None
            };
            (
                first,
                ws.analysis_generation(),
                self.doc_versions
                    .lock()
                    .get(&key)
                    .map(|revision| revision.document_version),
                pending,
            )
        };
        let Some((mut items, mut is_incomplete, mut symbol, mut semantic, source)) = first else {
            return Ok(None);
        };
        if let Some((seed_generation, mut ready)) =
            pending.filter(|(seed_generation, _)| *seed_generation == generation)
        {
            // shortcut: only the current open-file JDK seed participates; new imports after edits wait for indexing.
            let _ = ready.changed().await;
            let mut ws = self.ws.lock();
            {
                let mut seeds = self.jdk_source_seeds.lock();
                if seeds
                    .get(&key)
                    .is_some_and(|(_, seed)| seed.same_channel(&ready))
                {
                    seeds.remove(&key);
                }
            }
            if !ws.is_current_generation(seed_generation)
                || self
                    .doc_versions
                    .lock()
                    .get(&key)
                    .map(|revision| revision.document_version)
                    != version
            {
                return Ok(None);
            }
            let Some((
                current_items,
                current_incomplete,
                current_symbol,
                current_semantic,
                current_source,
            )) = query(&mut ws)
            else {
                return Ok(None);
            };
            if current_source != source {
                return Ok(None);
            }
            (items, is_incomplete, symbol, semantic) = (
                current_items,
                current_incomplete,
                current_symbol,
                current_semantic,
            );
        }
        let count = items.len();
        if let Some((kind, status, reasons)) = semantic {
            crate::trace::semantic(
                "completion",
                &key,
                pos.line,
                pos.character,
                symbol.as_deref(),
                kind,
                status,
                &reasons,
            );
        }
        crate::trace::request(
            "completion",
            start,
            &key,
            pos.line,
            pos.character,
            symbol.as_deref(),
            if count > 0 { "ok" } else { "empty" },
            count,
        );
        Ok((!items.is_empty()).then(|| {
            CompletionResponse::List(CompletionList {
                is_incomplete,
                items,
            })
        }))
    }

    async fn completion_resolve(&self, params: CompletionItem) -> Result<CompletionItem> {
        Ok(params)
    }
}

#[allow(deprecated)]
fn to_symbol_information(
    ws: &Workspace,
    symbol: crate::symbols::SymbolSummary,
) -> Option<SymbolInformation> {
    Some(SymbolInformation {
        name: symbol.name.clone(),
        kind: to_lsp_symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        location: symbol_to_location(ws, &symbol)?,
        container_name: symbol.detail(),
    })
}

#[allow(deprecated)]
fn to_document_symbol(
    line_index: &LineIndex,
    text: &str,
    symbol: crate::symbols::SymbolSummary,
) -> DocumentSymbol {
    let (sl, sc) = line_index.position(text, symbol.start_byte);
    let (el, ec) = line_index.position(text, symbol.end_byte);
    let range = Range {
        start: Position {
            line: sl,
            character: sc,
        },
        end: Position {
            line: el,
            character: ec,
        },
    };
    let detail = symbol.detail();
    DocumentSymbol {
        name: symbol.name,
        detail,
        kind: to_lsp_symbol_kind(symbol.kind),
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

fn symbol_to_location(ws: &Workspace, symbol: &crate::symbols::SymbolSummary) -> Option<Location> {
    let text = ws.doc_text(&symbol.file)?;
    let line_index = LineIndex::new(&text);
    let (sl, sc) = line_index.position(&text, symbol.start_byte);
    let (el, ec) = line_index.position(&text, symbol.end_byte);
    Some(Location::new(
        key_to_uri(&symbol.file)?,
        Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        },
    ))
}

fn to_lsp_symbol_kind(kind: SymbolKind) -> tower_lsp_server::ls_types::SymbolKind {
    match kind {
        SymbolKind::Class => tower_lsp_server::ls_types::SymbolKind::CLASS,
        SymbolKind::Interface => tower_lsp_server::ls_types::SymbolKind::INTERFACE,
        SymbolKind::Object => tower_lsp_server::ls_types::SymbolKind::OBJECT,
        SymbolKind::EnumClass => tower_lsp_server::ls_types::SymbolKind::ENUM,
        SymbolKind::TypeAlias => tower_lsp_server::ls_types::SymbolKind::CLASS,
        SymbolKind::EnumEntry => tower_lsp_server::ls_types::SymbolKind::ENUM_MEMBER,
        SymbolKind::Function => tower_lsp_server::ls_types::SymbolKind::FUNCTION,
        SymbolKind::Property => tower_lsp_server::ls_types::SymbolKind::PROPERTY,
        SymbolKind::Parameter => tower_lsp_server::ls_types::SymbolKind::VARIABLE,
        SymbolKind::TypeParameter => tower_lsp_server::ls_types::SymbolKind::TYPE_PARAMETER,
        SymbolKind::LocalVariable => tower_lsp_server::ls_types::SymbolKind::VARIABLE,
    }
}

fn to_lsp_folding_range(range: crate::ranges::FoldRange) -> FoldingRange {
    FoldingRange {
        start_line: range.start_line,
        start_character: None,
        end_line: range.end_line,
        end_character: None,
        kind: range.kind.map(to_lsp_folding_range_kind),
        collapsed_text: None,
    }
}

fn to_lsp_folding_range_kind(kind: crate::ranges::FoldKind) -> FoldingRangeKind {
    match kind {
        crate::ranges::FoldKind::Imports => FoldingRangeKind::Imports,
        crate::ranges::FoldKind::Comment => FoldingRangeKind::Comment,
    }
}

fn to_lsp_selection_range(
    line_index: &LineIndex,
    text: &str,
    range: crate::ranges::SelectionRange,
) -> SelectionRange {
    SelectionRange {
        range: byte_range_to_lsp(line_index, text, range.start_byte, range.end_byte),
        parent: range
            .parent
            .map(|parent| Box::new(to_lsp_selection_range(line_index, text, *parent))),
    }
}

fn byte_range_to_lsp(line_index: &LineIndex, text: &str, start: usize, end: usize) -> Range {
    let (sl, sc) = line_index.position(text, start);
    let (el, ec) = line_index.position(text, end);
    Range {
        start: Position {
            line: sl,
            character: sc,
        },
        end: Position {
            line: el,
            character: ec,
        },
    }
}

fn action_kind_allowed(
    kind: crate::actions::ActionKind,
    requested: Option<&Vec<CodeActionKind>>,
) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let action = action_kind_str(kind);
    requested.iter().any(|wanted| {
        let wanted = wanted.as_str();
        action == wanted
            || action
                .strip_prefix(wanted)
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

fn action_kind_str(kind: crate::actions::ActionKind) -> &'static str {
    match kind {
        crate::actions::ActionKind::QuickFix => "quickfix",
        crate::actions::ActionKind::RefactorRewrite => "refactor.rewrite",
        crate::actions::ActionKind::SourceOrganizeImports => "source.organizeImports",
        crate::actions::ActionKind::SourceFixAllKtlsp => "source.fixAll.ktlsp",
    }
}

fn to_lsp_code_action(
    ws: &Workspace,
    action: crate::actions::Action,
) -> Option<CodeActionOrCommand> {
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: action.title,
        kind: Some(to_lsp_code_action_kind(action.kind)),
        diagnostics: None,
        edit: Some(to_lsp_workspace_edit(ws, action.edits)?),
        command: None,
        is_preferred: Some(action.is_preferred),
        disabled: None,
        data: None,
    }))
}

fn to_lsp_code_action_kind(kind: crate::actions::ActionKind) -> CodeActionKind {
    match kind {
        crate::actions::ActionKind::QuickFix => CodeActionKind::QUICKFIX,
        crate::actions::ActionKind::RefactorRewrite => CodeActionKind::REFACTOR_REWRITE,
        crate::actions::ActionKind::SourceOrganizeImports => {
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS
        }
        crate::actions::ActionKind::SourceFixAllKtlsp => CodeActionKind::new("source.fixAll.ktlsp"),
    }
}

fn to_lsp_workspace_edit(
    ws: &Workspace,
    edits: Vec<crate::edit::TextEdit>,
) -> Option<WorkspaceEdit> {
    let mut changes: HashMap<Uri, Vec<TextEdit>> = HashMap::new();
    for edit in edits {
        let text = ws.doc_text(&edit.file)?;
        let line_index = LineIndex::new(&text);
        let uri = key_to_uri(&edit.file)?;
        changes.entry(uri).or_default().push(TextEdit {
            range: byte_range_to_lsp(&line_index, &text, edit.start_byte, edit.end_byte),
            new_text: edit.new_text,
        });
    }
    Some(WorkspaceEdit::new(changes))
}

fn to_lsp_text_edits_for_text(
    text: &str,
    edits: Vec<crate::edit::TextEdit>,
) -> Option<Vec<TextEdit>> {
    let line_index = LineIndex::new(text);
    let mut out = Vec::new();
    for edit in edits {
        out.push(TextEdit {
            range: byte_range_to_lsp(&line_index, text, edit.start_byte, edit.end_byte),
            new_text: edit.new_text,
        });
    }
    Some(out)
}

fn goto_type_response(locations: Vec<Location>) -> Option<GotoTypeDefinitionResponse> {
    match locations.len() {
        0 => None,
        1 => Some(GotoTypeDefinitionResponse::Scalar(
            locations.into_iter().next().unwrap(),
        )),
        _ => Some(GotoTypeDefinitionResponse::Array(locations)),
    }
}

fn goto_implementation_response(locations: Vec<Location>) -> Option<GotoImplementationResponse> {
    match locations.len() {
        0 => None,
        1 => Some(GotoImplementationResponse::Scalar(
            locations.into_iter().next().unwrap(),
        )),
        _ => Some(GotoImplementationResponse::Array(locations)),
    }
}

fn to_lsp_signature_help(help: crate::signature::SignatureHelp) -> SignatureHelp {
    SignatureHelp {
        signatures: help
            .signatures
            .into_iter()
            .map(|sig| SignatureInformation {
                label: sig.label,
                documentation: None,
                parameters: Some(
                    sig.parameters
                        .into_iter()
                        .map(|label| ParameterInformation {
                            label: ParameterLabel::Simple(label),
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: None,
            })
            .collect(),
        active_signature: Some(0),
        active_parameter: help.active_parameter,
    }
}

fn to_call_hierarchy_item(ws: &Workspace, item: &HierarchyItem) -> Option<CallHierarchyItem> {
    let text = ws.doc_text(&item.file)?;
    let line_index = LineIndex::new(&text);
    let range = byte_range_to_lsp(&line_index, &text, item.start_byte, item.end_byte);
    Some(CallHierarchyItem {
        name: item.name.clone(),
        kind: to_lsp_symbol_kind(item.kind),
        tags: None,
        detail: (!item.package.is_empty()).then(|| item.package.clone()),
        uri: key_to_uri(&item.file)?,
        range,
        selection_range: range,
        data: Some(serde_json::json!({
            "file": item.file,
            "start": item.start_byte,
            "end": item.end_byte,
            "name": item.name,
            "kind": format!("{:?}", item.kind),
            "package": item.package,
        })),
    })
}

fn from_call_hierarchy_item(item: &CallHierarchyItem) -> Option<HierarchyItem> {
    if let Some(data) = &item.data {
        if let Some(item) = hierarchy_item_from_data(data) {
            return Some(item);
        }
    }
    hierarchy_item_from_parts(
        &item.uri,
        &item.name,
        item.kind,
        item.detail.as_deref().unwrap_or(""),
        item.selection_range,
    )
}

fn to_type_hierarchy_item(ws: &Workspace, item: &HierarchyItem) -> Option<TypeHierarchyItem> {
    let text = ws.doc_text(&item.file)?;
    let line_index = LineIndex::new(&text);
    let range = byte_range_to_lsp(&line_index, &text, item.start_byte, item.end_byte);
    Some(TypeHierarchyItem {
        name: item.name.clone(),
        kind: to_lsp_symbol_kind(item.kind),
        tags: None,
        detail: (!item.package.is_empty()).then(|| item.package.clone()),
        uri: key_to_uri(&item.file)?,
        range,
        selection_range: range,
        data: Some(serde_json::json!({
            "file": item.file,
            "start": item.start_byte,
            "end": item.end_byte,
            "name": item.name,
            "kind": format!("{:?}", item.kind),
            "package": item.package,
        })),
    })
}

fn from_type_hierarchy_item(item: &TypeHierarchyItem) -> Option<HierarchyItem> {
    if let Some(data) = &item.data {
        if let Some(item) = hierarchy_item_from_data(data) {
            return Some(item);
        }
    }
    hierarchy_item_from_parts(
        &item.uri,
        &item.name,
        item.kind,
        item.detail.as_deref().unwrap_or(""),
        item.selection_range,
    )
}

fn hierarchy_item_from_parts(
    uri: &Uri,
    name: &str,
    kind: tower_lsp_server::ls_types::SymbolKind,
    package: &str,
    selection_range: Range,
) -> Option<HierarchyItem> {
    let file = uri_to_key(uri)?;
    let text = std::fs::read_to_string(&file).ok();
    let (start_byte, end_byte) = if let Some(text) = text {
        let line_index = LineIndex::new(&text);
        (
            line_index.offset(
                &text,
                selection_range.start.line,
                selection_range.start.character,
            ),
            line_index.offset(
                &text,
                selection_range.end.line,
                selection_range.end.character,
            ),
        )
    } else {
        (0, 0)
    };
    Some(HierarchyItem {
        name: name.to_string(),
        kind: from_lsp_symbol_kind(kind),
        package: package.to_string(),
        file,
        start_byte,
        end_byte,
    })
}

fn hierarchy_item_from_data(data: &serde_json::Value) -> Option<HierarchyItem> {
    let file = data.get("file")?.as_str()?.to_string();
    let name = data.get("name")?.as_str()?.to_string();
    let package = data
        .get("package")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let start_byte = data.get("start")?.as_u64()? as usize;
    let end_byte = data.get("end")?.as_u64()? as usize;
    let kind = match data.get("kind").and_then(|v| v.as_str()).unwrap_or("") {
        "Class" => SymbolKind::Class,
        "Interface" => SymbolKind::Interface,
        "Object" => SymbolKind::Object,
        "EnumClass" => SymbolKind::EnumClass,
        "EnumEntry" => SymbolKind::EnumEntry,
        "Function" => SymbolKind::Function,
        "Property" => SymbolKind::Property,
        "Parameter" => SymbolKind::Parameter,
        "TypeParameter" => SymbolKind::TypeParameter,
        _ => SymbolKind::LocalVariable,
    };
    Some(HierarchyItem {
        name,
        kind,
        package,
        file,
        start_byte,
        end_byte,
    })
}

fn from_lsp_symbol_kind(kind: tower_lsp_server::ls_types::SymbolKind) -> SymbolKind {
    match kind {
        tower_lsp_server::ls_types::SymbolKind::CLASS => SymbolKind::Class,
        tower_lsp_server::ls_types::SymbolKind::INTERFACE => SymbolKind::Interface,
        tower_lsp_server::ls_types::SymbolKind::OBJECT => SymbolKind::Object,
        tower_lsp_server::ls_types::SymbolKind::ENUM => SymbolKind::EnumClass,
        tower_lsp_server::ls_types::SymbolKind::ENUM_MEMBER => SymbolKind::EnumEntry,
        tower_lsp_server::ls_types::SymbolKind::FUNCTION => SymbolKind::Function,
        tower_lsp_server::ls_types::SymbolKind::PROPERTY => SymbolKind::Property,
        tower_lsp_server::ls_types::SymbolKind::TYPE_PARAMETER => SymbolKind::TypeParameter,
        _ => SymbolKind::LocalVariable,
    }
}

fn to_lsp_incoming_call(
    ws: &Workspace,
    call: crate::hierarchy::IncomingCall,
) -> Option<CallHierarchyIncomingCall> {
    Some(CallHierarchyIncomingCall {
        from: to_call_hierarchy_item(ws, &call.from)?,
        from_ranges: defs_to_ranges(ws, call.ranges)?,
    })
}

fn to_lsp_outgoing_call(
    ws: &Workspace,
    call: crate::hierarchy::OutgoingCall,
) -> Option<CallHierarchyOutgoingCall> {
    Some(CallHierarchyOutgoingCall {
        to: to_call_hierarchy_item(ws, &call.to)?,
        from_ranges: defs_to_ranges(ws, call.ranges)?,
    })
}

fn defs_to_ranges(ws: &Workspace, defs: Vec<Def>) -> Option<Vec<Range>> {
    let mut out = Vec::new();
    for def in defs {
        let text = ws.doc_text(&def.file)?;
        let line_index = LineIndex::new(&text);
        out.push(byte_range_to_lsp(
            &line_index,
            &text,
            def.start_byte,
            def.end_byte,
        ));
    }
    Some(out)
}

fn command_uri_position(args: &[serde_json::Value]) -> Option<(Uri, Position)> {
    let first = args.first()?;
    let uri = first.get("uri").and_then(|v| v.as_str()).or_else(|| {
        first
            .get("textDocument")
            .and_then(|td| td.get("uri"))
            .and_then(|v| v.as_str())
    })?;
    let position = first.get("position")?.clone();
    Some((uri.parse().ok()?, serde_json::from_value(position).ok()?))
}

fn semantic_tokens_legend() -> SemanticTokensLegend {
    SemanticTokensLegend {
        token_types: vec![
            SemanticTokenType::NAMESPACE,
            SemanticTokenType::CLASS,
            SemanticTokenType::INTERFACE,
            SemanticTokenType::new("object"),
            SemanticTokenType::ENUM,
            SemanticTokenType::FUNCTION,
            SemanticTokenType::PROPERTY,
            SemanticTokenType::VARIABLE,
            SemanticTokenType::PARAMETER,
            SemanticTokenType::TYPE_PARAMETER,
            SemanticTokenType::ENUM_MEMBER,
            SemanticTokenType::KEYWORD,
            SemanticTokenType::STRING,
            SemanticTokenType::NUMBER,
            SemanticTokenType::COMMENT,
        ],
        token_modifiers: vec![SemanticTokenModifier::DECLARATION],
    }
}

fn to_lsp_semantic_tokens(
    line_index: &LineIndex,
    text: &str,
    tokens: Vec<crate::semantic::SemanticToken>,
) -> Vec<SemanticToken> {
    let mut out = Vec::new();
    let mut prev_line = 0;
    let mut prev_start = 0;
    for token in tokens {
        let (line, start) = line_index.position(text, token.start_byte);
        let (end_line, end) = line_index.position(text, token.end_byte);
        if line != end_line || end <= start {
            continue;
        }
        let delta_line = line - prev_line;
        let delta_start = if delta_line == 0 {
            start.saturating_sub(prev_start)
        } else {
            start
        };
        out.push(SemanticToken {
            delta_line,
            delta_start,
            length: end - start,
            token_type: semantic_token_type_index(token.kind),
            token_modifiers_bitset: u32::from(token.declaration),
        });
        prev_line = line;
        prev_start = start;
    }
    out
}

fn semantic_token_type_index(kind: crate::semantic::SemanticTokenKind) -> u32 {
    match kind {
        crate::semantic::SemanticTokenKind::Namespace => 0,
        crate::semantic::SemanticTokenKind::Class => 1,
        crate::semantic::SemanticTokenKind::Interface => 2,
        crate::semantic::SemanticTokenKind::Object => 3,
        crate::semantic::SemanticTokenKind::Enum => 4,
        crate::semantic::SemanticTokenKind::Function => 5,
        crate::semantic::SemanticTokenKind::Property => 6,
        crate::semantic::SemanticTokenKind::Variable => 7,
        crate::semantic::SemanticTokenKind::Parameter => 8,
        crate::semantic::SemanticTokenKind::TypeParameter => 9,
        crate::semantic::SemanticTokenKind::EnumMember => 10,
        crate::semantic::SemanticTokenKind::Keyword => 11,
        crate::semantic::SemanticTokenKind::String => 12,
        crate::semantic::SemanticTokenKind::Number => 13,
        crate::semantic::SemanticTokenKind::Comment => 14,
    }
}

fn to_lsp_inlay_hint(
    line_index: &LineIndex,
    text: &str,
    hint: crate::hints::InlayHint,
) -> InlayHint {
    let (line, character) = line_index.position(text, hint.position_byte);
    InlayHint {
        position: Position { line, character },
        label: InlayHintLabel::String(hint.label),
        kind: Some(match hint.kind {
            crate::hints::InlayHintKind::Type => InlayHintKind::TYPE,
        }),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: None,
        data: None,
    }
}

/// The single `SymbolKind`/`is_keyword` -> `CompletionItemKind` mapping site. Stage C also threads
/// through the shaped `sortText`/`filterText`/`insertText`/`insertTextFormat`/`detail` and the
/// auto-import `additionalTextEdits` (a zero-width insert of one `import` line at column 0). This is
/// the only `ls-types`-aware completion code.
fn to_completion_item(it: ShapedItem, line_index: &LineIndex, text: &str) -> CompletionItem {
    let kind = if it.is_keyword {
        CompletionItemKind::KEYWORD
    } else {
        match it.kind {
            SymbolKind::Class => CompletionItemKind::CLASS,
            SymbolKind::Interface => CompletionItemKind::INTERFACE,
            SymbolKind::Object => CompletionItemKind::MODULE,
            SymbolKind::EnumClass => CompletionItemKind::ENUM,
            SymbolKind::TypeAlias => CompletionItemKind::CLASS,
            SymbolKind::EnumEntry => CompletionItemKind::ENUM_MEMBER,
            SymbolKind::Function => CompletionItemKind::FUNCTION,
            SymbolKind::Property => CompletionItemKind::PROPERTY,
            SymbolKind::Parameter => CompletionItemKind::VARIABLE,
            SymbolKind::TypeParameter => CompletionItemKind::TYPE_PARAMETER,
            SymbolKind::LocalVariable => CompletionItemKind::VARIABLE,
        }
    };
    CompletionItem {
        text_edit: it.replace_range.map(|range| {
            let (sl, sc) = line_index.position(text, range.start);
            let (el, ec) = line_index.position(text, range.end);
            CompletionTextEdit::Edit(TextEdit {
                range: Range::new(Position::new(sl, sc), Position::new(el, ec)),
                new_text: it.insert_text.clone(),
            })
        }),
        label: it.label,
        kind: Some(kind),
        sort_text: Some(it.sort_text),
        filter_text: Some(it.filter_text),
        insert_text: Some(it.insert_text),
        insert_text_format: Some(if it.is_snippet {
            InsertTextFormat::SNIPPET
        } else {
            InsertTextFormat::PLAIN_TEXT
        }),
        detail: it.detail,
        additional_text_edits: it.auto_import.map(|imp| {
            vec![TextEdit {
                range: Range {
                    start: Position {
                        line: imp.line,
                        character: 0,
                    },
                    end: Position {
                        line: imp.line,
                        character: 0,
                    },
                },
                new_text: format!("{}\n", imp.text),
            }]
        }),
        ..Default::default()
    }
}

/// Convert a core byte-range `Diagnostic` into an LSP `Diagnostic`, mapping byte offsets to UTF-16
/// positions (the only place this conversion happens for diagnostics) and the severity enum.
fn to_lsp_diagnostic(
    line_index: &LineIndex,
    text: &str,
    d: &crate::diagnostics::Diagnostic,
) -> Diagnostic {
    let (sl, sc) = line_index.position(text, d.start_byte);
    let (el, ec) = line_index.position(text, d.end_byte);
    Diagnostic {
        range: Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        },
        severity: Some(severity_to_lsp(d.severity)),
        code: d
            .code
            .map(|code| NumberOrString::String(code.as_str().to_string())),
        source: Some("ktlsp".into()),
        message: d.message.clone(),
        ..Default::default()
    }
}

/// The single `Severity` -> `DiagnosticSeverity` mapping.
fn severity_to_lsp(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

/// Publish the current fast diagnostics for `key`. Computes under the workspace lock and drops it
/// before the publish `.await`.
async fn publish_diagnostics(
    ws: &Arc<Mutex<Workspace>>,
    client: &Client,
    versions: &Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    key: &str,
) {
    let uri = match key_to_uri(key) {
        Some(u) => u,
        None => return,
    };
    let (items, _, version) = lsp_diagnostics_with_result(ws, versions, key);
    client.publish_diagnostics(uri, items, version).await;
    schedule_extension_sources(ws, client, versions);
}

fn schedule_extension_sources(
    workspace: &Arc<Mutex<Workspace>>,
    client: &Client,
    versions: &Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
) {
    let mut requests = workspace.lock().begin_extension_source_batch();
    if requests.is_empty() {
        return;
    }
    let workspace = workspace.clone();
    let client = client.clone();
    let versions = versions.clone();
    tokio::spawn(async move {
        loop {
            let prepared = tokio::task::spawn_blocking(move || {
                crate::analysis_extensions::prepare_sources(requests)
            })
            .await
            .unwrap_or_default();
            let changed = workspace.lock().finish_extension_source_batch(prepared);
            for key in changed {
                schedule_owned_diagnostic(&workspace, &client, &versions, key);
            }
            requests = workspace.lock().begin_extension_source_batch();
            if requests.is_empty() {
                break;
            }
        }
    });
}

fn lsp_diagnostics_with_result(
    ws: &Arc<Mutex<Workspace>>,
    versions: &Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    key: &str,
) -> (Vec<Diagnostic>, Option<String>, Option<i32>) {
    let (text, diagnostics, result_id, version) = {
        let mut ws = ws.lock();
        let version = versions
            .lock()
            .get(key)
            .and_then(|revision| revision.document_version);
        match ws.doc_text(key) {
            Some(text) => {
                let diagnostics = ws.diagnostics(key);
                // The compiler-store identity and merged items come from the same locked snapshot.
                let result_id = ws.kotlin_diagnostic_result_id(key);
                (Some(text), diagnostics, result_id, version)
            }
            None => (None, Vec::new(), None, version),
        }
    };
    let items = match text {
        Some(text) => {
            let line_index = LineIndex::new(&text);
            diagnostics
                .iter()
                .map(|d| to_lsp_diagnostic(&line_index, &text, d))
                .collect()
        }
        None => Vec::new(),
    };
    (items, result_id, version)
}

/// Convert a core `Def` (file key + byte range) into an LSP `Location`, reading the target file's
/// text to convert byte offsets to UTF-16 positions. `None` if the file is unreadable or its key
/// isn't a `file://`-convertible path.
fn def_to_location(ws: &Workspace, d: &Def) -> Option<Location> {
    let text = ws.doc_text(&d.file)?;
    let line_index = LineIndex::new(&text);
    let (sl, sc) = line_index.position(&text, d.start_byte);
    let (el, ec) = line_index.position(&text, d.end_byte);
    Some(Location::new(
        key_to_uri(&d.file)?,
        Range {
            start: Position {
                line: sl,
                character: sc,
            },
            end: Position {
                line: el,
                character: ec,
            },
        },
    ))
}

fn import_seed_locations(batches: &[crate::deps::FileSymbols], import_fqn: &str) -> Vec<Location> {
    let Some((package, name)) = import_fqn.rsplit_once('.') else {
        return Vec::new();
    };
    batches
        .iter()
        .flat_map(|batch| {
            let Ok(text) = std::fs::read_to_string(&batch.file) else {
                return Vec::new();
            };
            let line_index = LineIndex::new(&text);
            batch
                .symbols
                .iter()
                .filter(|symbol| symbol.name == name && symbol.package == package)
                .filter_map(|symbol| {
                    let (sl, sc) = line_index.position(&text, symbol.start_byte);
                    let (el, ec) = line_index.position(&text, symbol.end_byte);
                    Some(Location::new(
                        key_to_uri(&batch.file)?,
                        Range {
                            start: Position {
                                line: sl,
                                character: sc,
                            },
                            end: Position {
                                line: el,
                                character: ec,
                            },
                        },
                    ))
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn def_to_range(ws: &Workspace, d: &Def) -> Option<Range> {
    def_to_location(ws, d).map(|l| l.range)
}

#[cfg(test)]
mod tests {
    fn generation_one_workspace() -> Arc<Mutex<Workspace>> {
        let mut workspace = Workspace::new();
        assert_eq!(workspace.begin_analysis_generation(), 1);
        Arc::new(Mutex::new(workspace))
    }

    #[test]
    fn selected_sdk_seed_waits_for_gradle_model_and_preserves_standalone_default() {
        let file = Path::new("/pending/src/Use.java");
        assert_eq!(selected_jdk_source_for_file(None, file, true), None);
        assert_eq!(
            selected_jdk_source_for_file(None, file, false),
            crate::deps::jdk_src_zip()
        );
        let model = classpath::ProjectSnapshot::new(Vec::new());
        assert_eq!(selected_jdk_source_for_file(Some(&model), file, true), None);
    }

    const SDK_CALLER_JAVA: &str = "class Use { int value(java.util.concurrent.atomic.AtomicInteger i, String s) { i.incrementAndGet(); s.strip(); return 0; } }";
    const SDK_CALLER_KOTLIN: &str = "fun value(i: java.util.concurrent.atomic.AtomicInteger, s: java.lang.String): String { i.incrementAndGet(); return s.strip() }";

    // Uses the existing dependency-index entry point so this test compiles before SDK API support.
    fn caller_sdk_index_fixture() -> (PathBuf, classpath::ModuleClasspath, Arc<Mutex<Workspace>>) {
        use std::io::Write;
        let (root, mut modules) = classpath::tests::sdk_selection_fixture();
        let mut module = modules.remove(0);
        let source = &mut module.source_sets[0];
        let source_root = source.source_roots[0].clone();
        std::fs::create_dir_all(&source_root).unwrap();
        std::fs::write(source_root.join("Use.java"), SDK_CALLER_JAVA).unwrap();
        std::fs::write(source_root.join("Use.kt"), SDK_CALLER_KOTLIN).unwrap();
        let java = &mut source.compiler_tasks[0];
        let home = java.java_home.clone().unwrap();
        java.source_files = vec![source_root.join("Use.java")];
        java.classpath.clear();
        java.arguments = vec!["-proc:none".into()];
        java.options.insert("release".into(), serde_json::json!(8));
        java.options.get_mut("javaCompilerRecipe").unwrap()["api_options"] =
            serde_json::json!(["--release", "8", "-proc:none"]);
        let mut kotlin = java.clone();
        kotlin.language = classpath::CompilerLanguage::Kotlin;
        kotlin.task = ":compileKotlin".into();
        kotlin.task_type = "org.jetbrains.kotlin.gradle.tasks.KotlinCompile".into();
        kotlin.source_files = vec![source_root.join("Use.kt"), source_root.join("Use.java")];
        kotlin.arguments.clear();
        source.kotlin_compile_task = Some(kotlin.task.clone());
        source.compiler_tasks = vec![source.compiler_tasks[0].clone(), kotlin];
        for task in &mut source.compiler_tasks {
            task.options.insert(
                "sdkSourcePolicy".into(),
                serde_json::json!({
                    "adapter":"gradle-source-tree-stock-patterns-v1", "task":task.task,
                    "language":task.language, "authored_roots":[source_root],
                    "future_roots":[source_root], "generated_roots":[source_root.join("generated")],
                    "default_excludes": classpath::tests::sdk_source_default_excludes(),
                }),
            );
            task.options.insert(
                "sdkProducerClaims".into(),
                serde_json::json!({
                    "adapter":"gradle-producing-source-trees-v1", "task":task.task,
                    "language":task.language, "roots":[source_root], "complete":true,
                }),
            );
        }
        let fixtures: &[(&str, &[u8])] = &[
            (
                "java/lang/Object",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Object.sig"),
            ),
            (
                "java/lang/Number",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Number.sig"),
            ),
            (
                "java/lang/String",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/String.sig"),
            ),
            (
                "java/util/concurrent/atomic/AtomicInteger",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/AtomicInteger.sig"
                ),
            ),
            (
                "java/io/Serializable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Serializable.sig"
                ),
            ),
            (
                "java/lang/Comparable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Comparable.sig"
                ),
            ),
            (
                "java/lang/CharSequence",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/CharSequence.sig"
                ),
            ),
        ];
        let mut signatures =
            zip::ZipWriter::new(std::fs::File::create(home.join("lib/ct.sym")).unwrap());
        for (owner, bytes) in fixtures {
            signatures
                .start_file(
                    format!("8/java.base/{owner}.sig"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            signatures.write_all(bytes).unwrap();
        }
        signatures.finish().unwrap();
        let mut sources =
            zip::ZipWriter::new(std::fs::File::create(home.join("lib/src.zip")).unwrap());
        for (owner, text) in [
            ("java/lang/String", "package java.lang; public final class String { public String strip(){return this;} public String trim(){return this;} }"),
            ("java/util/concurrent/atomic/AtomicInteger", "package java.util.concurrent.atomic; public class AtomicInteger { public final int incrementAndGet(){return 0;} }"),
        ] {
            sources.start_file(format!("java.base/{owner}.java"), zip::write::SimpleFileOptions::default()).unwrap();
            sources.write_all(text.as_bytes()).unwrap();
        }
        sources.finish().unwrap();
        let model = Arc::new(classpath::ProjectSnapshot::new(vec![module.clone()]));
        let ws = Arc::new(Mutex::new(Workspace::new()));
        ws.lock().set_project_model(model.clone());
        let generation = ws.lock().analysis_generation();
        let stats = index_dependency_model(&ws, &root, Some(model), true, generation, None, true);
        assert_eq!(stats.failed, 0);
        let mut guard = ws.lock();
        for (name, text) in [
            ("Use.java", SDK_CALLER_JAVA),
            ("Use.kt", SDK_CALLER_KOTLIN),
            ("New.java", SDK_CALLER_JAVA),
            ("New.kt", SDK_CALLER_KOTLIN),
        ] {
            if name.starts_with("New") {
                assert!(!source_root.join(name).exists());
            }
            guard.open(
                &crate::paths::file_key(&source_root.join(name)),
                text.into(),
            );
        }
        guard.assume_index_complete_for_tests();
        drop(guard);
        (root, module, ws)
    }

    fn assert_sdk_caller_member(
        ws: &mut Workspace,
        file: &Path,
        prefix: &str,
        member: &str,
        expected: bool,
    ) {
        let key = crate::paths::file_key(file);
        let text = ws.doc_text(&key).unwrap();
        let offset = text.find(prefix).unwrap() + prefix.len();
        let found = ws
            .complete(&key, offset, true)
            .is_some_and(|result| result.items.iter().any(|item| item.label == member));
        assert_eq!(found, expected, "{file:?}: {member}");
        if expected {
            let start = text.find(&format!("{member}(")).unwrap();
            let definitions = ws.goto_definition(&key, start);
            assert_eq!(definitions.len(), 1, "{file:?}: exact {member} navigation");
        }
    }

    #[test]
    fn caller_sdk_release8_and_kotlin17_feed_current_and_unsaved_queries() {
        let (root, module, ws) = caller_sdk_index_fixture();
        let source_root = module.source_sets[0].source_roots[0].clone();
        let mut ws = ws.lock();
        for name in ["Use.java", "Use.kt", "New.java", "New.kt"] {
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "i.inc",
                "incrementAndGet",
                true,
            );
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "s.str",
                "strip",
                name.ends_with(".kt"),
            );
        }
        for (name, text, prefix, member) in [
            (
                "generated/Kapt.java",
                SDK_CALLER_JAVA,
                "i.inc",
                "incrementAndGet",
            ),
            ("generated/Ksp.kt", SDK_CALLER_KOTLIN, "s.str", "strip"),
            ("Script.kts", SDK_CALLER_KOTLIN, "s.str", "strip"),
            (
                "Foreign.java",
                "class Foreign { void use(foreign.Missing i) { i.incrementAndGet(); } }",
                "i.inc",
                "incrementAndGet",
            ),
        ] {
            let path = source_root.join(name);
            ws.open(&crate::paths::file_key(&path), text.into());
            assert_sdk_caller_member(&mut ws, &path, prefix, member, false);
        }
        drop(ws);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_kotlin_dotted_filenames_keep_current_and_unsaved_queries() {
        let (root, module, ws) = caller_sdk_index_fixture();
        let source_root = module.source_sets[0].source_roots[0].clone();
        let names = ["Platform.jvm.kt", "Caf\u{e9}.日本語.kt", "Cafe\u{301}.kt"];
        let mut captured = module.clone();
        for name in names {
            let path = source_root.join(name);
            std::fs::write(&path, SDK_CALLER_KOTLIN).unwrap();
            captured.source_sets[0].compiler_tasks[1]
                .source_files
                .push(path);
        }
        let mut ws = ws.lock();
        ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![captured])));
        for name in names {
            let path = source_root.join(name);
            ws.open(&crate::paths::file_key(&path), SDK_CALLER_KOTLIN.into());
            assert_sdk_caller_member(&mut ws, &path, "i.inc", "incrementAndGet", true);
            assert_sdk_caller_member(&mut ws, &path, "s.str", "strip", true);
            std::fs::remove_file(path).unwrap();
        }
        // The same buffers now have no on-disk file or exact compiler-input claim.
        ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![
            module.clone()
        ])));
        for name in names {
            let path = source_root.join(name);
            assert!(!path.exists());
            assert_sdk_caller_member(&mut ws, &path, "i.inc", "incrementAndGet", true);
            assert_sdk_caller_member(&mut ws, &path, "s.str", "strip", true);
        }
        for name in ["Platform.jvm.kts", ".#Platform.kt", "._Platform.kt"] {
            let path = source_root.join(name);
            ws.open(&crate::paths::file_key(&path), SDK_CALLER_KOTLIN.into());
            assert_sdk_caller_member(&mut ws, &path, "i.inc", "incrementAndGet", false);
        }
        let mut competing = module.clone();
        competing.module.push_str("-competing-kotlin-producer");
        competing.source_sets[0]
            .compiler_tasks
            .retain(|task| task.language == classpath::CompilerLanguage::Kotlin);
        competing.source_sets[0].compiler_tasks[0]
            .options
            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
        ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![
            module, competing,
        ])));
        for name in names {
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "i.inc",
                "incrementAndGet",
                false,
            );
        }
        drop(ws);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_stock_task_paths_feed_unsaved_java_and_kotlin_queries() {
        let (root, module, ws) = caller_sdk_index_fixture();
        let source_root = module.source_sets[0].source_roots[0].clone();
        let mut ws = ws.lock();
        for (name, text) in [
            ("nested.with.dots/Use.java", SDK_CALLER_JAVA),
            ("nested-with-hyphen/Use.java", SDK_CALLER_JAVA),
            ("nested space/A.B.java", SDK_CALLER_JAVA),
            ("nested.with.dots/Platform.jvm.kt", SDK_CALLER_KOTLIN),
            ("nested-with-hyphen/Use.kt", SDK_CALLER_KOTLIN),
            ("nested space/Cafe\u{301}.kt", SDK_CALLER_KOTLIN),
            ("_darcs/Use.java", SDK_CALLER_JAVA),
            ("_darcs/Use.kt", SDK_CALLER_KOTLIN),
        ] {
            let path = source_root.join(name);
            assert!(!path.exists(), "new buffer is not an exact compiler input");
            ws.open(&crate::paths::file_key(&path), text.into());
            assert_sdk_caller_member(&mut ws, &path, "i.inc", "incrementAndGet", true);
            assert_sdk_caller_member(&mut ws, &path, "s.str", "strip", name.ends_with(".kt"));
        }
        for name in [
            "CVS/Use.java",
            ".git/Use.kt",
            "backup~/Use.java",
            ".gitignore/Use.kt",
            "generated/nested.with.dots/Use.kt",
            "nested.with.dots/Use.kts",
        ] {
            let path = source_root.join(name);
            ws.open(
                &crate::paths::file_key(&path),
                if name.ends_with(".java") {
                    SDK_CALLER_JAVA.into()
                } else {
                    SDK_CALLER_KOTLIN.into()
                },
            );
            assert_sdk_caller_member(&mut ws, &path, "i.inc", "incrementAndGet", false);
        }
        let mut competing = module.clone();
        competing.module.push_str("-competing-producer");
        for task in &mut competing.source_sets[0].compiler_tasks {
            task.options
                .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
        }
        ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![
            module, competing,
        ])));
        for name in [
            "nested.with.dots/Use.java",
            "nested.with.dots/Platform.jvm.kt",
        ] {
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "i.inc",
                "incrementAndGet",
                false,
            );
        }
        drop(ws);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_missing_producer_claims_veto_current_and_unsaved_queries() {
        let (root, module, ws) = caller_sdk_index_fixture();
        let source_root = module.source_sets[0].source_roots[0].clone();
        let mut ws = ws.lock();
        for name in ["Use.java", "Use.kt", "New.java", "New.kt"] {
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "i.inc",
                "incrementAndGet",
                true,
            );
        }
        for (language, suffix) in [
            (classpath::CompilerLanguage::Java, "java"),
            (classpath::CompilerLanguage::Kotlin, "kt"),
        ] {
            for failure in [
                "unsupported-sdk",
                "null-policy",
                "missing-claims",
                "incomplete-producer",
            ] {
                let mut second = module.clone();
                second.module.push_str("-competing-producer");
                second.source_sets[0]
                    .compiler_tasks
                    .retain(|task| task.language == language);
                let task = &mut second.source_sets[0].compiler_tasks[0];
                match failure {
                    "unsupported-sdk" if language == classpath::CompilerLanguage::Java => {
                        task.options
                            .insert("release".into(), serde_json::json!(999));
                    }
                    "unsupported-sdk" => task.arguments.push("-Xjdk-release=999".into()),
                    "null-policy" => {
                        task.options
                            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                    }
                    "missing-claims" => {
                        task.options
                            .insert("sdkProducerClaims".into(), serde_json::Value::Null);
                    }
                    "incomplete-producer" => task.complete = false,
                    _ => unreachable!(),
                }
                ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![
                    module.clone(),
                    second,
                ])));
                for name in [format!("Use.{suffix}"), format!("New.{suffix}")] {
                    assert_sdk_caller_member(
                        &mut ws,
                        &source_root.join(name),
                        "i.inc",
                        "incrementAndGet",
                        false,
                    );
                }
                let other = if suffix == "java" { "kt" } else { "java" };
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join(format!("New.{other}")),
                    "i.inc",
                    "incrementAndGet",
                    true,
                );
            }
        }
        let mut auxiliary = module.clone();
        auxiliary.module.push_str("-kapt-nonproducer");
        let mut task = auxiliary.source_sets[0].compiler_tasks[1].clone();
        task.task = ":kaptGenerateStubsKotlin".into();
        task.task_type = "org.jetbrains.kotlin.gradle.internal.KaptGenerateStubsTask".into();
        task.options
            .insert("sdkProducerClaims".into(), serde_json::Value::Null);
        task.options
            .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
        auxiliary.source_sets[0].compiler_tasks = vec![task];
        auxiliary.source_sets[0].kotlin_compile_task = None;
        ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![
            module.clone(),
            auxiliary,
        ])));
        for name in ["New.java", "New.kt"] {
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(name),
                "i.inc",
                "incrementAndGet",
                true,
            );
        }
        for unrelated in [false, true] {
            for proof in ["known-empty", "missing", "opaque", "incomplete-compiler"] {
                let mut unknown = module.source_sets[0].clone();
                unknown.name = "unproved-inputs".into();
                unknown.source_roots = vec![root.join("other-declared-root")];
                unknown
                    .compiler_tasks
                    .retain(|task| task.language == classpath::CompilerLanguage::Java);
                unknown.kotlin_compile_task = None;
                let task = &mut unknown.compiler_tasks[0];
                task.task = ":compileUnknownJava".into();
                task.source_files.clear();
                task.options
                    .insert("sdkProducerClaims".into(), serde_json::Value::Null);
                task.options
                    .insert("sdkSourcePolicy".into(), serde_json::Value::Null);
                if proof == "known-empty" || proof == "opaque" {
                    task.options.insert(
                        "sdkProducerClaims".into(),
                        serde_json::json!({
                            "adapter":"gradle-producing-source-trees-v1", "task":task.task,
                            "language":task.language, "roots":[], "complete":proof == "known-empty",
                        }),
                    );
                }
                if proof == "incomplete-compiler" {
                    task.complete = false;
                }
                let model = if unrelated {
                    let mut other = module.clone();
                    other.module.push_str("-unrelated");
                    other.source_sets = vec![unknown];
                    classpath::ProjectSnapshot::new(vec![module.clone(), other])
                } else {
                    let mut local = module.clone();
                    local.source_sets.push(unknown);
                    classpath::ProjectSnapshot::new(vec![local])
                };
                ws.set_project_model(Arc::new(model));
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join("Use.java"),
                    "i.inc",
                    "incrementAndGet",
                    unrelated || proof == "known-empty",
                );
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join("New.java"),
                    "i.inc",
                    "incrementAndGet",
                    unrelated || proof == "known-empty",
                );
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join("New.kt"),
                    "i.inc",
                    "incrementAndGet",
                    true,
                );
            }
        }
        drop(ws);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn caller_sdk_same_source_set_rejects_multiple_actual_producers() {
        for (language, suffix) in [
            (classpath::CompilerLanguage::Java, "java"),
            (classpath::CompilerLanguage::Kotlin, "kt"),
        ] {
            let (root, mut module, workspace) = caller_sdk_index_fixture();
            let source_root = module.source_sets[0].source_roots[0].clone();
            let mut ws = workspace.lock();
            for name in [format!("Use.{suffix}"), format!("New.{suffix}")] {
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join(name),
                    "i.inc",
                    "incrementAndGet",
                    true,
                );
            }
            let mut unsupported = module.source_sets[0]
                .compiler_tasks
                .iter()
                .find(|task| task.language == language)
                .unwrap()
                .clone();
            if language == classpath::CompilerLanguage::Java {
                unsupported.task.push_str("Other");
                unsupported
                    .options
                    .insert("release".into(), serde_json::json!(999));
            } else {
                unsupported.arguments.push("-Xjdk-release=999".into());
            }
            module.source_sets[0].compiler_tasks.push(unsupported);
            ws.set_project_model(Arc::new(classpath::ProjectSnapshot::new(vec![module])));
            for name in [format!("Use.{suffix}"), format!("New.{suffix}")] {
                assert_sdk_caller_member(
                    &mut ws,
                    &source_root.join(name),
                    "i.inc",
                    "incrementAndGet",
                    false,
                );
            }
            let other = if suffix == "java" { "kt" } else { "java" };
            assert_sdk_caller_member(
                &mut ws,
                &source_root.join(format!("New.{other}")),
                "i.inc",
                "incrementAndGet",
                true,
            );
            drop(ws);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn selected_sdk_pool_tracks_two_archives_and_requires_every_origin_complete() {
        use std::io::Write;
        for failed_21 in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "ktlsp-sdk-pool-{}-{}-{}",
                std::process::id(),
                failed_21,
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let mut modules = Vec::new();
            let mut origins = Vec::new();
            for feature in [17, 21] {
                let home = root.join(format!("jdk{feature}"));
                std::fs::create_dir_all(home.join("lib")).unwrap();
                std::fs::write(
                    home.join("release"),
                    format!("JAVA_VERSION=\"{feature}.0.1\"\n"),
                )
                .unwrap();
                let src = home.join("lib/src.zip");
                let mut zip = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
                zip.start_file(
                    format!("java.base/java/lang/Only{feature}.java"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
                if failed_21 && feature == 21 {
                    zip.write_all(b"\xff").unwrap();
                } else {
                    zip.write_all(
                        format!(
                            "package java.lang; public class Only{feature} {{ public int value; }}"
                        )
                        .as_bytes(),
                    )
                    .unwrap();
                }
                zip.finish().unwrap();
                origins.push(classpath::jdk_origin_id(&src));
                let task = serde_json::from_value(serde_json::json!({
                    "task":format!(":sdk{feature}:compileJava"), "language":"java",
                    "task_type":"org.gradle.api.tasks.compile.JavaCompile_Decorated",
                    "arguments":[], "annotation_processor_path":[], "source_files":[],
                    "classpath":[], "java_home":home, "output_roots":[], "complete":true, "problem":null,
                    "options": {"fork":false,"forkJavaHome":null,"forkExecutable":null,"forkJvmArgs":[],
                        "release":null,"bootstrapClasspath":null,"javaCompilerRecipe":{
                            "adapter":"gradle-7.4.2-current-jdk17-javac-api-v1",
                            "compiler_type":"org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler",
                            "java_runtime_version":format!("{feature}.0.1"),"language_version":feature.to_string(),
                            "gradle_java_version":"17.0.7","gradle_file_encoding":"UTF-8",
                            "effective_charset":"UTF-8","locale":"en-US","api_options":[]}}
                })).unwrap();
                modules.push(classpath::ModuleClasspath {
                    module: format!(":sdk{feature}"),
                    project_dir: root.join(format!("module{feature}")),
                    source_sets: vec![classpath::SourceSetClasspath {
                        name: "main".into(),
                        source_roots: vec![root.join(format!("module{feature}/src"))],
                        compiler_tasks: vec![task],
                        ..Default::default()
                    }],
                    build: String::new(),
                    entries: Vec::new(),
                    unresolved: Vec::new(),
                });
            }
            let model = Arc::new(classpath::ProjectSnapshot::new(modules));
            assert_eq!(model.selected_jdk_sources().len(), 2);
            let ws = Arc::new(Mutex::new(Workspace::new()));
            ws.lock().set_project_model(model.clone());
            let generation = ws.lock().analysis_generation();
            let stats = index_dependency_model(
                &ws,
                &root,
                Some(model.clone()),
                true,
                generation,
                None,
                true,
            );
            assert_eq!(stats.jdk_complete, !failed_21);
            assert_eq!(stats.failed, usize::from(failed_21));
            assert_eq!(stats.jdk_files, if failed_21 { 1 } else { 2 });
            assert!(ws.lock().dependency_origin_complete(&origins[0]));
            assert_eq!(
                ws.lock().dependency_origin_complete(&origins[1]),
                !failed_21
            );
            let indexed_origins = ws
                .lock()
                .index
                .file_origins()
                .map(|(_, origin)| origin.origin_id.clone())
                .collect::<std::collections::BTreeSet<_>>();
            assert!(indexed_origins.contains(&origins[0]));
            assert_eq!(indexed_origins.contains(&origins[1]), !failed_21);
            let warm =
                index_dependency_model(&ws, &root, Some(model), true, generation, None, true);
            assert_eq!(warm.jdk_complete, !failed_21);
            assert_eq!(warm.jdk_reused, !failed_21);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    fn source_query_overlap_fixture(
        label: &str,
    ) -> (
        PathBuf,
        Arc<Mutex<Workspace>>,
        Arc<classpath::ProjectSnapshot>,
        Arc<classpath::ProjectSnapshot>,
    ) {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp-source-query-overlap-{label}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("api.jar");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
        archive
            .start_file(
                "fieldfixture/FieldOnly.class",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(include_bytes!(
                "../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class"
            ))
            .unwrap();
        archive.finish().unwrap();
        let dump = format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\n",
            root.display(), root.display(), binary.display(), binary.display()
        );
        let accepted = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(
            &format!("{dump}END\t:app\n"),
        )));
        let enriched = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(
            &format!("{dump}SOURCEQUERY\tmain\tcomplete\nPOMQUERY\tmain\tcomplete\nEND\t:app\n"),
        )));
        assert!(accepted.source_enrichment_needed());
        assert!(!enriched.source_enrichment_needed());
        let ws = Arc::new(Mutex::new(Workspace::new()));
        ws.lock().set_project_model(accepted.clone());
        (root, ws, accepted, enriched)
    }

    #[test]
    fn selected_source_query_overlaps_binary_preview_without_early_publication() {
        let (root, ws, accepted, enriched) = source_query_overlap_fixture("overlap");
        let binary_gate = ws.lock();
        let generation = binary_gate.analysis_generation();
        let (entered, pending) = std::sync::mpsc::sync_channel(1);
        let worker_ws = ws.clone();
        let worker_root = root.clone();
        let worker_accepted = accepted.clone();
        let query_result = enriched.clone();
        let worker = std::thread::spawn(move || {
            index_dependencies_with_source_query(
                &worker_ws,
                &worker_root,
                Some(worker_accepted),
                true,
                generation,
                None,
                &std::sync::atomic::AtomicBool::new(false),
                move |_, _, _| {
                    entered.send(()).unwrap();
                    Ok(query_result)
                },
            )
        });
        // The workspace lock prevents binary indexing from progressing. This is an ordering
        // barrier, not a response-time budget or repeated semantic query.
        let overlapped = pending.recv_timeout(Duration::from_secs(10)).is_ok();
        let not_detached = !worker.is_finished();
        let still_accepted = Arc::ptr_eq(&binary_gate.project_snapshot().unwrap(), &accepted);
        drop(binary_gate);
        worker.join().unwrap();
        assert!(
            overlapped,
            "source resolution must start while binary preview is blocked"
        );
        assert!(
            not_detached,
            "the indexing owner must await binary publication"
        );
        assert!(
            still_accepted,
            "source resolution alone must not publish its model"
        );
        assert!(Arc::ptr_eq(
            &ws.lock().project_snapshot().unwrap(),
            &enriched
        ));
        assert!(Arc::ptr_eq(&classpath::snapshot(&root).unwrap(), &enriched));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn jdk_cache_planning_overlaps_useful_modules_and_joins_stale_results() {
        use std::io::Write;
        use std::sync::atomic::{AtomicBool, Ordering};
        for superseded in [false, true] {
            let (root, ws, accepted, _) = source_query_overlap_fixture(if superseded {
                "jdk-plan-stale"
            } else {
                "jdk-plan-overlap"
            });
            let home = root.join("jdk17");
            std::fs::create_dir_all(home.join("lib")).unwrap();
            std::fs::write(home.join("release"), "JAVA_VERSION=\"17.0.1\"\n").unwrap();
            let src = home.join("lib/src.zip");
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&src).unwrap());
            zip.start_file(
                "java.base/java/lang/PlannerJdk.java",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"package java.lang; public class PlannerJdk { public int value; }")
                .unwrap();
            zip.finish().unwrap();
            let extract_root = crate::deps::extract_root();
            let mut kotlin = crate::parser::KotlinParser::new();
            let mut java = crate::java::JavaParser::new();
            assert_eq!(
                crate::deps::resolve_jdk_sources(&src, &extract_root, &mut kotlin, &mut java).len(),
                1
            );
            assert!(matches!(
                crate::deps::plan_jdk_index(&src, &extract_root),
                Some(crate::deps::JdkIndexPlan::Cached(_))
            ));

            let mut modules = accepted.modules.clone();
            modules[0].source_sets[0].compiler_tasks.push(serde_json::from_value(serde_json::json!({
                "task":":app:compileJava", "language":"java",
                "task_type":"org.gradle.api.tasks.compile.JavaCompile_Decorated",
                "arguments":[], "annotation_processor_path":[], "source_files":[],
                "classpath":[root.join("api.jar")], "java_home":home, "output_roots":[], "complete":true, "problem":null,
                "options":{"fork":false,"forkJavaHome":null,"forkExecutable":null,"forkJvmArgs":[],
                    "release":null,"bootstrapClasspath":null,"javaCompilerRecipe":{
                        "adapter":"gradle-7.4.2-current-jdk17-javac-api-v1",
                        "compiler_type":"org.gradle.jvm.toolchain.internal.DefaultToolchainJavaCompiler",
                        "java_runtime_version":"17.0.1","language_version":"17",
                        "gradle_java_version":"17.0.7","gradle_file_encoding":"UTF-8",
                        "effective_charset":"UTF-8","locale":"en-US","api_options":[]}}
            })).unwrap());
            let model = Arc::new(classpath::ProjectSnapshot::new(modules));
            assert_eq!(model.selected_jdk_sources(), [src.clone()]);
            let source =
                "package probe; import fieldfixture.FieldOnly; class Use { FieldOnly value; }";
            let key = crate::paths::file_key(&root.join("Use.java"));
            ws.lock().set_project_model(model.clone());
            ws.lock().open(&key, source.into());
            let generation = ws.lock().analysis_generation();
            let (entered, pending) = std::sync::mpsc::sync_channel(1);
            let (release, blocked) = std::sync::mpsc::sync_channel(1);
            let blocked = Arc::new(Mutex::new(blocked));
            let exited = Arc::new(AtomicBool::new(false));
            let planner_exited = exited.clone();
            let planner: Arc<JdkPlanner> = Arc::new(move |src, extract_root| {
                entered.send(()).unwrap();
                blocked.lock().recv().unwrap();
                let plan = crate::deps::plan_jdk_index(src, extract_root);
                planner_exited.store(true, Ordering::Release);
                plan
            });
            let worker_ws = ws.clone();
            let worker_root = root.clone();
            let worker = std::thread::spawn(move || {
                std::thread::scope(|scope| {
                    index_dependency_model_with_jdk_plan(
                        scope,
                        &worker_ws,
                        &worker_root,
                        Some(model),
                        true,
                        generation,
                        None,
                        true,
                        2,
                        planner,
                    )
                })
            });
            let began = pending.recv_timeout(Duration::from_secs(10)).is_ok();
            // Observe publication, not repeated content requests, while the planner is gated.
            let deadline = Instant::now() + Duration::from_secs(10);
            while ws.lock().index.lookup_by_name("FieldOnly").is_empty()
                && !worker.is_finished()
                && Instant::now() < deadline
            {
                std::thread::sleep(Duration::from_millis(1));
            }
            let useful = !ws
                .lock()
                .goto_definition(&key, source.rfind("FieldOnly").unwrap())
                .is_empty();
            let owned = !worker.is_finished();
            let origin = classpath::jdk_origin_id(&src);
            let unpublished = !ws.lock().dependency_origin_complete(&origin);
            if superseded {
                ws.lock().begin_analysis_generation();
            }
            release.send(()).ok();
            let stats = worker.join().unwrap();
            assert!(
                began && useful,
                "module navigation must work while JDK cache planning is blocked"
            );
            assert!(
                owned && unpublished,
                "the planner must remain owned and unpublished until released"
            );
            assert!(exited.load(Ordering::Acquire));
            assert_eq!(stats.jdk_complete, !superseded);
            assert_eq!(ws.lock().dependency_origin_complete(&origin), !superseded);
            assert_eq!(
                !ws.lock().index.lookup_by_name("PlannerJdk").is_empty(),
                !superseded
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn selected_source_query_joins_rejected_results_without_publication() {
        use std::sync::atomic::{AtomicBool, Ordering};
        for rejection in ["cancelled", "superseded", "failed"] {
            let (root, ws, accepted, enriched) = source_query_overlap_fixture(rejection);
            let generation = ws.lock().analysis_generation();
            let cancelled = Arc::new(AtomicBool::new(false));
            let exited = Arc::new(AtomicBool::new(false));
            let (entered, pending) = std::sync::mpsc::sync_channel(1);
            let (release, query_gate) = std::sync::mpsc::sync_channel(1);
            let worker_ws = ws.clone();
            let worker_root = root.clone();
            let worker_model = accepted.clone();
            let worker_cancelled = cancelled.clone();
            let query_exited = exited.clone();
            let worker = std::thread::spawn(move || {
                index_dependencies_with_source_query(
                    &worker_ws,
                    &worker_root,
                    Some(worker_model),
                    true,
                    generation,
                    None,
                    &worker_cancelled,
                    move |_, _, _| {
                        entered.send(()).unwrap();
                        query_gate.recv().unwrap();
                        query_exited.store(true, Ordering::Release);
                        if rejection == "failed" {
                            anyhow::bail!("fixture source resolution failed");
                        }
                        // Deliberately return metadata even after cancellation: acceptance,
                        // not a cooperative test closure, must reject this late result.
                        Ok(enriched)
                    },
                )
            });
            let began = pending.recv_timeout(Duration::from_secs(10)).is_ok();
            match rejection {
                "cancelled" => cancelled.store(true, Ordering::Release),
                "superseded" => {
                    ws.lock().begin_analysis_generation();
                }
                _ => {}
            }
            let joined_owner_pending = !worker.is_finished();
            release.send(()).ok();
            let stats = worker.join().unwrap();
            assert!(began, "{rejection}: query did not start");
            assert!(joined_owner_pending, "{rejection}: query escaped its owner");
            assert!(
                exited.load(Ordering::Acquire),
                "{rejection}: owner returned before query exit"
            );
            assert!(
                Arc::ptr_eq(&ws.lock().project_snapshot().unwrap(), &accepted),
                "{rejection}"
            );
            assert!(
                classpath::snapshot(&root).is_none(),
                "{rejection}: rejected model published"
            );
            if rejection == "failed" {
                assert!(
                    stats.files > 0,
                    "source failure must retain indexed binary facts"
                );
                assert!(!ws
                    .lock()
                    .index
                    .file_origins()
                    .collect::<Vec<_>>()
                    .is_empty());
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn selected_binary_completion_is_usable_before_source_query_finishes_and_shutdown_cancels_it(
    ) {
        use std::io::Write;
        use std::sync::atomic::Ordering;
        let root = std::env::temp_dir().join(format!(
            "ktlsp-pending-library-completion-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("api.jar");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
        archive
            .start_file(
                "fieldfixture/FieldOnly.class",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(include_bytes!(
                "../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class"
            ))
            .unwrap();
        archive.finish().unwrap();
        let model = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(
            &format!(
                "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\nEND\t:app\n",
                root.display(),
                root.display(),
                binary.display(),
                binary.display()
            ),
        )));
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        drop(socket);
        let backend = service.inner();
        let generation = {
            let mut ws = backend.ws.lock();
            ws.set_project_model(model.clone());
            let generation = ws.analysis_generation();
            if let Some(jdk) = crate::deps::jdk_src_zip() {
                ws.complete_dependency_origin(classpath::jdk_origin_id(&jdk));
            }
            generation
        };
        let cancelled = backend.refresh.lock().cancelled.clone();
        let (entered, pending) = std::sync::mpsc::sync_channel(1);
        let worker_ws = backend.ws.clone();
        let worker_root = root.clone();
        let worker = std::thread::spawn(move || {
            index_dependencies_with_source_query(
                &worker_ws,
                &worker_root,
                Some(model),
                true,
                generation,
                None,
                &cancelled,
                |_, _, cancelled| {
                    entered.send(()).unwrap();
                    while !cancelled.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    anyhow::bail!("source query cancelled")
                },
            )
        });
        pending.recv_timeout(Duration::from_secs(10)).unwrap();
        let binary_origin = classpath::maven_origin_id(
            &crate::coords::Coordinate::parse("fieldfixture:api:1").unwrap(),
        );
        let ready_started = Instant::now();
        while !backend
            .ws
            .lock()
            .index
            .file_origins()
            .any(|(_, origin)| origin.origin_id == binary_origin)
            && ready_started.elapsed() < Duration::from_secs(10)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let mut observed = Vec::new();
        for (name, source) in [
            (
                "Use.java",
                "package fieldfixture; class Use { void f(FieldOnly value) { value.co; } }",
            ),
            (
                "Use.kt",
                "package fieldfixture\nfun f(value: FieldOnly) { value.co }\n",
            ),
        ] {
            let key = crate::paths::file_key(&root.join(name));
            backend.ws.lock().open(key.clone(), source.to_string());
            let offset = source.find("value.co").unwrap() + "value.co".len();
            let (line, character) = LineIndex::new(source).position(source, offset);
            let position = Position::new(line, character);
            let result = backend
                .completion(CompletionParams {
                    text_document_position: TextDocumentPositionParams {
                        text_document: TextDocumentIdentifier {
                            uri: key_to_uri(&key).unwrap(),
                        },
                        position,
                    },
                    work_done_progress_params: Default::default(),
                    partial_result_params: Default::default(),
                    context: None,
                })
                .await
                .unwrap();
            let items = match result {
                Some(CompletionResponse::Array(items)) => items,
                Some(CompletionResponse::List(list)) => list.items,
                None => Vec::new(),
            };
            observed.push((name, items.iter().any(|item| item.label == "count")));
        }
        let query_was_pending = !worker.is_finished();
        let source_origin = classpath::maven_origin_id(
            &crate::coords::Coordinate::parse("fieldfixture:api:1").unwrap(),
        );
        let falsely_complete = backend.ws.lock().dependency_origin_complete(&source_origin);
        backend.shutdown().await.unwrap();
        let started = Instant::now();
        while !worker.is_finished() && started.elapsed() < Duration::from_secs(5) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            worker.is_finished(),
            "shutdown must finish the pending source worker"
        );
        worker.join().unwrap();
        assert!(
            query_was_pending,
            "completion must precede source query termination"
        );
        assert!(observed.iter().all(|(_, useful)| *useful), "{observed:?}");
        assert!(
            !falsely_complete,
            "positive binary facts do not certify source completeness"
        );
        assert!(backend.refresh.lock().stopped);
        assert!(!backend.ws.lock().is_current_generation(generation));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_gradle_source_archives_are_watched_and_changes_force_refresh() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-selected-source-watch-{}",
            std::process::id()
        ));
        let project = root.join("project");
        let repository = root.join("external repository");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&repository).unwrap();
        let source = repository.join("api [policy]-sources.jar");
        let parent = project.join("parent [encoding].pom");
        std::fs::write(&source, b"source-v1").unwrap();
        std::fs::write(&parent, b"parent-v1").unwrap();
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nSOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nPOMQUERY\tmain\tcomplete\nPOMARTIFACT\tmain\t{}\tMODULE\tvendor\tparent\t1\nEND\t:app\n",
            project.display(),
            repository.join("api.jar").display(),
            source.display(),
            parent.display()
        )));
        for artifact in [&source, &parent] {
            let key = crate::paths::file_key(artifact);
            let expected = glob_literal(&key);
            assert!(project_watchers(&project, Some(&model), false)
            .iter()
            .any(|watcher| {
                matches!(&watcher.glob_pattern, GlobPattern::String(pattern) if pattern==&expected)
            }));
            assert_eq!(project_change(&project, Some(&model), &key), None);
            std::fs::write(artifact, b"changed-source-v2").unwrap();
            assert_eq!(project_change(&project, Some(&model), &key), Some(true));
            std::fs::remove_file(artifact).unwrap();
            assert_eq!(project_change(&project, Some(&model), &key), Some(true));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    use super::*;
    use serde_json::json;

    #[test]
    fn notification_batch_stops_classifying_after_first_forced_refresh() {
        use std::cell::Cell;
        let cases: &[(&[Option<bool>], Option<bool>, usize)] = &[
            (&[], None, 0),
            (&[None, None], None, 2),
            (&[None, Some(false), None, Some(false)], Some(false), 4),
            (&[Some(false), None, Some(true), Some(false)], Some(true), 3),
            (&[Some(true), None, Some(false)], Some(true), 1),
        ];
        let observed: Vec<_> = cases
            .iter()
            .map(|(changes, _, _)| {
                let classified = Cell::new(0);
                let force = project_changes(changes.iter().copied().inspect(|_| {
                    classified.set(classified.get() + 1);
                }));
                (force, classified.get())
            })
            .collect();
        let expected: Vec<_> = cases
            .iter()
            .map(|(_, force, classified)| (*force, *classified))
            .collect();
        assert_eq!(observed, expected);
    }

    #[test]
    fn source_io_local_metadata_without_sources_stays_incomplete() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_local_metadata_controls_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (case, entry) in [
            ("jvm", "META-INF/api.kotlin_module"),
            ("native", "linkdata/package_api/0_api.knm"),
            ("nested", "META-INF/jarjar/library.jar"),
        ] {
            let binary = root.join(format!("{case}.jar"));
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
            archive
                .start_file(entry, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"unsupported definitions").unwrap();
            archive.finish().unwrap();
            for _ in 0..2 {
                let (tx, rx) = std::sync::mpsc::channel();
                spawn_local_jar_index_worker(
                    binary.clone(),
                    classpath::file_origin_id(&binary),
                    root.join("extracted"),
                    1,
                    tx,
                );
                let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(
                    !result.failed,
                    "{case}: readable input is not an IO failure"
                );
                assert!(
                    result.missing_source,
                    "{case}: empty stubs do not prove coverage"
                );
                assert!(result.batches.is_empty());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_io_unavailable_lazy_archives_do_not_become_empty_extractions() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("ktlsp_orphan_lazy_sources_{}", std::process::id()));
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        for materialize in [false, true] {
            let coord = crate::coords::Coordinate {
                group: "fixture".into(),
                artifact: "lazy".into(),
                version: materialize.to_string(),
            };
            let artifact_dir = repos.m2.join("fixture/lazy").join(&coord.version);
            std::fs::create_dir_all(&artifact_dir).unwrap();
            let binary = artifact_dir.join(format!("lazy-{}.jar", coord.version));
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
            archive
                .start_file("api/Useful.class", zip::write::SimpleFileOptions::default())
                .unwrap();
            archive
                .write_all(b"binary definitions remain selected")
                .unwrap();
            archive.finish().unwrap();
            let source = artifact_dir.join(format!("lazy-{}-sources.jar", coord.version));
            let write_source = || {
                let mut archive = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
                for name in ["Useful", "Other"] {
                    archive
                        .start_file(
                            format!("api/{name}.java"),
                            zip::write::SimpleFileOptions::default(),
                        )
                        .unwrap();
                    archive
                        .write_all(format!("package api; public class {name} {{}}").as_bytes())
                        .unwrap();
                }
                archive.finish().unwrap();
            };
            let resolve = || {
                let (tx, rx) = std::sync::mpsc::channel();
                spawn_coordinate_index_worker(
                    coord.clone(),
                    repos.clone(),
                    root.join("extracted"),
                    Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                    true,
                    vec![(binary.clone(), classpath::file_origin_id(&binary))],
                    None,
                    false,
                    1,
                    generation_one_workspace(),
                    tx,
                );
                rx.recv_timeout(Duration::from_secs(5)).unwrap()
            };
            write_source();
            let initial = resolve();
            assert!(!initial.failed && !initial.missing_source);
            assert_eq!(initial.batches.len(), 2);
            if materialize {
                assert!(crate::deps::materialize_source_file(Path::new(
                    &initial.batches[0].file
                )));
            }
            std::fs::remove_file(&source).unwrap();
            for _ in 0..2 {
                let missing = resolve();
                assert!(!missing.failed);
                assert!(
                    missing.missing_source,
                    "a marker or materialized subset cannot replace the missing archive"
                );
                assert!(missing.batches.is_empty());
            }
            write_source();
            let restored = resolve();
            assert!(!restored.failed && !restored.missing_source);
            assert_eq!(restored.batches.len(), 2);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_io_empty_coverage_uses_all_snapshot_selected_binaries() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_selected_empty_coverage_{}",
            std::process::id()
        ));
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let coord = crate::coords::Coordinate::parse("fixture:api:1").unwrap();
        let artifact_dir = repos.m2.join("fixture/api/1");
        std::fs::create_dir_all(&artifact_dir).unwrap();
        let default_binary = artifact_dir.join("api-1.jar");
        let selected_binary = artifact_dir.join("api-1-all.jar");
        let another_selected = artifact_dir.join("api-1-test.jar");
        let empty_jar = |path: &Path| {
            zip::ZipWriter::new(std::fs::File::create(path).unwrap())
                .finish()
                .unwrap();
        };
        empty_jar(&default_binary);
        empty_jar(&another_selected);
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&selected_binary).unwrap());
        archive
            .start_file("api/Real.class", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(b"actual selected binary has definitions")
            .unwrap();
        archive.finish().unwrap();
        let snapshot = || {
            let entries = vec![selected_binary.clone(), another_selected.clone()];
            classpath::ProjectSnapshot::new(vec![classpath::ModuleClasspath {
                module: ":".into(),
                project_dir: root.clone(),
                build: "fixture".into(),
                source_sets: vec![classpath::SourceSetClasspath {
                    name: "main".into(),
                    entries: entries.clone(),
                    artifacts: entries
                        .iter()
                        .map(|path| classpath::ResolvedArtifact {
                            path: path.clone(),
                            component: classpath::ComponentId::Module {
                                group: coord.group.clone(),
                                name: coord.artifact.clone(),
                                version: coord.version.clone(),
                            },
                        })
                        .collect(),
                    ..Default::default()
                }],
                entries,
                unresolved: Vec::new(),
            }])
        };
        let binaries = snapshot().coordinate_binaries(&coord).unwrap();
        assert_eq!(binaries.len(), 2);
        assert!(verified_empty_binaries(&[]).is_ok_and(|empty| !empty));
        let resolve = |binaries| {
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_coordinate_index_worker(
                coord.clone(),
                repos.clone(),
                root.join("extracted"),
                Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                true,
                binaries,
                None,
                false,
                1,
                generation_one_workspace(),
                tx,
            );
            rx.recv_timeout(Duration::from_secs(5)).unwrap()
        };
        for source_present in [false, true] {
            if source_present {
                empty_jar(&artifact_dir.join("api-1-sources.jar"));
            }
            let result = resolve(binaries.clone());
            assert!(!result.failed);
            assert!(
                result.missing_source,
                "an unrelated empty default JAR or empty source JAR cannot prove coverage"
            );
            assert!(result.batches.is_empty());
        }
        empty_jar(&selected_binary);
        let stale = resolve(binaries);
        assert!(stale.failed && stale.missing_source);
        let fresh = resolve(snapshot().coordinate_binaries(&coord).unwrap());
        assert!(!fresh.failed && !fresh.missing_source);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn empty_source_archives_are_successful_index_inputs() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_empty_source_archives_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let coord = crate::coords::Coordinate {
            group: "fixture".into(),
            artifact: "empty".into(),
            version: "1".into(),
        };
        let artifact_dir = root.join("m2/fixture/empty/1");
        std::fs::create_dir_all(&artifact_dir).unwrap();
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let binary = root.join("empty.jar");
        zip::ZipWriter::new(std::fs::File::create(&binary).unwrap())
            .finish()
            .unwrap();
        std::fs::copy(&binary, artifact_dir.join("empty-1.jar")).unwrap();
        for variant in 0..3 {
            let source = root.join("empty-sources.jar");
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&source).unwrap());
            if variant > 0 {
                let (name, body) = if variant == 1 {
                    ("module-info.java", "module fixture.empty {}")
                } else {
                    ("Empty.kt", "// compatibility artifact: no declarations")
                };
                archive
                    .start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(body.as_bytes()).unwrap();
            }
            archive.finish().unwrap();
            std::fs::copy(&source, artifact_dir.join("empty-1-sources.jar")).unwrap();
            for _ in 0..2 {
                let (tx, rx) = std::sync::mpsc::channel();
                spawn_local_jar_index_worker(
                    binary.clone(),
                    classpath::file_origin_id(&binary),
                    root.join("local-extracted"),
                    1,
                    tx,
                );
                let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(
                    !result.failed,
                    "a readable empty archive is not an IO failure"
                );
                assert!(result.batches.is_empty());
                assert!(!result.missing_source);
                assert_eq!(
                    result.local_origin,
                    Some(classpath::file_origin_id(&binary))
                );
                let (tx, rx) = std::sync::mpsc::channel();
                spawn_coordinate_index_worker(
                    coord.clone(),
                    repos.clone(),
                    root.join("maven-extracted"),
                    Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                    true,
                    vec![(
                        artifact_dir.join("empty-1.jar"),
                        classpath::file_origin_id(&artifact_dir.join("empty-1.jar")),
                    )],
                    None,
                    false,
                    1,
                    generation_one_workspace(),
                    tx,
                );
                let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(
                    !result.failed,
                    "an empty coordinate must succeed cold and cached"
                );
                assert!(!result.skipped);
                assert!(!result.missing_source);
                assert!(result.batches.is_empty());
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn requires_only_selected_binaries_succeed_without_ordinary_sources_cold_and_warm() {
        use std::io::Write;
        let fixtures: &[(&str, &[u8])] = &[
            (
                "kotlin-stdlib-jdk7-1.8.21",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk7-1.8.21.jar"),
            ),
            (
                "kotlin-stdlib-jdk8-1.8.21",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk8-1.8.21.jar"),
            ),
            (
                "kotlin-stdlib-jdk7-2.4.0",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk7-2.4.0.jar"),
            ),
            (
                "kotlin-stdlib-jdk8-2.4.0",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk8-2.4.0.jar"),
            ),
            (
                "kotlinx-coroutines-jdk8-1.11.0",
                include_bytes!("../tests/fixtures/module-only/kotlinx-coroutines-jdk8-1.11.0.jar"),
            ),
        ];
        let root = std::env::temp_dir().join(format!(
            "ktlsp_requires_only_workers_{}",
            std::process::id()
        ));
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        std::fs::create_dir_all(&root).unwrap();
        for (name, bytes) in fixtures {
            let coord = crate::coords::Coordinate {
                group: "fixture".into(),
                artifact: "empty".into(),
                version: (*name).into(),
            };
            let artifact_dir = repos.m2.join("fixture/empty").join(name);
            std::fs::create_dir_all(&artifact_dir).unwrap();
            let binary = root.join(format!("{name}.jar"));
            std::fs::write(&binary, bytes).unwrap();
            for with_sources in [false, true] {
                if with_sources {
                    let sources = root.join(format!("{name}-sources.jar"));
                    let mut archive = zip::ZipWriter::new(std::fs::File::create(&sources).unwrap());
                    archive
                        .start_file("module-info.java", zip::write::SimpleFileOptions::default())
                        .unwrap();
                    archive.write_all(b"module fixture.empty {}").unwrap();
                    archive.finish().unwrap();
                    std::fs::copy(
                        &sources,
                        artifact_dir.join(format!("empty-{name}-sources.jar")),
                    )
                    .unwrap();
                }
                for _ in 0..2 {
                    let (tx, rx) = std::sync::mpsc::channel();
                    spawn_coordinate_index_worker(
                        coord.clone(),
                        repos.clone(),
                        root.join("maven-extracted"),
                        Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                        true,
                        vec![(binary.clone(), classpath::file_origin_id(&binary))],
                        None,
                        false,
                        1,
                        generation_one_workspace(),
                        tx,
                    );
                    let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(
                        !result.failed
                            && !result.skipped
                            && !result.missing_source
                            && result.batches.is_empty(),
                        "{name}, sources={with_sources}"
                    );
                    let (tx, rx) = std::sync::mpsc::channel();
                    spawn_local_jar_index_worker(
                        binary.clone(),
                        classpath::file_origin_id(&binary),
                        root.join("local-extracted"),
                        1,
                        tx,
                    );
                    let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
                    assert!(
                        !result.failed
                            && !result.skipped
                            && !result.missing_source
                            && result.batches.is_empty(),
                        "{name}, sources={with_sources}"
                    );
                    assert_eq!(
                        result.local_origin,
                        Some(classpath::file_origin_id(&binary))
                    );
                }
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_module_binary_methods_are_positive_only_and_sources_keep_precedence() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_module_method_fallback_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let coord = crate::coords::Coordinate::parse("arrayfixture:binary-api:1").unwrap();
        let selected = root.join("selected-api.jar");
        let write = |path: &Path, entries: &[(&str, &[u8])]| {
            let mut jar = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            for (name, bytes) in entries {
                jar.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                jar.write_all(bytes).unwrap();
            }
            jar.finish().unwrap();
        };
        write(
            &selected,
            &[
                (
                    "arrayfixture/Factory.class",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class"
                    ),
                ),
                (
                    "arrayfixture/Item.class",
                    include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class"),
                ),
                (
                    "shadowfixture/Marker.class",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/Marker.class"
                    ),
                ),
                ("broken/Only.class", b"not a classfile"),
            ],
        );
        let artifact_dir = repos.m2.join("arrayfixture/binary-api/1");
        std::fs::create_dir_all(&artifact_dir).unwrap();
        let source = artifact_dir.join("binary-api-1-sources.jar");
        // A guessed default binary is deliberately not the selected classpath input.
        write(
            &artifact_dir.join("binary-api-1.jar"),
            &[(
                "pluginfixture/Person.class",
                include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class"),
            )],
        );
        let origin = classpath::file_origin_id(&selected);
        let resolve = |authoritative, binaries: Vec<(std::path::PathBuf, String)>| {
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_coordinate_index_worker(
                coord.clone(),
                repos.clone(),
                root.join("extracted"),
                Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                authoritative,
                binaries,
                None,
                false,
                1,
                generation_one_workspace(),
                tx,
            );
            rx.recv_timeout(Duration::from_secs(5)).unwrap()
        };
        let mut previous = None;
        for _ in 0..2 {
            let result = resolve(true, vec![(selected.clone(), origin.clone())]);
            assert!(!result.failed && !result.skipped && result.missing_source);
            assert_eq!(result.batches.len(), 3);
            assert!(result.batches.iter().all(|batch| {
                let origin = batch.origin.as_ref().unwrap();
                origin.origin_id == classpath::maven_origin_id(&coord)
                    && origin.representation == crate::project_model::SourceRepresentation::Stub
            }));
            assert!(result
                .batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| symbol.name == "rows"));
            let marker = result
                .batches
                .iter()
                .find(|batch| batch.symbols.iter().any(|symbol| symbol.name == "Marker"))
                .unwrap();
            assert!(marker.symbols.iter().any(|symbol| symbol.kind
                == ktcore::symbol::SymbolKind::Interface
                && symbol
                    .unavailable_members
                    .as_deref()
                    .is_some_and(|members| members.is_empty())));
            assert!(!marker.symbols.iter().any(|symbol| matches!(
                symbol.kind,
                ktcore::symbol::SymbolKind::Function | ktcore::symbol::SymbolKind::Property
            )));
            assert!(!result
                .batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| symbol.name == "getName" || symbol.name == "Only"));
            let bytes = bincode::serialize(&result.batches).unwrap();
            if let Some(previous) = &previous {
                assert_eq!(&bytes, previous);
            }
            previous = Some(bytes);
            let mut workspace = Workspace::new();
            for batch in result.batches {
                assert!(workspace.index_dependency_batch(batch));
            }
            workspace.bump_index_revision();
            for (key, source) in [
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.ro; } }",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f(factory: Factory) { factory.ro }\n",
                ),
            ] {
                workspace.open(key, source.to_string());
                let offset = source.find("factory.ro").unwrap() + "factory.ro".len();
                let query = workspace.completion_query(key, offset).unwrap();
                assert!(
                    query.candidates.iter().any(|item| item.label == "rows"),
                    "{key}: {} {:?}",
                    query.status_label(),
                    query.reason_labels()
                );
            }
        }
        let heuristic = resolve(false, vec![(selected.clone(), origin.clone())]);
        assert!(heuristic.missing_source && heuristic.batches.is_empty() && !heuristic.failed);
        let ambiguous = resolve(
            true,
            vec![
                (selected.clone(), origin.clone()),
                (
                    artifact_dir.join("binary-api-1.jar"),
                    classpath::file_origin_id(&artifact_dir.join("binary-api-1.jar")),
                ),
            ],
        );
        assert!(ambiguous.missing_source && ambiguous.batches.is_empty() && !ambiguous.failed);
        write(&source, &[("arrayfixture/Factory.java",
            b"package arrayfixture; public class Factory { public int sourceOnly() { return 1; } }")]);
        let preferred = resolve(true, vec![(selected.clone(), origin.clone())]);
        assert!(!preferred.failed && !preferred.missing_source);
        assert!(preferred
            .batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "sourceOnly"));
        assert!(!preferred
            .batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "rows"));
        assert!(preferred.batches.iter().all(|batch| batch
            .origin
            .as_ref()
            .unwrap()
            .representation
            == crate::project_model::SourceRepresentation::Source));
        for invalid_utf8 in [false, true] {
            if invalid_utf8 {
                write(
                    &source,
                    &[(
                        "arrayfixture/Factory.java",
                        b"package arrayfixture; // invalid UTF-8 \xff
public class Factory { public int unsafeSourceOnly() { return 1; } }",
                    )],
                );
            } else {
                std::fs::write(&source, b"unreadable source archive").unwrap();
            }
            for _ in 0..2 {
                let invalid_source = resolve(true, vec![(selected.clone(), origin.clone())]);
                assert!(
                    invalid_source.failed
                        && invalid_source.missing_source
                        && !invalid_source.skipped
                );
                assert_eq!(invalid_source.batches.len(), 3);
                assert!(invalid_source.source_error_binary_fallback());
                let mut opaque = resolve(true, vec![(selected.clone(), origin.clone())]);
                let marker = opaque
                    .batches
                    .iter_mut()
                    .find(|batch| batch.symbols.iter().any(|symbol| symbol.name == "Marker"))
                    .unwrap();
                for symbol in &mut marker.symbols {
                    symbol.unavailable_members = None;
                }
                assert!(!opaque.source_error_binary_fallback(), "a header without decoded type facts cannot pass alongside useful member batches");
                assert!(invalid_source.batches.iter().all(|batch| batch
                    .origin
                    .as_ref()
                    .unwrap()
                    .representation
                    == crate::project_model::SourceRepresentation::Stub));
                assert!(
                    !invalid_source
                        .batches
                        .iter()
                        .flat_map(|batch| &batch.symbols)
                        .any(|symbol| symbol.name == "sourceOnly"
                            || symbol.name == "unsafeSourceOnly")
                );
                let mut workspace = Workspace::new();
                assert!(!workspace.dependency_origin_complete(&classpath::maven_origin_id(&coord)));
                for batch in invalid_source.batches {
                    assert!(workspace.index_dependency_batch(batch));
                }
                workspace.bump_index_revision();
                for (key, text) in [
                    (
                        "CorruptSource.java",
                        "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.ro; } }",
                    ),
                    (
                        "CorruptSource.kt",
                        "import arrayfixture.Factory\nfun f(factory: Factory) { factory.ro }\n",
                    ),
                ] {
                    workspace.open(key, text.to_string());
                    let query = workspace
                        .completion_query(
                            key,
                            text.find("factory.ro").unwrap() + "factory.ro".len(),
                        )
                        .unwrap();
                    assert!(
                        query.candidates.iter().any(|item| item.label == "rows"),
                        "{key}: {} {:?}",
                        query.status_label(),
                        query.reason_labels()
                    );
                }
                assert!(
                    !workspace.dependency_origin_complete(&classpath::maven_origin_id(&coord)),
                    "binary positives do not grant source completeness"
                );
                let mut partial_source = resolve(true, vec![(selected.clone(), origin.clone())]);
                partial_source.batches[0]
                    .origin
                    .as_mut()
                    .unwrap()
                    .representation = crate::project_model::SourceRepresentation::Source;
                assert!(
                    !partial_source.source_error_binary_fallback(),
                    "failed partial source cannot pass the installer gate"
                );
                partial_source.batches[0]
                    .origin
                    .as_mut()
                    .unwrap()
                    .representation = crate::project_model::SourceRepresentation::Stub;
                partial_source.batches[0].origin.as_mut().unwrap().origin_id = "foreign".into();
                assert!(
                    !partial_source.source_error_binary_fallback(),
                    "foreign stub cannot pass the installer gate"
                );
            }
            let heuristic = resolve(false, vec![(selected.clone(), origin.clone())]);
            assert!(heuristic.failed && heuristic.missing_source && heuristic.batches.is_empty());
            let stale = resolve(
                true,
                vec![(selected.clone(), "old selected binary identity".into())],
            );
            assert!(stale.failed && stale.missing_source && stale.batches.is_empty());
            let ambiguous = resolve(
                true,
                vec![
                    (selected.clone(), origin.clone()),
                    (selected.clone(), origin.clone()),
                ],
            );
            assert!(ambiguous.failed && ambiguous.missing_source && ambiguous.batches.is_empty());
        }
        std::fs::remove_file(&source).unwrap();
        write(&selected, &[]);
        let changed = resolve(true, vec![(selected.clone(), origin)]);
        assert!(changed.failed && changed.missing_source && changed.batches.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_sources_require_a_verified_empty_authoritative_binary() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_empty_binary_controls_{}",
            std::process::id()
        ));
        let repos = crate::artifacts::Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        for case in [
            "absent",
            "class",
            "module-class",
            "kotlin-metadata",
            "native-metadata",
            "nested-archive",
            "corrupt",
            "fallback",
        ] {
            let coord = crate::coords::Coordinate {
                group: "fixture".into(),
                artifact: "empty".into(),
                version: case.into(),
            };
            let artifact_dir = repos.m2.join("fixture/empty").join(case);
            std::fs::create_dir_all(&artifact_dir).unwrap();
            let binary = artifact_dir.join(format!("empty-{case}.jar"));
            if case == "corrupt" {
                std::fs::write(&binary, b"unfinished build output").unwrap();
            } else if case != "absent" {
                let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
                let entry = match case {
                    "class" => Some("api/Only.class"),
                    "module-class" => Some("module-info.class"),
                    "kotlin-metadata" => Some("META-INF/api.kotlin_module"),
                    "native-metadata" => Some("linkdata/package_api/0_api.knm"),
                    "nested-archive" => Some("META-INF/jarjar/library.jar"),
                    _ => None,
                };
                if let Some(entry) = entry {
                    archive
                        .start_file(entry, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    archive
                        .write_all(b"input is conservatively incomplete")
                        .unwrap();
                }
                archive.finish().unwrap();
            }
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_coordinate_index_worker(
                coord,
                repos.clone(),
                root.join("extracted").join(case),
                Arc::new(Mutex::new(std::collections::BTreeSet::new())),
                case != "fallback",
                if case == "absent" {
                    Vec::new()
                } else {
                    vec![(binary.clone(), classpath::file_origin_id(&binary))]
                },
                None,
                false,
                1,
                generation_one_workspace(),
                tx,
            );
            let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(
                result.missing_source,
                "{case} must not prove complete source coverage"
            );
            assert_eq!(result.failed, case == "corrupt", "{case}");
            assert!(!result.skipped);
            assert!(result.batches.is_empty());
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_jdk_shard_reports_failure_without_partial_batches() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("ktlsp_bad_jdk_sources_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let jar = root.join("src.zip");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
        for (name, body) in [
            (
                "java/lang/Good.java",
                b"package java.lang; public class Good {}".as_slice(),
            ),
            ("java/lang/Bad.java", b"\xff".as_slice()),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(body).unwrap();
        }
        archive.finish().unwrap();
        let Some(crate::deps::JdkIndexPlan::Parse(plan)) =
            crate::deps::plan_jdk_index(&jar, &root.join("extracted"))
        else {
            panic!("new JDK archive must be parsed");
        };
        let (tx, rx) = std::sync::mpsc::channel();
        assert_eq!(
            spawn_jdk_shard_workers(plan, classpath::jdk_origin_id(&jar), 1, 1, tx),
            1
        );
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(result.failed);
        assert!(result.batches.is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_local_sources_do_not_report_complete_library_indexing() {
        use std::io::Write;
        let root =
            std::env::temp_dir().join(format!("ktlsp_bad_local_sources_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let jar = root.join("api.jar");
        let sources = root.join("api-sources.jar");
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&jar).unwrap());
        archive
            .start_file("api/Only.class", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"").unwrap();
        archive.finish().unwrap();
        for variant in 0..3 {
            let present = variant < 2;
            if variant == 0 {
                std::fs::write(&sources, b"incomplete build output").unwrap();
            } else if variant == 1 {
                let mut archive = zip::ZipWriter::new(std::fs::File::create(&sources).unwrap());
                for (name, body) in [
                    (
                        "api/Only.java",
                        b"package api; public class Only {}".as_slice(),
                    ),
                    ("api/Unreadable.java", b"\xff".as_slice()),
                ] {
                    archive
                        .start_file(name, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    archive.write_all(body).unwrap();
                }
                archive.finish().unwrap();
            } else {
                std::fs::remove_file(&sources).unwrap();
            }
            let (tx, rx) = std::sync::mpsc::channel();
            spawn_local_jar_index_worker(
                jar.clone(),
                classpath::file_origin_id(&jar),
                root.join("extracted"),
                1,
                tx,
            );
            let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(
                result.failed, present,
                "unreadable sources must keep coverage incomplete"
            );
            assert_eq!(
                result.batches.is_empty(),
                present,
                "corrupt sources must not silently become shallow stubs"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn native_file_keys_roundtrip_through_editor_uris() {
        let expected = "C:/project/space name/Main.kt";
        for path in [
            r"c:\project\space name\Main.kt",
            r"\\?\C:\project\space name\Main.kt",
        ] {
            let key = crate::paths::file_key(Path::new(path));
            let uri = key_to_uri(&key).unwrap();
            assert!(uri.as_str().starts_with("file://"));
            assert_eq!(uri_to_key(&uri).as_deref(), Some(expected));
        }
        let uri = "file:///c%3A/project/space%20name/Main.kt"
            .parse::<Uri>()
            .unwrap();
        assert_eq!(uri_to_key(&uri).as_deref(), Some(expected));
    }

    #[test]
    fn selected_external_closed_sources_have_literal_watches_and_source_refresh_events() {
        let temp = std::env::temp_dir().join(format!(
            "ktlsp_watch_selected_external_{}",
            std::process::id()
        ));
        let root = temp.join("workspace");
        let external = temp.join("external[generated]");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(&external).unwrap();
        let generated = external.join("Adapter[1].kt");
        let future = external.join("future[more]/NotYet.java");
        std::fs::write(&generated, "class Generated").unwrap();
        let neighbor = external.join("Unselected.kt");
        let mut task = crate::compile::kotlin_test_request(&root).task;
        task.source_files
            .extend([generated.clone(), future.clone()]);
        let mut auxiliary = task.clone();
        auxiliary.task = ":unselectedAuxiliary".into();
        auxiliary.source_files = vec![neighbor.clone()];
        let model = classpath::ProjectSnapshot::new(vec![classpath::ModuleClasspath {
            module: ":".into(),
            project_dir: root.clone(),
            build: ":".into(),
            source_sets: vec![classpath::SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![root.join("src")],
                kotlin_compile_task: Some(task.task.clone()),
                compiler_tasks: vec![task, auxiliary],
                ..Default::default()
            }],
            entries: Vec::new(),
            unresolved: Vec::new(),
        }]);
        assert!(model.file_origin(&generated).is_none());
        for file in [&generated, &future] {
            assert_eq!(
                project_change(&root, Some(&model), &crate::paths::file_key(file)),
                Some(false)
            );
        }
        assert_eq!(
            project_change(&root, Some(&model), &crate::paths::file_key(&neighbor)),
            None
        );
        let mut replacement_modules = model.modules.clone();
        replacement_modules[0].source_sets[0].kotlin_compile_task = None;
        let replacement = classpath::ProjectSnapshot::new(replacement_modules);
        assert_eq!(
            project_change(
                &root,
                Some(&replacement),
                &crate::paths::file_key(&generated)
            ),
            None
        );
        assert_eq!(
            project_change(&root, Some(&model), &crate::paths::file_key(&generated)),
            Some(false),
            "an older snapshot retains its exact selected inputs"
        );
        for relative in [false, true] {
            let watches = project_watchers(&root, Some(&model), relative);
            assert_eq!(
                watches.len(),
                3,
                "one workspace tree and two exact external files"
            );
            for expected in [r"Adapter\[1\].kt", r"future\[more\]/NotYet.java"] {
                assert!(
                    watches.iter().any(|watch| {
                        assert!(watch.kind.is_none());
                        match &watch.glob_pattern {
                            GlobPattern::Relative(pattern) => {
                                assert!(relative);
                                pattern.pattern == expected
                                    && matches!(&pattern.base_uri, OneOf::Right(uri)
                                    if uri_to_key(uri).as_deref()
                                        == Some(crate::paths::file_key(&external).as_str()))
                            }
                            GlobPattern::String(pattern) => {
                                assert!(!relative);
                                pattern
                                    == &format!(
                                        "{}/{}",
                                        glob_literal(&crate::paths::file_key(&external)),
                                        expected
                                    )
                            }
                        }
                    }),
                    "{expected}"
                );
            }
        }
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn workspace_watch_registration_is_bounded_for_large_source_models() {
        let temp =
            std::env::temp_dir().join(format!("ktlsp_watch_large_model_{}", std::process::id()));
        std::fs::create_dir_all(&temp).unwrap();
        let root = crate::paths::file_key(&temp);
        let mut dump = format!("PROJECT\t:app\t{root}\n");
        // Cardinalities from the real Gradle corpus registration that exhausted Neovim memory.
        for n in 0..4869 {
            dump.push_str(&format!("SOURCE\tmain\t{root}/sources/module-{n}/src\n"));
        }
        for n in 0..3925 {
            dump.push_str(&format!(
                "OUTPUT\tmain\t{root}/outputs/module-{n}/classes\n"
            ));
        }
        dump.push_str("END\t:app\n");
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&dump));
        assert_eq!(model.modules[0].source_sets[0].source_roots.len(), 4869);
        assert_eq!(model.modules[0].source_sets[0].output_roots.len(), 3925);
        for (path, expected) in [
            ("sources/module-0/src/Example.java", Some(false)),
            ("sources/module-4868/src/Generated.kt", Some(false)),
            ("outputs/module-0/classes/New.class", Some(true)),
            (
                "outputs/module-3924/classes/META-INF/plugin.inventory",
                Some(true),
            ),
            ("build.gradle.kts", Some(true)),
            ("nested/libs/version.jar", Some(true)),
            ("nested/libs/version-sources.jar", Some(true)),
            ("gradle/wrapper/gradle-wrapper.properties", Some(true)),
            ("gradle/libs.versions.toml", Some(true)),
            (".gradle", None),
            ("buildSrc/.gradle", None),
            (".gradle/buildOutputCleanup/cache.properties", None),
            ("gradle/custom.gradle", Some(true)),
            ("sources/module-0/src-other/Decoy.java", None),
            ("outputs/module-0/classes-other/Decoy.class", None),
            ("src/test/resources/Fake.kt", None),
            ("docs/readme.txt", None),
        ] {
            assert_eq!(
                project_change(&temp, Some(&model), &format!("{root}/{path}")),
                expected,
                "{path}"
            );
        }
        for relative in [false, true] {
            let watches = project_watchers(&temp, Some(&model), relative);
            assert_eq!(watches.len(), 1, "relative={relative}");
            assert!(
                watches[0].kind.is_none(),
                "create/change/delete all remain watched"
            );
            match &watches[0].glob_pattern {
                GlobPattern::String(pattern) => {
                    assert!(!relative);
                    assert_eq!(pattern, &format!("{}/**/*", glob_literal(&root)));
                }
                GlobPattern::Relative(pattern) => {
                    assert!(relative);
                    assert_eq!(pattern.pattern, "**/*");
                    assert!(matches!(
                        &pattern.base_uri,
                        OneOf::Right(uri) if uri_to_key(uri).as_deref() == Some(root.as_str())
                    ));
                }
            }
            assert!(serde_json::to_vec(&watches).unwrap().len() < 1024);
        }
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn workspace_watch_without_internal_sources_keeps_its_boundary() {
        let temp =
            std::env::temp_dir().join(format!("ktlsp_watch_model_boundary_{}", std::process::id()));
        let root = temp.join("workspace");
        let sibling = temp.join("workspace-sibling");
        for path in [&root, &sibling.join("sources"), &sibling.join("classes")] {
            std::fs::create_dir_all(path).unwrap();
        }
        let root_key = crate::paths::file_key(&root);
        let sibling_key = crate::paths::file_key(&sibling);
        let empty = classpath::ProjectSnapshot::new(Vec::new());
        for model in [None, Some(&empty)] {
            let watches = project_watchers(&root, model, true);
            assert_eq!(watches.len(), 2);
            assert!(watches.iter().all(|watch| matches!(
                &watch.glob_pattern,
                GlobPattern::Relative(pattern) if pattern.pattern != "**/*"
            )));
        }
        let internal = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{root_key}\nSOURCE\tmain\t{root_key}/future-source\nSCP\tmain\t{root_key}/libs/vendor.jar\nEND\t:app\n"
        )));
        assert_eq!(internal.local_jars().len(), 1);
        assert_eq!(project_watchers(&root, Some(&internal), true).len(), 1);
        let external = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{root_key}\nSOURCE\tmain\t{sibling_key}/sources\nOUTPUT\tmain\t{sibling_key}/classes\nSCP\tmain\t{sibling_key}/vendor.jar\nEND\t:app\n"
        )));
        let watches = project_watchers(&root, Some(&external), true);
        assert_eq!(watches.len(), 6);
        for (base, pattern) in [
            (format!("{sibling_key}/sources"), "**/*.{kt,kts,java}"),
            (format!("{sibling_key}/classes"), "**/*"),
            (sibling_key.clone(), "vendor.jar"),
            (sibling_key.clone(), "vendor-sources.jar"),
        ] {
            assert!(
                watches.iter().any(|watch| matches!(
                    &watch.glob_pattern,
                    GlobPattern::Relative(watch) if watch.pattern == pattern && matches!(
                        &watch.base_uri,
                        OneOf::Right(uri) if uri_to_key(uri).as_deref() == Some(base.as_str())
                    )
                )),
                "{base}/{pattern}"
            );
        }
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn normalized_output_lookup_preserves_windows_trailing_root_boundaries() {
        for (raw_root, raw_file, expected) in [
            (r"C:\work\classes", r"c:\work\classes", true),
            (r"C:\work\classes", r"C:\work\classes\Entry.class", true),
            (
                r"C:\work\classes",
                r"C:\work\classes-other\Entry.class",
                false,
            ),
            (r"C:\work\classes\", r"C:\work\classes", false),
            (r"C:\work\classes\", r"C:\work\classes\", true),
            (r"C:\work\classes\", r"C:\work\classes\Entry.class", true),
            ("C:/work/classes//", "C:/work/classes", false),
            ("C:/work/classes//", "C:/work/classes/", true),
            (r"\\?\C:\work\classes\", r"C:\work\classes", false),
            (
                r"\\?\C:\work\classes",
                r"\\?\C:\work\classes\Entry.class",
                true,
            ),
            (r"\\server\share\classes", r"\\server\share\classes", true),
            (r"\\server\share\classes\", r"\\server\share\classes", false),
            (r"\\server\share\classes\", r"\\server\share\classes\", true),
            (
                r"\\server\share\classes\",
                r"\\server\share\classes\Entry.class",
                true,
            ),
            (
                r"\\?\UNC\server\share\classes\",
                r"\\server\share\classes",
                false,
            ),
            (
                r"\\?\UNC\server\share\classes",
                "//server/share/classes/Entry.class",
                true,
            ),
            (
                r"\\server\share\classes",
                r"\\server\share\classes-other\Entry.class",
                false,
            ),
            (r"C:\", r"C:\", true),
            (r"C:\", "C:", false),
            (r"C:\", r"C:\Entry.class", true),
            (r"\\server\share\", r"\\server\share", false),
            (r"\\server\share\", r"\\server\share\", true),
            (r"\\server\share\", r"\\server\share\Entry.class", true),
            (r"C:\", "", false),
            (r"c:\", r"C:\Entry.class", true),
        ] {
            let root = crate::paths::file_key(Path::new(raw_root));
            let key = crate::paths::file_key(Path::new(raw_file));
            let legacy = path_is_under(&key, &root);
            assert_eq!(legacy, expected, "legacy {raw_root:?} -> {raw_file:?}");
            let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
                "PROJECT\t:app\tC:/workspace\nOUTPUT\tmain\t{raw_root}\nEND\t:app\n"
            )));
            assert_eq!(
                model.is_output_key(&key),
                legacy,
                "indexed {raw_root:?} -> {raw_file:?}"
            );
        }
    }

    #[test]
    #[cfg(windows)]
    fn output_lookup_preserves_existing_windows_uri_conversion_boundaries() {
        for (raw_root, file_uri, expected) in [
            (
                r"c:\work\classes",
                "file:///C:/work/classes/Entry.class",
                true,
            ),
            (r"\\?\C:\work\classes\", "file:///c:/work/classes", false),
            (
                r"\\?\C:\work\classes\",
                "file:///c:/work/classes/Entry.class",
                true,
            ),
            (
                r"\\?\UNC\server\share\classes\",
                "file://server/share/classes/Entry.class",
                // The vendored URI adapter does not currently produce a UNC key for an
                // authority host. Preserve that boundary here; this is not UNC URI support.
                false,
            ),
        ] {
            let uri: Uri = file_uri.parse().unwrap();
            let key = uri_to_key(&uri).unwrap();
            let root = crate::paths::file_key(Path::new(raw_root));
            let legacy = path_is_under(&key, &root);
            if file_uri == "file://server/share/classes/Entry.class" {
                eprintln!("existing authority-host URI boundary: {file_uri} -> {key:?}");
            }
            assert_eq!(legacy, expected, "{file_uri}");
            let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
                "PROJECT\t:app\tC:/workspace\nOUTPUT\tmain\t{raw_root}\nEND\t:app\n"
            )));
            assert_eq!(model.is_output_key(&key), legacy, "{file_uri}");
        }
    }

    #[test]
    fn output_root_lookup_retains_root_slashes_and_combined_provenance() {
        for (root, key, expected) in [
            ("/", "/", true),
            ("/", "/Entry.class", true),
            ("/", "", false),
            ("/", "relative", false),
            ("/classes/", "/classes", false),
            ("/classes/", "/classes/", true),
            ("/classes/", "/classes/Entry.class", true),
            ("/classes", "/classes", true),
            ("/classes//", "/classes/", true),
            ("/classes/", "/classes-other/Entry.class", false),
        ] {
            let legacy = path_is_under(key, root);
            assert_eq!(legacy, expected);
            let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
                "PROJECT\t:app\t/workspace\nOUTPUT\tmain\t{root}\nEND\t:app\n"
            )));
            assert_eq!(model.is_output_key(key), legacy, "{root:?} -> {key:?}");
        }
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(
            "PROJECT\t:app\t/workspace\nOUTPUT\tmain\t/classes/\nOUTPUT\ttest\t/classes\n\
             PROVIDES\tmain\t/classes/api.jar\nPROVIDES\tmain\t/provided.jar\nEND\t:app\n",
        ));
        assert!(
            model.is_output_key("/classes"),
            "non-trailing duplicate permits exact root"
        );
        assert!(
            model.is_output_key("/classes/api.jar/Nested.class"),
            "provided-only deepest key cannot hide a genuine output ancestor"
        );
        assert!(!model.is_output_key("/provided.jar/Nested.class"));
    }

    #[tokio::test]
    async fn selected_local_jar_same_metadata_replacement_retires_warm_semantics() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "ktlsp_same_metadata_local_jar_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("dependency.jar");
        let original =
            include_bytes!("../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class");
        let mut replacement = original.to_vec();
        for (old, new) in [(b"LABEL", b"TITLE"), (b"count", b"total")] {
            let offset = replacement
                .windows(old.len())
                .position(|part| part == old)
                .unwrap();
            replacement[offset..offset + old.len()].copy_from_slice(new);
        }
        let decoded = ktcore::classfile::decode("replacement", &replacement).unwrap();
        assert!(decoded
            .fields
            .iter()
            .any(|field| field.name.as_ref() == "TITLE"));
        assert!(decoded
            .fields
            .iter()
            .any(|field| field.name.as_ref() == "total"));
        let write = |bytes: &[u8]| {
            let mut archive = zip::ZipWriter::new(std::fs::File::create(&binary).unwrap());
            archive
                .start_file(
                    "fieldfixture/FieldOnly.class",
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            archive.write_all(bytes).unwrap();
            archive.finish().unwrap();
        };
        write(original);
        let metadata = std::fs::metadata(&binary).unwrap();
        let modified = metadata.modified().unwrap();
        let dump = format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nEND\t:app\n",
            root.display(),
            root.display(),
            binary.display()
        );
        let model = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(
            &dump,
        )));
        let origin = model.artifact_origin(&binary).unwrap().to_string();
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        drop(socket);
        let backend = service.inner();
        *backend.root.lock() = Some(root.clone());
        *backend.gradle_project.lock() = true;
        // Drive the model/index phases explicitly instead of launching Gradle in this fixture.
        backend.refresh.lock().running = true;
        backend.watches.lock().archive_contents = project_archive_contents(&root, Some(&model));
        backend.ws.lock().set_project_model(model.clone());
        let generation = backend.ws.lock().analysis_generation();
        let cold = index_dependency_model(
            &backend.ws,
            &root,
            Some(model.clone()),
            true,
            generation,
            None,
            true,
        );
        assert_eq!(cold.failed, 0);
        assert_eq!(cold.local_stub_files, 1);
        assert!(backend.ws.lock().dependency_origin_complete(&origin));
        let old_files: Vec<_> = backend
            .ws
            .lock()
            .index
            .file_origins()
            .filter(|(_, owner)| owner.origin_id == origin)
            .map(|(key, _)| key.to_string())
            .collect();
        assert_eq!(old_files.len(), 1);
        let extract = crate::deps::extract_root();
        let fingerprint = Path::new(&old_files[0])
            .strip_prefix(extract.join("local-jars"))
            .unwrap()
            .components()
            .next()
            .unwrap()
            .as_os_str()
            .to_owned();
        let old_cache = extract
            .parent()
            .unwrap()
            .join("symcache")
            .join(format!("{}.zst", fingerprint.to_string_lossy()));
        let cache_bytes =
            std::fs::read(&old_cache).expect("cold indexing must persist its real symbol cache");
        let cache_modified = std::fs::metadata(&old_cache).unwrap().modified().unwrap();
        let mut kotlin = crate::parser::KotlinParser::new();
        let mut java = crate::java::JavaParser::new();
        let warm = crate::deps::resolve_local_jar_stubs_for_origin(
            &binary,
            &origin,
            &extract,
            &mut kotlin,
            &mut java,
        )
        .unwrap();
        assert!(warm
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "count"));
        assert_eq!(
            std::fs::metadata(&old_cache).unwrap().modified().unwrap(),
            cache_modified,
            "warm resolution must load the existing disk cache"
        );
        let probes = [
            (
                "Value.java",
                "import fieldfixture.FieldOnly; class Use { void f(FieldOnly value) { value.; } }",
                "value.",
                "count",
                "total",
            ),
            (
                "Value.kt",
                "import fieldfixture.FieldOnly\nfun use(value: FieldOnly) { value. }",
                "value.",
                "count",
                "total",
            ),
            (
                "Static.java",
                "import fieldfixture.FieldOnly; class Use { void f() { FieldOnly.; } }",
                "FieldOnly.",
                "LABEL",
                "TITLE",
            ),
            (
                "Static.kt",
                "import fieldfixture.FieldOnly\nfun use() { FieldOnly. }",
                "FieldOnly.",
                "LABEL",
                "TITLE",
            ),
        ];
        for (file, source, ..) in probes {
            backend
                .ws
                .lock()
                .open(&crate::paths::file_key(&root.join(file)), source.into());
        }
        let check = |updated: bool| {
            for (file, source, needle, old, new) in probes {
                let key = crate::paths::file_key(&root.join(file));
                let query = backend
                    .ws
                    .lock()
                    .completion_query(&key, source.find(needle).unwrap() + needle.len())
                    .unwrap();
                let labels: Vec<_> = query
                    .candidates
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect();
                let (present, absent) = if updated { (new, old) } else { (old, new) };
                assert!(
                    labels.contains(&present),
                    "{file}: missing {present}: {labels:?}"
                );
                assert!(
                    !labels.contains(&absent),
                    "{file}: stale {absent}: {labels:?}"
                );
                assert_eq!(backend.ws.lock().doc_text(&key).as_deref(), Some(source));
            }
        };
        check(false);
        write(&replacement);
        std::fs::File::options()
            .write(true)
            .open(&binary)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert_eq!(std::fs::metadata(&binary).unwrap().len(), metadata.len());
        assert_eq!(
            std::fs::metadata(&binary).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(classpath::file_origin_id(&binary), origin);
        for _ in 0..2 {
            let batches = crate::deps::resolve_local_jar_stubs_for_origin(
                &binary,
                &origin,
                &extract,
                &mut kotlin,
                &mut java,
            )
            .unwrap();
            let names: Vec<_> = batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .map(|symbol| symbol.name.as_str())
                .collect();
            assert!(
                names.contains(&"TITLE") && names.contains(&"total"),
                "replaced archive reused stale disk facts: {names:?}"
            );
            assert!(
                !names.contains(&"LABEL") && !names.contains(&"count"),
                "stale disk members: {names:?}"
            );
        }
        backend
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent::new(
                    Uri::from_file_path(&binary).unwrap(),
                    FileChangeType::CHANGED,
                )],
            })
            .await;
        assert!(
            !backend.ws.lock().dependency_origin_complete(&origin),
            "watch must retire completed memory facts"
        );
        for file in &old_files {
            assert!(
                backend.ws.lock().index.entries_for_file(file).is_empty(),
                "old archive members survived: {file}"
            );
            assert!(backend.ws.lock().index.file_origin(file).is_none());
        }
        let refreshed = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(
            &dump,
        )));
        assert_eq!(refreshed.artifact_origin(&binary), Some(origin.as_str()));
        backend.ws.lock().set_project_model(refreshed.clone());
        let generation = backend.ws.lock().analysis_generation();
        let refreshed_stats = index_dependency_model(
            &backend.ws,
            &root,
            Some(refreshed.clone()),
            true,
            generation,
            None,
            true,
        );
        assert_eq!(refreshed_stats.failed, 0);
        assert_eq!(refreshed_stats.reused_local_jars, 0);
        assert_eq!(refreshed_stats.local_stub_files, 1);
        assert!(backend.ws.lock().dependency_origin_complete(&origin));
        check(true);
        let warm = index_dependency_model(
            &backend.ws,
            &root,
            Some(refreshed),
            true,
            generation,
            None,
            true,
        );
        assert_eq!(warm.reused_local_jars, 1);
        assert_eq!(
            std::fs::read(&old_cache).unwrap(),
            cache_bytes,
            "replacement must leave the old cache partition intact"
        );
        assert_eq!(
            std::fs::metadata(&old_cache).unwrap().modified().unwrap(),
            cache_modified
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn unchanged_selected_binary_jar_watch_does_not_force_model_reload() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_unchanged_binary_jar_watch_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("dependency.jar");
        let immutable = root.join("immutable.jar");
        std::fs::write(&immutable, b"selected Maven artifact").unwrap();
        let tracked = root.join("docs/tracked.jar");
        let unknown = root.join("untracked.jar");
        let wrapper = root.join("gradle/wrapper/gradle-wrapper.jar");
        std::fs::create_dir_all(tracked.parent().unwrap()).unwrap();
        std::fs::create_dir_all(wrapper.parent().unwrap()).unwrap();
        std::fs::write(&tracked, b"accepted tracked binary").unwrap();
        std::fs::write(&unknown, b"unknown existing binary").unwrap();
        std::fs::write(&wrapper, b"tracked Gradle wrapper").unwrap();
        std::fs::write(root.join("gradlew"), "wrapper fixture").unwrap();
        std::fs::write(
            root.join("settings.gradle"),
            "rootProject.name = 'watch-fixture'",
        )
        .unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec![
                "add",
                "docs/tracked.jar",
                "gradle/wrapper/gradle-wrapper.jar",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        for project in [false, true] {
            std::fs::write(&binary, b"accepted binary").unwrap();
            let key = crate::paths::file_key(&binary);
            let component = if project {
                "PROJECT\t:build-logic\t:java-api-extractor"
            } else {
                "FILE"
            };
            let model = Arc::new(classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{key}\nARTIFACT\tmain\t{key}\t{component}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\timmutable\t1\nSOURCESET\ttest\nSCP\ttest\t{key}\nARTIFACT\ttest\t{key}\t{component}\nEND\t:app\n",
            crate::paths::file_key(&root), crate::paths::file_key(&root.join("src")),
            crate::paths::file_key(&immutable), crate::paths::file_key(&immutable),
        ))));
            assert_eq!(
                model.local_jars().len(),
                usize::from(!project),
                "Project artifacts keep their own indexing route"
            );
            let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
            drop(socket);
            let backend = service.inner();
            *backend.root.lock() = Some(root.clone());
            *backend.gradle_project.lock() = true;
            backend.refresh.lock().running = true;
            backend.ws.lock().set_project_model(model.clone());
            let open_key = crate::paths::file_key(&root.join("src/Use.java"));
            backend
                .ws
                .lock()
                .open(&open_key, "class Use { int unsaved; }".into());
            let unbaselined = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&tracked).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > unbaselined,
                "tracked archive without an accepted baseline stays conservative"
            );
            let contents = project_archive_contents(&root, Some(&model));
            assert!(contents.contains_key(&key));
            for excluded in [&immutable, &unknown, &wrapper] {
                assert!(
                    !contents.contains_key(&crate::paths::file_key(excluded)),
                    "unexpected baseline: {}",
                    excluded.display()
                );
            }
            backend.watches.lock().archive_contents = contents.clone();
            let initial = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&binary).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert_eq!(
                backend.refresh.lock().requested,
                initial,
                "an unchanged selected JAR must not cancel editing or restart Gradle"
            );
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&tracked).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert_eq!(
                backend.refresh.lock().requested,
                initial,
                "unchanged Git-tracked unselected archive must not force import"
            );
            assert_eq!(
                contents.len(),
                2,
                "selected paths and tracked inventory must be deduplicated"
            );
            let before = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&unknown).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > before,
                "untracked existing JAR stays conservative"
            );
            let before = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&wrapper).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > before,
                "tracked Gradle wrapper stays configuration"
            );
            let original_tracked = std::fs::read(&tracked).unwrap();
            let tracked_modified = std::fs::metadata(&tracked).unwrap().modified().unwrap();
            std::fs::write(&tracked, b"replaced tracked binary").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&tracked)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(tracked_modified))
                .unwrap();
            assert_eq!(
                original_tracked.len(),
                std::fs::metadata(&tracked).unwrap().len() as usize
            );
            let before = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&tracked).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > before,
                "changed tracked bytes must force import"
            );
            std::fs::write(&tracked, original_tracked).unwrap();
            let modified = std::fs::metadata(&binary).unwrap().modified().unwrap();
            std::fs::write(&binary, b"replaced binary").unwrap();
            std::fs::File::options()
                .write(true)
                .open(&binary)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_modified(modified))
                .unwrap();
            assert_eq!(std::fs::metadata(&binary).unwrap().len(), 15);
            assert_eq!(
                std::fs::metadata(&binary).unwrap().modified().unwrap(),
                modified
            );
            assert!(
                !unchanged_archive(&contents, &key, FileChangeType::CHANGED),
                "same-size, same-mtime replacement must refresh"
            );
            assert_eq!(project_change(&root, Some(&model), &key), Some(true));
            let origin = model.artifact_origin(&binary).unwrap().to_string();
            backend.ws.lock().complete_dependency_origin(origin.clone());
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&binary).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > initial,
                "changed bytes must refresh File and Project artifacts"
            );
            assert!(
                !backend.ws.lock().dependency_origin_complete(&origin),
                "changed archive origin must be retired"
            );
            let replacement = project_archive_contents(&root, Some(&model));
            assert!(unchanged_archive(
                &replacement,
                &key,
                FileChangeType::CHANGED
            ));
            for kind in [FileChangeType::CREATED, FileChangeType::DELETED] {
                assert!(!unchanged_archive(&replacement, &key, kind));
                backend.watches.lock().archive_contents = replacement.clone();
                let before = backend.refresh.lock().requested;
                backend
                    .did_change_watched_files(DidChangeWatchedFilesParams {
                        changes: vec![FileEvent::new(Uri::from_file_path(&binary).unwrap(), kind)],
                    })
                    .await;
                assert!(
                    backend.refresh.lock().requested > before,
                    "create/delete cannot be suppressed"
                );
            }
            std::fs::remove_file(&binary).unwrap();
            assert!(!unchanged_archive(
                &replacement,
                &key,
                FileChangeType::CHANGED
            ));
            assert!(!unchanged_archive(
                &replacement,
                &key,
                FileChangeType::DELETED
            ));
            let before = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&binary).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > before,
                "unreadable archive cannot be suppressed"
            );
            std::fs::write(&binary, b"replaced binary").unwrap();
            assert!(
                !unchanged_archive(&HashMap::new(), &key, FileChangeType::CHANGED),
                "a new, unbaselined archive cannot hide its first event"
            );
            backend.watches.lock().archive_contents.clear();
            let before = backend.refresh.lock().requested;
            backend
                .did_change_watched_files(DidChangeWatchedFilesParams {
                    changes: vec![FileEvent::new(
                        Uri::from_file_path(&binary).unwrap(),
                        FileChangeType::CHANGED,
                    )],
                })
                .await;
            assert!(
                backend.refresh.lock().requested > before,
                "unknown baseline must remain conservative"
            );
            assert_eq!(
                backend.ws.lock().doc_text(&open_key).as_deref(),
                Some("class Use { int unsaved; }")
            );
            assert_eq!(
                project_change(
                    &root,
                    Some(&model),
                    &crate::paths::file_key(&root.join("unmodeled-build.jar"))
                ),
                Some(true)
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unchanged_selected_source_jar_watch_does_not_force_model_reload() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_unchanged_source_jar_watch_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("dependency.jar");
        let sources = root.join("dependency-sources.jar");
        std::fs::write(&binary, b"binary").unwrap();
        std::fs::write(&sources, b"sources").unwrap();
        let root_key = crate::paths::file_key(&root);
        let source_key = crate::paths::file_key(&sources);
        let binary_key = crate::paths::file_key(&binary);
        let snapshot = || {
            classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
                "PROJECT\t:app\t{root_key}\nSCP\tmain\t{binary_key}\nEND\t:app\n"
            )))
        };
        let model = snapshot();
        assert_eq!(project_change(&root, Some(&model), &source_key), None);
        assert_eq!(
            project_change(&root, Some(&model), &binary_key),
            Some(true),
            "binary events retain their existing policy"
        );

        std::fs::write(&sources, b"changed sources with a different size").unwrap();
        assert_eq!(project_change(&root, Some(&model), &source_key), Some(true));
        let model = snapshot();
        assert_eq!(project_change(&root, Some(&model), &source_key), None);

        std::fs::remove_file(&sources).unwrap();
        assert_eq!(project_change(&root, Some(&model), &source_key), Some(true));
        let absent = snapshot();
        assert_eq!(
            project_change(&root, Some(&absent), &source_key),
            Some(true),
            "missing selected sources stay relevant"
        );
        std::fs::write(&sources, b"created sources").unwrap();
        assert_eq!(
            project_change(&root, Some(&absent), &source_key),
            Some(true)
        );

        let model = snapshot();
        std::fs::write(&binary, b"changed binary with a different size").unwrap();
        assert_eq!(project_change(&root, Some(&model), &source_key), Some(true));
        assert_eq!(
            project_change(&root, Some(&model), &format!("{root_key}/new-sources.jar")),
            Some(true),
            "an unselected in-project archive keeps the existing discovery policy"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn watched_project_changes_cover_actual_custom_inputs_and_outputs() {
        let temp = std::env::temp_dir().join(format!("ktlsp_watch_inputs_{}", std::process::id()));
        let root_key = crate::paths::file_key(&temp.join("workspace"));
        let external = crate::paths::file_key(&temp.join("external"));
        for path in [
            &root_key,
            &format!("{external}/custom-generated"),
            &format!("{external}/classes"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let root = Path::new(&root_key);
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{root_key}/modules/editing\nSOURCE\tmain\t{external}/custom-generated\nOUTPUT\tmain\t{external}/classes\nSCP\tmain\t{external}/vendor.jar\nEND\t:app\n"
        )));
        assert_eq!(
            project_change(
                root,
                Some(&model),
                &format!("{external}/custom-generated/Generated.java")
            ),
            Some(false)
        );
        assert_eq!(
            project_change(root, Some(&model), &format!("{external}/classes/New.class")),
            Some(true)
        );
        assert_eq!(
            project_change(root, Some(&model), &format!("{external}/vendor.jar")),
            Some(true)
        );
        assert_eq!(
            project_change(
                root,
                Some(&model),
                &format!("{external}/vendor-sources.jar")
            ),
            Some(true)
        );
        assert_eq!(
            project_change(root, Some(&model), &format!("{root_key}/libs/new.jar")),
            Some(true)
        );
        assert_eq!(
            project_change(
                root,
                Some(&model),
                &format!("{external}/classes-other/Unrelated.class")
            ),
            None
        );
        assert_eq!(
            project_change(
                root,
                Some(&model),
                &format!("{root_key}/testData/Decoy.java")
            ),
            None
        );
        let watches = project_watchers(root, Some(&model), true);
        assert!(
            watches.iter().any(|watch| match &watch.glob_pattern {
                GlobPattern::Relative(pattern) => pattern.pattern == "vendor-sources.jar",
                _ => false,
            }),
            "source jar creation, replacement and deletion need automatic watches"
        );
        assert!(watches
            .iter()
            .all(|watch| matches!(watch.glob_pattern, GlobPattern::Relative(_))));
        for required in [
            format!("{external}/custom-generated"),
            format!("{external}/classes"),
            external,
        ] {
            assert!(
                watches.iter().any(|watch| match &watch.glob_pattern {
                    GlobPattern::Relative(pattern) => match &pattern.base_uri {
                        OneOf::Right(uri) => uri_to_key(uri).as_deref() == Some(required.as_str()),
                        _ => false,
                    },
                    _ => false,
                }),
                "no automatic watch for {required}"
            );
        }
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn automatic_watches_cover_new_roots_from_a_bounded_existing_ancestor() {
        let temp =
            std::env::temp_dir().join(format!("ktlsp_watch_new_root_{}", std::process::id()));
        let root = temp.join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let generated = root.join("future [exact]/generated");
        let output = root.join("future [exact]/classes");
        let external = temp.join("external/future [exact]");
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nOUTPUT\tmain\t{}\nSOURCE\tmain\t{}\nOUTPUT\tmain\t{}\nEND\t:app\n",
            root.display(),
            generated.display(),
            output.display(),
            external.join("generated").display(),
            external.join("classes").display()
        )));
        let watches = project_watchers(&root, Some(&model), true);
        assert_eq!(watches.len(), 3);
        assert!(watches.iter().any(|watch| matches!(
            &watch.glob_pattern,
            GlobPattern::Relative(pattern) if pattern.pattern == "**/*" && matches!(
                &pattern.base_uri,
                OneOf::Right(uri) if uri_to_key(uri).as_deref() == Some(crate::paths::file_key(&root).as_str())
            )
        )));
        for (file, force) in [
            (generated.join("New.java"), false),
            (output.join("plugin.inventory"), true),
            (external.join("generated/New.kt"), false),
            (external.join("classes/New.class"), true),
        ] {
            assert_eq!(
                project_change(&root, Some(&model), &crate::paths::file_key(&file)),
                Some(force)
            );
        }
        for required in [
            r"external/future \[exact\]/generated/**/*.{kt,kts,java}",
            r"external/future \[exact\]/classes/**/*",
        ] {
            assert!(
                watches.iter().any(|watch| match &watch.glob_pattern {
                    GlobPattern::Relative(pattern) =>
                        pattern.pattern == required
                            && match &pattern.base_uri {
                                OneOf::Right(uri) =>
                                    uri_to_key(uri).as_deref()
                                        == Some(crate::paths::file_key(&temp).as_str()),
                                _ => false,
                            },
                    _ => false,
                }),
                "missing bounded watch for {required}"
            );
        }
        // A missing model root directly below the filesystem root must never watch the drive.
        let drive = root.ancestors().last().unwrap();
        let absent = drive.join(format!("ktlsp-never-created-{}", std::process::id()));
        assert!(!absent.exists());
        assert!(project_watchers(&absent, None, true).is_empty());
        std::fs::remove_dir_all(temp).unwrap();
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn windows_neovim_registers_absolute_watches_without_changing_other_clients() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_neovim_watch_protocol_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        for (name, relative) in [("Neovim", false), ("Visual Studio Code", true)] {
            let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
            drop(socket);
            let backend = service.inner();
            backend
                .initialize(InitializeParams {
                    client_info: Some(ClientInfo {
                        name: name.to_string(),
                        version: None,
                    }),
                    capabilities: ClientCapabilities {
                        workspace: Some(WorkspaceClientCapabilities {
                            did_change_watched_files: Some(
                                DidChangeWatchedFilesClientCapabilities {
                                    dynamic_registration: Some(true),
                                    relative_pattern_support: Some(true),
                                },
                            ),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    workspace_folders: Some(vec![WorkspaceFolder {
                        uri: Uri::from_file_path(&root).unwrap(),
                        name: "watch-protocol".to_string(),
                    }]),
                    ..Default::default()
                })
                .await
                .unwrap();
            let state = backend.watches.lock();
            assert!(state.dynamic);
            assert_eq!(state.relative, relative, "{name}");
            let watchers = project_watchers(&root, None, state.relative);
            assert!(!watchers.is_empty());
            assert!(watchers.iter().all(|watcher| {
                matches!(watcher.glob_pattern, GlobPattern::Relative(_)) == relative
            }));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn initial_scan_resolves_closed_java_and_kotlin_before_refresh_debounce() {
        let root = std::env::temp_dir().join(format!("ktlsp_initial_scan_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("Helper.java"),
            "package app;\npublic class Helper {}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("KotlinHelper.kt"),
            "package app\nclass KotlinHelper\n",
        )
        .unwrap();
        let text = "package app;\nclass Main { Helper helper; KotlinHelper kotlin; }";
        let key = crate::paths::file_key(&root.join("Main.java"));
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        drop(socket);
        let backend = service.inner();
        backend
            .initialize(InitializeParams {
                workspace_folders: Some(vec![WorkspaceFolder {
                    uri: Uri::from_file_path(&root).unwrap(),
                    name: "initial-scan".to_string(),
                }]),
                ..Default::default()
            })
            .await
            .unwrap();
        backend.ws.lock().open(key.clone(), text.to_string());
        backend.initialized(InitializedParams {}).await;
        let result = tokio::time::timeout(PROJECT_REFRESH_DEBOUNCE, async {
            loop {
                let resolved = ["Helper", "KotlinHelper"].iter().all(|name| {
                    backend
                        .ws
                        .lock()
                        .goto_definition(&key, text.find(name).unwrap())
                        .iter()
                        .any(|definition| {
                            Path::new(&definition.file)
                                .file_stem()
                                .and_then(|stem| stem.to_str())
                                == Some(*name)
                        })
                });
                if resolved {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await;
        let (generation, java_entries, kotlin_entries) = {
            let ws = backend.ws.lock();
            (
                ws.analysis_generation(),
                ws.index.lookup_type("Helper").len(),
                ws.index.lookup_type("KotlinHelper").len(),
            )
        };
        assert!(
            result.is_ok(),
            "initial source definitions were unavailable before debounce; gradle={}, generation={}, Helper entries={}, KotlinHelper entries={}",
            *backend.gradle_project.lock(),
            generation,
            java_entries,
            kotlin_entries
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn watched_file_bursts_coalesce_and_active_refreshes_remain_singleflight() {
        async fn wait_for_fixture(
            backend: &Backend,
            root: &Path,
            expected_invocations: usize,
            completed: bool,
        ) {
            // Child markers and worker completion prove readiness; the owned harness bounds stalls.
            loop {
                let observed = std::fs::read_to_string(root.join("invocations"))
                    .map(|text| text.lines().count())
                    .unwrap_or_default();
                let (running, requested) = {
                    let state = backend.refresh.lock();
                    (state.running, state.requested)
                };
                let (baselined, model_loading) = {
                    let watches = backend.watches.lock();
                    (watches.configuration_baselined, watches.model_loading)
                };
                let generation = backend.ws.lock().analysis_generation();
                assert!(observed <= expected_invocations,
                    "unexpected fixture invocation: observed={observed}, expected={expected_invocations}, completed={completed}, running={running}, requested={requested}, generation={generation}, baselined={baselined}, model_loading={model_loading}");
                if observed == expected_invocations && (!completed || !running) {
                    return;
                }
                assert!(running,
                    "fixture worker terminated before readiness: observed={observed}, expected={expected_invocations}, completed={completed}, requested={requested}, generation={generation}, baselined={baselined}, model_loading={model_loading}, lease={}, overlap={}, root={}",
                    root.join("running").exists(), root.join("overlap").exists(), root.display());
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        let root = gradle_import_request_fixture("refresh-scheduler");
        let nested = root.join("included");
        let configuration_files = [
            root.join("gradle.properties"),
            root.join("gradle/wrapper/gradle-wrapper.properties"),
            nested.join("gradle.properties"),
            nested.join("gradle/wrapper/gradle-wrapper.properties"),
            root.join("gradle/wrapper/gradle-wrapper.jar"),
            nested.join("gradle/wrapper/gradle-wrapper.jar"),
        ];
        for file in &configuration_files {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "baseline=true\n").unwrap();
        }
        std::fs::write(nested.join("gradlew"), "wrapper launcher").unwrap();
        std::fs::write(nested.join("build.gradle.kts"), "plugins { java }").unwrap();
        let excluded_jars = [
            root.join("src/main/resources/gradle/wrapper/gradle-wrapper.jar"),
            root.join("libs/dependency.jar"),
        ];
        for file in &excluded_jars {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "baseline=true\n").unwrap();
        }
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        assert!(std::process::Command::new("git")
            .args(["add", "--", "."])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let root_contents = configuration_contents(&root);
        let nested_contents = configuration_contents(&nested);
        for file in &excluded_jars {
            let key = crate::paths::file_key(file);
            assert!(
                !root_contents.contains_key(&key),
                "non-wrapper JAR was baselined: {key}"
            );
            assert_eq!(project_change(&root, None, &key), Some(true));
        }
        for file in &configuration_files {
            let key = crate::paths::file_key(file);
            assert!(
                root_contents.contains_key(&key),
                "missing Git baseline: {key}"
            );
            assert_eq!(
                nested_contents.contains_key(&key),
                file.starts_with(&nested),
                "Git subroot baseline must stay scoped: {key}"
            );
        }
        let dump = format!(
            "PROJECT\t:app\t{}\nCP\t{}\nEND\t:app\n",
            root.display(),
            root.join("unbuilt.jar").display()
        );
        std::fs::write(root.join("dump.txt"), dump).unwrap();
        let wrapper = root.join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        });
        std::fs::write(&wrapper, if cfg!(windows) {
            // An OS lease survives only while this owned child is live. Cancellation can
            // leave a sentinel on disk, so its existence alone cannot prove overlap.
            "@echo off\r\npowershell.exe -NoProfile -NonInteractive -Command \"try { $lease=[IO.File]::Open('%~dp0running',[IO.FileMode]::OpenOrCreate,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None) } catch { [IO.File]::WriteAllText('%~dp0overlap','overlap'); exit 1 }; try { [IO.File]::AppendAllText('%~dp0invocations','invoked'+[Environment]::NewLine); if ([IO.File]::ReadAllLines('%~dp0invocations').Length -eq 2) { while ($true) { Start-Sleep -Milliseconds 20 } }; Start-Sleep -Milliseconds 600; [Console]::Write([IO.File]::ReadAllText('%~dp0dump.txt')) } finally { $lease.Dispose(); [IO.File]::Delete('%~dp0running') }\"\r\nexit /b %errorlevel%\r\n"
        } else {
            "#!/bin/sh\ncd \"$(dirname \"$0\")\"\nif [ -f running ] && kill -0 \"$(cat running)\" 2>/dev/null; then touch overlap; fi\necho $$ > running\necho invoked >> invocations\nif [ \"$(wc -l < invocations)\" -eq 2 ]; then while :; do sleep 0.02; done; fi\nsleep 0.6\ncat dump.txt\nrm running\n"
        }).unwrap();
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        // This scheduler test does not inspect outgoing notifications. Close their
        // receiver so log messages cannot block on an undrained client channel.
        drop(socket);
        let backend = service.inner();
        backend
            .initialize(InitializeParams {
                workspace_folders: Some(vec![WorkspaceFolder {
                    uri: Uri::from_file_path(&root).unwrap(),
                    name: "refresh".to_string(),
                }]),
                ..Default::default()
            })
            .await
            .unwrap();
        let event = || DidChangeWatchedFilesParams {
            changes: std::iter::once(root.join("build.gradle.kts"))
                .chain(configuration_files.iter().cloned())
                .map(|file| {
                    FileEvent::new(Uri::from_file_path(file).unwrap(), FileChangeType::CHANGED)
                })
                .collect(),
        };
        backend.initialized(InitializedParams {}).await;
        let invocations = || {
            std::fs::read_to_string(root.join("invocations"))
                .map(|text| text.lines().count())
                .unwrap_or_default()
        };
        wait_for_fixture(backend, &root, 1, false).await;
        assert_eq!(invocations(), 1, "initial import never started");
        let initial = backend.refresh.lock().requested;
        let initial_cancelled = backend.refresh.lock().cancelled.clone();
        for _ in 0..5 {
            backend.did_change_watched_files(event()).await;
        }
        for file in [
            root.join("gradle/wrapper/gradle-wrapper.jar"),
            nested.join("gradle/wrapper/gradle-wrapper.jar"),
        ] {
            backend
                .did_save(DidSaveTextDocumentParams {
                    text_document: TextDocumentIdentifier {
                        uri: Uri::from_file_path(file).unwrap(),
                    },
                    text: None,
                })
                .await;
        }
        assert_eq!(backend.refresh.lock().requested, initial);
        assert!(!initial_cancelled.load(std::sync::atomic::Ordering::Acquire));
        wait_for_fixture(backend, &root, 1, true).await;
        assert!(!backend.refresh.lock().running);
        assert_eq!(
            std::fs::read_to_string(root.join("invocations"))
                .unwrap()
                .lines()
                .count(),
            1
        );
        for plugin in ["base", "java", "base", "java", "base"] {
            std::fs::write(
                root.join("build.gradle.kts"),
                format!("plugins {{ {plugin} }}"),
            )
            .unwrap();
            backend.did_change_watched_files(event()).await;
        }
        // The lease file appears before logging. Wait for the durable second startup record;
        // that child then stays active until cancellation, without a 600 ms timing window.
        wait_for_fixture(backend, &root, 2, false).await;
        assert_eq!(invocations(), 2, "second refresh never completed startup");
        assert!(root.join("running").exists());
        let superseded = backend.ws.lock().analysis_generation();
        let active_cancelled = backend.refresh.lock().cancelled.clone();
        backend.did_change_watched_files(event()).await;
        assert_eq!(backend.ws.lock().analysis_generation(), superseded);
        assert!(!active_cancelled.load(std::sync::atomic::Ordering::Acquire));
        let build_jar = root.join("buildSrc/build/libs/buildSrc.jar");
        std::fs::create_dir_all(build_jar.parent().unwrap()).unwrap();
        std::fs::write(&build_jar, "first build-logic bytes").unwrap();
        let jar_event = |kind| DidChangeWatchedFilesParams {
            changes: vec![FileEvent::new(
                Uri::from_file_path(&build_jar).unwrap(),
                kind,
            )],
        };
        assert!(backend.watches.lock().model_loading);
        backend
            .did_change_watched_files(jar_event(FileChangeType::CHANGED))
            .await;
        let pending = backend.ws.lock().analysis_generation();
        assert_eq!(pending, superseded + 1);
        assert!(backend.refresh.lock().force);
        assert!(backend.watches.lock().pending_model_jar);
        assert!(
            !active_cancelled.load(std::sync::atomic::Ordering::Acquire),
            "a build-logic JAR must not kill its own model dump"
        );
        std::fs::write(&build_jar, "genuinely different build-logic bytes").unwrap();
        backend
            .did_change_watched_files(jar_event(FileChangeType::CHANGED))
            .await;
        std::fs::remove_file(&build_jar).unwrap();
        backend
            .did_change_watched_files(jar_event(FileChangeType::DELETED))
            .await;
        std::fs::write(&build_jar, "replacement build-logic bytes").unwrap();
        backend
            .did_change_watched_files(jar_event(FileChangeType::CREATED))
            .await;
        assert_eq!(backend.ws.lock().analysis_generation(), pending);
        assert!(backend.refresh.lock().force);
        assert!(!active_cancelled.load(std::sync::atomic::Ordering::Acquire));
        std::fs::write(root.join("build.gradle.kts"), "plugins { java }").unwrap();
        let mut mixed = event();
        mixed
            .changes
            .extend(jar_event(FileChangeType::CHANGED).changes);
        backend.did_change_watched_files(mixed).await;
        assert!(active_cancelled.load(std::sync::atomic::Ordering::Acquire));
        wait_for_fixture(backend, &root, 3, true).await;
        assert!(!backend.refresh.lock().running);
        assert!(!backend.watches.lock().model_loading);
        assert!(!backend.watches.lock().pending_model_jar);
        assert_eq!(
            std::fs::read_to_string(root.join("invocations"))
                .unwrap()
                .lines()
                .count(),
            3
        );
        assert!(!root.join("overlap").exists());
        let mut ws = backend.ws.lock();
        assert_eq!(ws.analysis_generation(), pending + 1);
        assert!(!ws.finish_analysis_generation(superseded, true, true));
        assert!(
            !ws.project_snapshot().unwrap().modules.is_empty(),
            "latest model was never published"
        );
        drop(ws);
        let mut watches = backend.watches.lock();
        for file in &excluded_jars {
            let key = crate::paths::file_key(file);
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
        }
        for file in [
            root.join("build.gradle.kts"),
            root.join("gradle/wrapper/gradle-wrapper.jar"),
        ] {
            let key = crate::paths::file_key(&file);
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CREATED));
            std::fs::remove_file(&key).unwrap();
            assert!(!watches.unchanged_configuration(&key, FileChangeType::DELETED));
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
            std::fs::create_dir(&key).unwrap();
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
            std::fs::remove_dir(&key).unwrap();
            std::fs::write(&key, "plugins { java }").unwrap();
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
            assert!(watches.unchanged_configuration(&key, FileChangeType::CHANGED));
            watches.configuration_contents.remove(&key);
            assert!(!watches.unchanged_configuration(&key, FileChangeType::CHANGED));
        }
        let unknown = crate::paths::file_key(&root.join("new.gradle.kts"));
        std::fs::write(&unknown, "plugins { java }").unwrap();
        assert!(!watches.unchanged_configuration(&unknown, FileChangeType::CHANGED));
        drop(watches);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_scope_defaults_to_open_files_only() {
        assert_eq!(diagnostic_scope_from(&None), DiagnosticScope::OpenFilesOnly);
        assert_eq!(
            diagnostic_scope_from(&Some(json!({ "diagnostics": { "scope": "workspace" } }))),
            DiagnosticScope::Workspace
        );
        assert_eq!(
            diagnostic_scope_from(&Some(json!({ "diagnostics": { "scope": "nope" } }))),
            DiagnosticScope::OpenFilesOnly
        );
    }

    #[test]
    fn kotlin_import_lookup_matches_import_and_usage_identifiers() {
        let text = "package app\nimport android.graphics.Bitmap\nfun use(value: Bitmap) = value\n";
        let import_offset = text.find("Bitmap").unwrap();
        let usage_offset = text.rfind("Bitmap").unwrap();
        assert_eq!(
            kotlin_import_fqn_for_identifier(text, import_offset).as_deref(),
            Some("android.graphics.Bitmap")
        );
        assert_eq!(
            kotlin_import_fqn_for_identifier(text, usage_offset).as_deref(),
            Some("android.graphics.Bitmap")
        );
    }

    #[test]
    fn java_import_lookup_matches_import_and_usage_identifiers() {
        let text = "import android.graphics.Bitmap;\nclass App { Bitmap value; }\n";
        let import_offset = text.find("Bitmap").unwrap();
        let usage_offset = text.rfind("Bitmap").unwrap();
        let mut java = crate::java::JavaParser::new();
        assert_eq!(
            java_import_fqn_for_identifier(text, import_offset, &mut java).as_deref(),
            Some("android.graphics.Bitmap")
        );
        assert_eq!(
            java_import_fqn_for_identifier(text, usage_offset, &mut java).as_deref(),
            Some("android.graphics.Bitmap")
        );
    }

    fn gradle_import_request_fixture(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_import_request_{name}_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("build.gradle.kts"), "plugins { java }").unwrap();
        let wrapper = root.join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        });
        std::fs::write(
            &wrapper,
            if cfg!(windows) {
                "@echo off\r\necho invoked> \"%~dp0gradle-invoked\"\r\nexit /b 1\r\n"
            } else {
                "#!/bin/sh\n: > \"$(dirname \"$0\")/gradle-invoked\"\nexit 1\n"
            },
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        root
    }

    #[tokio::test]
    async fn initialize_keeps_gradle_unknown_until_a_source_model_is_available() {
        for gradle in [true, false] {
            let root = std::env::temp_dir().join(format!(
                "ktlsp_initialize_scope_{gradle}_{}",
                std::process::id()
            ));
            let decoy = root.join("src/test/resources/vendor/Helper.java");
            std::fs::create_dir_all(decoy.parent().unwrap()).unwrap();
            std::fs::write(
                &decoy,
                "package vendor; public class Helper { public void forbidden() {} }",
            )
            .unwrap();
            if gradle {
                std::fs::write(root.join("build.gradle.kts"), "plugins { java }").unwrap();
            }
            let (service, _socket) = tower_lsp_server::LspService::new(Backend::new);
            let backend = service.inner();
            backend
                .initialize(InitializeParams {
                    workspace_folders: Some(vec![WorkspaceFolder {
                        uri: Uri::from_file_path(&root).unwrap(),
                        name: "pending-model".to_string(),
                    }]),
                    ..Default::default()
                })
                .await
                .unwrap();
            let mut ws = backend.ws.lock();
            assert_eq!(ws.scan(&root), usize::from(!gradle));
            for (name, text) in [
                (
                    "Use.java",
                    "package app; import vendor.Helper; class Use { void known() {} void run(Helper helper) { known(); helper.forbidden(); } }",
                ),
                (
                    "Use.kt",
                    "package app\nimport vendor.Helper\nfun known() {}\nfun run(helper: Helper) { known(); helper.forbidden() }\n",
                ),
            ] {
                let key = crate::paths::file_key(&root.join("src/main").join(name));
                ws.open(key.clone(), text.to_string());
                let forbidden = text.find("helper.forbidden").unwrap() + "helper.".len();
                assert_eq!(
                    ws.goto_definition(&key, forbidden).len(),
                    usize::from(!gradle),
                    "{name}: a closed resource is visible while the Gradle model is unknown"
                );
                assert_eq!(
                    ws.complete(&key, forbidden + 3, true)
                        .is_some_and(|completion| completion
                            .items
                            .iter()
                            .any(|item| item.label.starts_with("forbidden"))),
                    !gradle,
                    "{name}: resource member completion must respect pending Gradle scope"
                );
                let known = text.find("known();").unwrap();
                let targets = ws.goto_definition(&key, known);
                assert_eq!(targets.len(), 1, "{name}: same-file goto remains useful");
                assert_eq!(targets[0].file, key);
                assert!(
                    ws.complete(&key, known + 2, true)
                        .expect("same-file completion")
                        .items
                        .iter()
                        .any(|item| item.label.starts_with("known")),
                    "{name}: same-file completion remains useful"
                );
            }
            drop(ws);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn gradle_import_request_does_not_resolve_unpublished_model() {
        let root = gradle_import_request_fixture("unpublished");
        let text = "import vendor.Only; class Use { Only value; }";
        let (import, batches) = resolve_import_source_batches(
            &root,
            &root.join("src/main/java/Use.java"),
            text,
            text.rfind("Only").unwrap(),
            true,
            None,
        );
        let launched = root.join("gradle-invoked").exists();
        std::fs::remove_dir_all(root).unwrap();
        assert_eq!(import.as_deref(), Some("vendor.Only"));
        assert!(batches.is_empty());
        assert!(
            !launched,
            "an editor request launched Gradle without a published model"
        );
    }

    #[test]
    fn gradle_import_request_keeps_published_source_set_after_build_file_change() {
        use std::io::Write;

        let root = gradle_import_request_fixture("published");
        let selected_jar = root.join("selected.jar");
        let other_jar = root.join("other.jar");
        for jar in [&selected_jar, &other_jar] {
            let mut zip = zip::ZipWriter::new(std::fs::File::create(jar).unwrap());
            zip.start_file(
                "vendor/Only.class",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"").unwrap();
            zip.finish().unwrap();
        }
        let source_root = root.join("modules/editing/custom-java");
        let test_root = root.join("modules/editing/custom-test");
        let model = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nSOURCE\ttest\t{}\nSCP\ttest\t{}\nEND\t:app\n",
            root.join("modules/editing").display(),
            source_root.display(),
            selected_jar.display(),
            test_root.display(),
            other_jar.display(),
        )));
        std::fs::write(
            root.join("build.gradle.kts"),
            "plugins { java }; dependencies { implementation(files(\"other.jar\")) }",
        )
        .unwrap();
        for (file, text, expected_jar) in [
            (
                source_root.join("Use.java"),
                "import vendor.Only; class Use { Only value; }",
                &selected_jar,
            ),
            (
                test_root.join("Use.kt"),
                "import vendor.Only\nfun use(value: Only) = value\n",
                &other_jar,
            ),
        ] {
            let (import, batches) = resolve_import_source_batches(
                &root,
                &file,
                text,
                text.rfind("Only").unwrap(),
                true,
                Some(&model),
            );
            assert_eq!(import.as_deref(), Some("vendor.Only"));
            assert_eq!(batches.len(), 1);
            assert!(batches[0].symbols.iter().any(|symbol| {
                symbol.name == "Only" && symbol.package == "vendor" && symbol.kind.is_type_like()
            }));
            assert_eq!(
                batches[0].origin.as_ref().unwrap().origin_id,
                model.artifact_origin(expected_jar).unwrap()
            );
        }
        let text = "import vendor.Only; class Use { Only value; }";
        let unsupported = classpath::ProjectSnapshot::new(classpath::parse_dump(&format!(
            "PROJECT\t:app\t{}\nEND\t:app\n",
            root.display()
        )));
        for (file, model) in [
            (root.join("testData/Use.java"), &model),
            (source_root.join("Use.java"), &unsupported),
        ] {
            let (import, batches) = resolve_import_source_batches(
                &root,
                &file,
                text,
                text.rfind("Only").unwrap(),
                true,
                Some(model),
            );
            assert_eq!(import.as_deref(), Some("vendor.Only"));
            assert!(batches.is_empty());
        }
        let launched = root.join("gradle-invoked").exists();
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            !launched,
            "an editor request re-resolved changed Gradle metadata"
        );
    }
}

fn schedule_owned_diagnostic(
    ws: &Arc<Mutex<Workspace>>,
    client: &Client,
    versions: &Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    key: String,
) {
    let version = {
        let mut versions = versions.lock();
        let revision = versions.entry(key.clone()).or_default();
        revision.generation += 1;
        *revision
    };
    let ws = ws.clone();
    let client = client.clone();
    let versions = versions.clone();
    tokio::spawn(async move {
        tokio::time::sleep(DIAGNOSTIC_DEBOUNCE).await;
        if versions.lock().get(&key).copied() != Some(version) {
            return;
        }
        publish_diagnostics(&ws, &client, &versions, &key).await;
    });
}

type CompilerWatchSlot = Arc<Mutex<Option<Arc<crate::compiled_watch::NativeCompileWatch>>>>;

fn close_owned_compile_watch(
    slot: &CompilerWatchSlot,
    request: &Arc<crate::compile::ObservedCompileRequest>,
) {
    let removed = {
        let mut guard = slot.lock();
        if guard
            .as_ref()
            .is_some_and(|watch| Arc::ptr_eq(&watch.owner, request))
        {
            guard.take()
        } else {
            None
        }
    };
    if let Some(watch) = removed {
        watch.close();
    }
}

struct ObservedWorkerLease {
    request: Arc<crate::compile::ObservedCompileRequest>,
    workspace: std::sync::Weak<Mutex<Workspace>>,
    watches: std::sync::Weak<Mutex<Option<Arc<crate::compiled_watch::NativeCompileWatch>>>>,
    keep_watch: bool,
    // Owned by the blocking job, never by the cancellable LSP future.
    _permit: tokio::sync::OwnedSemaphorePermit,
}

impl Drop for ObservedWorkerLease {
    fn drop(&mut self) {
        if let Some(ws) = self.workspace.upgrade() {
            ws.lock().cancel_observed_compile(&self.request);
        }
        if !self.keep_watch || self.request.check_live().is_err() {
            if let Some(slot) = self.watches.upgrade() {
                close_owned_compile_watch(&slot, &self.request);
            }
        }
        // The permit is released only after ticket retirement and watch cleanup above.
    }
}

struct CancelObservedRequest {
    request: Arc<crate::compile::ObservedCompileRequest>,
    armed: bool,
    wake: tokio::sync::mpsc::Sender<()>,
}

impl Drop for CancelObservedRequest {
    fn drop(&mut self) {
        if self.armed {
            self.request
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            self.request
                .invalid
                .store(true, std::sync::atomic::Ordering::Release);
            let _ = self.wake.try_send(());
        }
    }
}

fn run_owned_compile(
    mut lease: ObservedWorkerLease,
    slot: CompilerWatchSlot,
    wake: tokio::sync::mpsc::Sender<()>,
) -> serde_json::Value {
    use crate::compiled_watch::{NativeCompileWatch, WatchPlan};
    let request = lease.request.clone();
    let result = (|| -> anyhow::Result<serde_json::Value> {
        request.check_live()?;
        // begin_observed_compile already retracted the previous owner's facts. Release its
        // native coverage even when arming the new owner later fails; keep joins on this worker.
        let previous = slot.lock().take();
        if let Some(previous) = previous {
            previous.close();
        }
        request.check_live()?;
        let initial = WatchPlan::build(&request, &request.initial_watch_paths())?;
        let watch = NativeCompileWatch::new(request.clone(), initial.clone(), wake)?;
        let previous = slot.lock().replace(watch.clone());
        if let Some(previous) = previous {
            previous.close();
        }
        request.check_live()?;
        // The same blocking job owns native exit, final watches, read verification, publication
        // and retirement. Cancellation of the awaiting future cannot release this lease.
        let mut result = crate::compile::run_observed_compile(request.clone(), &initial);
        if let Some(receipt) = result.receipt.clone() {
            let verified = (|| -> anyhow::Result<()> {
                request.check_live()?;
                // Receipt reads already passed the initial-plan check. This extension keeps
                // future output/input coverage; it cannot certify an earlier unobserved read.
                watch.extend(WatchPlan::build(&request, &receipt.watch_paths())?)?;
                receipt.verify_current()?;
                request.check_live()
            })();
            if let Err(error) = verified {
                result.receipt = None;
                result.owners.clear();
                result.reason = error.to_string();
            }
        }
        let reason = result.reason.clone();
        let executed = result.outcome.executed;
        let diagnostic_count = result.outcome.diagnostics.len();
        let ws = lease
            .workspace
            .upgrade()
            .ok_or_else(|| anyhow::anyhow!("backend-closed"))?;
        let (status, deferred) = ws.lock().finish_observed_compile(&request, result);
        lease.keep_watch = status == "observed-success" && request.check_live().is_ok();
        Ok(
            serde_json::json!({"status":status,"reason":reason,"task":request.task.task,
            "executed":executed,"diagnostics":diagnostic_count,"deferredOutputs":deferred,
            "compilerClosureComplete":false}),
        )
    })();
    result.unwrap_or_else(|error| {
        serde_json::json!({"status":"compile-unverified","reason":error.to_string(),
        "compilerClosureComplete":false})
    })
}

impl Backend {
    fn close_project_refresh(&self) {
        let mut refresh = self.refresh.lock();
        if !refresh.stopped {
            refresh.stopped = true;
            refresh
                .cancelled
                .store(true, std::sync::atomic::Ordering::Release);
            self.ws.lock().begin_analysis_generation();
        }
    }

    fn close_compiler_jobs(&self) {
        self.compile_workers.close();
        self.ws.lock().invalidate_kotlin_diagnostics();
        let environments = {
            let mut state = self.kotlin_diagnostics.lock();
            state.stopped = true;
            state.pending.clear();
            state.order.clear();
            let active = std::mem::take(&mut state.environments);
            state.retired.extend(active.into_values());
            std::mem::take(&mut state.retired)
        };
        for environment in environments {
            environment.close();
        }
        self.ws.lock().stop_observed_compile();
        let watch = self.compile_watch.lock().take();
        if let Some(watch) = watch {
            watch.close();
        }
    }

    async fn execute_observed_compile(&self, arguments: &[serde_json::Value]) -> serde_json::Value {
        let Some(uri) = arguments
            .first()
            .and_then(|value| {
                value
                    .as_str()
                    .or_else(|| value.get("uri").and_then(serde_json::Value::as_str))
            })
            .and_then(|uri| uri.parse::<Uri>().ok())
        else {
            return serde_json::json!({"status":"invalid-arguments"});
        };
        let Some(key) = uri_to_key(&uri) else {
            return serde_json::json!({"status":"invalid-uri"});
        };
        let Some(root) = self.root.lock().clone() else {
            return serde_json::json!({"status":"no-root"});
        };
        let project_watch_ready = {
            let state = self.watches.lock();
            state.dynamic && state.registration.is_some() && state.registered_options.is_some()
        };
        if !project_watch_ready {
            return serde_json::json!({"status":"project-freshness-watch-unavailable"});
        }
        let permit = match self.compile_workers.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => return serde_json::json!({"status":"compile-already-running-or-closed"}),
        };
        let (request, model) = match self.ws.lock().begin_observed_compile(&root, &key) {
            Ok(request) => request,
            Err(reason) => return serde_json::json!({"status":reason}),
        };
        let expected_options = {
            let state = self.watches.lock();
            serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers: project_watchers(&root, Some(&model), state.relative),
            })
            .unwrap()
        };
        if !self.watches.lock().acknowledges(&expected_options) {
            self.ws.lock().cancel_observed_compile(&request);
            return serde_json::json!({"status":"current-project-watch-ack-unavailable"});
        }
        let lease = ObservedWorkerLease {
            request: request.clone(),
            workspace: Arc::downgrade(&self.ws),
            watches: Arc::downgrade(&self.compile_watch),
            keep_watch: false,
            _permit: permit,
        };
        let (wake, mut events) = tokio::sync::mpsc::channel(1);
        let mut cancellation = CancelObservedRequest {
            request: request.clone(),
            armed: true,
            wake: wake.clone(),
        };
        let weak_ws = Arc::downgrade(&self.ws);
        let weak_slot = Arc::downgrade(&self.compile_watch);
        let owner = request.clone();
        let client = self.client.clone();
        let versions = self.doc_versions.clone();
        tokio::spawn(async move {
            if events.recv().await.is_some() {
                if let Some(ws) = weak_ws.upgrade() {
                    let changed = {
                        let mut ws = ws.lock();
                        if ws.invalidate_owned_compile(&owner) {
                            ws.take_extension_diagnostic_changes()
                        } else {
                            Vec::new()
                        }
                    };
                    for key in changed {
                        schedule_owned_diagnostic(&ws, &client, &versions, key);
                    }
                }
                // Never close/join notify under Workspace's mutex or in the notify callback.
                if let Some(slot) = weak_slot.upgrade() {
                    close_owned_compile_watch(&slot, &owner);
                }
            }
        });
        let slot = self.compile_watch.clone();
        let result =
            tokio::task::spawn_blocking(move || run_owned_compile(lease, slot, wake)).await;
        cancellation.armed = false;
        let changed = self.ws.lock().take_extension_diagnostic_changes();
        for key in changed {
            self.schedule_one_diagnostic(key);
        }
        match result {
            Ok(result) => result,
            Err(error) => {
                serde_json::json!({"status":"compile-worker-failed","reason":error.to_string()})
            }
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        let start = crate::trace::enabled().then(Instant::now);
        if let Some(start) = start {
            crate::trace::span(
                "server.backend.drop.start",
                "server",
                start,
                serde_json::json!({}),
            );
        }
        self.close_project_refresh();
        self.close_compiler_jobs();
        self.jdk_source_seeds.lock().clear();
        if let Some(start) = start {
            crate::trace::span(
                "server.backend.drop",
                "server",
                start,
                serde_json::json!({}),
            );
        }
    }
}

#[cfg(test)]
mod observed_worker_lease_tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn canceled_lsp_future_keeps_native_worker_busy_until_its_exit() {
        let semaphore = Arc::new(tokio::sync::Semaphore::new(1));
        let ws = Arc::new(Mutex::new(Workspace::new()));
        let slot: CompilerWatchSlot = Arc::new(Mutex::new(None));
        let request = crate::compile::observed_test_request(
            &std::env::temp_dir().join("ktlsp-worker-lease-test"),
        );
        let lease = ObservedWorkerLease {
            request: request.clone(),
            workspace: Arc::downgrade(&ws),
            watches: Arc::downgrade(&slot),
            keep_watch: false,
            _permit: semaphore.clone().try_acquire_owned().unwrap(),
        };
        let (started, ready) = tokio::sync::oneshot::channel();
        let (finish, finishing) = std::sync::mpsc::channel();
        let worker = tokio::task::spawn_blocking(move || {
            let _ = started.send(());
            let _ = finishing.recv();
            drop(lease);
        });
        ready.await.unwrap();
        assert!(
            semaphore.clone().try_acquire_owned().is_err(),
            "concurrent second request must be rejected"
        );
        let response = tokio::spawn(async move {
            let _ = worker.await;
        });
        let (wake, _events) = tokio::sync::mpsc::channel(1);
        let cancellation = CancelObservedRequest {
            request: request.clone(),
            armed: true,
            wake,
        };
        response.abort();
        let _ = response.await;
        drop(cancellation);
        assert!(request.cancelled.load(Ordering::Acquire));
        assert!(request.invalid.load(Ordering::Acquire));
        assert!(
            semaphore.clone().try_acquire_owned().is_err(),
            "future cancel is not native exit"
        );
        finish.send(()).unwrap();
        let permit =
            tokio::time::timeout(Duration::from_secs(5), semaphore.clone().acquire_owned())
                .await
                .unwrap()
                .unwrap();
        drop(permit);
        assert_eq!(semaphore.available_permits(), 1);
    }
}

#[cfg(test)]
mod compiled_watch_ack_tests {
    use super::*;

    #[test]
    fn initial_or_previous_watch_ack_cannot_prove_current_model_coverage() {
        let old = serde_json::json!({"watchers":[{"globPattern":"/repo/**/*.jar"}]});
        let current = serde_json::json!({"watchers":[{"globPattern":"/repo/**/*"}]});
        let mut state = WatchState::default();
        state.dynamic = true;
        state.registration = Some("old-ack".into());
        state.registered_options = Some(old.clone());
        assert!(state.acknowledges(&old));
        assert!(!state.acknowledges(&current));
        state.registered_options = Some(current.clone());
        assert!(state.acknowledges(&current));
        state.registration = None;
        assert!(!state.acknowledges(&current));
    }
}

#[derive(Default)]
struct KotlinDiagnosticState {
    pending: std::collections::BTreeMap<crate::project_model::ProjectScope, String>,
    order: std::collections::VecDeque<crate::project_model::ProjectScope>,
    environments: std::collections::BTreeMap<
        crate::project_model::ProjectScope,
        Arc<crate::compile::KotlinDiagnosticEnvironment>,
    >,
    retired: Vec<Arc<crate::compile::KotlinDiagnosticEnvironment>>,
    running: bool,
    stopped: bool,
}

impl KotlinDiagnosticState {
    fn enqueue(&mut self, scope: crate::project_model::ProjectScope, key: String) {
        if self.stopped {
            return;
        }
        if self.pending.insert(scope.clone(), key).is_none() {
            self.order.push_back(scope);
        }
    }

    fn next(&mut self) -> Option<String> {
        while let Some(scope) = self.order.pop_front() {
            if let Some(key) = self.pending.remove(&scope) {
                return Some(key);
            }
        }
        None
    }

    fn retain_scopes(
        &mut self,
        active: &std::collections::BTreeSet<crate::project_model::ProjectScope>,
    ) {
        self.pending.retain(|scope, _| active.contains(scope));
        self.order.retain(|scope| active.contains(scope));
        let retired: Vec<_> = self
            .environments
            .keys()
            .filter(|scope| !active.contains(*scope))
            .cloned()
            .collect();
        for scope in retired {
            if let Some(environment) = self.environments.remove(&scope) {
                self.retired.push(environment);
            }
        }
    }
}

fn cached_kotlin_diagnostic_environment(
    state: &Mutex<KotlinDiagnosticState>,
    request: &crate::compile::KotlinDiagnosticRequest,
    fallback: impl FnOnce() -> Option<Arc<crate::compile::KotlinDiagnosticEnvironment>>,
) -> Option<Arc<crate::compile::KotlinDiagnosticEnvironment>> {
    let cached = {
        let state = state.lock();
        state
            .environments
            .get(&request.scope)
            .filter(|environment| environment.matches(request))
            .cloned()
    };
    // Invalidation callbacks lock Workspace before state; release state before the fallback
    // accesses Workspace so a cache miss cannot invert that order.
    cached.or_else(fallback)
}

fn schedule_selected_kotlin_diagnostics(
    ws: &Arc<Mutex<Workspace>>,
    client: &Client,
    versions: &Arc<Mutex<HashMap<String, DiagnosticRevision>>>,
    state: &Arc<Mutex<KotlinDiagnosticState>>,
    workers: &Arc<tokio::sync::Semaphore>,
    watches: &Arc<Mutex<WatchState>>,
    root: &Path,
) {
    if !cfg!(windows) {
        return;
    }
    let targets = ws.lock().kotlin_diagnostic_targets();
    {
        let mut state = state.lock();
        if state.stopped {
            return;
        }
        state.retain_scopes(&targets.iter().map(|(scope, _)| scope.clone()).collect());
        for (scope, key) in targets {
            state.enqueue(scope, key);
        }
        if state.running || (state.pending.is_empty() && state.retired.is_empty()) {
            return;
        }
        state.running = true;
    }
    let ws = ws.clone();
    let client = client.clone();
    let versions = versions.clone();
    let state = state.clone();
    let workers = workers.clone();
    let watches = watches.clone();
    let root = root.to_path_buf();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(DIAGNOSTIC_DEBOUNCE).await;
            // Retired watches are joined away from interactive requests and Workspace's mutex.
            let retired = std::mem::take(&mut state.lock().retired);
            if !retired.is_empty() {
                let _ = tokio::task::spawn_blocking(move || {
                    for environment in retired {
                        environment.close();
                    }
                })
                .await;
            }
            // The shared semaphore limits selected diagnostics and observed Gradle builds together.
            // Waiting retains the latest pending source epoch instead of dropping a busy edit.
            let permit = match workers.clone().acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    state.lock().running = false;
                    return;
                }
            };
            let key = {
                let mut state = state.lock();
                if state.stopped {
                    state.running = false;
                    return;
                }
                match state.next() {
                    Some(key) => key,
                    None if !state.retired.is_empty() => continue,
                    None => {
                        state.running = false;
                        return;
                    }
                }
            };
            let selection = {
                let mut ws = ws.lock();
                ws.begin_kotlin_diagnostics(&key)
                    .map(|request| (request, ws.project_snapshot().unwrap()))
            };
            let (request, model) = match selection {
                Ok(selection) => selection,
                Err(reason) => {
                    tracing::debug!(reason, "selected Kotlin diagnostics unavailable");
                    continue;
                }
            };
            let expected_options = {
                let watches = watches.lock();
                serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                    watchers: project_watchers(&root, Some(&model), watches.relative),
                })
                .unwrap()
            };
            // Closed disk sources and source-set membership reuse current ACKed project coverage.
            if !watches.lock().acknowledges(&expected_options) {
                request
                    .invalid
                    .store(true, std::sync::atomic::Ordering::Release);
                tracing::debug!("selected Kotlin diagnostics need current project watch ACK");
                continue;
            }
            let (wake, mut events) = tokio::sync::mpsc::channel(1);
            let weak_ws = Arc::downgrade(&ws);
            let weak_state = Arc::downgrade(&state);
            let event_client = client.clone();
            let event_versions = versions.clone();
            let event_workers = workers.clone();
            let event_watches = watches.clone();
            let event_root = root.clone();
            let worker_ws = ws.clone();
            let worker_state = state.clone();
            let worker_request = request.clone();
            let result = tokio::task::spawn_blocking(move || {
                // The permit belongs to the blocking owner until native cleanup and both EOFs.
                let _permit = permit;
                let start = Instant::now();
                let scope = worker_request.scope.clone();
                let cached = cached_kotlin_diagnostic_environment(&worker_state, &worker_request,
                    || worker_ws.lock().compiler_api_environment(&worker_request));
                let environment = match cached {
                    Some(environment) => { drop(wake); Ok((environment, false)) }
                    None => {
                        let old = worker_state.lock().environments.remove(&scope);
                        if let Some(old) = old { old.close(); }
                        crate::compile::KotlinDiagnosticEnvironment::prepare(&worker_request, wake)
                            .map(|environment| (environment, true))
                    }
                };
                let (environment, prepared) = match environment {
                    Ok(environment) => environment,
                    Err(error) => {
                        // No callback is installed for failed preparation/cleanup: no retry storm.
                        tracing::debug!(%error, "selected Kotlin compiler inputs unavailable");
                        return worker_ws.lock().finish_kotlin_diagnostics(&worker_request,
                            crate::compile::KotlinDiagnosticOutcome::Unverified(error.to_string()),
                            Arc::new(std::sync::atomic::AtomicBool::new(false)));
                    }
                };
                {
                    let mut state = worker_state.lock();
                    if state.stopped {
                        drop(state);
                        environment.close();
                        return Vec::new();
                    }
                    state.environments.insert(scope.clone(), environment.clone());
                }
                if prepared {
                    let event_invalid = environment.invalid.clone();
                    // Install only after successful preparation and publication of the watch owner.
                    // A queued preparation event is still observed; an old/retired callback is ignored.
                    tokio::spawn(async move {
                        if events.recv().await.is_none() { return; }
                        let (Some(ws), Some(state)) = (weak_ws.upgrade(), weak_state.upgrade()) else { return; };
                        let keys = {
                            let mut ws = ws.lock();
                            let state = state.lock();
                            if state.stopped || !state.environments.get(&scope).is_some_and(|environment|
                                Arc::ptr_eq(&environment.invalid, &event_invalid)) { return; }
                            ws.invalidate_kotlin_scope(&scope);
                            let mut keys = ws.take_extension_diagnostic_changes();
                            keys.extend(ws.kotlin_diagnostic_targets().into_iter().filter(|(owner, _)| owner == &scope)
                                .map(|(_, key)| key));
                            keys.sort(); keys.dedup(); keys
                        };
                        for key in keys { schedule_owned_diagnostic(&ws, &event_client, &event_versions, key); }
                        schedule_selected_kotlin_diagnostics(&ws, &event_client, &event_versions, &state,
                            &event_workers, &event_watches, &event_root);
                    });
                }
                tracing::debug!(identity = ?environment.input_identity, task = worker_request.task.task.as_str(),
                    prepared, "selected Kotlin editor compile started");
                let outcome = crate::compile::run_kotlin_editor_compile(&worker_request, &environment);
                if let crate::compile::KotlinDiagnosticOutcome::Unverified(reason) = &outcome {
                    tracing::debug!(reason, "selected Kotlin compiler did not publish a result");
                }
                let (keys, proofs) = {
                    let mut ws = worker_ws.lock();
                    if let Some(snapshot) = environment.compiler_api.lock().ok().and_then(|mut snapshot| snapshot.take()) {
                        if let Err(error) = ws.accept_kotlin_compiler_api(&worker_request, environment.clone(), snapshot) {
                            tracing::debug!(%error, "current compiler API observation rejected");
                        }
                    }
                    let keys = ws.finish_kotlin_diagnostics(&worker_request, outcome, environment.invalid.clone());
                    let proofs: Vec<_> = keys.iter().filter_map(|key| ws.kotlin_diagnostic_trace(key)).collect();
                    (keys, proofs)
                };
                for mut proof in proofs {
                    if let ktcore::jvm::InputIdentity::Binary { content_sha256 } = &environment.input_identity {
                        let hash: String = content_sha256.iter().map(|byte| format!("{byte:02x}")).collect();
                        proof["compiler_inputs_sha256"] = serde_json::Value::String(hash);
                    }
                    crate::trace::span("kotlin_diagnostics_accepted", "compiler", start, proof);
                }
                keys
            }).await;
            match result {
                Ok(keys) => {
                    for key in keys {
                        publish_diagnostics(&ws, &client, &versions, &key).await;
                    }
                }
                Err(error) => {
                    request
                        .invalid
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::warn!(%error, "selected Kotlin diagnostic worker failed");
                }
            }
        }
    });
}

impl Backend {
    fn schedule_kotlin_diagnostics(&self) {
        if !*self.gradle_project.lock() {
            return;
        }
        let Some(root) = self.root.lock().clone() else {
            return;
        };
        schedule_selected_kotlin_diagnostics(
            &self.ws,
            &self.client,
            &self.doc_versions,
            &self.kotlin_diagnostics,
            &self.compile_workers,
            &self.watches,
            &root,
        );
    }
}

#[cfg(test)]
mod kotlin_diagnostic_queue_tests {
    use super::*;
    use crate::project_model::ProjectScope;

    #[test]
    fn environment_cache_miss_releases_state_before_workspace_fallback() {
        let state = Arc::new(Mutex::new(KotlinDiagnosticState::default()));
        let workspace = Arc::new(Mutex::new(Workspace::new()));
        let request = crate::compile::kotlin_test_request(Path::new("/diagnostic-lock-order"));
        // The invalidation callback holds Workspace before locking diagnostic state. Gate the
        // worker at the cache fallback while that callback's Workspace lock remains held.
        let workspace_guard = workspace.lock();
        let (entered, waiting) = std::sync::mpsc::sync_channel(0);
        let worker_state = state.clone();
        let worker_workspace = workspace.clone();
        let worker = std::thread::spawn(move || {
            cached_kotlin_diagnostic_environment(&worker_state, &request, || {
                entered.send(()).unwrap();
                worker_workspace.lock().compiler_api_environment(&request)
            })
        });
        waiting.recv().unwrap();
        let callback_can_lock_state = state.try_lock().is_some();
        drop(workspace_guard);
        assert!(worker.join().unwrap().is_none());
        assert!(callback_can_lock_state, "cache fallback holds diagnostic state while waiting for Workspace, blocking its invalidation callback");
    }

    #[test]
    fn busy_scope_keeps_latest_buffer_and_preserves_other_scope_progress() {
        let first = ProjectScope {
            module: ":first".into(),
            source_set: "main".into(),
        };
        let second = ProjectScope {
            module: ":second".into(),
            source_set: "main".into(),
        };
        let mut state = KotlinDiagnosticState::default();
        state.enqueue(first.clone(), "First.kt".into());
        state.enqueue(second.clone(), "Other.kt".into());
        for edit in 0..100 {
            state.enqueue(first.clone(), format!("Latest{edit}.kt"));
        }
        assert_eq!(state.pending.len(), 2);
        assert_eq!(
            state.order.len(),
            2,
            "edits do not create an unbounded JVM queue"
        );
        assert_eq!(state.next().as_deref(), Some("Latest99.kt"));
        assert_eq!(state.next().as_deref(), Some("Other.kt"));
        assert_eq!(state.next(), None);
        state.enqueue(first, "Closed.kt".into());
        state.enqueue(second.clone(), "Open.kt".into());
        state.retain_scopes(&std::collections::BTreeSet::from([second]));
        assert_eq!(state.next().as_deref(), Some("Open.kt"));
        assert_eq!(state.next(), None);
        state.stopped = true;
        state.enqueue(
            ProjectScope {
                module: ":stopped".into(),
                source_set: "main".into(),
            },
            "Ignored.kt".into(),
        );
        assert_eq!(state.next(), None);
    }
}

#[cfg(test)]
mod first_java_completion_seed_tests {
    use super::*;

    fn open_use(backend: &Backend, name: &str, source: &str) -> String {
        let key = crate::paths::file_key(&std::env::temp_dir().join(name).join("Use.java"));
        backend.ws.lock().open(key.clone(), source.to_string());
        backend
            .doc_versions
            .lock()
            .entry(key.clone())
            .or_default()
            .document_version = Some(1);
        key
    }

    fn pending_seed(backend: &Backend, key: &str) -> tokio::sync::watch::Sender<()> {
        let (published, ready) = tokio::sync::watch::channel(());
        let generation = backend.ws.lock().analysis_generation();
        backend
            .jdk_source_seeds
            .lock()
            .insert(key.to_string(), (generation, ready));
        published
    }

    fn params(key: &str, source: &str) -> CompletionParams {
        let offset = source.find("item.").unwrap() + "item.".len();
        let (line, character) = LineIndex::new(source).position(source, offset);
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: key_to_uri(key).unwrap(),
                },
                position: Position { line, character },
            },
            context: None,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    fn labels(response: Option<CompletionResponse>) -> Vec<String> {
        match response {
            Some(CompletionResponse::List(list)) => list.items,
            Some(CompletionResponse::Array(items)) => items,
            None => Vec::new(),
        }
        .into_iter()
        .map(|item| item.label)
        .collect()
    }

    fn publish_iterable(backend: &Backend, generation: u64) -> bool {
        // A minimal source fixture; the editor regression uses the actual selected JDK src.zip.
        let source =
            "package java.lang; public interface Iterable<T> { java.util.Iterator<T> iterator(); }";
        let mut java = crate::java::JavaParser::new();
        let tree = java.parse(source);
        let batch = crate::deps::FileSymbols {
            file: crate::paths::file_key(
                &std::env::temp_dir().join("ktlsp-first-seed-sdk/java/lang/Iterable.java"),
            ),
            symbols: crate::java::extract_symbols(&tree, source),
            jvm: None,
            library_source: None,
            origin: None,
        };
        let mut ws = backend.ws.lock();
        let indexed = ws.index_dependency_batch_for_generation(generation, batch);
        ws.bump_index_revision();
        indexed
    }

    const USE: &str = "package firstseed; class Use { void run(Iterable<Item> values) { for (var item : values) { item.; } } }";

    #[tokio::test]
    async fn first_unknown_java_completion_uses_the_pending_jdk_publication_once() {
        for (owner_name, owner, useful) in [
            (
                "Item.kt",
                "package firstseed\nclass Item { fun usefulKotlin() {} }",
                "usefulKotlin",
            ),
            (
                "Item.java",
                "package firstseed; class Item { public void usefulJava() {} }",
                "usefulJava",
            ),
        ] {
            let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
            drop(socket);
            let backend = service.inner();
            backend.ws.lock().open(owner_name, owner.to_string());
            let key = open_use(backend, owner_name, USE);
            let published = pending_seed(backend, &key);
            let generation = backend.ws.lock().analysis_generation();
            let mut response = Box::pin(backend.completion(params(&key, USE)));
            tokio::select! {
                biased;
                result = &mut response => panic!("first response escaped before actual source publication: {result:?}"),
                _ = tokio::task::yield_now() => {}
            }
            assert!(publish_iterable(backend, generation));
            drop(published);
            let got = labels(response.await.unwrap());
            assert!(
                got.iter().any(|label| label == useful),
                "first response: {got:?}"
            );
            assert!(backend.jdk_source_seeds.lock().is_empty());
            assert!(backend
                .ws
                .lock()
                .index
                .lookup_type("NotInJavaLang")
                .is_empty());
        }
    }

    #[tokio::test]
    async fn useful_java_completion_does_not_wait_for_pending_jdk_work() {
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        drop(socket);
        let backend = service.inner();
        backend.ws.lock().open(
            "Item.kt",
            "package firstseed\nclass Item { fun useful() {} }".to_string(),
        );
        let source = "package firstseed; class Use { void run(Item item) { item.; } }";
        let key = open_use(backend, "known-first-seed", source);
        let published = pending_seed(backend, &key);
        let mut response = Box::pin(backend.completion(params(&key, source)));
        let got = tokio::select! {
            biased;
            result = &mut response => labels(result.unwrap()),
            _ = tokio::task::yield_now() => panic!("a useful receiver waited for unrelated JDK work"),
        };
        assert!(got.iter().any(|label| label == "useful"), "{got:?}");
        assert!(published.receiver_count() > 0);
    }

    #[tokio::test]
    async fn failed_jdk_publication_does_not_invent_a_receiver() {
        let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
        drop(socket);
        let backend = service.inner();
        backend.ws.lock().open(
            "Item.kt",
            "package firstseed\nclass Item { fun useful() {} }".to_string(),
        );
        let key = open_use(backend, "failed-first-seed", USE);
        let published = pending_seed(backend, &key);
        drop(published);
        assert!(backend
            .completion(params(&key, USE))
            .await
            .unwrap()
            .is_none());
        assert!(backend.jdk_source_seeds.lock().is_empty());
        assert!(backend.ws.lock().index.lookup_type("Iterable").is_empty());
    }

    #[tokio::test]
    async fn jdk_wait_does_not_return_a_completion_for_an_edited_closed_or_stale_document() {
        for event in ["edit", "close", "reopen", "generation"] {
            let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
            drop(socket);
            let backend = service.inner();
            backend.ws.lock().open("Item.kt", "package firstseed\nclass Item { fun obsolete() {} }\nclass Other { fun current() {} }".to_string());
            let key = open_use(backend, event, USE);
            let published = pending_seed(backend, &key);
            let generation = backend.ws.lock().analysis_generation();
            let mut response = Box::pin(backend.completion(params(&key, USE)));
            tokio::select! {
                biased;
                result = &mut response => panic!("first response escaped publication: {result:?}"),
                _ = tokio::task::yield_now() => {}
            }
            let changed = USE.replace("Iterable<Item>", "Iterable<Other>");
            match event {
                "edit" => {
                    backend.ws.lock().change(&key, changed.clone());
                    backend
                        .doc_versions
                        .lock()
                        .get_mut(&key)
                        .unwrap()
                        .document_version = Some(2);
                }
                "close" | "reopen" => {
                    backend
                        .did_close(DidCloseTextDocumentParams {
                            text_document: TextDocumentIdentifier {
                                uri: key_to_uri(&key).unwrap(),
                            },
                        })
                        .await;
                    assert!(backend.jdk_source_seeds.lock().is_empty());
                    if event == "reopen" {
                        open_use(backend, event, &changed);
                    }
                }
                _ => {
                    backend.ws.lock().begin_analysis_generation();
                }
            }
            assert_eq!(publish_iterable(backend, generation), event != "generation");
            drop(published);
            assert!(
                response.await.unwrap().is_none(),
                "{event}: stale completion escaped"
            );
            assert!(backend.jdk_source_seeds.lock().is_empty());
            if event == "edit" || event == "reopen" {
                let got = labels(backend.completion(params(&key, &changed)).await.unwrap());
                assert!(got.iter().any(|label| label == "current"), "{got:?}");
                assert!(!got.iter().any(|label| label == "obsolete"), "{got:?}");
            }
        }
    }

    #[tokio::test]
    async fn jdk_publication_preserves_lexical_parameters_and_explicit_imports() {
        for source in [
            "package firstseed; class Iterable<T> {} class Use { void run(Iterable<Item> values) { for (var item : values) { item.; } } }",
            "package firstseed; class Use<Iterable> { void run(Iterable values) { for (var item : values) { item.; } } }",
            "package firstseed; import other.Iterable; class Use { void run(Iterable<Item> values) { for (var item : values) { item.; } } }",
        ] {
            let (service, socket) = tower_lsp_server::LspService::new(Backend::new);
            drop(socket);
            let backend = service.inner();
            backend.ws.lock().open(
                "Item.kt",
                "package firstseed\nclass Item { fun forbidden() {} }".to_string(),
            );
            backend.ws.lock().open(
                "Other.java",
                "package other; public class Iterable<T> {}".to_string(),
            );
            let key = open_use(backend, "lexical-first-seed", source);
            let published = pending_seed(backend, &key);
            let generation = backend.ws.lock().analysis_generation();
            assert!(publish_iterable(backend, generation));
            drop(published);
            assert!(
                backend
                    .completion(params(&key, source))
                    .await
                    .unwrap()
                    .is_none(),
                "{source}"
            );
            assert!(backend.jdk_source_seeds.lock().is_empty());
        }
    }
}

#[cfg(test)]
mod first_extension_source_tests {
    use super::*;
    use ktcore::{extensions::*, knowledge::Knowledge};
    use std::io::Write;

    static NEXT_FIXTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    const OWNER: &str = "package fixture; import org.spongepowered.asm.mixin.Shadow; public abstract class Consumer { @Shadow public abstract int bound(); }";
    const PLAIN: &str =
        "package fixture; class Plain { void ordinary() {} void run(Plain item) { item.ordinary(); } }";
    const PROVIDER: &str = "test.selected-shadow";
    const BACKLOG_TYPES: &[&str] = &[
        "arrayfixture.Factory",
        "arrayfixture.Item",
        "fieldfixture.FieldOnly",
        "genericfixture.Box",
        "genericfixture.Child",
        "genericfixture.Holder",
        "genericfixture.Item",
        "genericfixture.Other",
    ];
    macro_rules! backlog {
        ($($path:literal),+ $(,)?) => { &[$((
            $path,
            include_bytes!(concat!("../../ktcore/tests/fixtures/classfile/", $path, ".class")).as_slice(),
            include_bytes!(concat!("../../ktcore/tests/fixtures/classfile/sources/", $path, ".java")).as_slice(),
        )),+] };
    }
    const BACKLOG: &[(&str, &[u8], &[u8])] = backlog![
        "arrayfixture/Factory",
        "arrayfixture/Item",
        "fieldfixture/FieldOnly",
        "genericfixture/Box",
        "genericfixture/Child",
        "genericfixture/Holder",
        "genericfixture/Item",
        "genericfixture/Other",
    ];
    const SECOND_BACKLOG_TYPES: &[&str] = &[
        "protectedfixture.Position",
        "protectedfixture.ProtectedApi",
        "shadowfixture.Base",
        "shadowfixture.GenericMid",
        "shadowfixture.Marker",
        "shadowfixture.Mid",
        "shadowfixture.NestedBoundMid",
        "shadowfixture.PublicMid",
    ];
    const SECOND_BACKLOG: &[(&str, &[u8], &[u8])] = backlog![
        "protectedfixture/Position",
        "protectedfixture/ProtectedApi",
        "shadowfixture/Base",
        "shadowfixture/GenericMid",
        "shadowfixture/Marker",
        "shadowfixture/Mid",
        "shadowfixture/NestedBoundMid",
        "shadowfixture/PublicMid",
    ];
    struct Provider;
    impl AnalysisProvider for Provider {
        fn id(&self) -> &str {
            PROVIDER
        }
        fn analyze(&self, read: &mut ReadSession<'_>) -> std::result::Result<(), ContractError> {
            if read.source().contains("class Other") {
                for ty in BACKLOG_TYPES {
                    let _ = read.resolve_type(ty);
                }
            }
            if read.source().contains("sixteen background inputs") {
                for ty in SECOND_BACKLOG_TYPES {
                    let _ = read.resolve_type(ty);
                }
            }
            let direct = read
                .source()
                .split_once("// wait-for: ")
                .map(|(_, ty)| ty.trim().to_string());
            if let Some(ty) = direct {
                let _ = read.resolve_type(&ty);
            }
            for declaration in read.declarations().to_vec() {
                let shadow = declaration.annotations.iter().any(|annotation| {
                    matches!(read.resolve_type_ref(&annotation.ty), Knowledge::Found(ty)
                        if ty.qualified_name() == "org.spongepowered.asm.mixin.Shadow")
                });
                if !shadow {
                    continue;
                }
                let Knowledge::Found(owner) = read.resolve_type("fixture.Consumer") else {
                    continue;
                };
                let Knowledge::Found(target) = read.resolve_type("fixture.Target") else {
                    continue;
                };
                let Some(member) = read
                    .declared_members(&owner)?
                    .0
                    .into_iter()
                    .find(|member| member.id.definition() == &declaration.definition)
                else {
                    continue;
                };
                let range = declaration.definition.start_byte..declaration.definition.end_byte;
                let authored = read.declaration(range.clone())?;
                let authored_name = read.source()[range.clone()].to_string();
                let id = read.require(MemberRequirement {
                    target: Knowledge::Found(target),
                    authored: Some(authored),
                    names: MemberNames::Exact(vec![authored_name]),
                    kind: MemberKind::Method,
                    signature: member.signature,
                    dispatch: member.dispatch,
                    name_link: NameLink::Direct,
                })?;
                let found = read.resolution(id)?.binding.is_found();
                read.bind_site(id, range.clone(), range.clone(), RenameSubject::Target)?;
                if found {
                    read.diagnostic(ktcore::diagnostics::Diagnostic {
                        start_byte: range.start,
                        end_byte: range.end,
                        severity: ktcore::diagnostics::Severity::Hint,
                        code: None,
                        message: "selected-shadow-ready".into(),
                    })?;
                }
            }
            Ok(())
        }
    }

    struct Fixture {
        root: PathBuf,
        jar: PathBuf,
        key: String,
        target: String,
    }
    impl Fixture {
        fn new(backend: &Backend) -> Self {
            let root = std::env::temp_dir().join(format!(
                "ktlsp-first-extension-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT_FIXTURE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let src = root.join("src/main/java/fixture");
            std::fs::create_dir_all(&src).unwrap();
            for (name, text) in [
                ("Consumer.java", OWNER),
                (
                    "Target.java",
                    "package fixture; public class Target { public int bound() { return 1; } }",
                ),
                ("Plain.java", PLAIN),
                ("Other.java", "package fixture; class Other {}"),
            ] {
                std::fs::write(src.join(name), text).unwrap();
            }
            let jar = root.join("selected.jar");
            let mut binaries: Vec<_> = BACKLOG
                .iter()
                .map(|(path, bytes, _)| (format!("{path}.class"), *bytes))
                .collect();
            binaries.push((
                "org/spongepowered/asm/mixin/Shadow.class".into(),
                include_bytes!("../tests/fixtures/extension_sources/Shadow.class"),
            ));
            Self::zip(
                &jar,
                &binaries
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), *bytes))
                    .collect::<Vec<_>>(),
            );
            let mut sources: Vec<_> = BACKLOG
                .iter()
                .map(|(path, _, bytes)| (format!("{path}.java"), *bytes))
                .collect();
            sources.push((
                "org/spongepowered/asm/mixin/Shadow.java".into(),
                include_bytes!("../tests/fixtures/extension_sources/Shadow.java"),
            ));
            Self::zip(
                &root.join("selected-sources.jar"),
                &sources
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), *bytes))
                    .collect::<Vec<_>>(),
            );
            let key = crate::paths::file_key(&src.join("Consumer.java"));
            let target = crate::paths::file_key(&src.join("Target.java"));
            {
                let mut ws = backend.ws.lock();
                let generation = ws.begin_analysis_generation();
                ws.open(key.clone(), OWNER.into());
                let dump = format!(
                    "PROJECT\t:\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nEND\t:\n",
                    root.display(),
                    root.join("src/main/java").display(),
                    jar.display()
                );
                ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(
                    crate::classpath::parse_dump(&dump),
                )));
                assert_eq!(ws.scan_model_sources(), 3);
                assert!(ws.finish_analysis_generation(generation, true, true));
            }
            backend
                .doc_versions
                .lock()
                .entry(key.clone())
                .or_default()
                .document_version = Some(1);
            Self {
                root,
                jar,
                key,
                target,
            }
        }
        fn zip(path: &Path, entries: &[(&str, &[u8])]) {
            let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
            for (name, bytes) in entries {
                archive
                    .start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
        }
        fn index(&self, backend: &Backend, name: &str) {
            let batches = crate::deps::resolve_local_jar_stubs(
                &self.jar,
                &self.root.join("extract"),
                &mut crate::parser::KotlinParser::new(),
                &mut crate::java::JavaParser::new(),
            )
            .unwrap();
            let mut ws = backend.ws.lock();
            for batch in batches
                .into_iter()
                .filter(|batch| batch.file.ends_with(name))
            {
                assert!(batch.library_source.is_some());
                assert!(ws.index_dependency_batch(batch));
            }
            ws.bump_index_revision();
        }
        fn pending(&self, backend: &Backend) -> Vec<crate::analysis_extensions::SourceRequest> {
            let requests = backend.ws.lock().begin_extension_source_batch();
            assert_eq!(requests.len(), 1, "one exact selected annotation producer");
            requests
        }
        async fn prepare(
            requests: Vec<crate::analysis_extensions::SourceRequest>,
        ) -> Vec<crate::analysis_extensions::PreparedSource> {
            let expected = requests.len();
            let prepared = tokio::task::spawn_blocking(move || {
                crate::analysis_extensions::prepare_sources(requests)
            })
            .await
            .unwrap();
            assert_eq!(prepared.len(), expected);
            prepared
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    #[derive(Clone, Copy)]
    enum Feature {
        Completion,
        Definition,
        Diagnostic,
    }
    impl Feature {
        async fn request(self, backend: &Backend, key: &str) -> Result<serde_json::Value> {
            let offset = OWNER.find("bound").unwrap()
                + if matches!(self, Self::Completion) {
                    5
                } else {
                    0
                };
            let (line, character) = LineIndex::new(OWNER).position(OWNER, offset);
            let document = TextDocumentIdentifier {
                uri: key_to_uri(key).unwrap(),
            };
            let position = TextDocumentPositionParams {
                text_document: document.clone(),
                position: Position { line, character },
            };
            Ok(match self {
                Self::Completion => serde_json::to_value(
                    backend
                        .completion(CompletionParams {
                            text_document_position: position,
                            context: None,
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::Definition => serde_json::to_value(
                    backend
                        .goto_definition(GotoDefinitionParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::Diagnostic => serde_json::to_value(
                    backend
                        .diagnostic(DocumentDiagnosticParams {
                            text_document: document,
                            identifier: None,
                            previous_result_id: None,
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
            })
        }
        fn useful(self, value: &serde_json::Value, target: &str) -> bool {
            match self {
                Self::Completion => value
                    .get("items")
                    .and_then(|items| items.as_array())
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item["detail"] == "fixture.Target ()I")
                    }),
                Self::Definition => {
                    value
                        .get("uri")
                        .and_then(|uri| uri.as_str())
                        .is_some_and(|uri| {
                            uri.parse::<Uri>()
                                .ok()
                                .as_ref()
                                .and_then(uri_to_key)
                                .as_deref()
                                == Some(target)
                        })
                }
                Self::Diagnostic => value
                    .get("items")
                    .and_then(|items| items.as_array())
                    .is_some_and(|items| {
                        items
                            .iter()
                            .any(|item| item["message"] == "selected-shadow-ready")
                    }),
            }
        }
    }

    #[tokio::test]
    async fn first_extension_features_wait_for_selected_shadow_publication() {
        for feature in [
            Feature::Completion,
            Feature::Definition,
            Feature::Diagnostic,
        ] {
            let (service, socket) = tower_lsp_server::LspService::new(|client| {
                Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
            });
            drop(socket);
            let backend = service.inner();
            let fixture = Fixture::new(backend);
            fixture.index(backend, "Shadow.java");
            let requests = fixture.pending(backend);
            let shadow = backend
                .ws
                .lock()
                .index
                .lookup_type("Shadow")
                .into_iter()
                .find(|entry| entry.sym.package == "org.spongepowered.asm.mixin")
                .unwrap()
                .path
                .to_string();
            assert!(!Path::new(&shadow).exists());
            let mut response = Box::pin(feature.request(backend, &fixture.key));
            tokio::select! { biased; result = &mut response => panic!("first response escaped before selected source preparation: {result:?}"), _ = tokio::task::yield_now() => {} }
            let prepared = Fixture::prepare(requests).await;
            // Physical extraction alone does not certify accepted current facts or publication.
            assert!(Path::new(&shadow).is_file());
            tokio::select! { biased; result = &mut response => panic!("physical source write was mistaken for publication: {result:?}"), _ = tokio::task::yield_now() => {} }
            backend.ws.lock().finish_extension_source_batch(prepared);
            let result = response.await.unwrap();
            assert!(
                feature.useful(&result, &fixture.target),
                "first response: {result}"
            );
        }
    }

    #[tokio::test]
    async fn ninth_queued_extension_source_is_file_specific_and_advances_after_first_batch() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        fixture.index(backend, "");
        let unrelated = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(
            unrelated.len(),
            8,
            "the selected Shadow is queued beyond SOURCE_BATCH"
        );
        let plain = crate::paths::file_key(&fixture.root.join("src/main/java/fixture/Plain.java"));
        backend.ws.lock().open(plain.clone(), PLAIN.into());
        let offset = PLAIN.find("item.").unwrap() + 5;
        let (line, character) = LineIndex::new(PLAIN).position(PLAIN, offset);
        let mut ordinary = Box::pin(backend.completion(CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: key_to_uri(&plain).unwrap(),
                },
                position: Position { line, character },
            },
            context: None,
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }));
        let result = tokio::select! { biased; result=&mut ordinary => result.unwrap(), _=tokio::task::yield_now() => panic!("unrelated ordinary completion waited for archive work") };
        let result = serde_json::to_value(result).unwrap();
        assert!(result["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["label"] == "ordinary"));
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("queued selected producer escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        let prepared = Fixture::prepare(unrelated).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        let shadow = backend
            .ws
            .lock()
            .index
            .lookup_type("Shadow")
            .into_iter()
            .find(|entry| entry.sym.package == "org.spongepowered.asm.mixin")
            .unwrap()
            .path
            .to_string();
        assert!(
            !Path::new(&shadow).exists(),
            "the first eight actual files must exclude the selected ninth source"
        );
        tokio::select! { biased; result=&mut response => panic!("unrelated producer released selected waiter: {result:?}"), _=tokio::task::yield_now() => {} }
        // Resume the existing worker at its next-batch boundary, without retrying the feature RPC.
        schedule_extension_sources(&backend.ws, &backend.client, &backend.doc_versions);
        let result = response.await.unwrap();
        assert!(Path::new(&shadow).is_file());
        assert!(
            Feature::Definition.useful(&result, &fixture.target),
            "{result}"
        );
    }

    #[tokio::test]
    async fn ninth_queued_extension_source_closes_on_shutdown_before_its_batch_starts() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        fixture.index(backend, "");
        let first = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(first.len(), 8);
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("never-started selected source escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        backend.shutdown().await.unwrap();
        assert!(
            response.await.is_err(),
            "shutdown must close queued as well as running producers"
        );
        drop(first);
        backend.ws.lock().finish_extension_source_batch(Vec::new());
    }

    #[tokio::test]
    async fn failed_extension_source_is_unknown_and_not_retried_per_request() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        fixture.index(backend, "Shadow.java");
        let requests = fixture.pending(backend);
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("pending failed producer escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        drop(requests);
        backend.ws.lock().finish_extension_source_batch(Vec::new());
        let result = response.await.unwrap();
        assert!(!Feature::Definition.useful(&result, &fixture.target));
        let mut next = Box::pin(Feature::Definition.request(backend, &fixture.key));
        let result = tokio::select! { biased; result=&mut next => result.unwrap(), _=tokio::task::yield_now() => panic!("a failed exact source was awaited again") };
        assert!(!Feature::Definition.useful(&result, &fixture.target));
        assert!(backend.ws.lock().begin_extension_source_batch().is_empty());
    }

    #[tokio::test]
    async fn extension_source_wait_discards_stale_documents_and_shutdown_without_waiting_for_io() {
        for shutdown in [false, true] {
            let (service, socket) = tower_lsp_server::LspService::new(|client| {
                Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
            });
            drop(socket);
            let backend = service.inner();
            let fixture = Fixture::new(backend);
            fixture.index(backend, "Shadow.java");
            let requests = fixture.pending(backend);
            let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
            tokio::select! { biased; result=&mut response => panic!("pending source escaped: {result:?}"), _=tokio::task::yield_now() => {} }
            if shutdown {
                backend.shutdown().await.unwrap();
            } else {
                backend
                    .ws
                    .lock()
                    .change(&fixture.key, OWNER.replace("bound", "changed"));
                backend
                    .doc_versions
                    .lock()
                    .get_mut(&fixture.key)
                    .unwrap()
                    .document_version = Some(2);
                // The selected producer completes; the earlier document must never receive its result.
                let prepared = Fixture::prepare(requests).await;
                backend.ws.lock().finish_extension_source_batch(prepared);
                assert!(response.await.is_err());
                continue;
            }
            assert!(response.await.is_err());
            drop(requests);
            backend.ws.lock().finish_extension_source_batch(Vec::new());
        }
    }

    #[tokio::test]
    async fn canceled_extension_request_leaves_the_shared_source_producer_usable() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        fixture.index(backend, "Shadow.java");
        let requests = fixture.pending(backend);
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("pending source escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        drop(response);
        let prepared = Fixture::prepare(requests).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        assert!(Feature::Definition.useful(
            &Feature::Definition
                .request(backend, &fixture.key)
                .await
                .unwrap(),
            &fixture.target
        ));
    }
    impl Fixture {
        fn with_two_background_batches(backend: &Backend) -> Self {
            let fixture = Self::new(backend);
            let binary = fixture.root.join("background.jar");
            let binaries: Vec<_> = SECOND_BACKLOG
                .iter()
                .map(|(path, bytes, _)| (format!("{path}.class"), *bytes))
                .collect();
            Self::zip(
                &binary,
                &binaries
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), *bytes))
                    .collect::<Vec<_>>(),
            );
            let sources: Vec<_> = SECOND_BACKLOG
                .iter()
                .map(|(path, _, bytes)| (format!("{path}.java"), *bytes))
                .collect();
            Self::zip(
                &fixture.root.join("background-sources.jar"),
                &sources
                    .iter()
                    .map(|(path, bytes)| (path.as_str(), *bytes))
                    .collect::<Vec<_>>(),
            );
            std::fs::write(
                fixture.root.join("src/main/java/fixture/Other.java"),
                "package fixture; class Other { /* sixteen background inputs */ }",
            )
            .unwrap();
            {
                let mut ws = backend.ws.lock();
                let dump = format!("PROJECT\t:\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nSCP\tmain\t{}\nEND\t:\n",
                    fixture.root.display(), fixture.root.join("src/main/java").display(), fixture.jar.display(), binary.display());
                ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(
                    crate::classpath::parse_dump(&dump),
                )));
                ws.scan_model_sources();
                let generation = ws.analysis_generation();
                assert!(ws.finish_analysis_generation(generation, true, true));
            }
            // Distinct real artifacts may use different extraction roots; all background keys precede Shadow.
            for (jar, extract) in [(&binary, "extract/a"), (&fixture.jar, "extract/z")] {
                let batches = crate::deps::resolve_local_jar_stubs(
                    jar,
                    &fixture.root.join(extract),
                    &mut crate::parser::KotlinParser::new(),
                    &mut crate::java::JavaParser::new(),
                )
                .unwrap();
                let mut ws = backend.ws.lock();
                for batch in batches {
                    assert!(batch.library_source.is_some());
                    assert!(ws.index_dependency_batch(batch));
                }
                ws.bump_index_revision();
            }
            let shadow = Self::source_path(backend, "org.spongepowered.asm.mixin.Shadow");
            for ty in BACKLOG_TYPES.iter().chain(SECOND_BACKLOG_TYPES) {
                assert!(
                    Self::source_path(backend, ty) < shadow,
                    "fixture must prove global-order starvation"
                );
            }
            fixture
        }
        fn source_path(backend: &Backend, ty: &str) -> String {
            let (package, name) = ty.rsplit_once('.').unwrap();
            backend
                .ws
                .lock()
                .index
                .lookup_type(name)
                .into_iter()
                .find(|entry| entry.sym.package == package)
                .unwrap()
                .path
                .to_string()
        }
    }

    #[tokio::test]
    async fn active_extension_request_prioritizes_its_source_in_the_next_batch() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::with_two_background_batches(backend);
        let first = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(first.len(), 8);
        let shadow = Fixture::source_path(backend, "org.spongepowered.asm.mixin.Shadow");
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("pending feature escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        let prepared = Fixture::prepare(first).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        assert!(!Path::new(&shadow).exists());
        let second = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(second.len(), 8);
        let prepared = Fixture::prepare(second).await;
        assert!(
            Path::new(&shadow).is_file(),
            "the active source was postponed behind a second unrelated batch"
        );
        tokio::select! { biased; result=&mut response => panic!("physical write preceded acceptance: {result:?}"), _=tokio::task::yield_now() => {} }
        backend.ws.lock().finish_extension_source_batch(prepared);
        let result = response.await.unwrap();
        assert!(
            Feature::Definition.useful(&result, &fixture.target),
            "first response: {result}"
        );
        let remaining = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(
            remaining.len(),
            1,
            "foreground priority must leave the other background work scheduled"
        );
        let prepared = Fixture::prepare(remaining).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        assert!(backend.ws.lock().begin_extension_source_batch().is_empty());
        for ty in BACKLOG_TYPES.iter().chain(SECOND_BACKLOG_TYPES) {
            assert!(Path::new(&Fixture::source_path(backend, ty)).is_file());
        }
    }

    #[tokio::test]
    async fn canceled_extension_waiter_does_not_retain_queue_priority() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::with_two_background_batches(backend);
        let first = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(first.len(), 8);
        let shadow = Fixture::source_path(backend, "org.spongepowered.asm.mixin.Shadow");
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("pending feature escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        drop(response);
        let prepared = Fixture::prepare(first).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        let second = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(second.len(), 8);
        let prepared = Fixture::prepare(second).await;
        assert!(
            !Path::new(&shadow).exists(),
            "a canceled request retained its foreground position"
        );
        backend.ws.lock().finish_extension_source_batch(prepared);
        let last = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(last.len(), 1);
        let prepared = Fixture::prepare(last).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        assert!(Path::new(&shadow).is_file());
    }

    #[tokio::test]
    async fn active_extension_waiters_reserve_background_progress_in_each_batch() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::with_two_background_batches(backend);
        let first = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(first.len(), 8);
        let mut waiters = Vec::new();
        for (ordinal, ty) in BACKLOG_TYPES.iter().take(7).enumerate() {
            let key = crate::paths::file_key(
                &fixture
                    .root
                    .join(format!("src/main/java/fixture/Waiter{ordinal}.java")),
            );
            let mut ws = backend.ws.lock();
            ws.open(
                &key,
                format!("package fixture; class Waiter{ordinal} {{}} // wait-for: {ty}"),
            );
            waiters.push(
                ws.extension_source_wait(&key)
                    .expect("one exact source per active waiter"),
            );
        }
        waiters.push(
            backend
                .ws
                .lock()
                .extension_source_wait(&fixture.key)
                .unwrap(),
        );
        assert_eq!(waiters.len(), 8);
        let background = Fixture::source_path(backend, BACKLOG_TYPES[7]);
        let prepared = Fixture::prepare(first).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        let second = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(second.len(), 8);
        let prepared = Fixture::prepare(second).await;
        assert!(
            Path::new(&background).is_file(),
            "eight active sources starved the remaining background input"
        );
        backend.ws.lock().finish_extension_source_batch(prepared);
        let last = backend.ws.lock().begin_extension_source_batch();
        assert_eq!(last.len(), 1);
        let prepared = Fixture::prepare(last).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        assert!(waiters.iter().all(|ready| *ready.borrow()));
        assert!(backend.ws.lock().begin_extension_source_batch().is_empty());
    }
    #[derive(Clone, Copy)]
    enum ConsumerFeature {
        References,
        PrepareRename,
        Rename,
        Hover,
        CallHierarchy,
        Highlights,
        WorkspaceDiagnostics,
    }
    impl ConsumerFeature {
        async fn request(self, backend: &Backend, key: &str) -> Result<serde_json::Value> {
            let (line, character) =
                LineIndex::new(OWNER).position(OWNER, OWNER.find("bound").unwrap());
            let position = TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: key_to_uri(key).unwrap(),
                },
                position: Position { line, character },
            };
            Ok(match self {
                Self::References => serde_json::to_value(
                    backend
                        .references(ReferenceParams {
                            text_document_position: position,
                            context: ReferenceContext {
                                include_declaration: true,
                            },
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::PrepareRename => {
                    serde_json::to_value(backend.prepare_rename(position).await?).unwrap()
                }
                Self::Rename => serde_json::to_value(
                    backend
                        .rename(RenameParams {
                            text_document_position: position,
                            new_name: "renamed".into(),
                            work_done_progress_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::Hover => serde_json::to_value(
                    backend
                        .hover(HoverParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::CallHierarchy => serde_json::to_value(
                    backend
                        .prepare_call_hierarchy(CallHierarchyPrepareParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::Highlights => serde_json::to_value(
                    backend
                        .document_highlight(DocumentHighlightParams {
                            text_document_position_params: position,
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
                Self::WorkspaceDiagnostics => serde_json::to_value(
                    backend
                        .workspace_diagnostic(WorkspaceDiagnosticParams {
                            identifier: None,
                            previous_result_ids: Vec::new(),
                            work_done_progress_params: Default::default(),
                            partial_result_params: Default::default(),
                        })
                        .await?,
                )
                .unwrap(),
            })
        }
    }
    impl Fixture {
        async fn first_consumer_response(
            &self,
            backend: &Backend,
            feature: ConsumerFeature,
        ) -> serde_json::Value {
            self.index(backend, "Shadow.java");
            let requests = self.pending(backend);
            let mut response = Box::pin(feature.request(backend, &self.key));
            tokio::select! { biased; result=&mut response => panic!("extension consumer escaped before its selected source: {result:?}"), _=tokio::task::yield_now() => {} }
            let prepared = Self::prepare(requests).await;
            tokio::select! { biased; result=&mut response => panic!("extension consumer used a physical write before publication: {result:?}"), _=tokio::task::yield_now() => {} }
            backend.ws.lock().finish_extension_source_batch(prepared);
            response.await.unwrap()
        }
    }

    #[tokio::test]
    async fn first_extension_references_wait_for_selected_source_binding() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        let value = fixture
            .first_consumer_response(backend, ConsumerFeature::References)
            .await;
        let locations: Vec<Location> = serde_json::from_value(value).unwrap();
        assert_eq!(locations.len(), 2);
        let keys: std::collections::BTreeSet<_> = locations
            .iter()
            .map(|location| uri_to_key(&location.uri).unwrap())
            .collect();
        assert_eq!(
            keys,
            [fixture.key.clone(), fixture.target.clone()]
                .into_iter()
                .collect()
        );
        for location in locations {
            let key = uri_to_key(&location.uri).unwrap();
            let text = backend.ws.lock().doc_text(&key).unwrap();
            let index = LineIndex::new(&text);
            let start = index.offset(
                &text,
                location.range.start.line,
                location.range.start.character,
            );
            let end = index.offset(&text, location.range.end.line, location.range.end.character);
            assert_eq!(&text[start..end], "bound");
        }
    }

    #[tokio::test]
    async fn first_extension_prepare_rename_waits_for_selected_source_binding() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        let value = fixture
            .first_consumer_response(backend, ConsumerFeature::PrepareRename)
            .await;
        assert_eq!(value["placeholder"], "bound");
        let range: Range = serde_json::from_value(value["range"].clone()).unwrap();
        let index = LineIndex::new(OWNER);
        assert_eq!(
            &OWNER[index.offset(OWNER, range.start.line, range.start.character)
                ..index.offset(OWNER, range.end.line, range.end.character)],
            "bound"
        );
    }

    #[tokio::test]
    async fn first_extension_rename_waits_and_applies_consumer_and_target_edits() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        let value = fixture
            .first_consumer_response(backend, ConsumerFeature::Rename)
            .await;
        let edit: WorkspaceEdit = serde_json::from_value(value).unwrap();
        let changes = edit.changes.unwrap();
        assert_eq!(
            changes.len(),
            2,
            "rename must change the linked consumer and target"
        );
        let mut updated = HashMap::new();
        for (uri, edits) in changes {
            let key = uri_to_key(&uri).unwrap();
            assert!(key == fixture.key || key == fixture.target);
            assert_eq!(edits.len(), 1);
            let original = backend.ws.lock().doc_text(&key).unwrap();
            let index = LineIndex::new(&original);
            let edits: Vec<_> = edits
                .into_iter()
                .map(|edit| {
                    crate::edit::TextEdit::new(
                        &key,
                        index.offset(&original, edit.range.start.line, edit.range.start.character),
                        index.offset(&original, edit.range.end.line, edit.range.end.character),
                        edit.new_text,
                    )
                })
                .collect();
            let text = crate::edit::apply_to_text(&key, &original, &edits).unwrap();
            assert_eq!(text, original.replace("bound", "renamed"));
            assert!(!crate::java::JavaParser::new()
                .parse(&text)
                .root_node()
                .has_error());
            updated.insert(key, text);
        }
        let target = updated.remove(&fixture.target).unwrap();
        std::fs::write(&fixture.target, &target).unwrap();
        backend.ws.lock().reindex(&fixture.target, &target);
        let consumer = updated.remove(&fixture.key).unwrap();
        backend
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: key_to_uri(&fixture.key).unwrap(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: consumer.clone(),
                }],
            })
            .await;
        assert_eq!(backend.ws.lock().doc_text(&fixture.key).unwrap(), consumer);
        assert_eq!(std::fs::read_to_string(&fixture.target).unwrap(), target);
        let result = Feature::Definition
            .request(backend, &fixture.key)
            .await
            .unwrap();
        assert!(
            Feature::Definition.useful(&result, &fixture.target),
            "renamed binding must still navigate to the edited target: {result}"
        );
    }

    #[tokio::test]
    async fn first_extension_passive_consumers_wait_for_selected_binding() {
        for feature in [
            ConsumerFeature::Hover,
            ConsumerFeature::CallHierarchy,
            ConsumerFeature::Highlights,
        ] {
            let (service, socket) = tower_lsp_server::LspService::new(|client| {
                Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
            });
            drop(socket);
            let backend = service.inner();
            let fixture = Fixture::new(backend);
            let value = fixture.first_consumer_response(backend, feature).await;
            match feature {
                ConsumerFeature::Hover => assert!(
                    value["contents"]["value"]
                        .as_str()
                        .unwrap()
                        .contains("fixture.Target"),
                    "{value}"
                ),
                ConsumerFeature::CallHierarchy => {
                    let items: Vec<CallHierarchyItem> = serde_json::from_value(value).unwrap();
                    assert_eq!(items.len(), 1);
                    assert_eq!(
                        uri_to_key(&items[0].uri).as_deref(),
                        Some(fixture.target.as_str())
                    );
                }
                ConsumerFeature::Highlights => {
                    let items: Vec<DocumentHighlight> = serde_json::from_value(value).unwrap();
                    assert_eq!(items.len(), 1);
                    assert_eq!(
                        items[0].range.start.character as usize,
                        OWNER.find("bound").unwrap()
                    );
                }
                _ => unreachable!(),
            }
        }
    }

    #[tokio::test]
    async fn first_extension_workspace_diagnostics_wait_for_selected_binding() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        let value = fixture
            .first_consumer_response(backend, ConsumerFeature::WorkspaceDiagnostics)
            .await;
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["uri"], key_to_uri(&fixture.key).unwrap().as_str());
        assert!(items[0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["message"] == "selected-shadow-ready"));
    }

    #[tokio::test]
    async fn extension_source_wait_rejects_close_reopen_with_reused_version() {
        let (service, socket) = tower_lsp_server::LspService::new(|client| {
            Backend::with_analysis_providers(client, vec![Arc::new(Provider)])
        });
        drop(socket);
        let backend = service.inner();
        let fixture = Fixture::new(backend);
        fixture.index(backend, "Shadow.java");
        let requests = fixture.pending(backend);
        let generation = backend.ws.lock().analysis_generation();
        // Definition already uses the wait helper in the baseline, isolating its lifetime guard.
        let mut response = Box::pin(Feature::Definition.request(backend, &fixture.key));
        tokio::select! { biased; result=&mut response => panic!("selected source escaped: {result:?}"), _=tokio::task::yield_now() => {} }
        backend
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier {
                    uri: key_to_uri(&fixture.key).unwrap(),
                },
            })
            .await;
        backend
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: key_to_uri(&fixture.key).unwrap(),
                    language_id: "java".into(),
                    version: 1,
                    text: OWNER.replace("bound", "other"),
                },
            })
            .await;
        assert_eq!(backend.ws.lock().analysis_generation(), generation);
        assert_eq!(
            backend
                .doc_versions
                .lock()
                .get(&fixture.key)
                .unwrap()
                .document_version,
            Some(1)
        );
        let prepared = Fixture::prepare(requests).await;
        backend.ws.lock().finish_extension_source_batch(prepared);
        let error = response.await.expect_err(
            "the old document lifetime must not receive the reopened document's result",
        );
        assert_eq!(
            error.code,
            tower_lsp_server::jsonrpc::ErrorCode::ServerError(-32801)
        );
    }
}
