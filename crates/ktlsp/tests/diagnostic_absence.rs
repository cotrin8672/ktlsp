//! Negative diagnostics need a complete visible world, not just an empty goto result.

use ktlsp::diagnostics::DiagnosticCode;
use ktlsp::index::Tier;
use ktlsp::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
use ktlsp::workspace::Workspace;

fn unresolved_at(ws: &mut Workspace, key: &str, start: usize) -> bool {
    ws.diagnostics(key).iter().any(|diagnostic| {
        diagnostic.code == Some(DiagnosticCode::UnresolvedReference)
            && diagnostic.start_byte <= start
            && start < diagnostic.end_byte
    })
}

fn complete_workspace(files: &[(&str, &str)]) -> Workspace {
    let mut ws = Workspace::new();
    for (key, text) in files {
        ws.open(*key, (*text).into());
    }
    ws.assume_index_complete_for_tests();
    ws
}

#[test]
fn kotlin_missing_full_import_and_its_type_usage_are_diagnosed() {
    let text = "package app\nimport generated.Removed\nfun use(value: Removed) {}\n";
    let mut ws = complete_workspace(&[("Use.kt", text)]);
    assert!(unresolved_at(
        &mut ws,
        "Use.kt",
        text.find("Removed").unwrap()
    ));
    assert!(unresolved_at(
        &mut ws,
        "Use.kt",
        text.rfind("Removed").unwrap()
    ));
}

#[test]
fn kotlin_missing_aliased_import_uses_the_real_target_span() {
    let text = "package app\nimport generated.Removed as Renamed\nfun use(value: Renamed) {}\n";
    let mut ws = complete_workspace(&[("Use.kt", text)]);
    assert!(unresolved_at(
        &mut ws,
        "Use.kt",
        text.find("Removed").unwrap()
    ));
    assert!(!unresolved_at(
        &mut ws,
        "Use.kt",
        text.find("Renamed").unwrap()
    ));
    assert!(unresolved_at(
        &mut ws,
        "Use.kt",
        text.rfind("Renamed").unwrap()
    ));
}

#[test]
fn kotlin_unrelated_same_name_does_not_resolve_a_full_import() {
    let text = "package app\nimport missing.Known\nfun use(value: Known) {}\n";
    let mut ws = complete_workspace(&[
        ("Known.kt", "package other\nclass Known\n"),
        ("Use.kt", text),
    ]);
    assert!(unresolved_at(
        &mut ws,
        "Use.kt",
        text.find("Known").unwrap()
    ));
}

#[test]
fn kotlin_resolved_nested_alias_function_and_property_imports_are_clean() {
    let text = "package app\nimport dep.Outer.Inner\nimport dep.Alias as Renamed\nimport dep.produce as factory\nimport dep.count\nfun use(value: Inner, other: Renamed) {\n    factory()\n    count\n}\n";
    let mut ws = complete_workspace(&[
        ("Types.kt", "package dep\nclass Outer {\n    class Inner\n}\ntypealias Alias = Outer\nfun produce(): Int = 1\nval count: Int = 2\n"),
        ("Use.kt", text),
    ]);
    assert!(
        ws.diagnostics("Use.kt").is_empty(),
        "{:?}",
        ws.diagnostics("Use.kt")
    );
}

#[test]
fn kotlin_missing_import_waits_for_project_library_and_jdk_coverage() {
    let text = "package app\nimport generated.Removed\nfun use(value: Removed) {}\n";
    for missing in ["project", "library", "jdk"] {
        let mut ws = complete_workspace(&[("Use.kt", text)]);
        match missing {
            "project" => ws.set_project_scan_complete(false),
            "library" => ws.set_library_index_complete(false),
            "jdk" => ws.set_jdk_index_complete(false),
            _ => unreachable!(),
        }
        assert!(
            !unresolved_at(&mut ws, "Use.kt", text.find("Removed").unwrap()),
            "{missing}"
        );
    }
}

#[test]
fn kotlin_missing_import_stays_unknown_without_a_published_gradle_scope() {
    let text = "package app\nimport generated.Removed\nfun use(value: Removed) {}\n";
    let mut ws = complete_workspace(&[("Use.kt", text)]);
    ws.index.replace_scope_visibilities(Vec::new());
    assert!(!unresolved_at(
        &mut ws,
        "Use.kt",
        text.find("Removed").unwrap()
    ));
}

#[test]
fn callable_surface_stays_unknown_without_a_published_scope() {
    use ktlsp::resolve::{self, CompletenessFacts, IncompletenessReason};

    let mut ws = complete_workspace(&[("Current.java", "package app; class Current {}")]);
    let owner = ws.index.lookup_type("Current")[0].clone();
    ws.index.replace_scope_visibilities(Vec::new());
    let scope = ws.index.scoped_for_file("Use.java");
    assert!(matches!(
        resolve::declaration_surface_status(&scope, &owner, &CompletenessFacts::complete()),
        resolve::ResolutionStatus::Unknown(reasons) if reasons.contains(&IncompletenessReason::QueryScopeIncomplete)
    ));
}

fn project_member_diagnostics(owner_key: &str, owner: &str) {
    let text = "package app; import generated.Current; class Use { void run(Current value) { value.current(); value.removed(); } }";
    let mut ws = complete_workspace(&[(owner_key, owner), ("Use.java", text)]);
    assert!(!unresolved_at(
        &mut ws,
        "Use.java",
        text.find("current()").unwrap()
    ));
    assert!(
        unresolved_at(&mut ws, "Use.java", text.find("removed()").unwrap()),
        "{:?}",
        ws.diagnostics("Use.java")
    );
}

#[test]
fn java_missing_qualified_member_on_a_java_project_type_is_diagnosed() {
    project_member_diagnostics(
        "Current.java",
        "package generated; public class Current { public int current() { return 2; } }",
    );
}

#[test]
fn java_missing_qualified_member_on_a_kotlin_project_type_is_diagnosed() {
    project_member_diagnostics(
        "Current.kt",
        "package generated\nclass Current {\n    fun current(): Int = 2\n}\n",
    );
}

#[test]
fn java_known_inherited_qualified_member_is_not_diagnosed() {
    let text = "package app; class Use { void run(Child value) { value.current(); } }";
    let mut ws = complete_workspace(&[
        (
            "Base.java",
            "package app; class Base { int current() { return 2; } }",
        ),
        ("Child.java", "package app; class Child extends Base {}"),
        ("Use.java", text),
    ]);
    assert!(
        ws.diagnostics("Use.java").is_empty(),
        "{:?}",
        ws.diagnostics("Use.java")
    );
}

fn unknown_library_surface(representation: SourceRepresentation) {
    let owner = "package vendor; public class Current {}";
    let tree = ktlsp::java::JavaParser::new().parse(owner);
    let text = "package app; import vendor.Current; class Use { void run(Current value) { value.generated(); } }";
    let mut ws = complete_workspace(&[("Use.java", text)]);
    ws.index.replace_file(
        "Current.java",
        ktlsp::java::extract_symbols(&tree, owner),
        Tier::Durable,
    );
    let scope = ProjectScope {
        module: "app".into(),
        source_set: "main".into(),
    };
    ws.index.set_file_origin(
        "Use.java",
        FileOrigin {
            origin_id: "project-app".into(),
            representation: SourceRepresentation::Source,
            project_scope: Some(scope.clone()),
        },
    );
    ws.index.set_file_origin(
        "Current.java",
        FileOrigin {
            origin_id: "vendor-current".into(),
            representation,
            project_scope: None,
        },
    );
    ws.index.replace_scope_visibilities(vec![ScopeVisibility {
        scope,
        ordered_origins: vec!["project-app".into(), "vendor-current".into()],
    }]);
    assert!(
        !unresolved_at(&mut ws, "Use.java", text.find("generated()").unwrap()),
        "{:?}",
        ws.diagnostics("Use.java")
    );
}

#[test]
fn java_source_only_library_keeps_generated_member_absence_unknown() {
    unknown_library_surface(SourceRepresentation::Source);
}

#[test]
fn java_shallow_stub_keeps_member_absence_unknown() {
    unknown_library_surface(SourceRepresentation::Stub);
}

#[test]
fn project_receiver_keeps_inherited_source_only_library_surface_unknown() {
    let owner = "package vendor; public class Base { public int current() { return 2; } }";
    let tree = ktlsp::java::JavaParser::new().parse(owner);
    let java =
        "package app; class Use { void run(Child value) { value.current(); value.generated(); } }";
    let kotlin =
        "package app\nfun use(value: Child) {\n    value.current()\n    value.generated()\n}\n";
    let mut ws = complete_workspace(&[
        (
            "Child.java",
            "package app; class Child extends vendor.Base {}",
        ),
        ("Use.java", java),
        ("Use.kt", kotlin),
    ]);
    ws.index.replace_file(
        "Base.java",
        ktlsp::java::extract_symbols(&tree, owner),
        Tier::Durable,
    );
    for (key, text) in [("Use.java", java), ("Use.kt", kotlin)] {
        assert!(
            !unresolved_at(&mut ws, key, text.find("generated()").unwrap()),
            "{:?}",
            ws.diagnostics(key)
        );
        assert_eq!(
            ws.goto_definition(key, text.find("current()").unwrap())
                .len(),
            1
        );
    }
}

#[test]
fn java_missing_qualified_member_waits_for_unknown_superclass() {
    let text = "package app; class Use { void run(Child value) { value.generated(); } }";
    let mut ws = complete_workspace(&[
        ("Child.java", "package app; class Child extends Missing {}"),
        ("Use.java", text),
    ]);
    assert!(!unresolved_at(
        &mut ws,
        "Use.java",
        text.find("generated()").unwrap()
    ));
}

#[test]
fn java_implicit_object_members_do_not_become_false_absence_proofs() {
    let text = "package app; class Use { String use(Current value) { return value.toString(); } }";
    let mut ws = complete_workspace(&[
        ("Current.java", "package app; class Current {}"),
        ("Use.java", text),
    ]);
    let object =
        "package java.lang; public class Object { public String toString() { return null; } }";
    let tree = ktlsp::java::JavaParser::new().parse(object);
    ws.index.replace_file(
        "Object.java",
        ktlsp::java::extract_symbols(&tree, object),
        Tier::Durable,
    );
    assert!(!unresolved_at(
        &mut ws,
        "Use.java",
        text.find("toString").unwrap()
    ));
}

#[test]
fn java_field_and_method_annotations_keep_the_owner_surface_unknown() {
    let text = "package app; class Use { void run(Current value) { value.current(); value.generated(); } }";
    for member in [
        "@Generate private String name; public int current() { return 2; }",
        "@Generate public int current() { return 2; }",
    ] {
        let owner = format!("package app; class Current {{ {member} }} @interface Generate {{}}");
        let mut ws = complete_workspace(&[("Current.java", &owner), ("Use.java", text)]);
        assert!(!unresolved_at(
            &mut ws,
            "Use.java",
            text.find("generated()").unwrap()
        ));
        assert_eq!(
            ws.goto_definition("Use.java", text.find("current()").unwrap())
                .len(),
            1
        );
        ws.open(
            "Current.java",
            "package app; class Current { public int current() { return 2; } }".into(),
        );
        assert!(unresolved_at(
            &mut ws,
            "Use.java",
            text.find("generated()").unwrap()
        ));
    }
}

#[test]
fn lombok_source_only_getter_stays_unknown_until_annotations_are_removed() {
    // Same authored declaration as the independent javac/Kotlin Lombok ON/OFF compiler control.
    let person = "package pluginfixture;\n\nimport lombok.AllArgsConstructor;\nimport lombok.Getter;\n\n@Getter\n@AllArgsConstructor\npublic final class Person {\n    private String name;\n}\n";
    let text = "package app; import pluginfixture.Person; class Use { String use(Person value) { return value.getName(); } }";
    let mut ws = complete_workspace(&[("Person.java", person), ("Use.java", text)]);
    let start = text.find("getName").unwrap();
    assert!(!unresolved_at(&mut ws, "Use.java", start));
    assert!(
        ws.goto_definition("Use.java", start).is_empty(),
        "unknown is not a fabricated getter declaration"
    );
    ws.open(
        "Person.java",
        "package pluginfixture; public final class Person { private String name; }".into(),
    );
    assert!(unresolved_at(&mut ws, "Use.java", start));
    ws.open("Person.java", person.into());
    assert!(!unresolved_at(&mut ws, "Use.java", start));
}

#[test]
fn serialization_source_only_member_stays_unknown_without_a_compiler_projection() {
    // The independent compiler accepts Model.serializer() only with the serialization plugin.
    let model = "package pluginfixture\n\nimport kotlinx.serialization.Serializable\n\n@Serializable\ndata class Model(val label: String)\n";
    let text = "package app\nimport pluginfixture.Model\nfun use() { Model.serializer() }\n";
    let mut ws = complete_workspace(&[("Model.kt", model), ("Use.kt", text)]);
    let start = text.find("serializer").unwrap();
    assert!(!unresolved_at(&mut ws, "Use.kt", start));
    assert!(ws.goto_definition("Use.kt", start).is_empty());
    ws.open(
        "Model.kt",
        "package pluginfixture\ndata class Model(val label: String)\n".into(),
    );
    assert!(unresolved_at(&mut ws, "Use.kt", start));
}

#[test]
fn configured_gradle_source_world_does_not_certify_unknown_compiler_producers() {
    let text = "package app; import generated.Removed; class Use { int current() { return 2; } void run(Current value) { current(); generated(); value.current(); value.generated(); current(1); value.current(1); } }";
    let (mut ws, keys) = configured_workspace(&[
        (
            "Current.java",
            "package app; class Current { int current() { return 2; } }",
        ),
        ("Use.java", text),
    ]);
    let use_key = &keys[1];
    assert_eq!(
        ws.goto_definition(use_key, text.find("value.current()").unwrap() + 6)
            .len(),
        1
    );
    assert!(
        ws.diagnostics(use_key).iter().all(|diagnostic| !matches!(
            diagnostic.code,
            Some(DiagnosticCode::UnresolvedReference | DiagnosticCode::CallShapeMismatch)
        )),
        "{:?}",
        ws.diagnostics(use_key)
    );
}

fn configured_workspace(files: &[(&str, &str)]) -> (Workspace, Vec<String>) {
    use ktlsp::classpath::{ModuleClasspath, ProjectSnapshot, SourceSetClasspath};
    use std::sync::Arc;

    let root = std::env::temp_dir().join("ktlsp-diagnostic-configured-producer");
    let source_root = root.join("src/main/java");
    let mut ws = Workspace::new();
    ws.set_project_model(Arc::new(ProjectSnapshot::new(vec![ModuleClasspath {
        module: ":app".into(),
        project_dir: root,
        build: "fixture".into(),
        source_sets: vec![SourceSetClasspath {
            name: "main".into(),
            source_roots: vec![source_root.clone()],
            ..Default::default()
        }],
        entries: Vec::new(),
        unresolved: Vec::new(),
    }])));
    let keys = files
        .iter()
        .map(|(name, text)| {
            let key = source_root.join(name).to_string_lossy().replace('\\', "/");
            ws.open(&key, (*text).into());
            key
        })
        .collect();
    ws.assume_index_complete_for_tests();
    (ws, keys)
}

#[test]
fn kotlin_unknown_compiler_world_is_consistent_across_import_and_call_spellings() {
    let text = "package app\nimport generated.Removed\nimport generated.*\nfun use(value: Current, removed: Removed, unimported: Missing) {\n    value.current()\n    value.generated()\n    missing()\n    value.current(1)\n}\n";
    let (mut ws, keys) = configured_workspace(&[
        (
            "Current.kt",
            "package app\nclass Current {\n    fun current(): Int = 2\n}\n",
        ),
        ("Use.kt", text),
    ]);
    let use_key = &keys[1];
    assert_eq!(
        ws.goto_definition(use_key, text.find("current()").unwrap())
            .len(),
        1
    );
    assert!(
        ws.diagnostics(use_key).iter().all(|diagnostic| !matches!(
            diagnostic.code,
            Some(DiagnosticCode::UnresolvedReference | DiagnosticCode::CallShapeMismatch)
        )),
        "{:?}",
        ws.diagnostics(use_key)
    );
}

#[test]
fn java_annotation_guard_covers_bare_qualified_calls_and_overloads_only_for_its_owner() {
    let annotated = "package app; @Generate class Current { int current() { return 2; } void run() { generated(); this.generated(); current(1); this.current(1); } } @interface Generate {}";
    let plain = "package app; class Plain { void run() { missing(); this.missing(); } }";
    let mut ws = complete_workspace(&[("Current.java", annotated), ("Plain.java", plain)]);
    assert!(
        ws.diagnostics("Current.java")
            .iter()
            .all(|diagnostic| !matches!(
                diagnostic.code,
                Some(DiagnosticCode::UnresolvedReference | DiagnosticCode::CallShapeMismatch)
            )),
        "{:?}",
        ws.diagnostics("Current.java")
    );
    assert!(unresolved_at(
        &mut ws,
        "Plain.java",
        plain.find("missing()").unwrap()
    ));
    assert!(unresolved_at(
        &mut ws,
        "Plain.java",
        plain.rfind("missing()").unwrap()
    ));
    let without_annotation = annotated.replace("@Generate class", "class");
    ws.open("Current.java", without_annotation.clone());
    assert!(unresolved_at(
        &mut ws,
        "Current.java",
        without_annotation.find("generated()").unwrap()
    ));
    assert!(unresolved_at(
        &mut ws,
        "Current.java",
        without_annotation.rfind("generated()").unwrap()
    ));
    assert_eq!(
        ws.diagnostics("Current.java")
            .iter()
            .filter(|diagnostic| diagnostic.code == Some(DiagnosticCode::CallShapeMismatch))
            .count(),
        2
    );
}

#[test]
fn kotlin_annotation_guard_covers_implicit_and_qualified_overloads() {
    let annotated = "package app\n@Generate\nclass Current {\n    fun current(): Int = 2\n    fun run() {\n        current(1)\n        this.current(1)\n        this.generated()\n    }\n}\nannotation class Generate\n";
    let mut ws = complete_workspace(&[("Current.kt", annotated)]);
    assert!(
        ws.diagnostics("Current.kt")
            .iter()
            .all(|diagnostic| !matches!(
                diagnostic.code,
                Some(DiagnosticCode::UnresolvedReference | DiagnosticCode::CallShapeMismatch)
            )),
        "{:?}",
        ws.diagnostics("Current.kt")
    );
    let without_annotation = annotated.replace("@Generate\n", "");
    ws.open("Current.kt", without_annotation.clone());
    assert!(unresolved_at(
        &mut ws,
        "Current.kt",
        without_annotation.find("generated()").unwrap()
    ));
    assert_eq!(
        ws.diagnostics("Current.kt")
            .iter()
            .filter(|diagnostic| diagnostic.code == Some(DiagnosticCode::CallShapeMismatch))
            .count(),
        2
    );
}

#[test]
fn lombok_source_only_constructor_has_no_false_arity_error() {
    let person = "package pluginfixture;\nimport lombok.AllArgsConstructor;\nimport lombok.Getter;\n@Getter\n@AllArgsConstructor\npublic final class Person {\n    private String name;\n}\n";
    let text = "package app; import pluginfixture.Person; class Use { Person use() { return new Person(\"Ada\"); } }";
    let mut ws = complete_workspace(&[("Person.java", person), ("Use.java", text)]);
    assert!(ws
        .diagnostics("Use.java")
        .iter()
        .all(|diagnostic| diagnostic.code != Some(DiagnosticCode::CallShapeMismatch)));
    assert_eq!(
        ws.goto_definition("Use.java", text.find("new Person").unwrap() + 4)
            .len(),
        1
    );
}

#[test]
fn normalized_absence_query_retains_unknown_producer_and_dirty_package_reasons() {
    use ktlsp::infer::FileCtx;
    use ktlsp::resolve::{self, CompletenessFacts, IncompletenessReason};
    use ktlsp::types::Type;

    let ws = complete_workspace(&[(
        "Current.java",
        "package generated; public class Current { public int current() { return 2; } }",
    )]);
    let receiver = Type::class("Current", Some("generated".into()));
    let ctx = FileCtx::new("app".into(), Vec::new());
    assert!(!CompletenessFacts::default().compiler_producers_complete);
    let mut facts = CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    let status = resolve::member_name_status_for_receiver(
        &ws.index,
        "Use.java",
        &receiver,
        "generated",
        &facts,
        &ctx,
    );
    assert!(
        matches!(status, resolve::ResolutionStatus::Unknown(reasons) if reasons.contains(&IncompletenessReason::CompilerProducerIncomplete))
    );
    assert!(resolve::member_name_status_for_receiver(
        &ws.index, "Use.java", &receiver, "current", &facts, &ctx
    )
    .is_found());
    facts.compiler_producers_complete = true;
    facts.project_scan_complete = false;
    let status = resolve::member_name_status_for_receiver(
        &ws.index,
        "Use.java",
        &receiver,
        "generated",
        &facts,
        &ctx,
    );
    assert!(
        matches!(status, resolve::ResolutionStatus::Unknown(reasons) if reasons.contains(&IncompletenessReason::ProjectPackageIncomplete("generated".into())))
    );
}

#[test]
fn dirty_owner_source_does_not_certify_missing_members() {
    let root = std::env::temp_dir().join(format!("ktlsp-diagnostic-dirty-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let owner_key = root.join("Current.kt").to_string_lossy().replace('\\', "/");
    let use_key = root.join("Use.java").to_string_lossy().replace('\\', "/");
    let text = "package app; class Use { void run(Current value) { value.current(); value.generated(); } }";
    let mut ws = complete_workspace(&[
        (
            &owner_key,
            "package app\nclass Current {\n    fun current(): Int = 2\n}\n",
        ),
        (&use_key, text),
    ]);
    ws.scan(&root);
    assert!(unresolved_at(
        &mut ws,
        &use_key,
        text.find("generated()").unwrap()
    ));
    ws.open(
        &owner_key,
        "package app\nclass Current {\n    fun current(): Int = 2\n".into(),
    );
    assert!(!unresolved_at(
        &mut ws,
        &use_key,
        text.find("generated()").unwrap()
    ));
    assert_eq!(
        ws.goto_definition(&use_key, text.find("current()").unwrap())
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn kotlin_missing_import_with_an_unknown_producer_is_not_declared_absent() {
    use ktlsp::infer::FileCtx;
    use ktlsp::parser::Import;
    use ktlsp::resolve::{self, CompletenessFacts, IncompletenessReason};

    let ws = Workspace::new();
    let mut facts = CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    let import = Import {
        path: "generated.Removed".into(),
        alias: None,
        wildcard: false,
    };
    let status = resolve::import_target_status(
        &ws.index,
        "Use.kt",
        &import,
        &facts,
        &FileCtx::new("app".into(), Vec::new()),
    );
    assert!(
        matches!(status, resolve::ResolutionStatus::Unknown(reasons) if reasons.contains(&IncompletenessReason::CompilerProducerIncomplete))
    );
}

struct BinaryInventoryIndex {
    index: ktlsp::index::Index,
    classes: std::collections::BTreeMap<String, std::sync::Arc<ktcore::classfile::ClassFileFacts>>,
    caller_known: bool,
}

impl ktlsp::index::InferenceIndex for BinaryInventoryIndex {
    fn lookup_by_name(&self, name: &str) -> ktlsp::index::LookupEntries<'_> {
        ktlsp::index::InferenceIndex::lookup_by_name(&self.index, name)
    }
    fn members_of(&self, name: &str) -> ktlsp::index::LookupEntries<'_> {
        ktlsp::index::InferenceIndex::members_of(&self.index, name)
    }
    fn extensions_for(&self, name: &str) -> ktlsp::index::LookupEntries<'_> {
        ktlsp::index::InferenceIndex::extensions_for(&self.index, name)
    }
    fn origin_facts(&self) -> Option<&ktlsp::index::OriginFacts> {
        ktlsp::index::InferenceIndex::origin_facts(&self.index)
    }
    fn java_class_inventory(
        &self,
        owner: &ktlsp::index::Entry,
    ) -> Option<std::sync::Arc<ktcore::classfile::ClassFileFacts>> {
        self.caller_known
            .then(|| self.classes.get(owner.path.as_ref()).cloned())
            .flatten()
    }
}

fn binary_inventory() -> BinaryInventoryIndex {
    use ktcore::classfile::{ClassFileFacts, MemberFacts};
    use ktcore::jvm::{ClassIdentity, InputIdentity};
    use std::sync::Arc;
    let mut index = ktlsp::index::Index::new();
    let mut classes = std::collections::BTreeMap::new();
    for (file, source, internal, origin, parent, methods) in [
        (
            "Versioned.java",
            "package vendor; public class Versioned { public int newApi() { return 2; } }",
            "vendor/Versioned",
            "vendor-v2",
            Some("java/lang/Object"),
            vec!["newApi", "generated"],
        ),
        (
            "Object.java",
            "package java.lang; public class Object {}",
            "java/lang/Object",
            "jdk-test",
            None,
            vec!["toString"],
        ),
    ] {
        let tree = ktlsp::java::JavaParser::new().parse(source);
        index.replace_file(
            file,
            ktlsp::java::extract_symbols(&tree, source),
            Tier::Durable,
        );
        index.set_file_origin(
            file,
            FileOrigin {
                origin_id: origin.into(),
                representation: SourceRepresentation::Source,
                project_scope: None,
            },
        );
        classes.insert(
            file.into(),
            Arc::new(ClassFileFacts {
                identity: ClassIdentity {
                    origin_id: origin.into(),
                    internal_name: internal.into(),
                    input: InputIdentity::Binary {
                        content_sha256: [1; 32],
                    },
                },
                major_version: 61,
                minor_version: 0,
                access_flags: 1,
                super_name: parent.map(Arc::from),
                interfaces: Vec::new(),
                signature: None,
                source_file: Some(Arc::from(file)),
                attribute_names: Vec::new(),
                fields: Vec::new(),
                methods: methods
                    .into_iter()
                    .map(|name| MemberFacts {
                        name: Arc::from(name),
                        descriptor: Arc::from("()I"),
                        access_flags: 1,
                        signature: None,
                        attribute_names: Vec::new(),
                    })
                    .collect(),
            }),
        );
    }
    BinaryInventoryIndex {
        index,
        classes,
        caller_known: true,
    }
}

fn inventory_status(
    index: &BinaryInventoryIndex,
    file: &str,
    name: &str,
    facts: &ktlsp::resolve::CompletenessFacts,
) -> ktlsp::resolve::ResolutionStatus<()> {
    ktlsp::resolve::member_name_status_for_receiver(
        index,
        file,
        &ktlsp::types::Type::class("Versioned", Some("vendor".into())),
        name,
        facts,
        &ktlsp::infer::FileCtx::new("app".into(), Vec::new()),
    )
}

#[test]
fn selected_java_binary_inventory_diagnoses_an_obsolete_member_without_global_producer_closure() {
    let index = binary_inventory();
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    assert!(inventory_status(&index, "Use.java", "oldApi", &facts).is_definitely_absent());
    assert!(inventory_status(&index, "Use.java", "newApi", &facts).is_found());
    for name in ["generated", "toString"] {
        assert!(
            !inventory_status(&index, "Use.java", name, &facts).is_definitely_absent(),
            "{name}"
        );
    }
    assert!(!inventory_status(&index, "Use.kt", "oldApi", &facts).is_definitely_absent());
}

#[test]
fn java_binary_inventory_requires_the_current_origin_caller_and_every_ancestor() {
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    for gap in [
        "ancestor",
        "caller",
        "origin",
        "identity",
        "cycle",
        "annotation",
    ] {
        let mut index = binary_inventory();
        match gap {
            "ancestor" => {
                index.classes.remove("Object.java");
            }
            "caller" => index.caller_known = false,
            "origin" => {
                std::sync::Arc::make_mut(index.classes.get_mut("Versioned.java").unwrap())
                    .identity
                    .origin_id = "vendor-v1".into()
            }
            "identity" => {
                std::sync::Arc::make_mut(index.classes.get_mut("Versioned.java").unwrap())
                    .identity
                    .internal_name = "decoy/Versioned".into()
            }
            "cycle" => {
                std::sync::Arc::make_mut(index.classes.get_mut("Object.java").unwrap()).super_name =
                    Some("vendor/Versioned".into())
            }
            "annotation" => {
                let owner = index.index.lookup_type("Versioned")[0];
                facts.member_surfaces_unknown.insert(ktlsp::symbol::Def {
                    file: owner.path.to_string(),
                    start_byte: owner.sym.start_byte,
                    end_byte: owner.sym.end_byte,
                });
            }
            _ => unreachable!(),
        }
        assert!(
            !inventory_status(&index, "Use.java", "oldApi", &facts).is_definitely_absent(),
            "{gap}"
        );
        facts.member_surfaces_unknown.clear();
    }
}

#[test]
fn java_binary_inventory_visits_interfaces_and_accepts_a_diamond_without_closing_missing_branches()
{
    let mut index = binary_inventory();
    let source = "package vendor; public interface Contract {}";
    let tree = ktlsp::java::JavaParser::new().parse(source);
    index.index.replace_file(
        "Contract.java",
        ktlsp::java::extract_symbols(&tree, source),
        Tier::Durable,
    );
    index.index.set_file_origin(
        "Contract.java",
        FileOrigin {
            origin_id: "vendor-v2".into(),
            representation: SourceRepresentation::Source,
            project_scope: None,
        },
    );
    let mut contract = (*index.classes["Versioned.java"]).clone();
    contract.identity.internal_name = "vendor/Contract".into();
    contract.methods.clear();
    index
        .classes
        .insert("Contract.java".into(), std::sync::Arc::new(contract));
    std::sync::Arc::make_mut(index.classes.get_mut("Versioned.java").unwrap())
        .interfaces
        .push("vendor/Contract".into());
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    assert!(inventory_status(&index, "Use.java", "oldApi", &facts).is_definitely_absent());
    let inherited = index.classes["Object.java"].methods[0].clone();
    std::sync::Arc::make_mut(index.classes.get_mut("Contract.java").unwrap())
        .methods
        .push(inherited);
    assert!(!inventory_status(&index, "Use.java", "toString", &facts).is_definitely_absent());
    std::sync::Arc::make_mut(index.classes.get_mut("Contract.java").unwrap())
        .interfaces
        .push("missing/Parent".into());
    assert!(!inventory_status(&index, "Use.java", "oldApi", &facts).is_definitely_absent());
}
