//! Lazy binary inventories for negative Java diagnostics, using the caller's actual classpath.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use ktcore::classfile::ClassFileFacts;

use crate::classpath::{self, CompilerTask, ComponentId, ProjectSnapshot};
use crate::index::{Entry, InferenceIndex, LookupEntries, OriginFacts};

type FileStamp = (SystemTime, u64);
const MAX_CACHE_ENTRIES: usize = 4096;
const MAX_CLASS_BYTES: usize = 8 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct ClassInventoryCache {
    classes: BTreeMap<(PathBuf, String, String), CachedClass>,
    services: BTreeMap<(PathBuf, bool), (FileStamp, bool)>,
    class_bytes: usize,
}

struct CachedClass {
    stamp: Option<FileStamp>,
    facts: Option<Arc<ClassFileFacts>>,
}

impl ClassInventoryCache {
    pub(crate) fn clear(&mut self) {
        self.classes.clear();
        self.services.clear();
        self.class_bytes = 0;
    }

    /// Complete implicit root for a code action, from this caller's current selected JVM task.
    /// This uses the existing stamped/bounded decoded cache, without indexing the SDK.
    pub(crate) fn object_for_caller(
        &mut self,
        model: &ProjectSnapshot,
        caller: &str,
    ) -> Option<Arc<ClassFileFacts>> {
        use classpath::CompilerLanguage;
        let task = if crate::language::SourceLanguage::for_key(caller).is_java() {
            let task = model.plain_java_task(caller)?;
            if task.options.get("release") != Some(&serde_json::Value::Null)
                || task.options.get("bootstrapClasspath") != Some(&serde_json::Value::Null)
                || !self.plain_caller(task)
            {
                return None;
            }
            task
        } else {
            let (_, source) = model.owner_for_key(caller)?;
            let source = source?;
            if source.compiler_metadata_incomplete
                || source.classpath_incomplete
                || !source.unresolved.is_empty()
            {
                return None;
            }
            let selected = source.kotlin_compile_task.as_deref()?;
            let mut tasks = source
                .compiler_tasks
                .iter()
                .filter(|task| task.language == CompilerLanguage::Kotlin && task.task == selected);
            let task = tasks.next()?;
            if tasks.next().is_some()
                || !task.complete
                || task.problem.is_some()
                || task.kotlin_compiler.is_none()
                || !task.annotation_processor_path.is_empty()
                || !matches!(
                    task.task_type.as_str(),
                    "org.jetbrains.kotlin.gradle.tasks.KotlinCompile"
                        | "org.jetbrains.kotlin.gradle.tasks.KotlinCompile_Decorated"
                )
            {
                return None;
            }
            // Keep target-SDK substitutions, plugins and opaque/custom arguments unknown.
            let mut arguments = task.arguments.iter();
            while let Some(argument) = arguments.next() {
                match argument.as_str() {
                    "-d" | "-jvm-target" | "-module-name" | "-language-version"
                    | "-api-version" | "-classpath" | "-cp" | "-jdk-home" => {
                        let value = arguments.next()?;
                        if value.is_empty()
                            || value.starts_with(['-', '@'])
                            || value.contains('\0')
                            || argument == "-jdk-home"
                                && task.java_home.as_deref() != Some(Path::new(value))
                        {
                            return None;
                        }
                    }
                    "-no-stdlib"
                    | "-no-reflect"
                    | "-nowarn"
                    | "-Werror"
                    | "-progressive"
                    | "-Xallow-no-source-files" => {}
                    _ => return None,
                }
            }
            task
        };
        if !task
            .source_files
            .iter()
            .any(|path| crate::paths::file_key(path) == caller)
        {
            return None;
        }
        let home = task.java_home.as_ref()?;
        if !home.is_absolute() {
            return None;
        }
        let origin = classpath::jdk_origin_id(&home.join("lib/src.zip"));
        let class = self
            .read(
                &home.join("jmods/java.base.jmod"),
                "classes/java/lang/Object.class",
                &origin,
            )
            .ok()??;
        (class.identity.origin_id == origin && class.identity.internal_name == "java/lang/Object")
            .then_some(class)
    }

    #[cfg(test)]
    pub(crate) fn inject_object_for_tests(&mut self, home: &Path, facts: ClassFileFacts) {
        let path = home.join("jmods/java.base.jmod");
        let stamp = file_stamp(&path).unwrap().expect("explicit test SDK stamp");
        let origin = classpath::jdk_origin_id(&home.join("lib/src.zip"));
        assert_eq!(facts.identity.origin_id, origin);
        assert_eq!(facts.identity.internal_name, "java/lang/Object");
        self.classes.insert(
            (path, "classes/java/lang/Object.class".into(), origin),
            CachedClass {
                stamp: Some(stamp),
                facts: Some(Arc::new(facts)),
            },
        );
    }

    pub(crate) fn plain_caller(&mut self, task: &CompilerTask) -> bool {
        if task.source_files.iter().any(|path| {
            path.file_name()
                .is_some_and(|name| name == "module-info.java")
        }) {
            // shortcut: modular compiler service discovery needs a module-path inventory.
            return false;
        }
        let processors_disabled = task.annotation_processing_disabled();
        task.classpath.iter().all(|path| {
            if path.is_dir() {
                // Nested service files can change without changing the classpath root's mtime.
                return crate::jar::plain_java_classpath_entry(path, processors_disabled);
            }
            let Ok(Some(before)) = file_stamp(path) else {
                return false;
            };
            let key = (path.clone(), processors_disabled);
            if let Some((stamp, absent)) = self.services.get(&key) {
                if *stamp == before {
                    return *absent;
                }
            }
            let absent = crate::jar::plain_java_classpath_entry(path, processors_disabled);
            if file_stamp(path) != Ok(Some(before)) {
                return false;
            }
            if self.services.len() >= MAX_CACHE_ENTRIES {
                self.services.clear();
            }
            self.services.insert(key, (before, absent));
            absent
        })
    }

    pub(crate) fn inventory(
        &mut self,
        model: &ProjectSnapshot,
        task: &CompilerTask,
        caller: &str,
        owner: &Entry,
        origin: &str,
    ) -> Option<Arc<ClassFileFacts>> {
        let mut internal = owner.sym.package.replace('.', "/");
        if !internal.is_empty() {
            internal.push('/');
        }
        if let Some(container) = &owner.sym.container {
            internal.push_str(&container.replace('.', "$"));
            internal.push('$');
        }
        internal.push_str(&owner.sym.name);
        if origin.starts_with("jdk:") {
            if task.options.get("release") != Some(&serde_json::Value::Null) {
                // shortcut: --release selects ct.sym APIs; do not substitute the running JDK's jmods.
                return None;
            }
            let java_home = task.java_home.as_ref()?;
            if origin != classpath::jdk_origin_id(&java_home.join("lib/src.zip")) {
                return None;
            }
            // shortcut: support java.base first; other JDK modules keep their unknown surface.
            return self
                .read(
                    &java_home.join("jmods/java.base.jmod"),
                    &format!("classes/{internal}.class"),
                    origin,
                )
                .ok()?;
        }
        let (_, source) = model.owner_for_key(caller)?;
        let source = source?;
        let mut selected = source.entries.iter().filter(|path| {
            let component = source
                .artifacts
                .iter()
                .find(|artifact| artifact.path == **path)
                .map(|artifact| &artifact.component);
            match component {
                Some(ComponentId::Module {
                    group,
                    name,
                    version,
                }) => origin == format!("maven:{group}:{name}:{version}"),
                Some(ComponentId::Project { .. }) => false,
                _ => model.artifact_origin(path) == Some(origin),
            }
        });
        let binary = selected.next()?;
        if selected.next().is_some() {
            return None;
        }
        let revision = model.artifact_origin(binary)?;
        if classpath::file_origin_id(binary) != revision {
            return None;
        }
        let binary_key = crate::paths::file_key(binary);
        let entry = format!("{internal}.class");
        for path in &task.classpath {
            if let Some(class) = self.read(path, &entry, origin).ok()? {
                if crate::paths::file_key(path) != binary_key
                    || classpath::file_origin_id(binary) != revision
                {
                    return None;
                }
                return Some(class);
            }
        }
        None
    }

    fn read(
        &mut self,
        path: &Path,
        entry: &str,
        origin: &str,
    ) -> Result<Option<Arc<ClassFileFacts>>, ()> {
        let leaf = if path.is_dir() {
            path.join(entry)
        } else {
            path.to_path_buf()
        };
        let before = file_stamp(&leaf)?;
        let key = (path.to_path_buf(), entry.to_string(), origin.to_string());
        if let Some(cached) = self.classes.get(&key) {
            if cached.stamp == before {
                return Ok(cached.facts.clone());
            }
        }
        let facts = crate::jar::read_class_file(path, entry, origin)
            .map_err(|_| ())?
            .map(Arc::new);
        if file_stamp(&leaf)? != before {
            return Err(());
        }
        let bytes = 256
            + path.as_os_str().len()
            + entry.len()
            + origin.len()
            + facts.as_deref().map_or(0, class_weight);
        if bytes <= MAX_CLASS_BYTES {
            if self.classes.len() >= MAX_CACHE_ENTRIES || self.class_bytes + bytes > MAX_CLASS_BYTES
            {
                self.classes.clear();
                self.class_bytes = 0;
            }
            // Replacement may overcount; conservative accounting avoids retaining oversized facts.
            self.class_bytes += bytes;
            self.classes.insert(
                key,
                CachedClass {
                    stamp: before,
                    facts: facts.clone(),
                },
            );
        }
        Ok(facts)
    }
}

fn class_weight(class: &ClassFileFacts) -> usize {
    let text = |value: &Arc<str>| value.len() + 2 * std::mem::size_of::<usize>();
    std::mem::size_of::<ClassFileFacts>()
        + class.identity.origin_id.capacity()
        + class.identity.internal_name.capacity()
        + class.fields.capacity() * std::mem::size_of::<ktcore::classfile::MemberFacts>()
        + class.methods.capacity() * std::mem::size_of::<ktcore::classfile::MemberFacts>()
        + (class.interfaces.capacity() + class.attribute_names.capacity())
            * std::mem::size_of::<Arc<str>>()
        + class
            .super_name
            .iter()
            .chain(&class.signature)
            .chain(&class.source_file)
            .chain(&class.interfaces)
            .chain(&class.attribute_names)
            .map(text)
            .sum::<usize>()
        + class
            .fields
            .iter()
            .chain(&class.methods)
            .map(|member| {
                text(&member.name)
                    + text(&member.descriptor)
                    + member.attribute_names.capacity() * std::mem::size_of::<Arc<str>>()
                    + member
                        .signature
                        .iter()
                        .chain(&member.attribute_names)
                        .map(text)
                        .sum::<usize>()
            })
            .sum::<usize>()
}

fn file_stamp(path: &Path) -> Result<Option<FileStamp>, ()> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            Ok(Some((metadata.modified().map_err(|_| ())?, metadata.len())))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        _ => Err(()),
    }
}

pub(crate) struct BinaryDiagnostics<'a> {
    pub index: &'a dyn InferenceIndex,
    pub inventory: &'a dyn Fn(&Entry) -> Option<Arc<ClassFileFacts>>,
    pub source_members: &'a dyn Fn(&Entry) -> bool,
}

impl InferenceIndex for BinaryDiagnostics<'_> {
    fn java_source_members_complete(&self, owner: &Entry) -> bool {
        (self.source_members)(owner)
    }
    fn java_class_inventory(&self, owner: &Entry) -> Option<Arc<ClassFileFacts>> {
        (self.inventory)(owner)
    }
    fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
        self.index.lookup_by_name(name)
    }
    fn members_of(&self, name: &str) -> LookupEntries<'_> {
        self.index.members_of(name)
    }
    fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
        self.index.extensions_for(name)
    }
    fn origin_facts(&self) -> Option<&OriginFacts> {
        self.index.origin_facts()
    }
    fn scope_is_complete(&self) -> bool {
        self.index.scope_is_complete()
    }
    fn permits_unique_member_fallback(&self) -> bool {
        self.index.permits_unique_member_fallback()
    }
    fn may_have_compiled_callables(&self) -> bool {
        self.index.may_have_compiled_callables()
    }
    fn jvm_sources(&self, owner: &str) -> Vec<&ktcore::jvm::SourceFile> {
        self.index.jvm_sources(owner)
    }
    fn contributions_for_file(&self, file: &str) -> &[ktcore::extensions::ContributionSet] {
        self.index.contributions_for_file(file)
    }
    fn entries_with_prefix<'a>(
        &'a self,
        prefix: &'a str,
        top_level_only: bool,
    ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
        self.index.entries_with_prefix(prefix, top_level_only)
    }
    fn entries_for_file(&self, file: &str) -> Vec<Entry> {
        self.index.entries_for_file(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const PERSON: &[u8] =
        include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");

    fn jar(path: &Path, entries: &[(&str, &[u8])]) {
        let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for (name, bytes) in entries {
            archive
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
        }
        archive.finish().unwrap();
    }

    fn task(classpath: &[PathBuf]) -> CompilerTask {
        serde_json::from_value(serde_json::json!({
            "task": ":compileJava", "language": "java", "task_type": "org.gradle.api.tasks.compile.JavaCompile",
            "arguments": [], "annotation_processor_path": [], "source_files": ["/src/Use.java"],
            "classpath": classpath, "java_home": "/jdk", "options": {}, "output_roots": [],
            "complete": true, "problem": null
        })).unwrap()
    }

    #[test]
    fn binary_inventory_uses_the_actual_first_class_and_rejects_changed_selected_inputs() {
        use crate::classpath::{ModuleClasspath, ResolvedArtifact, SourceSetClasspath};
        let root =
            std::env::temp_dir().join(format!("ktlsp_binary_selection_{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let current = root.join("current.jar");
        let decoy = root.join("decoy.jar");
        for path in [&current, &decoy] {
            jar(path, &[("pluginfixture/Person.class", PERSON)]);
        }
        let model = ProjectSnapshot::new(vec![ModuleClasspath {
            module: ":".into(),
            project_dir: root.clone(),
            build: String::new(),
            unresolved: Vec::new(),
            entries: vec![current.clone(), decoy.clone()],
            source_sets: vec![SourceSetClasspath {
                name: "main".into(),
                source_roots: vec![root.join("src")],
                entries: vec![current.clone(), decoy.clone()],
                artifacts: [(&current, "2"), (&decoy, "1")]
                    .into_iter()
                    .map(|(path, version)| ResolvedArtifact {
                        path: path.clone(),
                        component: ComponentId::Module {
                            group: "vendor".into(),
                            name: "api".into(),
                            version: version.into(),
                        },
                    })
                    .collect(),
                ..Default::default()
            }],
        }]);
        let mut index = crate::index::Index::new();
        let source = "package pluginfixture; public class Person {}";
        let tree = crate::java::JavaParser::new().parse(source);
        index.replace_file(
            "Person.java",
            crate::java::extract_symbols(&tree, source),
            crate::index::Tier::Durable,
        );
        let owner = index.lookup_type("Person")[0];
        let caller = crate::paths::file_key(&root.join("src/Use.java"));
        let mut cache = ClassInventoryCache::default();
        let origin = "maven:vendor:api:2";
        let wrong_order = task(&[decoy.clone(), current.clone()]);
        assert!(cache
            .inventory(&model, &wrong_order, &caller, owner, origin)
            .is_none());
        let selected = task(&[current.clone(), decoy]);
        let first = cache
            .inventory(&model, &selected, &caller, owner, origin)
            .unwrap();
        let repeated = cache
            .inventory(&model, &selected, &caller, owner, origin)
            .unwrap();
        assert!(Arc::ptr_eq(&first, &repeated));
        std::fs::write(current, b"replaced without a model refresh").unwrap();
        assert!(cache
            .inventory(&model, &selected, &caller, owner, origin)
            .is_none());
        let java_home = root.join("jdk");
        std::fs::create_dir_all(java_home.join("jmods")).unwrap();
        jar(
            &java_home.join("jmods/java.base.jmod"),
            &[("classes/pluginfixture/Person.class", PERSON)],
        );
        let jdk_origin = classpath::jdk_origin_id(&java_home.join("lib/src.zip"));
        let mut jdk_task = selected;
        jdk_task.java_home = Some(java_home);
        for release in [
            serde_json::Value::Null,
            serde_json::json!(8),
            serde_json::json!(17),
        ] {
            jdk_task.options.insert("release".into(), release.clone());
            assert_eq!(
                cache
                    .inventory(&model, &jdk_task, &caller, owner, &jdk_origin)
                    .is_some(),
                release.is_null()
            );
        }
        jdk_task.options.remove("release");
        assert!(cache
            .inventory(&model, &jdk_task, &caller, owner, &jdk_origin)
            .is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_cache_rechecks_created_classes_and_compiler_services_and_stays_bounded() {
        let root = std::env::temp_dir().join(format!("ktlsp_binary_cache_{}", std::process::id()));
        std::fs::create_dir_all(root.join("pluginfixture")).unwrap();
        let mut cache = ClassInventoryCache::default();
        let entry = "pluginfixture/Person.class";
        assert!(cache.read(&root, entry, "origin").unwrap().is_none());
        let java = task(&[root.clone()]);
        assert!(cache.plain_caller(&java));
        std::fs::write(root.join(entry), PERSON).unwrap();
        assert!(cache.read(&root, entry, "origin").unwrap().is_some());
        std::fs::write(root.join(entry), b"invalid replacement").unwrap();
        assert!(cache.read(&root, entry, "origin").is_err());
        let service = root.join("META-INF/services/javax.annotation.processing.Processor");
        std::fs::create_dir_all(service.parent().unwrap()).unwrap();
        std::fs::write(service, b"").unwrap();
        assert!(!cache.plain_caller(&java));
        let binary = root.join("services.jar");
        jar(&binary, &[]);
        let java = task(&[binary.clone()]);
        assert!(cache.plain_caller(&java));
        jar(
            &binary,
            &[("META-INF/services/com.sun.source.util.Plugin", b"autostart")],
        );
        assert!(!cache.plain_caller(&java));
        for number in 0..300 {
            assert!(cache
                .read(&root, &format!("missing{number}.class"), "origin")
                .unwrap()
                .is_none());
        }
        assert!(cache.classes.contains_key(&(
            root.clone(),
            "missing0.class".into(),
            "origin".into()
        )));
        assert!(cache.classes.len() <= MAX_CACHE_ENTRIES && cache.class_bytes <= MAX_CLASS_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compiler_service_cache_separates_ap_disable_and_never_disables_plugins() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_proc_none_cache_{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let binary = root.join("processor.jar");
        jar(
            &binary,
            &[(
                "META-INF/services/javax.annotation.processing.Processor",
                b"ap.Provider",
            )],
        );
        let normal = task(&[binary.clone()]);
        let mut disabled = normal.clone();
        disabled.arguments.push("-proc:none".into());
        let mut cache = ClassInventoryCache::default();
        assert!(!cache.plain_caller(&normal));
        assert!(cache.plain_caller(&disabled));
        assert!(!cache.plain_caller(&normal));
        assert!(cache.plain_caller(&disabled));
        let plugin = root.join("plugin.jar");
        jar(
            &plugin,
            &[(
                "META-INF/services/com.sun.source.util.Plugin",
                b"plugin.Provider",
            )],
        );
        disabled.classpath = vec![plugin];
        assert!(!cache.plain_caller(&disabled));
        disabled.classpath.clear();
        disabled.source_files.push(root.join("module-info.java"));
        assert!(!cache.plain_caller(&disabled));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn static_receiver_diagnostics_do_not_walk_unrelated_index_files() {
        struct LocalIndex {
            index: crate::index::Index,
            global_walks: std::cell::Cell<usize>,
        }
        impl InferenceIndex for LocalIndex {
            fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::lookup_by_name(&self.index, name)
            }
            fn members_of(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::members_of(&self.index, name)
            }
            fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
                InferenceIndex::extensions_for(&self.index, name)
            }
            fn entries_for_file(&self, file: &str) -> Vec<Entry> {
                InferenceIndex::entries_for_file(&self.index, file)
            }
            fn entries_with_prefix<'a>(
                &'a self,
                prefix: &'a str,
                top_level_only: bool,
            ) -> Box<dyn Iterator<Item = &'a Entry> + 'a> {
                self.global_walks.set(self.global_walks.get() + 1);
                InferenceIndex::entries_with_prefix(&self.index, prefix, top_level_only)
            }
        }
        let source = "package app; class Known {} class Use { void f() { Known.missing(); } }";
        let tree = crate::java::JavaParser::new().parse(source);
        let mut inner = LocalIndex {
            index: crate::index::Index::new(),
            global_walks: std::cell::Cell::new(0),
        };
        inner.index.replace_file(
            "Use.java",
            crate::java::extract_symbols(&tree, source),
            crate::index::Tier::Volatile,
        );
        let index = BinaryDiagnostics {
            index: &inner,
            inventory: &|_| None,
            source_members: &|_| false,
        };
        let mut facts = crate::resolve::CompletenessFacts::complete();
        facts.project_packages_complete.insert("app".into());
        let diagnostics = crate::java::diagnostics(&index, "Use.java", &tree, source, &facts);
        assert!(diagnostics
            .iter()
            .any(|item| item.message.contains("missing")));
        assert_eq!(inner.global_walks.get(), 0);
    }
}
