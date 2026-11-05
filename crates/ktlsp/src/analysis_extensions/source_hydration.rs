//! Demand-only archive source preparation. Filesystem access and parsing run outside the
//! workspace lock; acceptance checks the exact selected source and current indexed symbols.

use std::{path::Path, sync::Arc};

use ktcore::{
    extensions::HostInputGuard,
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

pub(super) struct HydratedSource {
    stamp: SourceStamp,
    pub(super) revision: u64,
    source: Arc<LibrarySource>,
    retired: Arc<std::sync::atomic::AtomicBool>,
    abi_guard: Option<Arc<dyn HostInputGuard>>,
}
impl Drop for HydratedSource {
    fn drop(&mut self) { self.retired.store(true,std::sync::atomic::Ordering::Release); }
}

#[derive(Debug)]
struct DependencyProvenanceGuard {
    retired: Arc<std::sync::atomic::AtomicBool>,
    lease: Arc<crate::compiled_watch::HostInputLease>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct BinaryLeaseKey {
    model: usize,
    epoch: u64,
    archive: String,
    physical_origin: String,
    inputs: Vec<std::path::PathBuf>,
}

#[derive(Default)]
pub(super) struct BinaryLeasePool {
    leases: std::sync::Mutex<
        std::collections::BTreeMap<
            BinaryLeaseKey,
            std::sync::Weak<crate::compiled_watch::HostInputLease>,
        >,
    >,
}

impl BinaryLeasePool {
    // Called only during off-lock preparation; the pool never owns a native lease strongly.
    fn current(
        &self,
        request: &SourceRequest,
        physical_origin: &str,
    ) -> Option<Arc<crate::compiled_watch::HostInputLease>> {
        let mut inputs = request.source.provenance_inputs();
        inputs.sort();
        inputs.dedup();
        let key = BinaryLeaseKey {
            model: Arc::as_ptr(request.model.as_ref()?) as usize,
            epoch: request.epoch,
            archive: request.stamp.archive.clone(),
            physical_origin: physical_origin.into(),
            inputs,
        };
        let mut leases = self.leases.lock().ok()?;
        leases.retain(|_, lease| lease.upgrade().is_some_and(|lease| lease.is_current()));
        if let Some(lease) = leases
            .get(&key)
            .and_then(std::sync::Weak::upgrade)
            .filter(|lease| lease.is_current())
        {
            return Some(lease);
        }
        // Serialize creation in this off-lock pool so concurrent batches cannot duplicate watches.
        let plan = crate::compiled_watch::WatchPlan::read_inputs(&key.inputs).ok()?;
        let lease = crate::compiled_watch::HostInputLease::new(plan, Arc::new(|| {})).ok()?;
        leases.insert(key, Arc::downgrade(&lease));
        Some(lease)
    }
}

impl ktcore::extensions::HostInputGuard for DependencyProvenanceGuard {
    fn is_current(&self)->bool {
        !self.retired.load(std::sync::atomic::Ordering::Acquire) && self.lease.is_current()
    }
}

pub(crate) struct ProvenanceInput {
    pub revision: u64,
    pub archive_origin: Option<String>,
    pub selected: Option<(Arc<LibrarySource>, std::path::PathBuf, String, Arc<std::sync::atomic::AtomicBool>)>,
}
pub(crate) struct ProvenancePlan {
    pub evaluation: u64,
    pub model: Arc<crate::classpath::ProjectSnapshot>,
    pub inputs: std::collections::BTreeMap<String, ProvenanceInput>,
}
pub(crate) struct ProvenanceProof {
    pub revision: u64,
    pub origin: Option<String>,
    pub guard: Option<Arc<dyn ktcore::extensions::HostInputGuard>>,
}
pub(crate) struct PreparedProvenance {
    pub evaluation: u64,
    pub model: Arc<crate::classpath::ProjectSnapshot>,
    pub proofs: std::collections::BTreeMap<String, Result<ProvenanceProof,String>>,
}

/// Archive/policy IO and native watch registration stay outside the workspace lock.
pub(crate) fn prepare_source_provenance(plan: ProvenancePlan, wake: Arc<dyn Fn()+Send+Sync>) -> PreparedProvenance {
    let proofs = plan.inputs.into_iter().map(|(token,input)| {
        let proof = (|| {
            let Some((source,binary,origin,retired)) = input.selected else {
                return Ok(ProvenanceProof {revision:input.revision,origin:None,guard:None});
            };
            let current = || !retired.load(std::sync::atomic::Ordering::Acquire) && source.is_current() && source.content_is_current()
                && crate::classpath::file_origin_id(&binary)==origin
                && input.archive_origin.as_ref().is_none_or(|expected|
                    source.selected_archive().is_some_and(|archive|crate::classpath::file_origin_id(archive)==*expected));
            if !current() { return Err("Stale".into()); }
            let mut paths=source.provenance_inputs(); paths.push(binary.clone());
            let plan=crate::compiled_watch::WatchPlan::read_inputs(&paths).map_err(|error|error.to_string())?;
            let guard=crate::compiled_watch::HostInputLease::new(plan,wake.clone()).map_err(|error|error.to_string())?;
            if !current() || !guard.is_current() { return Err("Stale".into()); }
            Ok(ProvenanceProof {revision:input.revision,origin:Some(origin),guard:Some(Arc::new(DependencyProvenanceGuard {retired,lease:guard}))})
        })();
        (token,proof)
    }).collect();
    PreparedProvenance {evaluation:plan.evaluation,model:plan.model,proofs}
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
    use sha2::Digest;

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

    fn provenance_fixture(module: bool) -> (Fixture,Workspace,Arc<crate::classpath::ProjectSnapshot>,String) {
        use crate::classpath::*;
        let fixture=Fixture::new();
        let root=fixture.root.join("src"); fs::create_dir_all(&root).unwrap();
        let owner=crate::paths::file_key(&root.join("Owner.java"));
        let component=if module {ComponentId::Module {group:"fixture".into(),name:"library".into(),version:"1".into()}} else {ComponentId::File};
        let pom=fixture.root.join("library.pom");
        fs::write(&pom,"<project><modelVersion>4.0.0</modelVersion><groupId>fixture</groupId><artifactId>library</artifactId><version>1</version></project>").unwrap();
        let source=SourceSetClasspath {name:"main".into(),source_roots:vec![root],entries:vec![fixture.binary.clone()],
            artifacts:vec![ResolvedArtifact {path:fixture.binary.clone(),component:component.clone()}],
            source_artifacts:Some(vec![ResolvedArtifact {path:fixture.sources.clone(),component:component.clone()}]),
            source_poms:Some(vec![ResolvedArtifact {path:pom,component}]),
            runtime:Some(RuntimeClassInputs {adapter:"gradle-runtime-class-inputs-v1".into(),classpath:vec![fixture.binary.clone()],
                class_outputs:Vec::new(),complete:true,problem:None}),..Default::default()};
        let model=Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {module:":fixture".into(),project_dir:fixture.root.clone(),
            build:"fixture".into(),source_sets:vec![source],entries:Vec::new(),unresolved:Vec::new()}]));
        let mut ws=Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
        ws.set_project_model(model.clone());
        let batches=if module {
            let coord=crate::coords::Coordinate::parse("fixture:library:1").unwrap();
            let source=crate::deps::coordinate_source_for_model(&coord,&crate::artifacts::Repos::defaults(),&fixture.extract,Some(&model)).unwrap().unwrap();
            crate::deps::resolve_library_source(&source,&mut KotlinParser::new(),&mut JavaParser::new()).unwrap()
        } else {
            crate::deps::resolve_local_jar_stubs(&fixture.binary,&fixture.extract,&mut KotlinParser::new(),&mut JavaParser::new()).unwrap()
        };
        for batch in batches {assert!(ws.index_dependency_batch(batch));}
        ws.bump_index_revision(); ws.open(&owner,OWNER.into());
        assert_eq!(drain(&mut ws),2);
        (fixture,ws,model,owner)
    }

    fn binary_abi_fixture() -> (Fixture,Workspace,Arc<crate::classpath::ProjectSnapshot>,String) {
        binary_abi_fixture_kind(false)
    }

    fn binary_abi_fixture_kind(directory:bool) -> (Fixture,Workspace,Arc<crate::classpath::ProjectSnapshot>,String) {
        use crate::classpath::*;
        let mut fixture=Fixture::new();
        fs::remove_file(&fixture.sources).unwrap();
        if directory {
            fixture.binary=fixture.root.join("selected-classes");
            fs::create_dir_all(fixture.binary.join("arrayfixture")).unwrap();
            fs::write(fixture.binary.join("arrayfixture/Factory.class"),include_bytes!("../../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class")).unwrap();
            fs::write(fixture.binary.join("arrayfixture/Item.class"),include_bytes!("../../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class")).unwrap();
        }
        let root=fixture.root.join("src");fs::create_dir_all(&root).unwrap();
        let owner=crate::paths::file_key(&root.join("Owner.java"));
        let model=Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
            module:":fixture".into(),project_dir:fixture.root.clone(),build:"fixture".into(),
            source_sets:vec![SourceSetClasspath {name:"main".into(),source_roots:vec![root],
                entries:vec![fixture.binary.clone()],artifacts:vec![ResolvedArtifact {
                    path:fixture.binary.clone(),component:ComponentId::File}],..Default::default()}],
            entries:Vec::new(),unresolved:Vec::new(),
        }]));
        let mut ws=Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
        ws.set_project_model(model.clone());fixture.index(&mut ws);ws.open(&owner,OWNER.into());
        (fixture,ws,model,owner)
    }

    #[test]
    fn selected_binary_type_jvm_abi_has_physical_identity_without_source_authority() {
        let (fixture, mut ws, model, owner) = binary_abi_fixture();
        let mut requests = ws.begin_extension_source_batch();
        assert_eq!(requests.len(), 2);
        let later = requests.pop().unwrap();
        let first = prepare_sources(requests);
        let first_lease = first[0].binary.as_ref().unwrap().2.clone();
        ws.finish_extension_source_batch(first);
        let second = prepare_sources(vec![later]);
        assert!(
            Arc::ptr_eq(&first_lease, &second[0].binary.as_ref().unwrap().2),
            "the same archive must share its native lease across preparation batches"
        );
        ws.finish_extension_source_batch(second);
        let physical = model.artifact_origin(&fixture.binary).unwrap();
        let expected = ktcore::classfile::decode(
            physical,
            include_bytes!("../../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class"),
        )
        .unwrap();
        let (index, host) = ws.extension_test_inputs();
        let mut read = host.store.begin(index, "library-probe", &owner).unwrap();
        let Knowledge::Found(ty) = read.resolve_type("arrayfixture.Factory") else {
            panic!("missing binary type");
        };
        let facts = read.type_jvm(&ty).unwrap();
        let Knowledge::Found(class) = facts.abi else {
            panic!("missing selected binary ABI");
        };
        assert_eq!(class.identity, expected.identity);
        assert_eq!(facts.class_flags, Knowledge::Found(expected.access_flags));
        assert!(
            matches!(facts.declared_members_coverage,Knowledge::Unknown(ref reasons)
            if reasons.contains(&IncompleteReason::UnsupportedSource))
        );
        assert!(matches!(facts.jvm_abi_coverage, Knowledge::Unknown(_)));
        assert!(
            matches!(read.type_source_location(&ty).unwrap(),Knowledge::Unknown(ref reasons)
            if reasons.contains(&IncompleteReason::UnsupportedSource))
        );
        let pending = read.finish();
        host.store.commit(pending).unwrap();
        let guards: Vec<_> = host
            .hydrated_sources
            .values()
            .map(|source| {
                (
                    source.retired.clone(),
                    source.abi_guard.as_ref().unwrap().clone(),
                )
            })
            .collect();
        assert_eq!(guards.len(), 2);
        assert!(
            !Arc::ptr_eq(&guards[0].1, &guards[1].1),
            "type retirement guards stay independent"
        );
        guards[0].0.store(true, Ordering::Release);
        assert!(!guards[0].1.is_current());
        assert!(guards[1].1.is_current());
        assert!(
            first_lease.is_current(),
            "retiring a type must not retire the shared archive watch"
        );
    }

    #[test]
    fn selected_binary_type_jvm_abi_refuses_workspace_shadow_and_changed_epoch() {
        let (fixture,mut ws,_model,owner)=binary_abi_fixture();
        assert_eq!(drain(&mut ws),2);
        let shadow=crate::paths::file_key(&fixture.root.join("src/Factory.java"));
        ws.open(&shadow,"package arrayfixture; public class Factory { public int own() {return 1;} }".into());
        let (index,host)=ws.extension_test_inputs();
        let mut read=host.store.begin(index,"library-probe",&owner).unwrap();
        let Knowledge::Found(ty)=read.resolve_type("arrayfixture.Factory") else {panic!("missing workspace shadow");};
        assert_eq!(ty.definition().file,shadow);
        assert!(matches!(read.type_jvm(&ty).unwrap().abi,Knowledge::Unknown(_)));
        assert!(matches!(read.type_source_location(&ty).unwrap(),Knowledge::Found(_)));
        drop(read);
        let (fixture,mut ws,model,owner)=binary_abi_fixture();
        let prepared=prepare_sources(ws.begin_extension_source_batch());
        assert!(prepared.iter().any(|source|source.binary.is_some()));
        // Replacing the accepted model retires even a successfully decoded off-lock batch.
        ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(model.modules.clone())));
        ws.finish_extension_source_batch(prepared);
        assert!(ws.extension_test_inputs().1.hydrated_sources.is_empty());
        let _=(fixture,owner);
    }

    #[test]
    fn selected_binary_type_jvm_abi_rejects_duplicate_variant_and_changed_content() {
        for change in ["duplicate","variant","content"] {
            let (fixture,mut ws,model,owner)=binary_abi_fixture();
            if change!="content" {
                let mut modules=model.modules.clone();
                let scope=&mut modules[0].source_sets[0];
                if change=="duplicate" {scope.entries.push(fixture.binary.clone());}
                else {
                    let different=fixture.root.join("different.jar");fs::copy(&fixture.binary,&different).unwrap();
                    scope.entries=vec![different.clone()];scope.artifacts[0].path=different;
                }
                ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(modules)));
                drain(&mut ws);
            } else {
                assert_eq!(drain(&mut ws),2);
                let (index,host)=ws.extension_test_inputs();
                let mut read=host.store.begin(index,"library-probe",&owner).unwrap();
                let Knowledge::Found(ty)=read.resolve_type("arrayfixture.Factory") else {panic!("missing binary type");};
                assert!(matches!(read.type_jvm(&ty).unwrap().abi,Knowledge::Found(_)));
                let pending=read.finish();
                let guard=host.hydrated_sources[&ty.definition().file].abi_guard.as_ref().unwrap().clone();
                fs::write(&fixture.binary,b"changed selected binary").unwrap();
                for _ in 0..100 {
                    if !guard.is_current() {break;}
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                assert!(!guard.is_current(),"selected binary native guard did not retire");
                assert_eq!(host.store.resume(index,&owner,pending).err(),Some(ContractError::Stale));
                host.refresh_host_inputs();
            }
            let (index,host)=ws.extension_test_inputs();
            let mut read=host.store.begin(index,"library-probe",&owner).unwrap();
            if let Knowledge::Found(ty)=read.resolve_type("arrayfixture.Factory") {
                assert!(matches!(read.type_jvm(&ty).unwrap().abi,Knowledge::Unknown(_)),"{change}");
            }
        }
    }

    #[test]
    fn selected_class_directory_java_kotlin_type_ref_member_and_abi() {
        let (fixture,mut ws,model,_)=binary_abi_fixture_kind(true);
        assert_eq!(model.local_jars(),vec![fixture.binary.clone()]);
        assert_eq!(drain(&mut ws),2);
        for (name,text) in [("Use.java","package client; class Use {}"),("Use.kt","package client\nclass Use")] {
            let file=crate::paths::file_key(&fixture.root.join("src").join(name));ws.open(&file,text.into());
            let (index,host)=ws.extension_test_inputs();
            let mut read=host.store.begin(index,"library-probe",&file).unwrap();
            let mut reference=ktcore::types::TypeRef::simple("Factory");reference.qualification=Some("arrayfixture".into());
            let Knowledge::Found(ty)=read.resolve_type_ref(&reference) else {panic!("directory TypeRef {name}");};
            assert!(matches!(read.reference_type(&ty).unwrap(),Knowledge::Found(_)));
            assert!(read.declared_members(&ty).unwrap().0.iter().any(|member|member.name=="rows"));
            let Knowledge::Found(class)=read.type_jvm(&ty).unwrap().abi else {panic!("directory ABI {name}");};
            let bytes=include_bytes!("../../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class");
            assert_eq!(class.identity,ktcore::classfile::decode(model.artifact_origin(&fixture.binary).unwrap(),bytes).unwrap().identity);
            assert!(matches!(read.type_source_location(&ty).unwrap(),Knowledge::Unknown(_)));
        }
        let mut modules=model.modules.clone();modules[0].source_sets[0].output_roots=vec![fixture.binary.clone()];
        let owned=crate::classpath::ProjectSnapshot::new(modules);
        assert!(owned.local_jars().is_empty(),"workspace outputs must keep existing receipt ownership");
    }

    #[test]
    fn selected_class_directory_edit_removal_membership_and_shadow_retire_facts() {
        for change in ["edit","remove","add","shadow"] {
            let (fixture,mut ws,_,owner)=binary_abi_fixture_kind(true);assert_eq!(drain(&mut ws),2);
            let before=crate::deps::file_content_digest(&fixture.binary).unwrap();
            let (index,host)=ws.extension_test_inputs();let mut read=host.store.begin(index,"library-probe",&owner).unwrap();
            let Knowledge::Found(ty)=read.resolve_type("arrayfixture.Factory") else {panic!("missing initial directory type");};
            assert!(read.declared_members(&ty).unwrap().0.iter().any(|member|member.name=="rows"));
            let pending=read.finish();
            if change=="shadow" {
                let path=crate::paths::file_key(&fixture.root.join("src/Factory.java"));
                ws.open(&path,"package arrayfixture; public class Factory {}".into());
                let (index,host)=ws.extension_test_inputs();let mut read=host.store.begin(index,"library-probe",&owner).unwrap();
                let Knowledge::Found(ty)=read.resolve_type("arrayfixture.Factory") else {panic!("missing source shadow");};
                assert_eq!(ty.definition().file,path);assert!(matches!(read.type_jvm(&ty).unwrap().abi,Knowledge::Unknown(_)));
            } else {
                let guard=host.hydrated_sources[&ty.definition().file].abi_guard.as_ref().unwrap().clone();
                let class=fixture.binary.join("arrayfixture/Factory.class");
                match change {
                    "edit"=>fs::write(class,b"changed class bytes").unwrap(),
                    "remove"=>fs::remove_file(class).unwrap(),
                    _=>fs::write(fixture.binary.join("arrayfixture/Added.class"),b"new member").unwrap(),
                }
                assert_ne!(crate::deps::file_content_digest(&fixture.binary).unwrap(),before);
                for _ in 0..100 {if !guard.is_current() {break;}std::thread::sleep(std::time::Duration::from_millis(10));}
                assert!(!guard.is_current());assert_eq!(host.store.resume(index,&owner,pending).err(),Some(ContractError::Stale));
            }
        }
    }

    fn planned_provenance(ws:&mut Workspace,model:&Arc<crate::classpath::ProjectSnapshot>,owner:&str,name:&str)
        ->(crate::extension_ipc::Evaluation,PendingContributions,String,ProvenancePlan) {
        use serde_json::json;
        let open=ws.open_doc_keys();
        let (index,host)=ws.extension_test_inputs();
        let mut evaluation=crate::extension_ipc::Evaluation::new("library-probe".into(),owner.into(),1);
        evaluation.project=Some(model.clone()); evaluation.project_operations=u32::MAX;
        let mut read=host.store.begin(index,"library-probe",owner).unwrap();
        let resolved=evaluation.batch(&mut read,"analysis/readFacts",json!({"evaluation":evaluation.id.to_string(),
            "queries":[{"op":"resolveType","name":name}]})).unwrap();
        assert_eq!(resolved["facts"][0]["state"],"found","{resolved}");
        let token=resolved["facts"][0]["value"]["id"].as_str().unwrap().to_owned();
        let pending=read.finish();
        let targets=evaluation.source_provenance_targets("analysis/readFacts",&json!({"evaluation":evaluation.id.to_string(),
            "queries":[{"op":"sourceProvenance","target":token}]})).unwrap();
        let file=&targets[0].1.definition().file;
        let inputs=host.provenance_input(index,model,Path::new(owner),file,open.contains(file))
            .map(|input|std::collections::BTreeMap::from([(token.clone(),input)])).unwrap_or_default();
        let plan=ProvenancePlan {evaluation:evaluation.id,model:model.clone(),inputs};
        (evaluation,pending,token,plan)
    }

    fn read_provenance(ws:&mut Workspace,owner:&str,evaluation:&mut crate::extension_ipc::Evaluation,
        pending:PendingContributions,token:&str)->Result<(serde_json::Value,PendingContributions),String> {
        let (index,host)=ws.extension_test_inputs();
        let mut read=host.store.resume(index,owner,pending).map_err(|error|format!("{error:?}"))?;
        let facts=evaluation.batch(&mut read,"analysis/readFacts",serde_json::json!({"evaluation":evaluation.id.to_string(),
            "queries":[{"op":"sourceProvenance","target":token}]}));
        let pending=read.finish(); facts.map(|facts|(facts,pending))
    }

    #[test]
    fn source_provenance_local_and_maven_bind_the_selected_physical_runtime_input() {
        for module in [false,true] {
            let (fixture,mut ws,model,owner)=provenance_fixture(module);
            let (mut evaluation,pending,token,plan)=planned_provenance(&mut ws,&model,&owner,"arrayfixture.Item");
            assert_eq!(plan.inputs.len(),1);
            evaluation.source_provenance_proofs=prepare_source_provenance(plan,Arc::new(||{})).proofs;
            let (facts,_)=read_provenance(&mut ws,&owner,&mut evaluation,pending,&token).unwrap();
            let fact=&facts["facts"][0];assert_eq!(fact["state"],"found");
            eprintln!("SOURCE_PROVENANCE_WIRE {} {fact}",if module {"maven"} else {"local"});
            assert_eq!(fact["value"].as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
                std::collections::BTreeSet::from(["target","kind","source","jvmOwner","originId"]));
            assert_eq!(fact["value"]["source"].as_object().unwrap().len(),2);
            assert_eq!(fact["value"]["source"]["identity"].as_object().unwrap().len(),2);
            assert_eq!(fact["value"]["source"]["identity"]["kind"],"source");
            assert!(fact["value"]["source"]["identity"]["revision"].is_string());
            assert_eq!(fact["value"]["target"],token);assert_eq!(fact["value"]["kind"],"selectedDependencySource");
            assert_eq!(fact["value"]["jvmOwner"],"arrayfixture/Item");
            assert_eq!(fact["value"]["originId"],model.artifact_origin(&fixture.binary).unwrap());
            assert_eq!(fact["value"]["source"]["sha256"],format!("{:x}",sha2::Sha256::digest(ITEM.as_bytes())));
            let file=evaluation.source_provenance_targets("analysis/readFacts",&serde_json::json!({"evaluation":evaluation.id.to_string(),
                "queries":[{"op":"sourceProvenance","target":token}]})).unwrap()[0].1.definition().file.clone();
            let (index,host)=ws.extension_test_inputs();let hydrated=&host.hydrated_sources[&file];
            let mut different=model.modules.clone();
            let other=fixture.root.join("different-runtime.jar");fs::copy(&fixture.binary,&other).unwrap();
            different[0].source_sets[0].runtime.as_mut().unwrap().classpath=vec![other];
            let different=crate::classpath::ProjectSnapshot::new(different);
            assert!(different.dependency_runtime_selection(Path::new(&owner),&hydrated.stamp.origin,&hydrated.source).is_none());
            assert!(host.provenance_input(index,&different,Path::new(&owner),&file,false).is_none());
            let different=Arc::new(different); ws.set_project_model(different.clone());
            assert_eq!(drain(&mut ws),2);
            let (mut evaluation,pending,token,plan)=planned_provenance(&mut ws,&different,&owner,"arrayfixture.Item");
            assert!(plan.inputs.is_empty());
            evaluation.source_provenance_proofs=prepare_source_provenance(plan,Arc::new(||{})).proofs;
            let (facts,_)=read_provenance(&mut ws,&owner,&mut evaluation,pending,&token).unwrap();
            let fact=&facts["facts"][0];assert_eq!(fact["state"],"unknown");
            assert!(fact["reasons"].as_array().is_some_and(|reasons|!reasons.is_empty()));
            eprintln!("SOURCE_PROVENANCE_WIRE variant-mismatch {fact}");
        }
    }

    #[test]
    fn source_provenance_revokes_same_body_overlays_and_inflight_inputs() {
        for change in ["open","comment","delete","model","index","archive","inflight-open"] {
            let (fixture,mut ws,model,owner)=provenance_fixture(false);
            let (mut evaluation,pending,token,plan)=planned_provenance(&mut ws,&model,&owner,"arrayfixture.Item");
            assert_eq!(plan.inputs.len(),1);
            let file=ws.index.lookup_type("Item").into_iter().find(|entry|entry.sym.package=="arrayfixture").unwrap().path.to_string();
            if matches!(change,"archive"|"inflight-open") {
                if change=="archive" {fixture.write_sources("\n// changed selection\n");}
                else {ws.open(&file,ITEM.into());}
                assert!(prepare_source_provenance(plan,Arc::new(||{})).proofs.values().all(|proof|proof.as_ref().is_err_and(|error|error=="Stale")));
                continue;
            }
            evaluation.source_provenance_proofs=prepare_source_provenance(plan,Arc::new(||{})).proofs;
            let guard=evaluation.source_provenance_proofs[&token].as_ref().unwrap().guard.as_ref().unwrap().clone();
            let (facts,pending)=read_provenance(&mut ws,&owner,&mut evaluation,pending,&token).unwrap();
            assert_eq!(facts["facts"][0]["state"],"found");
            if change=="model" {ws.set_project_model(Arc::new(crate::classpath::ProjectSnapshot::new(model.modules.clone())));}
            else if change=="index" {
                ws.index_dependency_file("/other/Added.java",vec![ktcore::symbol::IndexedSymbol::new(
                    "Added",ktcore::symbol::SymbolKind::Class,"other",None,6,11)]);
                ws.bump_index_revision();
            }
            else {
                ws.open(&file,ITEM.into());
                if change=="comment" {ws.change(&file,format!("{ITEM}\n// same symbols\n"));}
                if change=="delete" {ws.change(&file,"package arrayfixture; class Item {}".into());}
                assert!(!ws.extension_test_inputs().1.hydrated_sources.contains_key(&file));
                assert!(!guard.is_current(),"even an identical-text open retires the old dependency proof");
            }
            assert!(ws.extension_test_inputs().1.store.validate_pending(&pending).is_err(),"{change}");
            if matches!(change,"open"|"comment"|"delete") {
                let (_,_,_,plan)=planned_provenance(&mut ws,&model,&owner,"arrayfixture.Item");
                assert!(plan.inputs.is_empty(),"an open library body cannot regain readonly dependency authority");
            }
        }
    }

    #[test]
    fn source_provenance_workspace_generated_and_negotiation_do_not_change_type_jvm() {
        let (_fixture,mut ws,model,owner)=provenance_fixture(false);
        let generated=crate::paths::file_key(&model.modules[0].source_sets[0].source_roots[0].join("generated/Generated.java"));
        let mut modules=model.modules.clone();
        let root=modules[0].source_sets[0].source_roots[0].clone();
        modules[0].source_sets[0].compiler_tasks.push(serde_json::from_value(serde_json::json!({
            "task":":fixture:compileJava","language":"java","task_type":"JavaCompile","arguments":[],
            "annotation_processor_path":[],"source_files":[owner,generated],"java_home":null,"output_roots":[],
            "complete":false,"problem":"SDK not needed for source ownership",
            "options":{"sdkSourcePolicy":{"adapter":"gradle-source-tree-stock-patterns-v1","language":"java",
                "task":":fixture:compileJava","authored_roots":[root],"future_roots":[],
                "generated_roots":[root.join("generated")]}}
        })).unwrap());
        let model=Arc::new(crate::classpath::ProjectSnapshot::new(modules));
        ws.set_project_model(model.clone());
        ws.open(&generated,"package client; class Generated {}".into());
        assert_eq!(model.extension_source_editable(Path::new(&generated),Path::new(&owner)),Some(false));
        let (mut evaluation,pending,token,plan)=planned_provenance(&mut ws,&model,&owner,"client.Generated");
        evaluation.source_provenance_proofs=prepare_source_provenance(plan,Arc::new(||{})).proofs;
        let (facts,pending)=read_provenance(&mut ws,&owner,&mut evaluation,pending,&token).unwrap();
        assert_eq!(facts["facts"][0]["value"]["kind"],"currentWorkspaceSource");
        let fact=&facts["facts"][0];eprintln!("SOURCE_PROVENANCE_WIRE workspace {fact}");
        assert_eq!(fact["value"].as_object().unwrap().keys().map(String::as_str).collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["target","kind","source"]));
        assert_eq!(fact["value"]["source"].as_object().unwrap().len(),2);
        assert_eq!(fact["value"]["source"]["identity"].as_object().unwrap().len(),2);
        assert_eq!(fact["value"]["source"]["identity"]["kind"],"source");
        assert!(fact["value"]["source"]["identity"]["revision"].is_string());
        assert_eq!(fact["value"]["source"]["sha256"],format!("{:x}",sha2::Sha256::digest(b"package client; class Generated {}")));
        assert!(facts["facts"][0]["value"].get("originId").is_none());
        let (index,host)=ws.extension_test_inputs();
        let mut read=host.store.resume(index,&owner,pending).unwrap();
        let jvm=evaluation.batch(&mut read,"analysis/readFacts",serde_json::json!({"evaluation":evaluation.id.to_string(),
            "queries":[{"op":"typeJvm","target":token}]})).unwrap();
        assert!(jvm["facts"][0].get("sourceProvenance").is_none());
        assert_eq!(jvm["facts"][0]["declaredMembers"]["coverage"]["state"],"unknown");
        let pending=read.finish();
        evaluation.project_operations=0;
        assert!(read_provenance(&mut ws,&owner,&mut evaluation,pending,&token).err().unwrap().contains("not negotiated"));
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
            let mut selected_sources: Vec<Arc<LibrarySource>> = Vec::new();
            for batch in batches {
                // Open-file import seeding resolves each type separately, allocating new handles.
                let batch = crate::deps::resolve_local_jar_stubs(
                    &fixture.binary, &fixture.extract, &mut KotlinParser::new(), &mut JavaParser::new(),
                ).unwrap().into_iter().find(|candidate|candidate.file==batch.file).unwrap();
                let source = batch.library_source.as_ref().unwrap();
                assert!(selected_sources.iter().all(|previous|
                    !Arc::ptr_eq(previous,source) && previous.as_ref()==source.as_ref()));
                selected_sources.push(source.clone());
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
                "equal inputs from separate resolver calls share one selection for all three types");
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
    fn binary_sources_with_distinct_origins_do_not_share_selection() {
        let fixtures = [Fixture::new(),Fixture::new()];
        let mut requests = Vec::new();
        for fixture in &fixtures {
            fs::remove_file(&fixture.sources).unwrap();
            let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Probe)]);
            for batch in crate::deps::resolve_local_jar_stubs(
                &fixture.binary, &fixture.extract,
                &mut KotlinParser::new(), &mut JavaParser::new(),
            ).unwrap() { assert!(ws.index_dependency_batch(batch)); }
            ws.bump_index_revision();
            ws.open("Owner.java", "class Owner {}".into());
            let (index, host) = ws.extension_test_inputs();
            let mut read = host.store.begin(index,"library-probe","Owner.java").unwrap();
            assert!(matches!(read.resolve_type("arrayfixture.Item"),Knowledge::Unknown(_)));
            let pending = read.finish();
            host.store.commit(pending).unwrap();
            let batch = ws.begin_extension_source_batch();
            assert_eq!(batch.len(),1);
            requests.extend(batch);
        }
        assert_ne!(requests[0].stamp.origin,requests[1].stamp.origin);
        assert_ne!(requests[0].source,requests[1].source);
        let selections = crate::jar::class_selection_count();
        let prepared = prepare_sources(requests);
        assert_eq!(crate::jar::class_selection_count()-selections,2,
            "different selected inputs require their own proofs despite identical class bytes");
        assert_eq!(prepared.len(),2);
        assert_ne!(prepared[0].request.file,prepared[1].request.file);
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
    pub(crate) model: Option<Arc<crate::classpath::ProjectSnapshot>>,
    binary_leases: Arc<BinaryLeasePool>,
    _signal: SourceSignal,
}

pub(crate) struct PreparedSource {
    request: SourceRequest,
    text: Arc<str>,
    tree: Tree,
    references: PreparedSourceReferences,
    binary: Option<(String,Vec<Arc<ktcore::classfile::ClassFileFacts>>,Arc<crate::compiled_watch::HostInputLease>)>,
}

/// No network lookup: the dependency resolver has already selected this exact source archive.
pub(crate) fn prepare_sources(requests: Vec<SourceRequest>) -> Vec<PreparedSource> {
    let mut archives: Vec<(&Arc<LibrarySource>, Vec<&Path>)> = Vec::new();
    for request in &requests {
        if request.stamp.origin.representation != SourceRepresentation::Stub { continue; }
        if let Some((_,files)) = archives.iter_mut().find(|(source,_)|source.as_ref()==request.source.as_ref()) {
            files.push(Path::new(&request.file));
        } else {
            archives.push((&request.source,vec![Path::new(&request.file)]));
        }
    }
    // Separate resolver calls may allocate equal inputs; share only within this batch.
    let mut binary_types = Vec::new();
    for (source,files) in archives {
        if source.content_is_current() { binary_types.push((Arc::clone(source),source.binary_types(&files))); }
    }
    requests
        .into_iter()
        .filter_map(|request| {
            let path = Path::new(&request.file);
            // A name-only preview is not proof that the selected archive declares this type.
            if request.stamp.origin.representation == SourceRepresentation::Stub
                && (!request.source.content_is_current()
                    || !binary_types.iter().any(|(source,files)|source.as_ref()==request.source.as_ref() && files.contains(path)))
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
            let binary=(|| {
                let physical=request.model.as_ref()?.dependency_binary_origin(&request.stamp.origin,&request.source)?;
                let archive=request.source.selected_archive()?;
                if crate::classpath::file_origin_id(archive)!=physical {return None;}
                let guard=request.binary_leases.current(&request,&physical)?;
                let mut classes=Vec::new();
                for symbol in request.stamp.symbols.iter().filter(|symbol|symbol.kind.is_type_like()) {
                    let name=symbol.container.as_ref().map_or_else(||symbol.name.clone(),
                        |owner|format!("{owner}.{}",symbol.name)).replace('.',"$");
                    let internal=if symbol.package.is_empty() {name} else {format!("{}/{name}",symbol.package.replace('.',"/"))};
                    classes.push(request.source.binary_class(&internal,&physical,||guard.is_current())?);
                }
                (!classes.is_empty() && guard.is_current() && crate::classpath::file_origin_id(archive)==physical)
                    .then_some((physical,classes,guard))
            })();
            if !request.source.is_current() || !request.source.content_is_current() {
                return None;
            }
            Some(PreparedSource {
                request,
                text,
                tree,
                references,
                binary,
            })
        })
        .collect()
}

impl AnalysisExtensions {
    pub(super) fn provenance_input(&self, index: &Index, model: &crate::classpath::ProjectSnapshot,
        owner: &Path, file: &str, open: bool) -> Option<ProvenanceInput> {
        let origin=index.file_origin(file)?;
        let revision=self.store.source_revision(file)?;
        if origin.representation!=SourceRepresentation::Source { return None; }
        if origin.project_scope.is_some() {
            return (model.unique_source_origin(Path::new(file)).as_ref()==Some(origin))
                .then_some(ProvenanceInput {revision,archive_origin:None,selected:None});
        }
        if open { return None; }
        let hydrated=self.hydrated_sources.get(file)?;
        let (_,selected)=self.library_sources.get(file)?;
        if hydrated.revision!=revision || hydrated.source.as_ref()!=selected.as_ref()
            || self.source_stamp(index,file).as_ref()!=Some(&hydrated.stamp) { return None; }
        let (binary,physical)=model.dependency_runtime_selection(owner,origin,selected)?;
        let archive_origin=selected.selected_archive().and_then(|archive|model.artifact_origin(archive)).map(str::to_owned);
        Some(ProvenanceInput {revision,archive_origin,selected:Some((selected.clone(),binary,physical,hydrated.retired.clone()))})
    }

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
            .is_some_and(|hydrated| Some(&hydrated.stamp) != stamp.as_ref()
                || self.library_sources.get(file).is_none_or(|(_,selected)|selected.as_ref()!=hydrated.source.as_ref()))
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
                model: None,
                binary_leases: self.binary_leases.clone(),
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
                || source.request.stamp.origin.representation == SourceRepresentation::Stub && self.inputs.contains_key(file)
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
            let retired=Arc::new(std::sync::atomic::AtomicBool::new(false));
            let abi_guard=if let Some((physical,classes,lease))=source.binary {
                let guard:Arc<dyn HostInputGuard>=Arc::new(DependencyProvenanceGuard {retired:retired.clone(),lease});
                if self.store.bind_dependency_abi(index,file,&physical,classes,guard.clone()).is_err() {continue;}
                Some(guard)
            } else {None};
            self.hydrated_sources.insert(file.clone(), HydratedSource {
                stamp:source.request.stamp, revision:0, source:source.request.source,
                retired,abi_guard,
            });
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

    pub(super) fn retire_stale_binary_sources(&mut self) -> bool {
        let stale:Vec<_>=self.hydrated_sources.iter().filter(|(_,source)|
            source.abi_guard.as_ref().is_some_and(|guard|!guard.is_current())).map(|(file,_)|file.clone()).collect();
        for file in &stale {
            self.forget_hydrated_source(file);
            self.attempted_sources.remove(file);
        }
        !stale.is_empty()
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
