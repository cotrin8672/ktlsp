//! Exhaustive physical JVM inputs, distinct from source currency and runtime Code authority.
use std::{collections::BTreeMap, fs, io::Read, path::{Path, PathBuf}, sync::Arc};
use ktcore::{classfile::ClassFileFacts, extensions::HostInputGuard};
use crate::{classpath::ProjectSnapshot, compiled_watch::{HostInputLease, WatchPlan}};

const MAX_CATALOG_CLASSES: usize = 200_000;

pub(crate) struct CatalogPlan {
    pub evaluation: u64,
    pub model: Arc<ProjectSnapshot>,
    pub owner: PathBuf,
    pub catalog: Option<Arc<JvmCatalog>>,
    pub reads: Vec<usize>,
}
pub(crate) struct PreparedCatalog {
    pub evaluation: u64,
    pub model: Arc<ProjectSnapshot>,
    pub catalog: Result<Arc<JvmCatalog>, String>,
    pub classes: BTreeMap<usize, Result<Arc<ClassFileFacts>, String>>,
}
pub(crate) struct CatalogEntry {
    pub internal_name: String,
    pub origin: String,
    pub association: &'static str,
    pub order: usize,
    pub authority: &'static str,
    path: PathBuf,
    archive_index: Option<usize>,
    archive_name: String,
    size: u64,
    version: u16,
}
pub(crate) struct JvmCatalog {
    pub model: Arc<ProjectSnapshot>,
    pub entries: Vec<CatalogEntry>,
    pub complete: bool,
    pub problems: Vec<String>,
    pub guards: Vec<Arc<HostInputLease>>,
    read_coverage: WatchPlan,
    feature: Option<u16>,
}
impl JvmCatalog {
    pub(crate) fn current(&self) -> bool { self.guards.iter().all(|guard| guard.is_current()) }
    fn problem(&mut self, problem: impl Into<String>) {
        self.complete = false;
        if self.problems.len() < 32 { self.problems.push(problem.into().chars().take(512).collect()); }
    }
    fn append(&mut self, entry: CatalogEntry) {
        if self.entries.len() >= MAX_CATALOG_CLASSES { self.problem("catalog-class-limit"); }
        else { self.entries.push(entry); }
    }
    fn input(&mut self, path: &Path, origin: &str, association: &'static str, order: usize,
        prefix: &str, authority: &'static str) -> Result<(), String> {
        let metadata = match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            value => value.map_err(|error| error.to_string())?,
        };
        if metadata.file_type().is_symlink() { return Err("catalog-symlink".into()); }
        if metadata.is_dir() {
            for entry in walkdir::WalkDir::new(path).follow_links(false) {
                if !self.current() { return Err("Stale".into()); }
                let entry = entry.map_err(|error| error.to_string())?;
                if entry.file_type().is_symlink() { self.problem("catalog-symlink"); continue; }
                if entry.file_type().is_dir() {
                    if !self.read_coverage.includes_directory_watch(entry.path()) { return Err("catalog-read-not-prearmed".into()); }
                    continue;
                }
                if !entry.file_type().is_file() { continue; }
                let relative = crate::paths::file_key(entry.path().strip_prefix(path).map_err(|error|error.to_string())?);
                let Some(name) = relative.strip_suffix(".class") else { continue; };
                if !crate::runtime_bytes::valid_internal_name(name) { self.problem("invalid-class-entry"); continue; }
                self.append(CatalogEntry { internal_name:name.into(), origin:origin.into(), association, order, authority,
                    path:entry.path().into(), archive_index:None, archive_name:String::new(), size:entry.metadata().map_err(|error|error.to_string())?.len(),version:0 });
                if self.entries.len() >= MAX_CATALOG_CLASSES { self.problem("catalog-class-limit"); break; }
            }
            return Ok(());
        }
        if !metadata.is_file() { return Err("catalog-input-not-file".into()); }
        // Existing verification recognizes requires-only module artifacts as genuinely empty.
        if prefix.is_empty() && crate::jar::may_contain_definitions(path).is_ok_and(|present| !present) { return Ok(()); }
        let mut archive = zip::ZipArchive::new(fs::File::open(path).map_err(|error|error.to_string())?)
            .map_err(|error|error.to_string())?;
        let selected=crate::jar::selected_class_entries(&mut archive,prefix,self.feature,||self.current())?;
        for selected in selected {
            if !self.current() { return Err("Stale".into()); }
            let name=selected.internal_name.as_str();
            if name=="module-info" {
                let entry=archive.by_index(selected.index).map_err(|error|error.to_string())?;
                let mut entry=entry;
                if entry.size()>ktcore::classfile::MAX_CLASS_FILE_BYTES as u64 {self.problem("module-descriptor-limit");continue;}
                let mut bytes=Vec::new();
                entry.by_ref().take(ktcore::classfile::MAX_CLASS_FILE_BYTES as u64+1).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
                if ktcore::classfile::decode(origin,&bytes).is_ok_and(|class|
                    class.identity.internal_name=="module-info" && class.access_flags==0x8000
                    && class.super_name.is_none() && class.interfaces.is_empty() && class.fields.is_empty() && class.methods.is_empty()) {continue;}
                self.problem("unproved-module-descriptor");continue;
            }
            self.append(CatalogEntry { internal_name:name.into(), origin:origin.into(), association, order, authority,
                path:path.into(), archive_index:Some(selected.index), archive_name:selected.name, size:selected.size,version:selected.version });
            if self.entries.len() >= MAX_CATALOG_CLASSES { self.problem("catalog-class-limit"); break; }
        }
        Ok(())
    }
    fn prepare(model: Arc<ProjectSnapshot>, owner: &Path, wake: Arc<dyn Fn()+Send+Sync>) -> Result<Arc<Self>,String> {
        let (scope, source, sdk) = if let Some(context) = model.extension_project_context(owner) {
            (context.scope, context.source, context.sdk.cloned())
        } else {
            let ktcore::knowledge::Knowledge::Found(context) = model.resource_owner_context(owner) else { return Err("missing-selected-jvm-scope".into()); };
            (context.scope.clone(), model.source_set_for_scope(&context.scope).ok_or("missing-source-set")?, model.sdk_api_for_scope(&context.scope).cloned())
        };
        let outputs = source.output_roots.clone();
        let classpath = source.entries.clone();
        let complete = !source.classpath_incomplete && source.unresolved.is_empty();
        let mut inputs=outputs.iter().chain(&classpath).cloned().collect::<Vec<_>>();
        if let Some(selection)=&sdk {inputs.push(selection.home.join("release"));}
        let plan = WatchPlan::read_inputs(&inputs).map_err(|error|error.to_string())?;
        let lease = HostInputLease::new(plan.clone(), wake.clone()).map_err(|error|error.to_string())?;
        let sdk_release=sdk.as_ref().map(|selection| {
            let release=selection.home.join("release");
            let revision=crate::classpath::file_origin_id(&release);(release,revision)
        });
        if let Some((release,revision))=&sdk_release {
            if model.artifact_origin(release).is_some_and(|origin|origin!=revision) {return Err("Stale".into());}
        }
        // This is the raw selected physical JVM view, not a javac/Kotlin compiler
        // interpretation. Its MR profile matches normal-runtime Code: SDK home.
        let feature=sdk.as_ref().and_then(|selection|crate::sdk_runtime::runtime_feature(selection).ok());
        let mut catalog = Self { model:model.clone(), entries:Vec::new(), complete, problems:Vec::new(), guards:vec![lease],read_coverage:plan,feature };
        if !complete { catalog.problem("incomplete-selected-classpath"); }
        for (association, paths) in [("output", outputs), ("compileClasspath", classpath)] {
            for (order, path) in paths.iter().enumerate() {
                if model.artifact_origin(path).is_some_and(|origin|crate::classpath::file_origin_id(path)!=origin) {return Err("Stale".into());}
                let origin = model.artifact_origin(path).map(str::to_owned)
                    .unwrap_or_else(||format!("{association}:{}#{}:{order}",scope.module,scope.source_set));
                if let Err(error) = catalog.input(path, &origin, association, order, "", "compiled-input") {
                    if error == "Stale" { return Err(error); } catalog.problem(error);
                }
            }
        }
        match sdk {
            None => catalog.problem("missing-selected-sdk"),
            Some(selection) if selection.release.is_some() => {
                let plan = WatchPlan::read_inputs(&[selection.archive.clone()]).map_err(|error|error.to_string())?;
                catalog.guards.push(HostInputLease::new(plan,wake).map_err(|error|error.to_string())?);
                if crate::classpath::file_origin_id(&selection.archive) != selection.archive_revision
                    || crate::sdk_runtime::file_sha256(&selection.archive).map_err(|error|format!("{error:?}"))? != selection.archive_sha256
                { return Err("Stale".into()); }
                let mut archive = zip::ZipArchive::new(fs::File::open(&selection.archive).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
                let selected:BTreeMap<_,_>=selection.entries.iter().filter_map(|(name,entry)|entry.as_deref().map(|entry|(entry,name.as_str()))).collect();
                if selection.entries.values().any(Option::is_none) {catalog.problem("ambiguous-sdk-api-entry");}
                for index in 0..archive.len() {
                    if !catalog.current() { return Err("Stale".into()); }
                    let entry = archive.by_index(index).map_err(|error|error.to_string())?;
                    let entry_name=entry.name().map_err(|error|error.to_string())?;
                    let Some(name)=selected.get(entry_name.as_ref()) else {continue;};
                    catalog.append(CatalogEntry {internal_name:(*name).into(), origin:selection.origin(), association:"sdk", order:0,
                        authority:"sdk-api-header", path:selection.archive.clone(), archive_index:Some(index), archive_name:entry_name.into_owned(), size:entry.size(),version:0});
                }
            }
            Some(selection) => {
                let sdk = crate::sdk_runtime::SelectedSdkRuntime::for_selection(&selection,wake).map_err(|error|format!("{error:?}"))?;
                if catalog.feature!=Some(sdk.feature()) {return Err("Stale".into());}
                catalog.guards.push(sdk.guard());
                for (order,(path,prefix)) in sdk.archives().iter().enumerate() {
                    if let Err(error) = catalog.input(path,&selection.origin(),"sdk",order,prefix,"sdk-runtime-input") {
                        if error == "Stale" { return Err(error); } catalog.problem(error);
                    }
                }
            }
        }
        if sdk_release.as_ref().is_some_and(|(path,revision)|crate::classpath::file_origin_id(path)!=*revision) {return Err("Stale".into());}
        if !catalog.current() { return Err("Stale".into()); }
        catalog.entries.sort_by(|a,b| a.internal_name.cmp(&b.internal_name).then(a.association.cmp(b.association)).then(a.order.cmp(&b.order)));
        Ok(Arc::new(catalog))
    }
    fn read(&self, index: usize) -> Result<Arc<ClassFileFacts>, String> {
        if !self.current() { return Err("Stale".into()); }
        let entry = self.entries.get(index).ok_or("foreign-jvm-entry")?;
        let mut bytes = Vec::new();
        let limit = ktcore::classfile::MAX_CLASS_FILE_BYTES;
        if entry.size > limit as u64 { return Err("class-input-limit".into()); }
        if let Some(index) = entry.archive_index {
            let mut archive = zip::ZipArchive::new(fs::File::open(&entry.path).map_err(|error|error.to_string())?).map_err(|error|error.to_string())?;
            let mut selected = archive.by_index(index).map_err(|error|error.to_string())?;
            if selected.name().map_err(|error|error.to_string())?.as_ref() != entry.archive_name || selected.size() != entry.size { return Err("Stale".into()); }
            selected.by_ref().take(limit as u64 + 1).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
        } else {
            let mut selected = fs::File::open(&entry.path).map_err(|error|error.to_string())?;
            selected.by_ref().take(limit as u64+1).read_to_end(&mut bytes).map_err(|error|error.to_string())?;
        }
        if bytes.len() != entry.size as usize || bytes.len() > limit { return Err("class-input-size-changed".into()); }
        let class = ktcore::classfile::decode(&entry.origin,&bytes).map_err(|error|error.to_string())?;
        if class.identity.internal_name != entry.internal_name { return Err("class-entry-identity-mismatch".into()); }
        if entry.version>0 && u32::from(class.major_version)>u32::from(entry.version)+44 {return Err("versioned class exceeds its Java release".into());}
        if entry.authority == "sdk-api-header" && class.methods.iter().any(|method|method.attribute_names.iter().any(|name|name.as_ref()=="Code")) {
            return Err("sdk-header-has-Code".into());
        }
        if !self.current() { return Err("Stale".into()); }
        Ok(Arc::new(class))
    }
}

pub(crate) fn prepare(plan: CatalogPlan, wake: Arc<dyn Fn()+Send+Sync>) -> PreparedCatalog {
    let catalog = match plan.catalog {
        Some(catalog) if Arc::ptr_eq(&catalog.model,&plan.model) && catalog.current() => Ok(catalog),
        Some(_) => Err("Stale".into()),
        None => JvmCatalog::prepare(plan.model.clone(),&plan.owner,wake),
    };
    let mut classes = BTreeMap::new();
    if let Ok(catalog) = &catalog {
        let mut size = 0u64;
        for index in plan.reads {
            size = size.saturating_add(catalog.entries.get(index).map_or(u64::MAX,|entry|entry.size));
            classes.insert(index, if size > ktcore::classfile::MAX_CLASS_FILE_BYTES as u64 {
                Err(crate::extension_ipc::READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into())
            } else { catalog.read(index) });
        }
    }
    PreparedCatalog {evaluation:plan.evaluation, model:plan.model, catalog, classes}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn selected_model_catalog_and_runtime_share_mr_bytes_and_native_invalidation() {
        let (root,modules)=crate::classpath::tests::sdk_selection_fixture();
        let mut module=modules[1].clone();
        let source=&mut module.source_sets[0];
        source.compiler_tasks.truncate(1);
        source.kotlin_compile_task=None;
        let home=source.compiler_tasks[0].java_home.clone().unwrap();
        let owner=source.source_roots[0].join("Use.java");
        fs::create_dir_all(owner.parent().unwrap()).unwrap();
        fs::write(&owner,"class Use {}").unwrap();
        let output=module.project_dir.join("build/classes/java/main");
        fs::create_dir_all(output.join("pluginfixture")).unwrap();
        let class=include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        fs::write(output.join("pluginfixture/Person.class"),class).unwrap();
        let compile=root.join("compile-only.jar");
        let mr=root.join("multi-release.jar");
        let base=include_bytes!("../tests/fixtures/multi-release/8/Api.class");
        let previous=include_bytes!("../tests/fixtures/multi-release/17/Api.class");
        let selected=include_bytes!("../tests/fixtures/multi-release/21/Api.class");
        let only=include_bytes!("../tests/fixtures/multi-release/11/Only.class");
        let module_info=include_bytes!("../tests/fixtures/multi-release/module-info.class");
        let mut archive=zip::ZipWriter::new(fs::File::create(&mr).unwrap());
        for (name,bytes) in [("META-INF/MANIFEST.MF",&b"Manifest-Version: 1.0\r\nMulti-Release: true\r\n\r\n"[..]),
            ("mr/Api.class",&base[..]),("META-INF/versions/17/mr/Api.class",&previous[..]),
            ("META-INF/versions/21/mr/Api.class",&selected[..]),("META-INF/versions/22/mr/Api.class",b"inactive future class"),
            ("META-INF/versions/11/mr/Only.class",&only[..]),
            ("META-INF/versions/9/module-info.class",&module_info[..])] {
            archive.start_file(name,zip::write::SimpleFileOptions::default()).unwrap();archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
        fs::create_dir_all(home.join("jmods")).unwrap();
        for (path,name) in [(&compile,"pluginfixture/Person.class"),
            (&home.join("jmods/java.base.jmod"),"classes/pluginfixture/Person.class")] {
            let mut archive=zip::ZipWriter::new(fs::File::create(path).unwrap());
            archive.start_file(name,zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(class).unwrap();archive.finish().unwrap();
        }
        source.output_roots=vec![output, module.project_dir.join("build/resources/main")];
        source.entries=vec![compile.clone(),mr.clone()];
        source.runtime=Some(crate::classpath::RuntimeClassInputs {adapter:"gradle-runtime-class-inputs-v1".into(),
            classpath:vec![mr],class_outputs:Vec::new(),complete:true,problem:None});
        module.entries=source.entries.clone();
        let model=Arc::new(ProjectSnapshot::new(vec![module]));
        assert!(model.sdk_api_for_file(&owner).is_some());
        let (invalidated,observed)=std::sync::mpsc::channel();
        let prepared=prepare(CatalogPlan {evaluation:17,model:model.clone(),owner:owner.clone(),
            catalog:None,reads:Vec::new()},Arc::new(move || {let _=invalidated.send(());}));
        assert_eq!(prepared.evaluation,17);
        assert!(Arc::ptr_eq(&prepared.model,&model));
        let catalog=prepared.catalog.unwrap_or_else(|error|panic!("selected catalog failed: {error}"));
        assert!(catalog.current());assert!(catalog.complete,"{:?}",catalog.problems);
        assert_eq!(catalog.entries.len(),5,"the absent physical output and module descriptor do not add ordinary classes");
        for association in ["output","compileClasspath","sdk"] {
            let (index,entry)=catalog.entries.iter().enumerate().find(|(_,entry)|entry.association==association && entry.internal_name=="pluginfixture/Person").unwrap();
            assert_eq!(entry.internal_name,"pluginfixture/Person");
            assert_eq!(entry.authority,if association=="sdk" {"sdk-runtime-input"} else {"compiled-input"});
            assert_eq!(catalog.read(index).unwrap().identity.internal_name,"pluginfixture/Person");
        }
        let (index,entry)=catalog.entries.iter().enumerate().find(|(_,entry)|entry.internal_name=="mr/Api").unwrap();
        assert_eq!(entry.archive_name,"META-INF/versions/21/mr/Api.class");
        let abi=catalog.read(index).unwrap();
        let runtime=crate::runtime_bytes::RuntimeClassScope::for_project(model.clone(),&owner,Arc::new(||{})).unwrap();
        let code=runtime.read_class("mr/Api").unwrap().unwrap();
        assert_eq!(&*code.bytes,&selected[..]);assert_eq!(abi.identity.input,code.class.identity.input);
        assert!(!code.api_header);
        assert!(catalog.entries.iter().any(|entry|entry.internal_name=="mr/Only"));
        assert_eq!(&*runtime.read_class("mr/Only").unwrap().unwrap().bytes,&only[..]);
        fs::OpenOptions::new().append(true).open(&compile).unwrap().write_all(b"changed").unwrap();
        observed.recv_timeout(std::time::Duration::from_secs(5)).expect("selected compile input edit must reach its native observer");
        assert!(!catalog.current());
        assert!(matches!(catalog.read(0),Err(error) if error=="Stale"));
        drop(runtime);drop(catalog);drop(model);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn physical_catalog_pages_duplicates_compile_abi_headers_and_stale_reads() {
        let root=std::env::temp_dir().join(format!("ktlsp-jvm-catalog-{}-{}",std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir_all(&root).unwrap();
        let first=root.join("compile-only.jar"); let second=root.join("duplicate.jar"); let sdk=root.join("ct.sym");
        let class=include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        for path in [&first,&second] {
            let mut archive=zip::ZipWriter::new(fs::File::create(path).unwrap());
            archive.start_file("pluginfixture/Person.class",zip::write::SimpleFileOptions::default()).unwrap();
            archive.write_all(class).unwrap();
            if path==&second {
                // Physical entry pagination is not an assertion that these bodies form valid ABIs.
                for number in 0..6000 {
                    archive.start_file(format!("paging/Entry{number:05}.class"),zip::write::SimpleFileOptions::default()).unwrap();
                }
            }
            archive.finish().unwrap();
        }
        let header=include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Object.sig");
        let mut archive=zip::ZipWriter::new(fs::File::create(&sdk).unwrap());
        archive.start_file("8/java.base/java/lang/Object.sig",zip::write::SimpleFileOptions::default()).unwrap();
        archive.write_all(header).unwrap(); archive.finish().unwrap();
        let plan=WatchPlan::read_inputs(&[first.clone(),second.clone(),sdk.clone()]).unwrap();
        let (invalidated,observed)=std::sync::mpsc::channel();
        let guard=HostInputLease::new(plan.clone(),Arc::new(move || {let _=invalidated.send(());})).unwrap();
        let model=Arc::new(ProjectSnapshot::new(Vec::new()));
        let mut catalog=JvmCatalog {model:model.clone(),entries:Vec::new(),complete:true,problems:Vec::new(),
            guards:vec![guard.clone()],read_coverage:plan.clone(),feature:Some(8)};
        catalog.input(&first,"compile-first","compileClasspath",0,"","compiled-input").unwrap();
        catalog.input(&second,"compile-second","compileClasspath",1,"","compiled-input").unwrap();
        catalog.append(CatalogEntry {internal_name:"java/lang/Object".into(),origin:"selected-release8".into(),
            association:"sdk",order:0,authority:"sdk-api-header",path:sdk,archive_index:Some(0),
            archive_name:"8/java.base/java/lang/Object.sig".into(),size:header.len() as u64,version:0});
        catalog.entries.sort_by(|a,b|a.internal_name.cmp(&b.internal_name).then(a.order.cmp(&b.order)));
        assert!(catalog.complete);
        assert_eq!(catalog.entries.iter().filter(|entry|entry.internal_name=="pluginfixture/Person").count(),2);
        let incomplete=Arc::new(JvmCatalog {model,entries:Vec::new(),complete:false,problems:vec!["incomplete-selected-classpath".into()],
            guards:vec![guard],read_coverage:plan,feature:Some(8)});
        crate::extension_ipc::assert_jvm_catalog_wire(Arc::new(catalog),incomplete,&first,observed);
        fs::remove_dir_all(root).unwrap();
    }
}
