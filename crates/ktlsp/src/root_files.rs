//! Filtered files under a host-registered workspace root; no Gradle resource policy.
use std::{collections::{BTreeMap,BTreeSet},path::{Path,PathBuf},sync::{Arc,atomic::{AtomicBool,AtomicU64,Ordering}}};
use ktcore::extensions::HostInputGuard;
use sha2::{Digest,Sha256};
use crate::workspace::OwnedDocumentSnapshot;

const MAX_VISITED:usize=100_000;
const MAX_FILES:usize=16_000;
const MAX_BODY:usize=16*1024*1024;
static NEXT_ROOT:AtomicU64=AtomicU64::new(1);

#[derive(Clone,Debug,Default,PartialEq,Eq,PartialOrd,Ord,serde::Deserialize,serde::Serialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub(crate) struct Filter {
    #[serde(default)] pub basenames:BTreeSet<String>,
    #[serde(default)] pub extensions:BTreeSet<String>,
    #[serde(default)] pub excluded_directories:BTreeSet<String>,
}
impl Filter {
    pub(crate) fn validate(&self)->Result<(),String> {
        let literal=|value:&str| !value.is_empty() && value.len()<=256 && value!="." && value!=".."
            && !value.chars().any(|c| c.is_control() || matches!(c,'/'|'\\'|':'|'*'|'?'|'['|']'));
        if self.basenames.len()+self.extensions.len()+self.excluded_directories.len()>128
            || self.basenames.iter().chain(&self.extensions).chain(&self.excluded_directories).any(|value|!literal(value))
            || self.extensions.iter().any(|value|value.starts_with('.')) {return Err("invalid root-file filter".into());}
        Ok(())
    }
    pub(crate) fn effective(&self)->Self {
        Self {basenames:self.basenames.iter().map(|name|name.to_lowercase()).collect(),
            extensions:self.extensions.iter().map(|name|name.to_lowercase()).collect(),
            excluded_directories:self.excluded_directories.iter().map(|name|name.to_lowercase()).collect()}
    }
    fn includes(&self,relative:&Path,excluded:&BTreeSet<String>)->bool {
        if relative.parent().is_some_and(|parents|parents.components().any(|part|
            excluded.contains(&part.as_os_str().to_string_lossy().to_lowercase()))) {return false;}
        let name=relative.file_name().and_then(|name|name.to_str());
        (self.basenames.is_empty() && self.extensions.is_empty())
            || name.is_some_and(|name|self.basenames.contains(&name.to_lowercase()))
            || relative.extension().and_then(|extension|extension.to_str()).is_some_and(|extension|self.extensions.contains(&extension.to_lowercase()))
    }
}

pub(crate) struct CapturePlan {
    pub evaluation:u64,pub generation:u64,pub model:Arc<crate::classpath::ProjectSnapshot>,
    pub root:PathBuf,pub filter:Filter,pub overlays:BTreeMap<String,OwnedDocumentSnapshot>,pub epoch:Arc<AtomicBool>,
}
pub(crate) struct PreparedCatalog {
    pub evaluation:u64,pub generation:u64,pub model:Arc<crate::classpath::ProjectSnapshot>,pub root:PathBuf,
    pub catalog:Result<Arc<Catalog>,String>,
    pub filter:Filter,
}
#[derive(Debug)]
struct Guard {lease:Arc<crate::compiled_watch::HostInputLease>,epoch:Arc<AtomicBool>}
impl HostInputGuard for Guard {fn is_current(&self)->bool {!self.epoch.load(Ordering::Acquire) && self.lease.is_current()}}
pub(crate) struct Entry {pub path:PathBuf,pub relative:String,pub uri:String,pub document:Option<OwnedDocumentSnapshot>}
pub(crate) struct Catalog {
    pub revision:u64,pub root:PathBuf,pub uri:String,pub filter:Filter,pub entries:Vec<Entry>,
    pub complete:bool,pub problems:BTreeSet<&'static str>,pub guard:Arc<dyn HostInputGuard>,
}
pub(crate) fn uri(path:&Path)->Result<String,String> {
    tower_lsp_server::ls_types::Uri::from_file_path(path).map(|uri|uri.to_string()).ok_or_else(||"invalid root-file URI".into())
}
fn safe_path(root:&Path,path:&Path,allow_missing:bool)->Result<(),String> {
    let relative=path.strip_prefix(root).map_err(|_|"root-file-escape")?;
    if relative.components().any(|part|matches!(part,std::path::Component::ParentDir|std::path::Component::CurDir)) {
        return Err("root-file-escape".into());
    }
    for ancestor in path.ancestors().take_while(|ancestor|ancestor.starts_with(root)) {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() || (ancestor!=path && !meta.is_dir())
                || (ancestor==path && !meta.is_file())=>return Err("unsafe-root-file-path".into()),
            Err(error) if allow_missing && ancestor!=root && error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(_)=>return Err("unavailable-root-file-path".into()),
            _=>{}
        }
    }
    Ok(())
}
pub(crate) fn prepare(plan:CapturePlan,wake:Arc<dyn Fn()+Send+Sync>)->PreparedCatalog {
    prepare_with_limits(plan,wake,MAX_VISITED,MAX_FILES)
}
fn prepare_with_limits(plan:CapturePlan,wake:Arc<dyn Fn()+Send+Sync>,visited_limit:usize,file_limit:usize)->PreparedCatalog {
    let mut result=Err("Stale".to_owned());
    for _ in 0..3 {
        result=(||->Result<Arc<Catalog>,String> {
        if plan.epoch.load(Ordering::Acquire) {return Err("Stale".into());}
        plan.filter.validate()?;
        let effective=plan.filter.effective();let excluded=&effective.excluded_directories;
        let coverage=crate::compiled_watch::WatchPlan::filtered_root(&plan.root,excluded).map_err(|error|error.to_string())?;
        let lease=crate::compiled_watch::HostInputLease::new(coverage,wake.clone()).map_err(|error|error.to_string())?;
        let guard:Arc<dyn HostInputGuard>=Arc::new(Guard {lease,epoch:plan.epoch.clone()});
        let mut catalog=Catalog {revision:NEXT_ROOT.fetch_add(1,Ordering::Relaxed),root:plan.root.clone(),uri:uri(&plan.root)?,
            filter:plan.filter.clone(),entries:Vec::new(),complete:true,problems:BTreeSet::new(),guard};
        let mut files=BTreeMap::new();
        let walk=walkdir::WalkDir::new(&plan.root).follow_links(false).into_iter().filter_entry(|entry|
            entry.depth()==0 || !entry.file_type().is_dir()
                || !excluded.contains(&entry.file_name().to_string_lossy().to_lowercase()));
        for (visited,entry) in walk.enumerate() {
            if visited>=visited_limit || files.len()>=file_limit {catalog.problems.insert("root-file-inventory-limit");break;}
            if !catalog.guard.is_current() {return Err("Stale".into());}
            let entry=match entry {Ok(entry)=>entry,Err(_)=>{catalog.problems.insert("unreadable-root-file-tree");continue;}};
            if entry.file_type().is_symlink() {catalog.problems.insert("symlink-root-file");continue;}
            if !entry.file_type().is_file() {continue;}
            let relative=entry.path().strip_prefix(&plan.root).map_err(|_|"root-file-escape")?;
            if effective.includes(relative,excluded) {files.insert(crate::paths::file_key(entry.path()),entry.path().to_path_buf());}
        }
        for key in plan.overlays.keys() {
            let path=Path::new(key);
            let Ok(relative)=path.strip_prefix(&plan.root) else {continue;};
            if relative.components().any(|part|matches!(part,std::path::Component::ParentDir|std::path::Component::CurDir)) {return Err("root-file-overlay-escape".into());}
            if effective.includes(relative,excluded) {files.insert(crate::paths::file_key(path),path.into());}
        }
        if files.len()>file_limit {catalog.problems.insert("root-file-inventory-limit");}
        for (key,path) in files.into_iter().take(file_limit) {
            if safe_path(&plan.root,&path,plan.overlays.contains_key(&key)).is_err() {catalog.problems.insert("unsafe-root-file-path");continue;}
            catalog.entries.push(Entry {relative:crate::paths::file_key(path.strip_prefix(&plan.root).map_err(|_|"root-file-escape")?),
                uri:uri(&path)?,document:plan.overlays.get(&key).cloned(),path});
        }
        if !catalog.guard.is_current() {return Err("Stale".into());}
        catalog.complete=catalog.problems.is_empty();Ok(Arc::new(catalog))
        })();
        // Retry acquisition only: a stale attempt contributes no entries or read authority.
        if !matches!(&result,Err(error) if error=="Stale") || plan.epoch.load(Ordering::Acquire) {break;}
    }
    PreparedCatalog {evaluation:plan.evaluation,generation:plan.generation,model:plan.model,root:plan.root,filter:plan.filter,catalog:result}
}

pub(crate) struct Body {pub text:Arc<str>,pub sha256:String,pub revision:u64,pub document_version:Option<i32>}
pub(crate) struct ReadPlan {pub evaluation:u64,pub file:String,pub catalog:Arc<Catalog>,pub index:usize,pub byte_budget:usize}
pub(crate) struct PreparedRead {pub evaluation:u64,pub file:String,pub catalog:Arc<Catalog>,pub body:Result<Arc<Body>,String>}
pub(crate) fn validate_cached_read(results:&BTreeMap<String,Result<Arc<Body>,String>>,file:&str,
    body:&Result<Arc<Body>,String>,limit:usize)->Result<(),String> {
    let bytes=results.iter().filter(|(key,_)|key.as_str()!=file).filter_map(|(_,body)|body.as_ref().ok())
        .try_fold(0usize,|used,body|used.checked_add(body.text.len())).ok_or("root-file cache budget exceeded")?;
    if bytes.checked_add(body.as_ref().map_or(0,|body|body.text.len())).is_none_or(|bytes|bytes>limit) {
        return Err("root-file cache budget exceeded".into());
    }
    Ok(())
}
pub(crate) fn read(plan:ReadPlan)->PreparedRead {
    let body=(||->Result<Arc<Body>,String> {
        if !plan.catalog.guard.is_current() {return Err("Stale".into());}
        let entry=plan.catalog.entries.get(plan.index).ok_or("foreign root-file token")?;
        safe_path(&plan.catalog.root,&entry.path,entry.document.is_some())?;
        let limit=MAX_BODY.min(plan.byte_budget);
        let (text,revision,document_version)=if let Some(document)=&entry.document {
            if document.text.len()>limit {return Err("root-file-size-limit".into());}
            (document.text.clone(),document.revision,document.document_version)
        } else {
            let before=std::fs::symlink_metadata(&entry.path).map_err(|_|"unreadable-root-file")?;
            if !before.is_file() || before.file_type().is_symlink() || before.len()>limit as u64 {return Err("unsupported-root-file".into());}
            let mut file=std::fs::File::open(&entry.path).map_err(|_|"unreadable-root-file")?;
            let bytes=crate::resource_inputs::read_current(&mut file,plan.catalog.guard.as_ref(),limit as u64).map_err(str::to_owned)?;
            let after=std::fs::symlink_metadata(&entry.path).map_err(|_|"Stale")?;
            if !after.is_file() || after.file_type().is_symlink() || after.len()!=before.len()
                || after.modified().ok()!=before.modified().ok() || bytes.len() as u64!=before.len() {return Err("Stale".into());}
            (Arc::<str>::from(String::from_utf8(bytes).map_err(|_|"root-file-not-utf8")?),plan.catalog.revision,None)
        };
        safe_path(&plan.catalog.root,&entry.path,entry.document.is_some())?;
        if !plan.catalog.guard.is_current() {return Err("Stale".into());}
        Ok(Arc::new(Body {sha256:format!("{:x}",Sha256::digest(text.as_bytes())),text,revision,document_version}))
    })();
    PreparedRead {evaluation:plan.evaluation,file:plan.file,catalog:plan.catalog,body}
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new()->Self {
            let path=std::env::temp_dir().join(format!("ktlsp-root-files-{}-{}",std::process::id(),NEXT_ROOT.fetch_add(1,Ordering::Relaxed)));
            std::fs::create_dir_all(&path).unwrap();Self(path)
        }
        fn write(&self,relative:&str,text:&str)->PathBuf {
            let path=self.0.join(relative);std::fs::create_dir_all(path.parent().unwrap()).unwrap();std::fs::write(&path,text).unwrap();path
        }
    }
    impl Drop for Temp {fn drop(&mut self) {
        assert!(self.0.starts_with(std::env::temp_dir()) && self.0.file_name().unwrap().to_string_lossy().starts_with("ktlsp-root-files-"));
        let _=std::fs::remove_dir_all(&self.0);
    }}
    fn plan(root:&Path,filter:Filter,overlays:BTreeMap<String,OwnedDocumentSnapshot>,epoch:Arc<AtomicBool>)->CapturePlan {
        CapturePlan {evaluation:7,generation:1,model:Arc::new(crate::classpath::ProjectSnapshot::new(Vec::new())),
            root:root.into(),filter,overlays,epoch}
    }
    fn json_filter()->Filter {Filter {extensions:BTreeSet::from(["json".into()]),..Default::default()}}

    #[test]
    fn root_file_filters_are_literal_and_share_effective_exclusions() {
        for value in ["../outside","C:root","*","a/b","a\\b",".","..",""] {
            let filter=Filter {basenames:BTreeSet::from([value.into()]),..Default::default()};
            assert!(filter.validate().is_err(),"{value}");
        }
        let filter=Filter {excluded_directories:BTreeSet::from(["BUILD".into()]),..json_filter()};
        let effective=filter.effective();assert_eq!(effective.excluded_directories,BTreeSet::from(["build".into()]));
        assert!(!effective.includes(Path::new("build/overlay.json"),&effective.excluded_directories));
        assert!(!effective.includes(Path::new("BUILD/overlay.json"),&effective.excluded_directories));
        assert!(effective.includes(Path::new(".gradle/loom-cache/mapping.json"),&effective.excluded_directories));
        assert!(safe_path(Path::new("/root"),Path::new("/root/../other.json"),true).is_err());
    }

    #[test]
    fn root_file_cache_budget_accepts_multiple_small_reads_and_rejects_aggregate_overflow() {
        let body=|text:&str|Ok::<_,String>(Arc::new(Body {text:Arc::from(text),sha256:String::new(),revision:1,document_version:None}));
        let mut results=BTreeMap::new();
        validate_cached_read(&results,"one",&body("12"),4).unwrap();results.insert("one".into(),body("12"));
        validate_cached_read(&results,"two",&body("34"),4).unwrap();results.insert("two".into(),body("34"));
        assert!(validate_cached_read(&results,"three",&body("5"),4).is_err(),"individual size cannot bypass aggregate admission");
        assert_eq!(results.len(),2,"rejection must leave the accepted cache intact");
        validate_cached_read(&results,"one",&body("1"),4).unwrap();
        assert!(validate_cached_read(&results,"two",&body("345"),4).is_err());
    }

    #[test]
    fn retired_root_overlay_epoch_cancels_before_native_acquisition() {
        let root=std::env::temp_dir().join(format!("ktlsp-root-files-cancelled-{}-{}",
            std::process::id(),NEXT_ROOT.fetch_add(1,Ordering::Relaxed)));
        assert!(!root.exists());
        let epoch=Arc::new(AtomicBool::new(true));
        let prepared=prepare(plan(&root,json_filter(),BTreeMap::new(),epoch.clone()),Arc::new(||
            panic!("a cancelled logical epoch must not create a native lease")));
        assert!(matches!(prepared.catalog,Err(error) if error=="Stale"),
            "logical cancellation must precede even an unavailable root's filesystem acquisition");
        assert!(epoch.load(Ordering::Acquire));assert!(!root.exists());
    }

    #[test]
    #[cfg(windows)]
    fn root_files_include_unregistered_and_new_unsaved_configs_without_resource_claims() {
        let temp=Temp::new();
        temp.write("shared.mixins.json","{\"shared\":true}");
        let client=temp.write("modules/mod/config/client.mixins.json","{\"disk\":true}");
        temp.write(".hidden.mixins.json","{}");temp.write(".gradle/loom-cache/mapping.json","{}");
        temp.write("UPPER.MIXINS.JSON5","{}");temp.write("named.config","{}");
        temp.write("copy/shared.mixins.json","{}");temp.write("build/copy.mixins.json","{}");
        let text:Arc<str>=Arc::from("{\"unsaved\":\"雪😀\"}");
        let overlay=OwnedDocumentSnapshot {text:text.clone(),document_version:Some(8),language_id:"json".into(),revision:42};
        let new=temp.0.join("modules/mod/new.mixins.json");
        let overlays=BTreeMap::from([(crate::paths::file_key(&client),overlay.clone()),
            (crate::paths::file_key(&new),overlay.clone()),
            (crate::paths::file_key(&temp.0.join("build/overlay-only.json")),overlay)]);
        let filter=Filter {excluded_directories:BTreeSet::from(["copy".into(),"BUILD".into()]),
            basenames:BTreeSet::from(["NAMED.CONFIG".into()]),extensions:BTreeSet::from(["JsOn".into(),"JSON5".into()])};
        let epoch=Arc::new(AtomicBool::new(false));
        let catalog=prepare(plan(&temp.0,filter,overlays,epoch.clone()),Arc::new(||{})).catalog.unwrap();
        assert!(catalog.complete);assert_eq!(catalog.entries.len(),7);
        assert!(catalog.entries.iter().any(|entry|entry.relative=="UPPER.MIXINS.JSON5"));
        assert!(catalog.entries.iter().any(|entry|entry.relative=="named.config"));
        assert!(catalog.entries.iter().all(|entry|!entry.relative.starts_with("build/") && !entry.relative.starts_with("copy/")));
        assert!(catalog.entries.iter().any(|entry|entry.relative==".gradle/loom-cache/mapping.json"));
        let index=catalog.entries.iter().position(|entry|entry.path==new).unwrap();
        assert_eq!(catalog.entries[index].uri,uri(&new).unwrap());
        let body=read(ReadPlan {evaluation:7,file:"issued".into(),catalog:catalog.clone(),index,byte_budget:MAX_BODY}).body.unwrap();
        assert_eq!(body.text,text);assert_eq!(body.revision,42);assert_eq!(body.document_version,Some(8));
        assert_eq!(body.sha256,format!("{:x}",Sha256::digest(text.as_bytes())));
        epoch.store(true,Ordering::Release);
        assert!(matches!(read(ReadPlan {evaluation:7,file:"issued".into(),catalog,index,byte_budget:MAX_BODY}).body,Err(error) if error=="Stale"));
    }

    #[test]
    #[cfg(windows)]
    fn root_file_add_delete_and_limits_retire_negative_and_snapshot_authority() {
        let temp=Temp::new();temp.write("first.json","{}");temp.write("second.json","{}");
        let capture=||plan(&temp.0,json_filter(),BTreeMap::new(),Arc::new(AtomicBool::new(false)));
        let limited=prepare_with_limits(capture(),Arc::new(||{}),MAX_VISITED,1).catalog.unwrap();
        assert!(!limited.complete && limited.problems.contains("root-file-inventory-limit"));
        let (wake,events)=std::sync::mpsc::channel();
        let catalog=prepare(capture(),Arc::new(move||{let _=wake.send(());})).catalog.unwrap();
        assert!(catalog.complete);
        let added=temp.write("added.json","{}");
        events.recv_timeout(std::time::Duration::from_secs(5)).expect("addition must invalidate negative membership");
        assert!(!catalog.guard.is_current());
        let (wake,events)=std::sync::mpsc::channel();
        let next=prepare(capture(),Arc::new(move||{let _=wake.send(());})).catalog.unwrap();
        assert_eq!(next.entries.len(),3);std::fs::remove_file(added).unwrap();
        events.recv_timeout(std::time::Duration::from_secs(5)).expect("deletion must invalidate catalog membership");
        assert!(!next.guard.is_current());
        let current=prepare(capture(),Arc::new(||{})).catalog.unwrap();
        assert!(read(ReadPlan {evaluation:7,file:"issued".into(),catalog:current,index:0,byte_budget:1}).body.is_err());
        assert!(prepare(plan(&temp.0.join("first.json"),json_filter(),BTreeMap::new(),Arc::new(AtomicBool::new(false))),Arc::new(||{})).catalog.is_err());
        assert!(safe_path(&temp.0,&temp.0.join("first.json/child.json"),false).is_err());
    }

    #[test]
    #[cfg(not(windows))]
    fn unsupported_root_watch_never_reports_false_empty_coverage() {
        let temp=Temp::new();
        assert!(prepare(plan(&temp.0,json_filter(),BTreeMap::new(),Arc::new(AtomicBool::new(false))),Arc::new(||{})).catalog.is_err());
    }
}
