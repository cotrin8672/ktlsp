//! Host-selected runtime bytes, independent of source-callable reference eligibility.
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::classpath::ProjectSnapshot;
use crate::compiled_watch::{HostInputLease, WatchPlan};
use ktcore::extensions::HostInputGuard;

#[derive(Debug)]
pub(crate) enum ClassBytesUnavailable {
    IncompleteRuntimeModel,
    InvalidInternalName,
    InvalidInput(String),
    ChangedInput,
}

pub(crate) struct RuntimeClassBytes {
    pub bytes: Arc<[u8]>,
    pub class: Arc<ktcore::classfile::ClassFileFacts>,
    pub api_header: bool,
}

pub(crate) struct ClassReadRequest {
    pub internal_name: String,
    pub expected: Option<ktcore::jvm::ClassIdentity>,
    pub bytes: bool,
}

pub(crate) struct ClassReadPlan {
    pub evaluation: u64,
    pub model: Arc<ProjectSnapshot>,
    pub owner: PathBuf,
    pub scope: Option<Arc<RuntimeClassScope>>,
    pub requests: Vec<ClassReadRequest>,
}

pub(crate) struct ClassReadResult {
    pub internal_name: String,
    pub identity: Option<ktcore::jvm::ClassIdentity>,
    pub byte_length: usize,
    pub bytes: Option<Arc<[u8]>>,
    pub unavailable: Option<String>,
    pub api_header: Option<Arc<ktcore::classfile::ClassFileFacts>>,
}

pub(crate) struct PreparedClassRead {
    pub evaluation: u64,
    pub model: Arc<ProjectSnapshot>,
    pub scope: Option<Arc<RuntimeClassScope>>,
    pub results: Result<Vec<ClassReadResult>, String>,
}

pub(crate) fn prepare_class_read(plan: ClassReadPlan, wake: Arc<dyn Fn() + Send + Sync>)
    -> PreparedClassRead
{
    let scope = match plan.scope {
        Some(scope) if Arc::ptr_eq(&scope.model, &plan.model) => Ok(scope),
        Some(_) => Err(ClassBytesUnavailable::ChangedInput),
        None => RuntimeClassScope::for_project(plan.model.clone(), &plan.owner, wake),
    };
    let (scope, results) = match scope {
        Err(ClassBytesUnavailable::ChangedInput) => (None, Err("Stale".into())),
        Err(error) => (None, Ok(plan.requests.into_iter().map(|request| ClassReadResult {
            internal_name: request.internal_name, identity: None, byte_length: 0, bytes: None,
            unavailable: Some(format!("{error:?}")), api_header: None,
        }).collect())),
        Ok(scope) => {
            let mut retained = 0usize;
            let result = (|| {
                let mut results = Vec::new();
                for request in plan.requests {
                    let read = scope.read_class(&request.internal_name);
                    let value = match read {
                        Ok(Some(value)) => {
                            if request.expected.as_ref().is_some_and(|expected| expected != &value.class.identity) {
                                return Err("Stale".into());
                            }
                            if request.bytes {
                                retained = retained.checked_add(value.bytes.len()).ok_or(crate::extension_ipc::READ_FACTS_RESPONSE_BUDGET_EXCEEDED)?;
                                if retained > ktcore::classfile::MAX_CLASS_FILE_BYTES {
                                    return Err(crate::extension_ipc::READ_FACTS_RESPONSE_BUDGET_EXCEEDED.into());
                                }
                            }
                            ClassReadResult { internal_name: request.internal_name,
                                identity: Some(value.class.identity.clone()), byte_length: value.bytes.len(),
                                api_header: value.api_header.then(||value.class.clone()),
                                bytes: (request.bytes && !value.api_header).then_some(value.bytes), unavailable: None }
                        }
                        Ok(None) => ClassReadResult { internal_name: request.internal_name,
                            identity: None, byte_length: 0, bytes: None,
                            unavailable: Some("BYTECODE_UNAVAILABLE".into()), api_header: None },
                        Err(ClassBytesUnavailable::ChangedInput) => return Err("Stale".into()),
                        Err(error) => ClassReadResult { internal_name: request.internal_name,
                            identity: None, byte_length: 0, bytes: None,
                            unavailable: Some(format!("{error:?}")), api_header: None },
                    };
                    results.push(value);
                }
                Ok(results)
            })();
            (Some(scope), result)
        }
    };
    PreparedClassRead { evaluation: plan.evaluation, model: plan.model, scope, results }
}

struct RuntimeEntry {
    path: PathBuf,
    origin: String,
}

/// Shared once by an evaluation's selected scope, never once per returned class handle.
pub(crate) struct RuntimeClassScope {
    pub model: Arc<ProjectSnapshot>,
    entries: Vec<RuntimeEntry>,
    lease: Arc<HostInputLease>,
    sdk: Arc<crate::sdk_runtime::SelectedSdkRuntime>,
}

pub(crate) fn valid_internal_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 4096
        && !name.chars().any(|ch| ch.is_control() || matches!(ch, '\\' | '.' | ':' | ';' | '[' | ']'))
        && name.split('/').all(|part| !part.is_empty())
}

impl RuntimeClassScope {
    /// This entire constructor runs off Workspace's lock. Legacy models are unavailable.
    pub(crate) fn for_project(
        model: Arc<ProjectSnapshot>, owner: &Path, wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Arc<Self>, ClassBytesUnavailable> {
        let (scope, source, sdk_selection) = if let Some(context) = model.extension_project_context(owner) {
            (context.scope, context.source, context.sdk.cloned())
        } else {
            let ktcore::knowledge::Knowledge::Found(context) = model.resource_owner_context(owner) else {
                return Err(ClassBytesUnavailable::IncompleteRuntimeModel);
            };
            let source = model.source_set_for_scope(&context.scope)
                .ok_or(ClassBytesUnavailable::IncompleteRuntimeModel)?;
            let sdk = model.sdk_api_for_scope(&context.scope).cloned();
            (context.scope, source, sdk)
        };
        let inputs = source.runtime.as_ref().filter(|inputs| inputs.usable())
            .ok_or(ClassBytesUnavailable::IncompleteRuntimeModel)?;
        let sdk_selection = sdk_selection.ok_or(ClassBytesUnavailable::IncompleteRuntimeModel)?;
        let scope = format!("{}#{}", scope.module, scope.source_set);
        let entries = inputs.classpath.iter().enumerate().map(|(slot, path)| RuntimeEntry {
            path: path.clone(),
            origin: model.artifact_origin(path).map(str::to_owned)
                .unwrap_or_else(|| format!("runtime:{scope}:{slot}")),
        }).collect();
        let plan = WatchPlan::read_inputs(&inputs.classpath)
            .map_err(|error|ClassBytesUnavailable::InvalidInput(error.to_string()))?;
        let lease = HostInputLease::new(plan, wake.clone())
            .map_err(|error|ClassBytesUnavailable::InvalidInput(error.to_string()))?;
        let sdk = crate::sdk_runtime::SelectedSdkRuntime::for_selection(&sdk_selection, wake)
            .map_err(|error|ClassBytesUnavailable::InvalidInput(format!("{error:?}")))?;
        if !lease.is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
        Ok(Arc::new(Self { model, entries, lease, sdk: Arc::new(sdk) }))
    }

    pub(crate) fn guards(&self) -> [Arc<HostInputLease>; 2] { [self.lease.clone(), self.sdk.guard()] }

    fn is_current(&self) -> bool { self.guards().iter().all(|guard|guard.is_current()) }

    /// The first selected runtime entry containing the requested class wins. A malformed
    /// earlier entry cannot be bypassed in favor of a convenient later definition.
    pub(crate) fn read_class(&self, internal_name: &str)
        -> Result<Option<RuntimeClassBytes>, ClassBytesUnavailable>
    {
        if !valid_internal_name(internal_name) { return Err(ClassBytesUnavailable::InvalidInternalName); }
        if !self.is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
        match self.sdk.read_class(internal_name)
            .map_err(|error|ClassBytesUnavailable::InvalidInput(format!("{error:?}")))?
        {
            crate::sdk_runtime::SdkClassLookup::Found(value) => {
                if !self.is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
                return Ok(Some(RuntimeClassBytes { bytes: value.bytes, class: value.class, api_header: false }));
            }
            crate::sdk_runtime::SdkClassLookup::ApiHeader(class) => {
                if !self.is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
                return Ok(Some(RuntimeClassBytes { bytes: Arc::from([]), class, api_header: true }));
            }
            crate::sdk_runtime::SdkClassLookup::DefinitelyAbsent => {}
            crate::sdk_runtime::SdkClassLookup::CodeUnavailable =>
                return Err(ClassBytesUnavailable::InvalidInput("selected SDK bytecode is unavailable".into())),
        }
        read_ordered_class(&self.entries, internal_name, Some(self.sdk.feature()), ||self.is_current())
    }
}

fn read_ordered_class(entries: &[RuntimeEntry], internal_name: &str, feature:Option<u16>, is_current: impl Fn() -> bool)
    -> Result<Option<RuntimeClassBytes>, ClassBytesUnavailable>
{
    if !valid_internal_name(internal_name) { return Err(ClassBytesUnavailable::InvalidInternalName); }
    if !is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
        for entry in entries {
            let bytes = match std::fs::symlink_metadata(&entry.path) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(ClassBytesUnavailable::InvalidInput(error.to_string())),
                Ok(metadata) if metadata.file_type().is_symlink() =>
                    return Err(ClassBytesUnavailable::InvalidInput("runtime symlink".into())),
                Ok(_) => crate::jar::read_effective_class_bytes(&entry.path,internal_name,"",feature,&is_current)
                    .map_err(|error|if error=="Stale" {ClassBytesUnavailable::ChangedInput} else {ClassBytesUnavailable::InvalidInput(error)})?,
            };
            let Some(bytes) = bytes else { continue };
            let class = ktcore::classfile::decode(&entry.origin, &bytes)
                .map_err(|error|ClassBytesUnavailable::InvalidInput(error.to_string()))?;
            if class.identity.internal_name != internal_name {
                return Err(ClassBytesUnavailable::InvalidInput("runtime class entry/name mismatch".into()));
            }
            if !is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
            return Ok(Some(RuntimeClassBytes { bytes: bytes.into(), class: Arc::new(class), api_header: false }));
        }
        if !is_current() { return Err(ClassBytesUnavailable::ChangedInput); }
        Ok(None)


}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_runtime_reads_never_bypass_an_earlier_owner_or_publish_after_cancel() {
        use std::{cell::Cell, fs, time::{SystemTime, UNIX_EPOCH}};
        let root = std::env::temp_dir().join(format!("ktlsp_runtime_order_{}_{}",
            std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()));
        let first = root.join("first"); let second = root.join("second");
        fs::create_dir_all(first.join("pluginfixture")).unwrap();
        fs::create_dir_all(second.join("pluginfixture")).unwrap();
        let bytes = include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        fs::write(first.join("pluginfixture/Person.class"), bytes).unwrap();
        fs::write(second.join("pluginfixture/Person.class"), b"invalid later owner").unwrap();
        let entries = vec![RuntimeEntry {path:first.clone(),origin:"first-runtime".into()},
            RuntimeEntry {path:second.clone(),origin:"second-runtime".into()}];
        let found = read_ordered_class(&entries, "pluginfixture/Person", Some(21), ||true).unwrap().unwrap();
        assert_eq!(&*found.bytes, bytes);
        assert_eq!(found.class.identity.origin_id, "first-runtime");
        fs::write(first.join("pluginfixture/Person.class"), b"invalid earlier owner").unwrap();
        fs::write(second.join("pluginfixture/Person.class"), bytes).unwrap();
        assert!(read_ordered_class(&entries, "pluginfixture/Person", Some(21), ||true).is_err(),
            "an invalid earlier runtime owner cannot fall through to a valid later class");
        fs::remove_file(first.join("pluginfixture/Person.class")).unwrap();
        let found = read_ordered_class(&entries, "pluginfixture/Person", Some(21), ||true).unwrap().unwrap();
        assert_eq!(found.class.identity.origin_id, "second-runtime");
        let checks = Cell::new(0);
        assert!(matches!(read_ordered_class(&entries, "pluginfixture/Person", Some(21), || {
            checks.set(checks.get()+1); checks.get()==1
        }), Err(ClassBytesUnavailable::ChangedInput)), "retirement after IO rejects publication");
        assert!(read_ordered_class(&entries,"pluginfixture/Missing",Some(21),||true).unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exact_runtime_internal_name_is_a_name_not_a_filesystem_grant() {
        for name in ["java/util/List", "some/pkg/Outer$Inner", "DefaultPackage"] {
            assert!(valid_internal_name(name), "{name}");
        }
        for name in ["", "/java/lang/Object", "../Target", "a/../Target", "C:/Target",
            "a\\Target", "java.lang.Object", "[Ljava/lang/Object;", "a//Target", "a/", "a\0b"] {
            assert!(!valid_internal_name(name), "{name:?}");
        }
    }
}
