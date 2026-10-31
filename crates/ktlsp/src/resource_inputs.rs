//! Host-selected generic resources. All discovery and byte reads run off the workspace lock.
use std::{collections::{BTreeMap,BTreeSet}, fs::File, io::Read, path::{Path,PathBuf}, sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}}};
use sha2::{Digest,Sha256};
use ktcore::{extensions::HostInputGuard,project_model::ProjectScope};
use crate::{resource_model::ResourceClaims,workspace::OwnedDocumentSnapshot};

pub(crate) struct ResourceCapturePlan {
    pub evaluation:u64,
    pub generation:u64,
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub scope:ProjectScope,
    pub claims:Option<ResourceClaims>,
    pub compile_classpath:Vec<PathBuf>,
    pub runtime_classpath:Option<Vec<PathBuf>>,
    pub runtime_complete:bool,
    pub classpath_complete:bool,
    pub directory_inputs:BTreeSet<PathBuf>,
    pub configuration_base:PathBuf,
    pub overlays:BTreeMap<String,OwnedDocumentSnapshot>,
    pub overlay_epoch:Arc<AtomicBool>,
}

pub(crate) struct PreparedResourceCatalog {
    pub evaluation:u64,
    pub generation:u64,
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub catalog:Result<Arc<ResourceCatalog>,&'static str>,
}

#[derive(Debug)]
struct ResourceGuard {
    lease:Arc<crate::compiled_watch::HostInputLease>,
    overlay_epoch:Arc<AtomicBool>,
}
impl HostInputGuard for ResourceGuard {
    fn is_current(&self)->bool { !self.overlay_epoch.load(Ordering::Acquire) && self.lease.is_current() }
}

/// Runs only in the existing blocking IPC worker. Native coverage is armed before
/// traversing any directory or accepting positive/negative inventory facts.
pub(crate) fn prepare_resource_catalog(plan:ResourceCapturePlan,wake:Arc<dyn Fn()+Send+Sync>)->PreparedResourceCatalog {
    let evaluation=plan.evaluation;let generation=plan.generation;let model=plan.model.clone();
    let prepare=||->Result<Arc<ResourceCatalog>,&'static str> {
        if plan.overlay_epoch.load(Ordering::Acquire) { return Err("stale-resource-overlay"); }
        let mut inputs:BTreeMap<String,PathBuf>=BTreeMap::new();
        let claims=plan.claims.as_ref().filter(|claims|claims.valid());
        for path in claims.into_iter().flat_map(|claims|claims.roots.iter().chain(&claims.output_roots).chain(&claims.configuration_inputs))
            .chain(&plan.compile_classpath).chain(plan.runtime_classpath.iter().flatten()) {
            inputs.entry(crate::paths::file_key(path)).or_insert_with(||path.clone());
        }
        let coverage=crate::compiled_watch::WatchPlan::read_inputs(&inputs.into_values().collect::<Vec<_>>())
            .map_err(|_|"resource-watch-coverage-unavailable")?;
        let lease=crate::compiled_watch::HostInputLease::new(coverage.clone(),wake).map_err(|_|"resource-native-watch-unavailable")?;
        let guard:Arc<dyn HostInputGuard>=Arc::new(ResourceGuard {lease,overlay_epoch:plan.overlay_epoch});
        if !guard.is_current() { return Err("stale-resource-overlay"); }
        if plan.claims.as_ref().is_some_and(|claims|!plan.model.resource_configuration_current(&claims.configuration_inputs)) {
            return Err("resource-model-configuration-not-current");
        }
        Ok(Arc::new(ResourceCatalog::prepare(ResourceSelection {
            model:plan.model,scope:plan.scope,claims:plan.claims,compile_classpath:plan.compile_classpath,
            runtime_classpath:plan.runtime_classpath,runtime_complete:plan.runtime_complete,
            classpath_complete:plan.classpath_complete,directory_inputs:plan.directory_inputs,
            configuration_base:plan.configuration_base,overlays:plan.overlays,guard,read_coverage:coverage,
        })))
    };
    PreparedResourceCatalog {evaluation,generation,model,catalog:prepare()}
}

static NEXT_CATALOG:AtomicU64=AtomicU64::new(1);
#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord,serde::Deserialize)]
#[serde(rename_all="camelCase")]
pub(crate) enum ResourceAssociation { Source, Output, CompileClasspath, RuntimeClasspath, Configuration }
impl ResourceAssociation {
    pub(crate) fn name(self)->&'static str { match self {
        Self::Source=>"source",Self::Output=>"output",Self::CompileClasspath=>"compileClasspath",
        Self::RuntimeClasspath=>"runtimeClasspath",Self::Configuration=>"configuration",
    }}
}

#[derive(Clone,Debug)]
pub(crate) enum ResourceLocation { File(PathBuf), Archive { archive:PathBuf, entry:String, index:usize } }

#[derive(Clone)]
pub(crate) struct ResourceEntry {
    pub association:ResourceAssociation,
    pub order:usize,
    pub relative_path:String,
    pub location:ResourceLocation,
    pub document:Option<OwnedDocumentSnapshot>,
    pub editable:bool,
}
impl std::fmt::Debug for ResourceEntry {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ResourceEntry").field("association",&self.association).field("order",&self.order)
            .field("relative_path",&self.relative_path).field("location",&self.location)
            .field("document_version",&self.document.as_ref().and_then(|doc|doc.document_version)).finish()
    }
}

pub(crate) struct ResourceSelection {
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub scope:ProjectScope,
    pub claims:Option<ResourceClaims>,
    pub compile_classpath:Vec<PathBuf>,
    pub runtime_classpath:Option<Vec<PathBuf>>,
    pub runtime_complete:bool,
    pub classpath_complete:bool,
    pub directory_inputs:BTreeSet<PathBuf>,
    pub configuration_base:PathBuf,
    pub overlays:BTreeMap<String,OwnedDocumentSnapshot>,
    pub guard:Arc<dyn HostInputGuard>,
    pub read_coverage:crate::compiled_watch::WatchPlan,
}

pub(crate) struct ResourceCatalog {
    pub revision:u64,
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub scope:ProjectScope,
    pub entries:Vec<ResourceEntry>,
    pub complete:bool,
    pub problems:Vec<&'static str>,
    pub guard:Arc<dyn HostInputGuard>,
    read_coverage:crate::compiled_watch::WatchPlan,
    bodies:parking_lot::Mutex<CachedResourceBodies>,
}
#[derive(Default)]
struct CachedResourceBodies {bytes:usize,entries:BTreeMap<usize,Arc<ResourceBytes>>}
impl std::fmt::Debug for ResourceCatalog {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ResourceCatalog").field("scope",&self.scope).field("entries",&self.entries.len())
            .field("complete",&self.complete).field("problems",&self.problems).finish()
    }
}

impl ResourceCatalog {
    pub(crate) fn prepare(selection:ResourceSelection)->Self {
        let mut result=Self {revision:NEXT_CATALOG.fetch_add(1,Ordering::Relaxed),model:selection.model,scope:selection.scope,entries:Vec::new(),complete:true,
            problems:Vec::new(),guard:selection.guard,read_coverage:selection.read_coverage,bodies:parking_lot::Mutex::new(CachedResourceBodies::default())};
        if !result.guard.is_current() { result.problem("stale-resource-selection");return result; }
        match selection.claims.as_ref().filter(|claims|claims.valid()) {
            Some(claims)=>{
                if !claims.complete {result.problem("incomplete-resource-selection");}
                for (order,tree) in claims.trees.iter().enumerate() {
                    if !tree.future_complete {result.problem("opaque-resource-filter");continue;}
                    result.directory(&tree.root,ResourceAssociation::Source,order,
                        &selection.overlays,|relative|claims.selects(tree,relative)==Some(true),true);
                }
                // Explicit selected files retain positive authority even when a custom
                // selector cannot attest future membership or absence.
                for file in &claims.files {
                    if result.entries.iter().any(|entry|entry.association==ResourceAssociation::Source
                        && entry.relative_path==file.relative_path
                        && matches!(&entry.location,ResourceLocation::File(path) if crate::paths::file_key(path)==crate::paths::file_key(&file.path))) {continue;}
                    let Some(order)=claims.roots.iter().position(|root|file.path.starts_with(root)) else {result.problem("resource-root-escape");continue;};
                    if !result.read_coverage.covers_read(&crate::paths::file_key(&file.path),false) {
                        result.problem("resource-read-not-prearmed");continue;
                    }
                    if !selection.overlays.contains_key(&crate::paths::file_key(&file.path)) {
                        match file.path.symlink_metadata() {
                            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink()=>{},
                            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>continue,
                            _=>{result.problem("unreadable-selected-resource");continue;},
                        }
                    }
                    result.entries.push(ResourceEntry {association:ResourceAssociation::Source,order,
                        relative_path:file.relative_path.clone(),location:ResourceLocation::File(file.path.clone()),
                        document:selection.overlays.get(&crate::paths::file_key(&file.path)).cloned(),editable:true});
                }
                for (order,root) in claims.output_roots.iter().enumerate() {
                    result.directory(root,ResourceAssociation::Output,order,&selection.overlays,|_|true,false);
                }
                for (order,path) in claims.configuration_inputs.iter().enumerate() {
                    let relative=path.strip_prefix(&selection.configuration_base).ok().map(crate::paths::file_key)
                        .filter(|relative|crate::resource_model::safe_relative(relative))
                        .or_else(||path.file_name().and_then(|name|name.to_str()).map(str::to_owned));
                    let Some(name)=relative else {result.problem("invalid-configuration-name");continue;};
                    let overlay=selection.overlays.get(&crate::paths::file_key(path)).cloned();
                    match path.symlink_metadata() {
                        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink()=>
                            result.entries.push(ResourceEntry {association:ResourceAssociation::Configuration,order,
                                relative_path:name.clone(),location:ResourceLocation::File(path.clone()),document:overlay,editable:true}),
                        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{
                            if overlay.is_some() { result.entries.push(ResourceEntry {association:ResourceAssociation::Configuration,order,
                                relative_path:name.clone(),location:ResourceLocation::File(path.clone()),document:overlay,editable:true}); }
                        }
                        _=>result.problem("unreadable-configuration-input"),
                    }
                }
            }
            None=>result.problem("resource-model-unavailable"),
        }
        if !selection.classpath_complete {result.problem("incomplete-compile-resource-classpath");}
        for (order,path) in selection.compile_classpath.iter().enumerate() {
            if selection.directory_inputs.contains(path) && !path.try_exists().unwrap_or(true) { continue; }
            result.classpath(path,ResourceAssociation::CompileClasspath,order,&selection.overlays);
        }
        match &selection.runtime_classpath {
            Some(paths)=>{
                if !selection.runtime_complete {result.problem("incomplete-runtime-resource-classpath");}
                for (order,path) in paths.iter().enumerate() {
                    if selection.directory_inputs.contains(path) && !path.try_exists().unwrap_or(true) { continue; }
                    result.classpath(path,ResourceAssociation::RuntimeClasspath,order,&selection.overlays);
                }
            }
            None=>result.problem("runtime-resource-classpath-unavailable"),
        }
        result.entries.sort_by(|a,b|(a.association,a.order,&a.relative_path).cmp(&(b.association,b.order,&b.relative_path)));
        if !result.guard.is_current() {result.problem("stale-resource-inventory");}
        result
    }

    fn problem(&mut self,problem:&'static str) { self.complete=false;if !self.problems.contains(&problem) {self.problems.push(problem);} }

    fn cached_body(&self,index:usize)->Option<Arc<ResourceBytes>> {
        if !self.guard.is_current() {return None;}
        self.bodies.lock().entries.get(&index).cloned()
    }
    fn cache_body(&self,index:usize,body:Arc<ResourceBytes>)->Arc<ResourceBytes> {
        let mut bodies=self.bodies.lock();
        if let Some(previous)=bodies.entries.get(&index) {return previous.clone();}
        if bodies.bytes+body.bytes.len()<=64*1024*1024 {
            bodies.bytes+=body.bytes.len();bodies.entries.insert(index,body.clone());
        }
        body
    }

    fn directory(&mut self,root:&Path,association:ResourceAssociation,order:usize,
        overlays:&BTreeMap<String,OwnedDocumentSnapshot>,select:impl Fn(&str)->bool,editable:bool) {
        if !self.guard.is_current() {self.problem("stale-resource-inventory");return;}
        let mut files=BTreeMap::new();
        let present=match root.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink()=>true,
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>false,
            _=>{self.problem("unreadable-resource-root");return;},
        };
        if present {
            if !self.read_coverage.covers_read(&crate::paths::file_key(root),true) {
                self.problem("resource-tree-not-prearmed");return;
            }
            for entry in walkdir::WalkDir::new(root).follow_links(false) {
                if !self.guard.is_current() {self.problem("stale-resource-inventory");break;}
                let entry=match entry {Ok(entry)=>entry,Err(_)=>{self.problem("unreadable-resource-tree");continue;}};
                if entry.file_type().is_symlink() {self.problem("symlink-resource-input");continue;}
                if !entry.file_type().is_file() {continue;}
                let Ok(relative)=entry.path().strip_prefix(root) else {self.problem("resource-root-escape");continue;};
                let relative=crate::paths::file_key(relative);
                if !crate::resource_model::safe_relative(&relative) {self.problem("invalid-resource-relative-name");continue;}
                if select(&relative) {files.insert(relative,entry.path().to_path_buf());}
                if files.len()>1_000_000 {self.problem("resource-inventory-limit");break;}
            }
        }
        for path in overlays.keys().map(Path::new) {
            let Ok(relative)=path.strip_prefix(root) else {continue;};
            let relative=crate::paths::file_key(relative);
            if crate::resource_model::safe_relative(&relative) && select(&relative) { files.insert(relative,path.into()); }
        }
        for (relative_path,path) in files {
            if self.entries.len()>=1_000_000 {self.problem("resource-inventory-limit");break;}
            self.entries.push(ResourceEntry {association,order,relative_path,
                document:overlays.get(&crate::paths::file_key(&path)).cloned(),location:ResourceLocation::File(path),editable});
        }
    }

    fn classpath(&mut self,path:&Path,association:ResourceAssociation,order:usize,
        overlays:&BTreeMap<String,OwnedDocumentSnapshot>) {
        if !self.read_coverage.covers_read(&crate::paths::file_key(path),false) { self.problem("resource-read-not-prearmed");return; }
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink()=>{
                self.directory(path,association,order,overlays,|relative|!relative.ends_with(".class"),false);
            }
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink()=>{
                let archive=File::open(path).and_then(|file|zip::ZipArchive::new(file).map_err(std::io::Error::other));
                let mut archive=match archive {Ok(archive)=>archive,Err(_)=>{self.problem("unreadable-resource-archive");return;}};
                for index in 0..archive.len() {
                    if self.entries.len()>=1_000_000 {self.problem("resource-inventory-limit");break;}
                    if !self.guard.is_current() {self.problem("stale-resource-inventory");break;}
                    let entry=match archive.by_index(index) {Ok(entry)=>entry,Err(_)=>{self.problem("invalid-resource-archive-entry");continue;}};
                    let name=match entry.name() {Ok(name)=>name.into_owned(),Err(_)=>{self.problem("invalid-resource-archive-name");continue;}};
                    if entry.is_dir() || name.ends_with(".class") {continue;}
                    if entry.unix_mode().is_some_and(|mode|mode & 0o170000 == 0o120000) {self.problem("symlink-resource-entry");continue;}
                    if !crate::resource_model::safe_relative(&name) {self.problem("resource-archive-root-escape");continue;}
                    self.entries.push(ResourceEntry {association,order,relative_path:name.clone(),
                        location:ResourceLocation::Archive {archive:path.into(),entry:name,index},document:None,editable:false});
                }
            }
            _=>self.problem("unavailable-resource-classpath-entry"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ResourceBytes {
    pub bytes:Arc<[u8]>,
    pub sha256:String,
    pub document_version:Option<i32>,
    pub utf8:bool,
    pub guard:Arc<dyn HostInputGuard>,
}

pub(crate) struct ResourceReadPlan {
    pub evaluation:u64,
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub token:String,
    pub catalog:Arc<ResourceCatalog>,
    pub index:usize,
    pub entry:ResourceEntry,
    pub guard:Arc<dyn HostInputGuard>,
    pub byte_budget:usize,
}
pub(crate) struct PreparedResourceRead {
    pub evaluation:u64,
    pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub token:String,
    pub content:Result<Arc<ResourceBytes>,&'static str>,
}
pub(crate) fn prepare_resource_read(plan:ResourceReadPlan)->PreparedResourceRead {
    let content=if let Some(body)=plan.catalog.cached_body(plan.index) {Ok(body)}
        else {prepare_resource_bytes(&plan.entry,plan.guard,plan.byte_budget).map(|body|plan.catalog.cache_body(plan.index,Arc::new(body)))};
    PreparedResourceRead {evaluation:plan.evaluation,model:plan.model,token:plan.token,content}
}

pub(crate) fn read_current(reader:&mut impl Read,guard:&dyn HostInputGuard,limit:u64)->Result<Vec<u8>,&'static str> {
    let mut bytes=Vec::new();let mut chunk=[0;64*1024];
    loop {
        if !guard.is_current() {return Err("stale-resource-input");}
        let count=reader.read(&mut chunk).map_err(|_|"invalid-resource-payload")?;
        if count==0 {break;}
        if bytes.len() as u64+count as u64>limit {return Err("unsupported-resource-size");}
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(bytes)
}
impl std::fmt::Debug for ResourceBytes {
    fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result {
        f.debug_struct("ResourceBytes").field("length",&self.bytes.len()).field("sha256",&self.sha256)
            .field("document_version",&self.document_version).finish()
    }
}

pub(crate) fn prepare_resource_bytes(entry:&ResourceEntry,guard:Arc<dyn HostInputGuard>,budget:usize)->Result<ResourceBytes,&'static str> {
    if !guard.is_current() {return Err("stale-resource-input");}
    let limit=(64*1024*1024u64).min(budget as u64);
    let bytes:Vec<u8>=if let Some(document)=&entry.document {
        if document.text.len() as u64>limit {return Err("resource-byte-budget-exceeded");}
        document.text.as_bytes().into()
    }
    else {match &entry.location {
        ResourceLocation::File(path)=>{
            let meta=path.symlink_metadata().map_err(|_|"unavailable-resource-file")?;
            if !meta.is_file() || meta.file_type().is_symlink() || meta.len()>limit {return Err("unsupported-resource-file");}
            let mut file=File::open(path).map_err(|_|"unreadable-resource-file")?;
            let bytes=read_current(&mut file,guard.as_ref(),limit)?;
            let after=path.symlink_metadata().map_err(|_|"stale-resource-file")?;
            if after.len()!=meta.len() || after.modified().ok()!=meta.modified().ok() || bytes.len() as u64!=meta.len() {return Err("stale-resource-file");}
            bytes
        }
        ResourceLocation::Archive {archive,entry,index}=>{
            let file=File::open(archive).map_err(|_|"unavailable-resource-archive")?;
            let mut zip=zip::ZipArchive::new(file).map_err(|_|"unreadable-resource-archive")?;
            let mut file=zip.by_index(*index).map_err(|_|"unavailable-resource-entry")?;
            if file.name().map_err(|_|"invalid-resource-entry-name")?.as_ref()!=entry {return Err("stale-resource-entry");}
            if file.is_dir() || file.size()>limit {return Err("unsupported-resource-entry");}
            read_current(&mut file,guard.as_ref(),limit)?
        }
    }};
    if bytes.len() as u64>limit || !guard.is_current() {return Err("stale-or-oversized-resource-input");}
    Ok(ResourceBytes {sha256:format!("{:x}",Sha256::digest(&bytes)),utf8:std::str::from_utf8(&bytes).is_ok(),bytes:bytes.into(),
        document_version:entry.document.as_ref().and_then(|doc|doc.document_version),guard})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_model::{ResourceFile,ResourcePatterns,ResourceTree};
    use std::io::Write;
    #[derive(Debug)]
    struct Flag(AtomicBool);
    impl HostInputGuard for Flag {fn is_current(&self)->bool {!self.0.load(Ordering::Acquire)}}
    struct Temp(PathBuf);
    impl Temp {
        fn new()->Self {
            let path=std::env::temp_dir().join(format!("ktlsp-resource-inputs-{}-{}",std::process::id(),NEXT_CATALOG.fetch_add(1,Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();Self(path)
        }
    }
    impl Drop for Temp {fn drop(&mut self) {let _=std::fs::remove_dir_all(&self.0);}}

    #[test]
    fn owned_resource_catalog_uses_current_wire_binary_payloads_and_ordered_archives() {
        let dir=Temp::new();let root=dir.0.join("resources");let output=dir.0.join("processed");
        std::fs::create_dir_all(root.join("META-INF")).unwrap();std::fs::create_dir_all(&output).unwrap();
        let source=root.join("config.json");std::fs::write(&source,"disk bytes").unwrap();
        std::fs::write(root.join("not-selected.txt"),"excluded").unwrap();
        let binary=output.join("payload.bin");std::fs::write(&binary,[0,255,128]).unwrap();
        let jar=dir.0.join("dep.jar");let mut writer=zip::ZipWriter::new(File::create(&jar).unwrap());
        writer.start_file("META-INF/services/vendor.Service",zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(b"vendor.Implementation\n").unwrap();
        writer.start_file("vendor/Hidden.class",zip::write::SimpleFileOptions::default()).unwrap();writer.write_all(b"class bytes are not resources").unwrap();writer.finish().unwrap();
        let claims=ResourceClaims {adapter:"gradle-source-set-resources-v1".into(),roots:vec![root.clone()],
            trees:vec![ResourceTree{root:root.clone(),patterns:vec![ResourcePatterns{includes:vec!["**/*.json".into()],excludes:vec![]}],future_complete:true}],
            files:vec![ResourceFile{path:source.clone(),relative_path:"config.json".into()}],output_roots:vec![output.clone()],
            configuration_inputs:vec![dir.0.join("gradle.properties")],default_excludes:vec![],complete:true,problem:None};
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let overlay=OwnedDocumentSnapshot{text:Arc::from("{\"雪\":\"😀 unsaved\"}\n"),document_version:Some(7),language_id:"json".into(),revision:4};
        let overlays=BTreeMap::from([(crate::paths::file_key(&source),overlay.clone())]);
        let coverage=crate::compiled_watch::WatchPlan::read_inputs(&[root.clone(),output.clone(),jar.clone(),dir.0.join("gradle.properties")]).unwrap();
        let mut catalog=ResourceCatalog::prepare(ResourceSelection {
            model:Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new())),scope:ProjectScope{module:"resource-fixture".into(),source_set:"main".into()},
            claims:Some(claims.clone()),compile_classpath:vec![jar.clone()],runtime_classpath:Some(vec![jar.clone()]),runtime_complete:true,
            classpath_complete:true,directory_inputs:BTreeSet::new(),configuration_base:dir.0.clone(),overlays:overlays.clone(),guard:flag.clone(),read_coverage:coverage.clone(),
        });
        assert!(catalog.complete,"{:?}",catalog.problems);
        assert!(!catalog.entries.iter().any(|entry|entry.relative_path.ends_with(".class") || entry.relative_path=="not-selected.txt"));
        let entry=catalog.entries.iter().find(|entry|entry.association==ResourceAssociation::Source).unwrap();
        let content=prepare_resource_bytes(entry,flag.clone(),1024).unwrap();
        assert_eq!(&*content.bytes,overlay.text.as_bytes());assert!(content.utf8);assert_eq!(content.document_version,Some(7));
        assert_eq!(content.sha256,format!("{:x}",Sha256::digest(overlay.text.as_bytes())));
        assert_eq!(std::fs::read_to_string(&source).unwrap(),"disk bytes","reading an unsaved overlay never rewrites the resource");
        assert!(prepare_resource_bytes(entry,flag.clone(),1).is_err());
        let binary_entry=catalog.entries.iter().find(|entry|entry.relative_path=="payload.bin").unwrap();
        let content=prepare_resource_bytes(binary_entry,flag.clone(),1024).unwrap();assert!(!content.utf8);assert_eq!(&*content.bytes,&[0,255,128]);
        let archives:Vec<_>=catalog.entries.iter().filter(|entry|entry.relative_path=="META-INF/services/vendor.Service").collect();
        assert_eq!(archives.len(),2);assert_eq!(archives[0].association,ResourceAssociation::CompileClasspath);assert_eq!(archives[1].association,ResourceAssociation::RuntimeClasspath);
        assert_eq!(&*prepare_resource_bytes(archives[0],flag.clone(),1024).unwrap().bytes,b"vendor.Implementation\n");
        // Custom selectors retain actual selected inputs but never admit new overlays.
        let mut opaque=claims;opaque.complete=false;opaque.trees[0].future_complete=false;
        let mut extra=overlays;extra.insert(crate::paths::file_key(&root.join("new.json")),overlay);
        catalog=ResourceCatalog::prepare(ResourceSelection {
            model:catalog.model.clone(),scope:catalog.scope.clone(),claims:Some(opaque),compile_classpath:vec![],runtime_classpath:Some(vec![]),
            runtime_complete:true,classpath_complete:true,directory_inputs:BTreeSet::new(),configuration_base:dir.0.clone(),overlays:extra,
            guard:flag.clone(),read_coverage:coverage,
        });
        assert!(!catalog.complete);assert!(catalog.entries.iter().any(|entry|entry.relative_path=="config.json"));
        assert!(!catalog.entries.iter().any(|entry|entry.relative_path=="new.json"));
        flag.0.store(true,Ordering::Release);
        assert!(prepare_resource_bytes(&catalog.entries[0],flag,1024).is_err());
    }

    #[test]
    fn resource_payload_crc_and_mid_read_invalidation_never_issue_partial_content() {
        let dir=Temp::new();let jar=dir.0.join("bad.jar");let payload=b"UNIQUE-RESOURCE-PAYLOAD";
        let mut zip=zip::ZipWriter::new(File::create(&jar).unwrap());
        zip.start_file("a.bin",zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored)).unwrap();zip.write_all(payload).unwrap();zip.finish().unwrap();
        let mut bytes=std::fs::read(&jar).unwrap();let offset=bytes.windows(payload.len()).position(|slice|slice==payload).unwrap();bytes[offset]^=1;std::fs::write(&jar,bytes).unwrap();
        let flag=Arc::new(Flag(AtomicBool::new(false)));
        let entry=ResourceEntry{association:ResourceAssociation::CompileClasspath,order:0,relative_path:"a.bin".into(),
            location:ResourceLocation::Archive{archive:jar,entry:"a.bin".into(),index:0},document:None,editable:false};
        assert!(prepare_resource_bytes(&entry,flag,1024).is_err(),"an accepted archive directory is not a payload CRC certificate");
        #[derive(Debug)] struct CancelOnChunk(std::sync::atomic::AtomicUsize);
        impl HostInputGuard for CancelOnChunk {fn is_current(&self)->bool {self.0.fetch_add(1,Ordering::AcqRel)<2}}
        let guard=CancelOnChunk(std::sync::atomic::AtomicUsize::new(0));
        assert!(read_current(&mut std::io::Cursor::new(vec![1;192*1024]),&guard,256*1024).is_err());
    }
}
