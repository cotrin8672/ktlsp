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
            let declarations=read.declarations().to_vec();
            let Some(authored) = declarations
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
    fn requested_library_body_retains_current_command_but_not_staged_index_changes() {
        use serde_json::json;
        for staged_index in [false,true] {
            let fixture = Fixture::new();
            let mut ws = fixture.workspace();
            assert!(!bound(&mut ws));
            ws.register_ipc_provider_commands("body-command",vec!["fixture-child".into()],vec!["fixture.types".into()]).unwrap();
            let owner = ws.extension_source_input_identity("Owner.java").unwrap();
            let work = ws.begin_provider_command("body-command","Owner.java","fixture.types".into(),vec![],json!({})).unwrap();
            let params = json!({"evaluation":work.evaluation.to_string(),"queries":[
                {"op":"resolveType","name":"arrayfixture.Factory"},{"op":"resolveType","name":"arrayfixture.Item"}]});
            let missing = ws.ipc_batch(work.evaluation,"analysis/readFacts",params.clone()).unwrap();
            assert!(missing["facts"].as_array().unwrap().iter().all(|fact|fact["state"]=="unknown"));
            let namespace = ws.extension_test_inputs().1.store.namespace_revision();
            let capture = ws.begin_ipc_requested_source_capture(work.evaluation).unwrap();
            let prepared = crate::workspace::prepare_ipc_source_capture(capture);
            if staged_index {
                assert!(ws.index_dependency_file("/other/Added.java",vec![ktcore::symbol::IndexedSymbol::new(
                    "Added",ktcore::symbol::SymbolKind::Class,"other",None,6,11)]));
                assert!(ws.extension_test_inputs().1.pending_index);
            }
            assert!(ws.finish_ipc_source_capture(prepared)>0);
            let (_,host) = ws.extension_test_inputs();
            assert!(!host.pending_index,"real staged index changes must be flushed before retaining bodies");
            assert_eq!(host.store.namespace_revision(),namespace,"body retention must not invent new names");
            assert!(Arc::ptr_eq(&owner,&ws.extension_source_input_identity("Owner.java").unwrap()));
            assert!(bound(&mut ws),"committed MissingSource consumers must be reevaluated with member facts");
            if staged_index {
                assert!(!ws.ipc_evaluation_current(work.evaluation));
                assert_eq!(ws.reread_ipc_facts(work.evaluation,params).unwrap_err(),"Stale");
                continue;
            }
            assert!(ws.ipc_evaluation_current(work.evaluation));
            let found = ws.reread_ipc_facts(work.evaluation,params).unwrap();
            assert!(found["facts"].as_array().unwrap().iter().all(|fact|fact["state"]=="found"));
            let members = ws.ipc_batch(work.evaluation,"analysis/readFacts",json!({"evaluation":work.evaluation.to_string(),
                "queries":[{"op":"declaredMembers","target":found["facts"][0]["value"]["id"]}]})).unwrap();
            assert!(members["facts"][0]["members"].as_array().unwrap().iter()
                .any(|member|member["name"]=="rows" && member["signature"]["value"]["descriptor"]=="()[[Larrayfixture/Item;"),"{members}");
            let (_,host) = ws.extension_test_inputs();
            assert_eq!(host.finish_ipc_command(work.evaluation,Ok(found.clone())).unwrap(),found);
        }
    }

    #[test]
    fn library_body_with_unindexed_declarations_is_not_retained() {
        let fixture = Fixture::new();
        fixture.write_sources("\nclass Unindexed {}\n");
        let mut ws = fixture.workspace();
        let expected = ws.index.lookup_type("Factory").into_iter()
            .find(|entry|entry.sym.package=="arrayfixture").unwrap().path.to_string();
        let entries = ws.index.entries_for_file(&expected);
        assert!(entries.iter().any(|entry|entry.sym.name=="Unindexed"),"the real jar body must declare the extra type");
        let symbols = entries.into_iter().filter(|entry|entry.sym.name!="Unindexed")
            .map(|entry|(*entry.sym).clone()).collect();
        assert!(ws.index_dependency_file(&expected,symbols));
        ws.bump_index_revision();
        let requests = ws.begin_extension_source_batch();
        assert!(requests.iter().find(|request|request.file==expected).unwrap().stamp.symbols.iter()
            .all(|symbol|symbol.name!="Unindexed"),"the current index stamp must deliberately omit that type");
        let prepared = prepare_sources(requests);
        assert!(!prepared.iter().any(|source|source.request.file==expected),"body facts must match the complete indexed symbol inventory");
        ws.finish_extension_source_batch(prepared);
        assert!(!bound(&mut ws));
        let (_,host) = ws.extension_test_inputs();
        assert!(host.store.source_declarations(&expected).is_none());
        assert!(!host.inputs.contains_key(&expected));
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
        assert_eq!(drain(&mut ws), 2);
        assert!(!bound(&mut ws), "nominal hydration cannot certify preview members");
        let (index, host) = ws.extension_test_inputs();
        for name in ["Factory", "Item"] {
            let entry = index.lookup_type(name).into_iter()
                .find(|entry| entry.sym.package == "arrayfixture").unwrap();
            assert_eq!(index.file_origin(&entry.path).unwrap().representation, SourceRepresentation::Stub);
            assert_eq!(host.store.source_declarations(&entry.path), Some([].as_slice()));
            assert!(!host.inputs.contains_key(entry.path.as_ref()));
            let mut read = host.store.begin(index, "library-probe", "Owner.java").unwrap();
            let Knowledge::Found(ty) = read.resolve_type(&format!("arrayfixture.{name}")) else {
                panic!("decoded binary type identity must be available");
            };
            assert!(matches!(read.declared_members(&ty).unwrap().1, Knowledge::Unknown(_)));
        }
    }

    #[test]
    fn binary_annotation_refs_hydrate_exact_identity_without_authored_source_facts() {
        const MIXIN: &[u8] = include_bytes!("../../../ktcore/tests/fixtures/classfile/mixin087/Mixin.class");
        const INJECT: &[u8] = include_bytes!("../../../ktcore/tests/fixtures/classfile/mixin087/Inject.class");
        let fixture = Fixture::new();
        fs::remove_file(&fixture.sources).unwrap();
        Fixture::zip(&fixture.binary, &[
            ("org/spongepowered/asm/mixin/Mixin.class", MIXIN),
            ("org/spongepowered/asm/mixin/injection/Inject.class", INJECT),
            ("decoy/Mixin.class", MIXIN),
        ]);
        for (file, text) in [
            ("ItemMixin.java", "import net.minecraft.world.item.Item;\r\nimport org.spongepowered.asm.mixin.Mixin;\r\nimport org.spongepowered.asm.mixin.injection.Inject;\r\n@Mixin(Item.class) class ItemMixin { @Inject(method=\"is\") }\r\n"),
            ("ItemMixin.kt", "import org.spongepowered.asm.mixin.Mixin\nimport org.spongepowered.asm.mixin.injection.Inject\n@Mixin(Any::class) class ItemMixin { @Inject(method=[\"is\"]) fun probe() {} }"),
        ] {
            // The second language also exercises the warm symbol-cache path for this exact jar.
            let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
            let batches = crate::deps::resolve_local_jar_stubs(
                &fixture.binary, &fixture.extract, &mut KotlinParser::new(), &mut JavaParser::new(),
            ).unwrap();
            assert_eq!(batches.len(), 3);
            for batch in batches {
                assert!(batch.library_source.is_some());
                assert_eq!(batch.origin.as_ref().unwrap().representation, SourceRepresentation::Stub);
                assert!(!Path::new(&batch.file).is_file());
                assert!(ws.index_dependency_batch(batch));
            }
            ws.bump_index_revision();
            ws.open(file, text.into());
            let (index, host) = ws.extension_test_inputs();
            let mut read = host.store.begin(index, "library-probe", file).unwrap();
            let declarations = read.declarations().to_vec();
            let sites = read.annotation_sites().sites;
            assert!(declarations.iter().flat_map(|declaration| &declaration.annotations)
                .any(|annotation| annotation.ty.name == "Mixin"));
            assert!(sites.iter().any(|site| site.annotation.ty.name == "Inject"));
            let references: Vec<_> = declarations.iter().flat_map(|declaration| &declaration.annotations)
                .map(|annotation| annotation.ty.clone())
                .chain(sites.iter().map(|site| site.annotation.ty.clone())).collect();
            for reference in &references {
                assert!(matches!(read.resolve_type_ref(reference), Knowledge::Unknown(_)));
            }
            assert!(matches!(read.resolve_type("decoy.Mixin"), Knowledge::Unknown(_)));
            let pending = read.finish();
            host.store.commit(pending).unwrap();
            let requests = ws.begin_extension_source_batch();
            assert_eq!(requests.len(),3);
            let selections = crate::jar::class_selection_count();
            let scalar: std::collections::BTreeSet<_> = requests.iter()
                .filter(|request|request.source.has_binary_type(Path::new(&request.file)))
                .map(|request|request.file.clone()).collect();
            assert_eq!(crate::jar::class_selection_count()-selections,3);
            let selections = crate::jar::class_selection_count();
            let prepared = prepare_sources(requests);
            assert_eq!(crate::jar::class_selection_count()-selections, 1,
                "one accepted archive handle is selected once for all three requested types");
            assert_eq!(prepared.iter().map(|source|source.request.file.clone()).collect::<std::collections::BTreeSet<_>>(),scalar);
            assert_eq!(prepared.len(),2,"the decoy's different internal name must stay unproved");
            ws.finish_extension_source_batch(prepared);
            assert!(ws.begin_extension_source_batch().is_empty(),"accepted batch must not repeat its fetches");
            let (index, host) = ws.extension_test_inputs();
            let mut read = host.store.begin(index, "library-probe", file).unwrap();
            let mut resolved_files = std::collections::BTreeSet::new();
            for reference in &references {
                let expected = match reference.name.as_str() {
                    "Mixin" => "org.spongepowered.asm.mixin.Mixin",
                    "Inject" => "org.spongepowered.asm.mixin.injection.Inject",
                    name => panic!("unexpected annotation {name}"),
                };
                let Knowledge::Found(ty) = read.resolve_type_ref(reference) else {
                    panic!("{file}: indexed binary annotation {expected} must resolve");
                };
                assert_eq!(ty.qualified_name(), expected);
                assert!(matches!(read.declared_members(&ty).unwrap().1, Knowledge::Unknown(_)));
                assert_eq!(index.file_origin(&ty.definition().file).unwrap().representation, SourceRepresentation::Stub);
                resolved_files.insert(ty.definition().file.clone());
            }
            assert!(matches!(read.resolve_type("decoy.Mixin"), Knowledge::Unknown(_)),
                "a class entry with a different decoded internal name cannot attest the preview");
            drop(read);
            for dependency in resolved_files {
                assert_eq!(host.store.source_declarations(&dependency), Some([].as_slice()));
                assert!(!host.inputs.contains_key(&dependency));
            }

            host.register_ipc_commands(index, "annotation-wire", vec!["unused-test-child".into()],
                vec!["fixture.annotations".into()]).unwrap();
            let work = host.begin_ipc_command(index, None, "annotation-wire", file,
                "fixture.annotations".into(), Vec::new(), serde_json::json!({})).unwrap();
            // Simulate the existing capability handshake; retain the command's real read lifetime.
            let mut evaluation = host.ipc_evaluations.remove(&work.evaluation).unwrap();
            evaluation.project_operations = u32::MAX;
            let pending = evaluation.pending.take().unwrap();
            let mut read = host.store.resume(index, file, pending).unwrap();
            let packet = evaluation.batch(&mut read, "analysis/readFacts", serde_json::json!({
                "evaluation":work.evaluation.to_string(),
                "queries":[{"op":"sourceFacts"},{"op":"annotationSites"}]
            })).unwrap();
            evaluation.pending = Some(read.finish());
            host.ipc_evaluations.insert(work.evaluation, evaluation);
            let references: Vec<_> = packet["facts"][0]["declarations"].as_array().unwrap().iter()
                .flat_map(|declaration| declaration["annotations"].as_array().unwrap())
                .chain(packet["facts"][1]["annotations"].as_array().unwrap())
                .map(|annotation| serde_json::json!({"op":"resolveTypeRef","reference":annotation["typeRef"]}))
                .collect();
            assert!(references.len() >= 3, "both sourceFacts and orphan-capable sites issue type refs");
            let result = host.ipc_batch(index, work.evaluation, "analysis/readFacts", serde_json::json!({
                "evaluation":work.evaluation.to_string(),"queries":references,
            })).unwrap();
            for fact in result["facts"].as_array().unwrap() {
                assert_eq!(fact["state"], "found");
                assert!(matches!(fact["value"]["qualifiedName"].as_str(),
                    Some("org.spongepowered.asm.mixin.Mixin" | "org.spongepowered.asm.mixin.injection.Inject")));
            }
            assert!(host.ipc_current(work.evaluation));
            assert_eq!(host.finish_ipc_command(work.evaluation, Ok(result.clone())).unwrap(), result);
            assert!(ws.begin_extension_source_batch().is_empty());
            // Re-run from the original lazy state, retaining the shared parsed-symbol cache.
            for entry in ws.index.lookup_type("Mixin").into_iter().chain(ws.index.lookup_type("Inject")) {
                let _ = fs::remove_file(entry.path.as_ref());
            }
        }
        let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
        for batch in crate::deps::resolve_local_jar_stubs(
            &fixture.binary, &fixture.extract, &mut KotlinParser::new(), &mut JavaParser::new(),
        ).unwrap() { assert!(ws.index_dependency_batch(batch)); }
        ws.bump_index_revision();
        ws.open("Changed.java", "class Changed {}".into());
        let (index, host) = ws.extension_test_inputs();
        let mut read = host.store.begin(index,"library-probe","Changed.java").unwrap();
        assert!(matches!(read.resolve_type("org.spongepowered.asm.mixin.Mixin"),Knowledge::Unknown(_)));
        let pending = read.finish();
        host.store.commit(pending).unwrap();
        let requests = ws.begin_extension_source_batch();
        assert_eq!(requests.len(),1);
        let source = requests[0].source.clone();
        assert!(source.content_is_current());
        let modified = fs::metadata(&fixture.binary).unwrap().modified().unwrap();
        let mut bytes = fs::read(&fixture.binary).unwrap();
        bytes[0] ^= 1;
        fs::write(&fixture.binary, bytes).unwrap();
        fs::File::options().write(true).open(&fixture.binary).unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified)).unwrap();
        assert!(source.is_current(), "the metadata-only identity is deliberately unchanged");
        assert!(!source.content_is_current(), "binary hydration must also check selected content");
        let selections = crate::jar::class_selection_count();
        assert!(prepare_sources(requests).is_empty(),"changed bytes cannot reuse an earlier batch's proof");
        assert_eq!(crate::jar::class_selection_count(),selections,"reject changed content before archive selection");
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
    #[test]
    fn selected_source_producers_wake_and_replace_at_unchanged_epoch() {
        let fixture = Fixture::new();
        let mut index = Index::new();
        let batch = crate::deps::resolve_local_jar_stubs(
            &fixture.binary,
            &fixture.extract,
            &mut KotlinParser::new(),
            &mut JavaParser::new(),
        )
        .unwrap()
        .into_iter()
        .find(|batch| batch.file.ends_with("Factory.java"))
        .unwrap();
        index.replace_file(&batch.file, batch.symbols.clone(), Tier::Durable);
        index.set_file_origin(&batch.file, batch.origin.clone().unwrap());
        let mut host = AnalysisExtensions::new(vec![Arc::new(Probe)]);
        host.select_library_source(
            &index,
            &batch.file,
            batch.origin.clone(),
            batch.library_source.clone(),
        );
        let epoch = host.source_epoch;
        let stamp = host.source_stamp(&index, &batch.file).unwrap();
        let selected = std::collections::BTreeSet::from([batch.file.clone()]);
        assert!(host.source_waits_for(&index, &selected).is_empty());
        assert!(host.source_producers.is_empty());
        let old = host
            .source_producer(&batch.file, stamp.clone())
            .ready
            .subscribe();
        assert!(host.source_waits_for(&index, &selected).is_empty(), "queued work is not an attempted input");
        host.attempted_sources
            .insert(batch.file.clone(), stamp.clone());
        let waiting = host.source_waits_for(&index, &selected);
        assert_eq!(waiting.len(), 1);
        assert!(waiting[0].same_channel(&old));
        assert!(host.source_waits_for(&index, &std::collections::BTreeSet::from(["unrelated.java".into()])).is_empty());
        assert_eq!(host.attempted_sources.len(), 1);
        // Re-publishing identical authority must preserve the active producer and its attempt.
        host.select_library_source(
            &index,
            &batch.file,
            batch.origin.clone(),
            batch.library_source.clone(),
        );
        assert!(!*old.borrow());
        assert_eq!(host.attempted_sources.get(&batch.file), Some(&stamp));
        // The selected representation can disappear without advancing the model generation.
        host.select_library_source(&index, &batch.file, None, None);
        assert!(*old.borrow());
        assert!(host.source_waits_for(&index, &selected).is_empty());
        assert_eq!(host.source_epoch, epoch);
        assert!(!host.attempted_sources.contains_key(&batch.file));
        host.select_library_source(
            &index,
            &batch.file,
            batch.origin.clone(),
            batch.library_source.clone(),
        );
        let fresh = host.source_producer(&batch.file, stamp).ready.subscribe();
        assert!(!old.same_channel(&fresh));
        assert!(!*fresh.borrow());
        // A changed indexed symbol generation is also an exact authority change, not a global wait.
        let mut changed = batch.symbols.clone();
        changed[0].start_byte += 1;
        index.replace_file(&batch.file, changed, Tier::Durable);
        host.select_library_source(&index, &batch.file, batch.origin, batch.library_source);
        assert!(*fresh.borrow());
        assert_eq!(host.source_epoch, epoch);
    }

    #[test]
    fn forgotten_source_producers_wake_queued_and_running_waiters_at_unchanged_epoch() {
        for running in [false, true] {
            let fixture = Fixture::new();
            let mut index = Index::new();
            let batch = crate::deps::resolve_local_jar_stubs(
                &fixture.binary,
                &fixture.extract,
                &mut KotlinParser::new(),
                &mut JavaParser::new(),
            )
            .unwrap()
            .into_iter()
            .find(|batch| batch.file.ends_with("Factory.java"))
            .unwrap();
            index.replace_file(&batch.file, batch.symbols, Tier::Durable);
            index.set_file_origin(&batch.file, batch.origin.clone().unwrap());
            let mut host = AnalysisExtensions::new(vec![Arc::new(Probe)]);
            host.select_library_source(&index, &batch.file, batch.origin, batch.library_source);
            let epoch = host.source_epoch;
            let stamp = host.source_stamp(&index, &batch.file).unwrap();
            let ready = host
                .source_producer(&batch.file, stamp.clone())
                .ready
                .subscribe();
            if running {
                host.attempted_sources.insert(batch.file.clone(), stamp);
            }
            host.forget_hydrated_source(&batch.file);
            assert!(*ready.borrow());
            assert!(!host.source_producers.contains_key(&batch.file));
            assert!(!host.attempted_sources.contains_key(&batch.file));
            assert_eq!(host.source_epoch, epoch);
        }
    }
}

pub(super) struct SourceProducer {
    stamp: SourceStamp,
    epoch: u64,
    ready: tokio::sync::watch::Sender<bool>,
}

// A failed/panicking load must release its exact waiters too; success stays owned until acceptance.
struct SourceSignal(tokio::sync::watch::Sender<bool>);
impl Drop for SourceSignal {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}

pub(crate) struct SourceRequest {
    file: String,
    stamp: SourceStamp,
    epoch: u64,
    source: Arc<LibrarySource>,
    _signal: SourceSignal,
}

pub(crate) struct PreparedSource {
    request: SourceRequest,
    text: Arc<str>,
    tree: Tree,
    references: PreparedSourceReferences,
}

/// No network lookup: the dependency resolver has already selected this exact source archive.
pub(crate) fn prepare_sources(requests: Vec<SourceRequest>) -> Vec<PreparedSource> {
    let mut archives: Vec<(&Arc<LibrarySource>, Vec<&Path>)> = Vec::new();
    for request in &requests {
        if request.stamp.origin.representation != SourceRepresentation::Stub { continue; }
        if let Some((_,files)) = archives.iter_mut().find(|(source,_)|Arc::ptr_eq(source,&request.source)) {
            files.push(Path::new(&request.file));
        } else {
            archives.push((&request.source,vec![Path::new(&request.file)]));
        }
    }
    // Reuse selection only within this batch and this exact accepted archive handle.
    let mut binary_types = std::collections::BTreeMap::new();
    for (source,files) in archives {
        if source.content_is_current() { binary_types.insert(Arc::as_ptr(source),source.binary_types(&files)); }
    }
    requests
        .into_iter()
        .filter_map(|request| {
            let path = Path::new(&request.file);
            // A name-only preview is not proof that the selected archive declares this type.
            if request.stamp.origin.representation == SourceRepresentation::Stub
                && (!request.source.content_is_current()
                    || !binary_types.get(&Arc::as_ptr(&request.source)).is_some_and(|files|files.contains(path)))
            {
                return None;
            }
            if !request.source.is_current() || !crate::deps::materialize_source_file(path) {
                return None;
            }
            let text: Arc<str> = std::fs::read_to_string(path).ok()?.into();
            let tree = if request.file.ends_with(".java") {
                crate::java::JavaParser::new().parse(&text)
            } else {
                crate::parser::KotlinParser::new().parse(&text)
            };
            if request.stamp.origin.representation == SourceRepresentation::Source {
                let language = crate::language::SourceLanguage::for_project_path(path)?;
                let mut actual = crate::language::symbol_facts(language,&tree,&text).symbols;
                let mut expected: Vec<_> = request.stamp.symbols.iter().map(|symbol|symbol.as_ref()).collect();
                actual.sort_unstable_by_key(|symbol|(symbol.start_byte,symbol.end_byte));
                expected.sort_unstable_by_key(|symbol|(symbol.start_byte,symbol.end_byte));
                if !actual.iter().eq(expected) { return None; }
            }
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
            if !request.source.is_current() || !request.source.content_is_current() {
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
        // Stub identities are usable, but never acquire authored project-source ownership.
        if !matches!(origin.representation, SourceRepresentation::Source | SourceRepresentation::Stub)
            || origin.project_scope.is_some()
        {
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
        if let (Some(origin), Some(source)) = (origin, source) {
            self.library_sources.insert(file.into(), (origin, source));
        } else {
            self.library_sources.remove(file);
        }
        // Providers may register after indexing; retain archive handles without loading text.
        if !self.retain_all_sources {
            return;
        }
        let stamp = self.source_stamp(index, file);
        if self.source_producers.get(file).is_some_and(|producer| {
            Some(&producer.stamp) != stamp.as_ref() || producer.epoch != self.source_epoch
        }) {
            self.forget_source_producer(file);
        }
        if self
            .hydrated_sources
            .get(file)
            .is_some_and(|hydrated| Some(hydrated) != stamp.as_ref())
        {
            self.forget_hydrated_source(file);
        }
    }

    fn source_producer(&mut self, file: &str, stamp: SourceStamp) -> &mut SourceProducer {
        let replace = self
            .source_producers
            .get(file)
            .is_none_or(|producer| producer.stamp != stamp || producer.epoch != self.source_epoch);
        if replace {
            if let Some(old) = self.source_producers.remove(file) {
                old.ready.send_replace(true);
            }
            let (ready, _) = tokio::sync::watch::channel(false);
            self.source_producers.insert(
                file.into(),
                SourceProducer {
                    stamp,
                    epoch: self.source_epoch,
                    ready,
                },
            );
        }
        self.source_producers.get_mut(file).unwrap()
    }

    pub(crate) fn source_wait(
        &mut self,
        index: &Index,
        owner: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        if self.suspended {
            return None;
        }
        for file in self.store.requested_source_files_for(owner) {
            let Some(stamp) = self.source_stamp(index, &file) else {
                continue;
            };
            if self.attempted_sources.get(&file) == Some(&stamp) {
                // A completed failed input stays Unknown instead of retrying on every request.
                if !self.source_producers.get(&file).is_some_and(|producer| {
                    producer.stamp == stamp
                        && producer.epoch == self.source_epoch
                        && !*producer.ready.borrow()
                }) {
                    continue;
                }
            }
            let producer = self.source_producer(&file, stamp);
            let ready = producer.ready.subscribe();
            let pending = !*ready.borrow();
            if pending {
                return Some(ready);
            }
        }
        None
    }

    pub(crate) fn take_source_requests(&mut self, index: &Index) -> Vec<SourceRequest> {
        self.take_source_requests_matching(index, None)
    }

    pub(crate) fn source_waits_for(
        &self,
        index: &Index,
        selected: &std::collections::BTreeSet<String>,
    ) -> Vec<tokio::sync::watch::Receiver<bool>> {
        if self.suspended {
            return Vec::new();
        }
        selected.iter().filter_map(|file| {
            let stamp = self.source_stamp(index, file)?;
            let producer = self.source_producers.get(file)?;
            (self.attempted_sources.get(file) == Some(&stamp)
                && producer.stamp == stamp
                && producer.epoch == self.source_epoch
                && !*producer.ready.borrow())
                .then(|| producer.ready.subscribe())
        }).collect()
    }

    pub(crate) fn take_source_requests_for(
        &mut self,
        index: &Index,
        selected: &std::collections::BTreeSet<String>,
    ) -> Vec<SourceRequest> {
        self.take_source_requests_matching(index, Some(selected))
    }

    fn take_source_requests_matching(
        &mut self,
        index: &Index,
        selected: Option<&std::collections::BTreeSet<String>>,
    ) -> Vec<SourceRequest> {
        if self.suspended {
            return Vec::new();
        }
        let mut requests = Vec::new();
        let mut wanted = self.store.requested_source_files();
        wanted.extend(self.wanted_ipc_sources());
        let obsolete: Vec<_> = self
            .source_producers
            .keys()
            .filter(|file| !wanted.contains(*file))
            .cloned()
            .collect();
        for file in obsolete {
            if let Some(producer) = self.source_producers.remove(&file) {
                producer.ready.send_replace(true);
            }
        }
        if let Some(selected) = selected {
            wanted.retain(|file| selected.contains(file));
        }
        let priority: std::collections::BTreeSet<_> = self
            .source_producers
            .iter()
            .filter(|(_, producer)| {
                producer.ready.receiver_count() > 0 && !*producer.ready.borrow()
            })
            .map(|(file, _)| file.clone())
            .collect();
        let mut foreground = wanted.iter().filter(|file| priority.contains(*file));
        let mut background = wanted.iter().filter(|file| !priority.contains(*file));
        while requests.len() < SOURCE_BATCH {
            // Serve active requests first while reserving one slot for background progress.
            let next = if requests.len() + 1 == SOURCE_BATCH {
                background.next().or_else(|| foreground.next())
            } else {
                foreground.next().or_else(|| background.next())
            };
            let Some(file) = next.cloned() else {
                break;
            };
            let Some(stamp) = self.source_stamp(index, &file) else {
                continue;
            };
            if self.attempted_sources.get(&file) == Some(&stamp) {
                continue;
            }
            self.attempted_sources.insert(file.clone(), stamp.clone());
            let producer = self.source_producer(&file, stamp.clone());
            let signal = SourceSignal(producer.ready.clone());
            requests.push(SourceRequest {
                source: self.library_sources[&file].1.clone(),
                file,
                stamp,
                epoch: self.source_epoch,
                _signal: signal,
            });
        }
        requests
    }

    pub(crate) fn accept_sources(&mut self, index: &Index, prepared: Vec<PreparedSource>) -> bool {
        if prepared.is_empty() { return false; }
        self.flush_index_changed(index);
        let mut wanted = self.store.requested_source_files();
        wanted.extend(self.wanted_ipc_sources());
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
            // The body matched this complete symbol stamp during preparation, and the
            // current origin/symbols/epoch above prove no indexed names changed since then.
            let retain_namespace = source.request.stamp.origin.representation == SourceRepresentation::Source
                && self.store.source_declarations(file).is_none();
            if source.request.stamp.origin.representation == SourceRepresentation::Stub {
                // Only nominal identities: preview declarations/members are not authored facts.
                if self.store.replace_prepared_source_declarations(
                    index,
                    source.references,
                    ktcore::knowledge::Knowledge::Unknown(vec![
                        ktcore::extensions::IncompleteReason::UnsupportedSource,
                    ]),
                    Vec::new(),
                ).is_err() {
                    continue;
                }
            } else {
                self.inputs.insert(
                    file.clone(),
                    SourceInput {
                        text: source.text,
                        tree: source.tree,
                        compiled: None,
                        compiler_classes: Vec::new(),
                        prepared_references: Some(source.references),
                    },
                );
            }
            self.hydrated_sources
                .insert(file.clone(), source.request.stamp);
            accepted.push((file.clone(),retain_namespace));
        }
        if accepted.is_empty() {
            return false;
        }
        for (file,retain_namespace) in accepted {
            if self.normalize_input(index,&file,false,retain_namespace).is_err() {
                self.forget_hydrated_source(&file);
            }
        }
        self.evaluate(index);
        true
    }

    pub(super) fn forget_source_producer(&mut self, file: &str) {
        if let Some(producer) = self.source_producers.remove(file) {
            if !*producer.ready.borrow()
                && self.attempted_sources.get(file) == Some(&producer.stamp)
            {
                self.attempted_sources.remove(file);
            }
            producer.ready.send_replace(true);
        }
    }

    fn forget_hydrated_source(&mut self, file: &str) {
        self.forget_source_producer(file);
        if self.hydrated_sources.remove(file).is_some() {
            self.inputs.remove(file);
            self.store.remove_source(file);
        }
    }

    pub(super) fn invalidate_hydrated_sources(&mut self) {
        self.source_epoch = self.source_epoch.wrapping_add(1);
        for (file, producer) in std::mem::take(&mut self.source_producers) {
            if !*producer.ready.borrow()
                && self.attempted_sources.get(&file) == Some(&producer.stamp)
            {
                self.attempted_sources.remove(&file);
            }
            producer.ready.send_replace(true);
        }
        for (file, _) in std::mem::take(&mut self.hydrated_sources) {
            self.attempted_sources.remove(&file);
            self.inputs.remove(&file);
            self.store.remove_source(&file);
        }
    }
}
