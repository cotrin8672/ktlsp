//! Exact normal-runtime bytes from one selected SDK; source/ct.sym APIs do not grant this role.
use crate::{
    classpath::SdkApiSelection,
    compiled_watch::{HostInputLease, WatchPlan},
};
use ktcore::{classfile::ClassFileFacts, extensions::HostInputGuard};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf, sync::Arc};

#[derive(Debug)]
pub(crate) enum SdkClassUnavailable {
    InvalidRuntime(String),
    AmbiguousClass,
    ChangedInput,
}

#[derive(Debug)]
pub(crate) struct SdkClassBytes {
    pub bytes: Arc<[u8]>,
    pub class: Arc<ClassFileFacts>,
}

#[derive(Debug)]
pub(crate) enum SdkClassLookup {
    Found(SdkClassBytes),
    ApiHeader(Arc<ClassFileFacts>),
    DefinitelyAbsent,
    CodeUnavailable,
}

/// Retain once per accepted SDK/scope, including negative lookups; no per-class native watcher.
pub(crate) struct SelectedSdkRuntime {
    selection: SdkApiSelection,
    archives: Vec<(PathBuf, &'static str)>,
    revisions: Vec<String>,
    lease: Arc<HostInputLease>,
    feature: u16,
}

impl SelectedSdkRuntime {
    pub(crate) fn for_selection(
        selection: &SdkApiSelection,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self, SdkClassUnavailable> {
        if !selection.home.is_absolute()
            || if selection.release.is_some() {
                selection.archive != selection.home.join("lib/ct.sym")
            } else {
                selection.source_zip.as_ref() != Some(&selection.archive)
            }
        {
            return Err(invalid("SDK selection has no normal-runtime identity"));
        }
        let release=selection.home.join("release");
        let release_revision=crate::classpath::file_origin_id(&release);
        let archives = runtime_archives(selection)?;
        let mut files = vec![selection.home.join("release"), selection.archive.clone()];
        files.extend(archives.iter().map(|(path, _)| path.clone()));
        files.extend([
            selection.home.join("lib/rt.jar"),
            selection.home.join("jre/lib/rt.jar"),
        ]);
        let jmods = selection.home.join("jmods");
        if !jmods.is_dir() {
            files.push(jmods);
        }
        let directories = archives
            .iter()
            .filter_map(|(path, _)| path.parent().map(PathBuf::from))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let plan = WatchPlan::read_source_inventory(&files, &directories)
            .map_err(|error| invalid(error.to_string()))?;
        let lease = HostInputLease::new(plan, wake).map_err(|error| invalid(error.to_string()))?;
        // Close the membership-before-watch window; byte reads happen only under the live lease.
        if runtime_archives(selection)? != archives {
            return Err(SdkClassUnavailable::ChangedInput);
        }
        if crate::classpath::file_origin_id(&selection.archive) != selection.archive_revision
            || file_sha256(&selection.archive)? != selection.archive_sha256
            || !lease.is_current()
        {
            return Err(SdkClassUnavailable::ChangedInput);
        }
        let revisions = archives
            .iter()
            .map(|(path, _)| crate::classpath::file_origin_id(path))
            .collect();
        let feature=runtime_feature(selection)?;
        if crate::classpath::file_origin_id(&release)!=release_revision || !lease.is_current() {
            return Err(SdkClassUnavailable::ChangedInput);
        }
        Ok(Self {
            selection: selection.clone(),
            archives,
            revisions,
            lease,
            feature,
        })
    }

    pub(crate) fn guard(&self) -> Arc<HostInputLease> {
        self.lease.clone()
    }

    pub(crate) fn archives(&self) -> &[(PathBuf, &'static str)] { &self.archives }
    pub(crate) fn feature(&self) -> u16 {self.feature}

    pub(crate) fn read_class(
        &self,
        internal_name: &str,
    ) -> Result<SdkClassLookup, SdkClassUnavailable> {
        if internal_name.is_empty()
            || internal_name.contains(['\\', '.', ';', '[', '\0'])
            || internal_name.split('/').any(str::is_empty)
        {
            return Err(invalid("invalid internal class name"));
        }
        if !self.lease.is_current() {
            return Err(SdkClassUnavailable::ChangedInput);
        }
        if self.selection.release.is_some() {
            if let Some(Some(entry)) = self.selection.entries.get(internal_name) {
                if !entry.ends_with(".sig") {
                    return Err(invalid("SDK API entry is not a signature"));
                }
                let header = crate::jar::read_class_file(
                    &self.selection.archive,
                    entry,
                    &self.selection.origin(),
                )
                .map_err(invalid)?;
                if !self.lease.is_current() {
                    return Err(SdkClassUnavailable::ChangedInput);
                }
                return match header {
                    Some(header)
                        if header.identity.internal_name == internal_name
                            && !header.methods.iter().any(|method| {
                                method
                                    .attribute_names
                                    .iter()
                                    .any(|name| name.as_ref() == "Code")
                            }) =>
                    {
                        Ok(SdkClassLookup::ApiHeader(Arc::new(header)))
                    }
                    Some(_) => Err(invalid("SDK API signature has invalid identity or Code")),
                    None => Ok(SdkClassLookup::CodeUnavailable),
                };
            }
        }
        let mut found = None;
        for ((archive, prefix), revision) in self.archives.iter().zip(&self.revisions) {
            if crate::classpath::file_origin_id(archive) != *revision {
                return Err(SdkClassUnavailable::ChangedInput);
            }
            let bytes = crate::jar::read_effective_class_bytes(archive,internal_name,prefix,Some(self.feature),||self.lease.is_current())
                .map_err(|error|if error=="Stale" {SdkClassUnavailable::ChangedInput} else {invalid(error)})?;
            if let Some(bytes) = bytes {
                if found.is_some() {
                    return Err(SdkClassUnavailable::AmbiguousClass);
                }
                let class = ktcore::classfile::decode(&self.selection.origin(), &bytes)
                    .map_err(|error| invalid(error.to_string()))?;
                if class.identity.internal_name.as_str() != internal_name {
                    return Err(invalid("SDK class identity differs from selected entry"));
                }
                found = Some(SdkClassBytes {
                    bytes: bytes.into(),
                    class: Arc::new(class),
                });
            }
            if crate::classpath::file_origin_id(archive) != *revision {
                return Err(SdkClassUnavailable::ChangedInput);
            }
        }
        if !self.lease.is_current() {
            return Err(SdkClassUnavailable::ChangedInput);
        }
        Ok(match (found, self.selection.release) {
            (Some(_), Some(_)) => SdkClassLookup::CodeUnavailable,
            (Some(class), None) => SdkClassLookup::Found(class),
            (None, Some(_)) if self.selection.entries.contains_key(internal_name) => {
                SdkClassLookup::CodeUnavailable
            }
            (None, _) => SdkClassLookup::DefinitelyAbsent,
        })
    }
}

fn invalid(message: impl Into<String>) -> SdkClassUnavailable {
    SdkClassUnavailable::InvalidRuntime(message.into())
}

fn runtime_archives(
    selection: &SdkApiSelection,
) -> Result<Vec<(PathBuf, &'static str)>, SdkClassUnavailable> {
    let feature=runtime_feature(selection)?;
    runtime_archives_for_feature(selection,feature)
}

pub(crate) fn runtime_feature(selection:&SdkApiSelection)->Result<u16,SdkClassUnavailable> {
    let mut release = Vec::new();
    fs::File::open(selection.home.join("release"))
        .map_err(|error| invalid(error.to_string()))?
        .take(64 * 1024 + 1)
        .read_to_end(&mut release)
        .map_err(|error| invalid(error.to_string()))?;
    if release.len() > 64 * 1024 {
        return Err(invalid("SDK release input too large"));
    }
    std::str::from_utf8(&release)
        .ok()
        .and_then(crate::classpath::sdk_feature_version)
        .ok_or_else(|| invalid("SDK release input is invalid"))
}

fn runtime_archives_for_feature(selection:&SdkApiSelection,feature:u16)
    ->Result<Vec<(PathBuf,&'static str)>,SdkClassUnavailable> {
    let mut legacy = Vec::new();
    for relative in ["jre/lib/rt.jar", "lib/rt.jar"] {
        let path = selection.home.join(relative);
        if path.exists() {
            let meta = fs::symlink_metadata(&path).map_err(|error| invalid(error.to_string()))?;
            if !meta.is_file() || meta.file_type().is_symlink() {
                return Err(invalid("invalid SDK runtime archive"));
            }
            legacy.push((path, ""));
        }
    }
    let jmods = selection.home.join("jmods");
    if feature < 9 {
        if legacy.len() != 1 {
            return Err(invalid("normal SDK runtime unavailable or ambiguous"));
        }
        return Ok(legacy);
    }
    let meta = fs::symlink_metadata(&jmods).map_err(|error| invalid(error.to_string()))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(invalid("invalid SDK module directory"));
    }
    let mut archives = Vec::new();
    let mut entries = 0usize;
    for entry in fs::read_dir(&jmods).map_err(|error| invalid(error.to_string()))? {
        entries += 1;
        if entries >= crate::compiled_watch::MAX_INPUT_PATHS - 4 {
            return Err(invalid("SDK module inventory too large"));
        }
        let path = entry.map_err(|error| invalid(error.to_string()))?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jmod") {
            continue;
        }
        let meta = fs::symlink_metadata(&path).map_err(|error| invalid(error.to_string()))?;
        if !meta.is_file() || meta.file_type().is_symlink() {
            return Err(invalid("invalid SDK module archive"));
        }
        archives.push((path, "classes/"));
    }
    if !archives.iter().any(|(path, _)| {
        path.file_name()
            .is_some_and(|name| name == "java.base.jmod")
    }) {
        return Err(invalid("SDK base runtime module is missing"));
    }
    archives.sort();
    Ok(archives)
}

pub(crate) fn file_sha256(path: &std::path::Path) -> Result<String, SdkClassUnavailable> {
    let mut file = fs::File::open(path).map_err(|error| invalid(error.to_string()))?;
    let mut hash = Sha256::new();
    let mut bytes = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| invalid(error.to_string()))?;
        if count == 0 {
            break;
        }
        bytes += count;
        if bytes > 256 * 1024 * 1024 {
            return Err(invalid("SDK selection archive too large"));
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    const CLASS: &[u8] =
        include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn fixture(modules: bool) -> (Fixture, SdkApiSelection) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "ktlsp_sdk_runtime_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("release"),
            if modules {
                b"JAVA_VERSION=\"17.0.1\"\n".as_slice()
            } else {
                b"JAVA_VERSION=\"1.8.0_202\"\n".as_slice()
            },
        )
        .unwrap();
        let archive = root.join("src.zip");
        fs::write(&archive, b"navigation-only").unwrap();
        write_runtime(
            &root.join(if modules {
                "jmods/java.base.jmod"
            } else {
                "jre/lib/rt.jar"
            }),
            modules,
        );
        let selection = SdkApiSelection {
            home: root.clone(),
            source_zip: Some(archive.clone()),
            release: None,
            archive_sha256: file_sha256(&archive).unwrap(),
            archive_revision: crate::classpath::file_origin_id(&archive),
            archive,
            entries: Arc::new(BTreeMap::new()),
        };
        (Fixture(root), selection)
    }
    fn write_runtime(path: &std::path::Path, module: bool) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut file = fs::File::create(path).unwrap();
        if module {
            file.write_all(b"JM\x01\x00").unwrap();
        }
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file(
            if module {
                "classes/pluginfixture/Person.class"
            } else {
                "pluginfixture/Person.class"
            },
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(CLASS).unwrap();
        zip.finish().unwrap();
    }
    #[test]
    fn sdk_runtime_preserves_bytes_code_and_negative_guards_for_legacy_and_modules() {
        for modules in [false, true] {
            let (_fixture, selection) = fixture(modules);
            let runtime = SelectedSdkRuntime::for_selection(&selection, Arc::new(|| {})).unwrap();
            let SdkClassLookup::Found(found) = runtime.read_class("pluginfixture/Person").unwrap()
            else {
                panic!("runtime class")
            };
            assert_eq!(&*found.bytes, CLASS);
            assert_eq!(found.class.identity.internal_name, "pluginfixture/Person");
            assert!(found.class.methods.iter().any(|method| method
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Code")));
            assert!(matches!(
                runtime.read_class("pluginfixture/Missing").unwrap(),
                SdkClassLookup::DefinitelyAbsent
            ));
            assert!(
                Arc::ptr_eq(&runtime.guard(), &runtime.guard()),
                "negative and positive reads share one watch"
            );
            assert!(runtime.read_class("../Person").is_err());
            let path = selection.home.join(if modules {
                "jmods/java.base.jmod"
            } else {
                "jre/lib/rt.jar"
            });
            fs::OpenOptions::new()
                .append(true)
                .open(path)
                .unwrap()
                .write_all(b"changed")
                .unwrap();
            assert!(matches!(
                runtime.read_class("pluginfixture/Person"),
                Err(SdkClassUnavailable::ChangedInput)
            ));
        }
    }
    #[test]
    fn sdk_runtime_rejects_release_headers_duplicate_modules_and_stale_selection() {
        let (_fixture, selection) = fixture(true);
        let mut release_selection = selection.clone();
        release_selection.release = Some(8);
        release_selection.archive = selection.home.join("lib/ct.sym");
        fs::create_dir_all(release_selection.archive.parent().unwrap()).unwrap();
        zip::ZipWriter::new(fs::File::create(&release_selection.archive).unwrap())
            .finish()
            .unwrap();
        release_selection.archive_sha256 = file_sha256(&release_selection.archive).unwrap();
        release_selection.archive_revision =
            crate::classpath::file_origin_id(&release_selection.archive);
        release_selection.entries = Arc::new(BTreeMap::from([(
            "pluginfixture/MissingApiClass".into(),
            Some("8/api.sig".into()),
        )]));
        let runtime =
            SelectedSdkRuntime::for_selection(&release_selection, Arc::new(|| {})).unwrap();
        assert!(matches!(
            runtime.read_class("pluginfixture/Person").unwrap(),
            SdkClassLookup::CodeUnavailable
        ));
        assert!(matches!(
            runtime.read_class("pluginfixture/MissingApiClass").unwrap(),
            SdkClassLookup::CodeUnavailable
        ));
        assert!(matches!(
            runtime
                .read_class("pluginfixture/AbsentFromApiAndRuntime")
                .unwrap(),
            SdkClassLookup::DefinitelyAbsent
        ));
        drop(runtime);
        write_runtime(&selection.home.join("jmods/another.jmod"), true);
        let runtime = SelectedSdkRuntime::for_selection(&selection, Arc::new(|| {})).unwrap();
        assert!(matches!(
            runtime.read_class("pluginfixture/Person"),
            Err(SdkClassUnavailable::AmbiguousClass)
        ));
        drop(runtime);
        fs::write(&selection.archive, b"changed-navigation").unwrap();
        assert!(matches!(
            SelectedSdkRuntime::for_selection(&selection, Arc::new(|| {})),
            Err(SdkClassUnavailable::ChangedInput)
        ));
    }

    #[test]
    #[ignore = "requires an actual captured accepted producer model and existing selected SDK"]
    fn actual_selected_sdk_runtime_returns_object_code_and_absent_control() {
        let model = std::env::var("KTLSP_SDK_RUNTIME_MODEL").expect("captured model path");
        let source = std::env::var("KTLSP_SDK_RUNTIME_SOURCE").expect("captured source path");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(model).unwrap()).unwrap();
        let modules = serde_json::from_value(value["modules"].clone()).unwrap();
        let model = crate::classpath::ProjectSnapshot::new(modules);
        let selection = model
            .sdk_api_for_file(std::path::Path::new(&source))
            .expect("accepted source's selected SDK");
        let runtime = SelectedSdkRuntime::for_selection(selection, Arc::new(|| {})).unwrap();
        let SdkClassLookup::Found(object) = runtime.read_class("java/lang/Object").unwrap() else {
            panic!("real selected full SDK Object")
        };
        assert_eq!(object.class.identity.internal_name, "java/lang/Object");
        assert_eq!(object.class.identity.origin_id, selection.origin());
        assert!(object.class.methods.iter().any(|method| method
            .attribute_names
            .iter()
            .any(|name| name.as_ref() == "Code")));
        let exact = runtime
            .archives
            .iter()
            .find_map(|(path, prefix)| {
                crate::jar::read_class_bytes(path, &format!("{prefix}java/lang/Object.class"))
                    .unwrap()
            })
            .unwrap();
        assert_eq!(&*object.bytes, exact);
        assert!(matches!(
            runtime
                .read_class("ktlsp_missing_sdk_fixture/DefinitelyAbsent")
                .unwrap(),
            SdkClassLookup::DefinitelyAbsent
        ));
        assert!(runtime.guard().is_current());
    }

    #[test]
    #[ignore = "requires an actual captured accepted producer model and existing selected SDK"]
    fn actual_selected_lower_release_sdk_keeps_generic_header_without_promoting_code() {
        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(std::env::var("KTLSP_SDK_RUNTIME_MODEL").unwrap()).unwrap(),
        )
        .unwrap();
        let model = crate::classpath::ProjectSnapshot::new(
            serde_json::from_value(value["modules"].clone()).unwrap(),
        );
        let source = std::env::var("KTLSP_SDK_RUNTIME_SOURCE").unwrap();
        let mut selection = model
            .sdk_api_for_file(std::path::Path::new(&source))
            .unwrap()
            .clone();
        selection.release = Some(8);
        selection.archive = selection.home.join("lib/ct.sym");
        let bytes = fs::read(&selection.archive).unwrap();
        selection.entries = Arc::new(crate::sdk_api::release_entries(&bytes, 8).unwrap());
        selection.archive_sha256 = format!("{:x}", Sha256::digest(&bytes));
        selection.archive_revision = crate::classpath::file_origin_id(&selection.archive);
        let runtime = SelectedSdkRuntime::for_selection(&selection, Arc::new(|| {})).unwrap();
        let SdkClassLookup::ApiHeader(list) = runtime.read_class("java/util/List").unwrap() else {
            panic!("selected release List header")
        };
        assert_eq!(list.identity.origin_id, selection.origin());
        assert!(list
            .signature
            .as_ref()
            .is_some_and(|signature| signature.contains("<E:")));
        assert!(list
            .interfaces
            .iter()
            .any(|name| name.as_ref() == "java/util/Collection"));
        assert!(list
            .methods
            .iter()
            .any(|method| method.name.as_ref() == "get"
                && method.descriptor.as_ref() == "(I)Ljava/lang/Object;"
                && method.signature.as_deref() == Some("(I)TE;")));
        assert!(!list.methods.iter().any(|method| method
            .attribute_names
            .iter()
            .any(|name| name.as_ref() == "Code")));
        assert!(matches!(
            runtime
                .read_class("ktlsp_missing_sdk_fixture/DefinitelyAbsent")
                .unwrap(),
            SdkClassLookup::DefinitelyAbsent
        ));
        assert!(!selection
            .entries
            .contains_key("java/lang/invoke/DirectMethodHandle"));
        assert!(matches!(
            runtime
                .read_class("java/lang/invoke/DirectMethodHandle")
                .unwrap(),
            SdkClassLookup::CodeUnavailable
        ));
        assert!(runtime.guard().is_current());
    }
}
