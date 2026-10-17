//! Demand-only archive source preparation. Filesystem access and parsing run outside the
//! workspace lock; acceptance checks the exact selected source and current indexed symbols.

use std::{path::Path, sync::Arc};

use ktcore::{
    extensions::source::{prepare_source_references, PreparedSourceReferences},
    index::{Index, Tier},
    project_model::{FileOrigin, SourceRepresentation},
    symbol::IndexedSymbol,
};
use tree_sitter::Tree;

use super::{AnalysisExtensions, SourceInput};
use crate::deps::LibrarySource;

const SOURCE_BATCH: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SourceStamp {
    archive: String,
    origin: FileOrigin,
    symbols: Vec<Arc<IndexedSymbol>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{java::JavaParser, parser::KotlinParser, workspace::Workspace};
    use ktcore::{
        diagnostics::{Diagnostic, Severity},
        extensions::*,
        knowledge::Knowledge,
    };
    use std::{
        fs,
        io::Write,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    const OWNER: &str =
        "package client; abstract class Owner { abstract arrayfixture.Item[][] mirror(); }";
    const FACTORY: &str =
        include_str!("../../../ktcore/tests/fixtures/classfile/sources/arrayfixture/Factory.java");
    const ITEM: &str =
        include_str!("../../../ktcore/tests/fixtures/classfile/sources/arrayfixture/Item.java");

    struct Probe;
    impl AnalysisProvider for Probe {
        fn id(&self) -> &str {
            "library-probe"
        }
        fn analyze(&self, read: &mut ReadSession<'_>) -> Result<(), ContractError> {
            let Some(authored) = read
                .declarations()
                .iter()
                .find(|d| {
                    read.source()
                        .get(d.definition.start_byte..d.definition.end_byte)
                        == Some("mirror")
                })
                .cloned()
            else {
                return Ok(());
            };
            let declaration =
                read.declaration(authored.definition.start_byte..authored.definition.end_byte)?;
            let target = read.resolve_type("arrayfixture.Factory");
            // Use the same source-backed member API as the external shadow consumer.
            let Knowledge::Found(owner) = read.resolve_type("client.Owner") else {
                return Err(ContractError::MissingSource);
            };
            let member = read
                .declared_members(&owner)?
                .0
                .into_iter()
                .find(|member| member.id.definition() == &authored.definition)
                .unwrap();
            let id = read.require(MemberRequirement {
                target,
                authored: Some(declaration),
                names: MemberNames::Exact(vec!["rows".into()]),
                kind: MemberKind::Method,
                signature: member.signature.clone(),
                dispatch: member.dispatch.clone(),
                name_link: NameLink::IndependentAlias,
            })?;
            let found = read.resolution(id)?.binding.is_found();
            read.bind_site(
                id,
                authored.definition.start_byte..authored.definition.end_byte,
                authored.definition.start_byte..authored.definition.end_byte,
                RenameSubject::Authored,
            )?;
            read.diagnostic(Diagnostic {
                start_byte: authored.definition.start_byte,
                end_byte: authored.definition.end_byte,
                severity: Severity::Hint,
                code: None,
                message: format!("library-probe:{found}"),
            })
        }
    }

    struct Fixture {
        root: PathBuf,
        binary: PathBuf,
        sources: PathBuf,
        extract: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "ktlsp-library-hydration-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&root).unwrap();
            let binary = root.join("fixture.jar");
            Self::zip(
                &binary,
                &[
                    (
                        "arrayfixture/Factory.class",
                        include_bytes!(
                            "../../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class"
                        ),
                    ),
                    (
                        "arrayfixture/Item.class",
                        include_bytes!(
                            "../../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class"
                        ),
                    ),
                ],
            );
            let result = Self {
                binary,
                sources: root.join("fixture-sources.jar"),
                extract: root.join("extract"),
                root,
            };
            result.write_sources("");
            result
        }
        fn zip(path: &Path, entries: &[(&str, &[u8])]) {
            let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
            for (name, bytes) in entries {
                zip.start_file(*name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        fn write_sources(&self, suffix: &str) {
            Self::zip(
                &self.sources,
                &[
                    (
                        "arrayfixture/Factory.java",
                        format!("{FACTORY}{suffix}").as_bytes(),
                    ),
                    ("arrayfixture/Item.java", ITEM.as_bytes()),
                ],
            );
        }
        fn index(&self, ws: &mut Workspace) -> Vec<String> {
            // Model refresh retires the prior selected representation before publishing its replacement.
            let old: std::collections::BTreeSet<_> = ["Factory", "Item"]
                .into_iter()
                .flat_map(|name| ws.index.lookup_type(name))
                .filter(|entry| entry.sym.package == "arrayfixture")
                .map(|entry| entry.path.to_string())
                .collect();
            for file in old {
                ws.remove_dependency_file(&file);
            }
            let batches = crate::deps::resolve_local_jar_stubs(
                &self.binary,
                &self.extract,
                &mut KotlinParser::new(),
                &mut JavaParser::new(),
            )
            .unwrap();
            assert_eq!(batches.len(), 2);
            let files = batches.iter().map(|b| b.file.clone()).collect();
            for batch in batches {
                assert!(ws.index_dependency_batch(batch));
            }
            // Dependency callers publish once after their complete indexed batch.
            ws.bump_index_revision();
            files
        }
        fn workspace(&self) -> Workspace {
            let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
            self.index(&mut ws);
            ws.open("Owner.java", OWNER.into());
            ws
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    fn bound(ws: &mut Workspace) -> bool {
        ws.diagnostics("Owner.java")
            .iter()
            .any(|d| d.message == "library-probe:true")
    }
    fn drain(ws: &mut Workspace) -> usize {
        let mut count = 0;
        loop {
            let requests = ws.begin_extension_source_batch();
            if requests.is_empty() {
                return count;
            }
            count += requests.len();
            let changed = ws.finish_extension_source_batch(prepare_sources(requests));
            assert!(
                changed
                    .iter()
                    .all(|key| !key.ends_with("Factory.java") && !key.ends_with("Item.java")),
                "closed archive facts must not schedule editor diagnostics: {changed:?}"
            );
            assert!(count <= 4, "same input must not be fetched repeatedly");
        }
    }

    #[test]
    fn archive_batch_preserves_existing_project_scope_and_conflicting_origin() {
        for project_scope in [
            None,
            Some(ktcore::project_model::ProjectScope {
                module: "current-project".into(),
                source_set: "main".into(),
            }),
        ] {
            let fixture = Fixture::new();
            let batches = crate::deps::resolve_local_jar_stubs(
                &fixture.binary,
                &fixture.extract,
                &mut KotlinParser::new(),
                &mut JavaParser::new(),
            )
            .unwrap();
            let selected = batches
                .iter()
                .find(|batch| batch.file.ends_with("Factory.java"))
                .unwrap();
            assert!(selected.library_source.is_some());
            let file = selected.file.clone();
            let existing = FileOrigin {
                origin_id: "project:existing-owner".into(),
                representation: SourceRepresentation::Source,
                project_scope,
            };
            assert_ne!(selected.origin.as_ref(), Some(&existing));
            let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
            ws.index.set_file_origin(&file, existing.clone());
            for batch in batches {
                assert!(ws.index_dependency_batch(batch));
            }
            assert_eq!(ws.index.file_origin(&file), Some(&existing));
            ws.open("Owner.java", OWNER.into());
            assert!(!bound(&mut ws));
            let expected_item = ws
                .index
                .lookup_type("Item")
                .into_iter()
                .find(|entry| entry.sym.package == "arrayfixture")
                .unwrap()
                .path
                .to_string();
            let requests = ws.begin_extension_source_batch();
            assert_eq!(
                requests
                    .iter()
                    .map(|request| request.file.clone())
                    .collect::<Vec<_>>(),
                vec![expected_item.clone()],
                "only the unconflicted Item may be requested"
            );
            let prepared = prepare_sources(requests);
            assert_eq!(
                prepared
                    .iter()
                    .map(|source| source.request.file.clone())
                    .collect::<Vec<_>>(),
                vec![expected_item],
                "Factory must never become an accepted source candidate"
            );
            ws.finish_extension_source_batch(prepared);
            assert_eq!(ws.index.file_origin(&file), Some(&existing));
            assert!(!bound(&mut ws));
            assert!(ws.begin_extension_source_batch().is_empty());
        }
    }

    #[test]
    fn actual_jar_sources_hydrate_nominal_type_and_member_through_the_host() {
        let fixture = Fixture::new();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws), "indexed symbols alone are incomplete");
        assert_eq!(drain(&mut ws), 2);
        assert!(bound(&mut ws));
        let target = ws.goto_definition("Owner.java", OWNER.find("mirror").unwrap());
        assert_eq!(target.len(), 1);
        assert!(target[0].file.ends_with("arrayfixture/Factory.java"));
        assert_eq!(target[0].start_byte, FACTORY.find("rows").unwrap());
        assert_eq!(drain(&mut ws), 0);
    }

    #[test]
    fn failed_archive_demand_survives_unrelated_edits_but_changed_archive_retries() {
        let fixture = Fixture::new();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws));
        fs::remove_file(&fixture.sources).unwrap();
        let requests = ws.begin_extension_source_batch();
        let expected_files: std::collections::BTreeSet<_> = ["Factory", "Item"]
            .into_iter()
            .map(|name| {
                ws.index
                    .lookup_type(name)
                    .into_iter()
                    .find(|entry| entry.sym.package == "arrayfixture")
                    .unwrap()
                    .path
                    .to_string()
            })
            .collect();
        assert_eq!(
            requests
                .iter()
                .map(|request| request.file.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected_files
        );
        assert_eq!(requests.len(), 2);
        ws.finish_extension_source_batch(prepare_sources(requests));
        assert!(ws.begin_extension_source_batch().is_empty());
        ws.open("Other.kt", "class Other { fun value() = 1 }".into());
        ws.change("Other.kt", "class Other { fun value() = 2 }".into());
        assert!(!bound(&mut ws));
        assert_eq!(
            drain(&mut ws),
            0,
            "no repeated materialization on unrelated keystrokes"
        );
        fixture.write_sources("\n// new selected source input\n");
        fixture.index(&mut ws);
        assert!(!bound(&mut ws));
        assert_eq!(drain(&mut ws), 2);
        assert!(bound(&mut ws));
    }

    #[test]
    fn disabled_and_stale_archive_results_are_discarded_then_current_demand_retries() {
        let fixture = Fixture::new();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws));
        let first = prepare_sources(ws.begin_extension_source_batch());
        assert!(!first.is_empty());
        ws.configure_analysis_provider("library-probe", false, Arc::from("{}"));
        ws.finish_extension_source_batch(first);
        assert!(ws.begin_extension_source_batch().is_empty());
        ws.configure_analysis_provider("library-probe", true, Arc::from("{}"));
        let old = prepare_sources(ws.begin_extension_source_batch());
        assert!(!old.is_empty());
        fixture.write_sources("\n// archive changed while batch was in flight\n");
        fixture.index(&mut ws);
        assert!(!bound(&mut ws));
        ws.finish_extension_source_batch(old);
        assert!(!bound(&mut ws));
        assert_eq!(drain(&mut ws), 2);
        assert!(bound(&mut ws));
    }

    #[test]
    fn actual_binary_stub_is_not_certified_as_complete_authored_source() {
        let fixture = Fixture::new();
        fs::remove_file(&fixture.sources).unwrap();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws));
        assert_eq!(drain(&mut ws), 0);
    }

    #[test]
    fn changed_indexed_symbol_facts_reject_an_inflight_source_reply() {
        let fixture = Fixture::new();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws));
        let prepared = prepare_sources(ws.begin_extension_source_batch());
        let expected_files: std::collections::BTreeSet<_> = ["Factory", "Item"]
            .into_iter()
            .map(|name| {
                ws.index
                    .lookup_type(name)
                    .into_iter()
                    .find(|entry| entry.sym.package == "arrayfixture")
                    .unwrap()
                    .path
                    .to_string()
            })
            .collect();
        assert_eq!(
            prepared
                .iter()
                .map(|source| source.request.file.clone())
                .collect::<std::collections::BTreeSet<_>>(),
            expected_files
        );
        assert_eq!(prepared.len(), 2);
        let file = prepared
            .iter()
            .find(|source| source.request.file.ends_with("Factory.java"))
            .unwrap()
            .request
            .file
            .clone();
        let symbols = ws
            .index
            .entries_for_file(&file)
            .into_iter()
            .filter(|entry| entry.sym.name != "rows")
            .map(|entry| (*entry.sym).clone())
            .collect();
        assert!(ws.index_dependency_file(&file, symbols));
        ws.bump_index_revision();
        assert!(!bound(&mut ws));
        ws.finish_extension_source_batch(prepared);
        assert!(!bound(&mut ws));
        let current = ws.begin_extension_source_batch();
        assert_eq!(current.len(), 1, "changed symbol input may be loaded again");
        assert_eq!(current[0].file, file);
        ws.finish_extension_source_batch(Vec::new());
    }

    #[test]
    fn current_unsaved_library_text_replaces_hydrated_facts_and_close_restores_disk_source() {
        let fixture = Fixture::new();
        let mut ws = fixture.workspace();
        assert!(!bound(&mut ws));
        assert_eq!(drain(&mut ws), 2);
        assert!(bound(&mut ws));
        let file = ws
            .index
            .lookup_type("Factory")
            .into_iter()
            .find(|entry| entry.sym.package == "arrayfixture")
            .unwrap()
            .path
            .to_string();
        ws.open(&file, FACTORY.into());
        ws.change(&file, FACTORY.replace(" rows()", " renamed()"));
        assert!(!bound(&mut ws));
        assert_eq!(
            drain(&mut ws),
            0,
            "archive load cannot overwrite authoritative unsaved text"
        );
        ws.change(&file, FACTORY.into());
        assert!(bound(&mut ws));
        ws.close(&file);
        assert!(bound(&mut ws));
        assert_eq!(drain(&mut ws), 0);
    }
}

pub(crate) struct SourceRequest {
    file: String,
    stamp: SourceStamp,
    epoch: u64,
    source: Arc<LibrarySource>,
}

pub(crate) struct PreparedSource {
    request: SourceRequest,
    text: Arc<str>,
    tree: Tree,
    references: PreparedSourceReferences,
}

/// No network lookup: the dependency resolver has already selected this exact source archive.
pub(crate) fn prepare_sources(requests: Vec<SourceRequest>) -> Vec<PreparedSource> {
    requests
        .into_iter()
        .filter_map(|request| {
            let path = Path::new(&request.file);
            if !request.source.is_current() || !crate::deps::materialize_source_file(path) {
                return None;
            }
            let text: Arc<str> = std::fs::read_to_string(path).ok()?.into();
            let tree = if request.file.ends_with(".java") {
                crate::java::JavaParser::new().parse(&text)
            } else {
                crate::parser::KotlinParser::new().parse(&text)
            };
            let mut local = Index::new();
            local.replace_file(
                &request.file,
                request
                    .stamp
                    .symbols
                    .iter()
                    .map(|symbol| (**symbol).clone())
                    .collect(),
                Tier::Volatile,
            );
            let references = prepare_source_references(&local, &request.file, text.clone(), &tree);
            if !request.source.is_current() {
                return None;
            }
            Some(PreparedSource {
                request,
                text,
                tree,
                references,
            })
        })
        .collect()
}

impl AnalysisExtensions {
    fn source_stamp(&self, index: &Index, file: &str) -> Option<SourceStamp> {
        let (origin, source) = self.library_sources.get(file)?;
        // Synthetic previews cannot certify an authored source inventory.
        if origin.representation != SourceRepresentation::Source || origin.project_scope.is_some() {
            return None;
        }
        if index.file_origin(file) != Some(origin) {
            return None;
        }
        let symbols: Vec<_> = index
            .entries_for_file(file)
            .into_iter()
            .map(|entry| entry.sym)
            .collect();
        (!symbols.is_empty()).then(|| SourceStamp {
            archive: source.identity(),
            origin: origin.clone(),
            symbols,
        })
    }

    pub(crate) fn select_library_source(
        &mut self,
        index: &Index,
        file: &str,
        origin: Option<FileOrigin>,
        source: Option<Arc<LibrarySource>>,
    ) {
        if !self.retain_all_sources {
            return;
        }
        if let (Some(origin), Some(source)) = (origin, source) {
            self.library_sources.insert(file.into(), (origin, source));
        } else {
            self.library_sources.remove(file);
        }
        if self.hydrated_sources.get(file) != self.source_stamp(index, file).as_ref() {
            self.forget_hydrated_source(file);
        }
    }

    pub(crate) fn take_source_requests(&mut self, index: &Index) -> Vec<SourceRequest> {
        if self.suspended {
            return Vec::new();
        }
        let mut requests = Vec::new();
        for file in self.store.requested_source_files() {
            let Some(stamp) = self.source_stamp(index, &file) else {
                continue;
            };
            if self.attempted_sources.get(&file) == Some(&stamp) {
                continue;
            }
            self.attempted_sources.insert(file.clone(), stamp.clone());
            requests.push(SourceRequest {
                source: self.library_sources[&file].1.clone(),
                file,
                stamp,
                epoch: self.source_epoch,
            });
            if requests.len() == SOURCE_BATCH {
                break;
            }
        }
        requests
    }

    pub(crate) fn accept_sources(&mut self, index: &Index, prepared: Vec<PreparedSource>) -> bool {
        let wanted = self.store.requested_source_files();
        let mut accepted = Vec::new();
        for source in prepared {
            let file = &source.request.file;
            if self.suspended
                || source.request.epoch != self.source_epoch
                || !wanted.contains(file)
                || self.source_stamp(index, file).as_ref() != Some(&source.request.stamp)
            {
                // A cancelled successful load may be demanded again after re-enabling the provider.
                if self.attempted_sources.get(file) == Some(&source.request.stamp) {
                    self.attempted_sources.remove(file);
                }
                continue;
            }
            self.inputs.insert(
                file.clone(),
                SourceInput {
                    text: source.text,
                    tree: source.tree,
                    compiled: None,
                    prepared_references: Some(source.references),
                },
            );
            self.hydrated_sources
                .insert(file.clone(), source.request.stamp);
            accepted.push(file.clone());
        }
        if accepted.is_empty() {
            return false;
        }
        for file in accepted {
            if self.normalize(index, &file).is_err() {
                self.forget_hydrated_source(&file);
            }
        }
        self.evaluate(index);
        true
    }

    fn forget_hydrated_source(&mut self, file: &str) {
        if self.hydrated_sources.remove(file).is_some() {
            self.inputs.remove(file);
            self.store.remove_source(file);
        }
    }

    pub(super) fn invalidate_hydrated_sources(&mut self) {
        self.source_epoch = self.source_epoch.wrapping_add(1);
        for (file, _) in std::mem::take(&mut self.hydrated_sources) {
            self.attempted_sources.remove(&file);
            self.inputs.remove(&file);
            self.store.remove_source(&file);
        }
    }
}
