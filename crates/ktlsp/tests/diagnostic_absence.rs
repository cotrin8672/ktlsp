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
    source_owners: std::collections::BTreeSet<String>,
}

impl ktlsp::index::InferenceIndex for BinaryInventoryIndex {
    fn java_source_members_complete(&self, owner: &ktlsp::index::Entry) -> bool {
        self.source_owners.contains(owner.path.as_ref())
    }
    fn lookup_by_name(&self, name: &str) -> ktlsp::index::LookupEntries<'_> {
        ktlsp::index::InferenceIndex::lookup_by_name(&self.index, name)
    }
    fn members_of(&self, name: &str) -> ktlsp::index::LookupEntries<'_> {
        ktlsp::index::InferenceIndex::members_of(&self.index, name)
    }
    fn entries_for_file(&self, file: &str) -> Vec<ktlsp::index::Entry> {
        ktlsp::index::InferenceIndex::entries_for_file(&self.index, file)
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
        source_owners: Default::default(),
    }
}

#[test]
fn required_interface_methods_use_proven_implementations_and_withhold_provider_conflicts() {
    use ktcore::members::required_interface_methods;
    use ktlsp::resolve::{CompletenessFacts, ResolutionStatus};
    let replace = |index: &mut BinaryInventoryIndex, file, source: &str| {
        let tree = ktlsp::java::JavaParser::new().parse(source);
        assert!(!tree.root_node().has_error(), "{source}");
        index.index.replace_file(
            file,
            ktlsp::java::extract_symbols(&tree, source),
            Tier::Volatile,
        );
    };
    for (body, extra, provided) in [
        ("", "", false),
        ("public int required() { return 1; }", "", true),
        ("public long required() { return 1; }", "", false),
        ("private int required() { return 1; }", "", false),
        ("public static int required() { return 1; }", "", false),
        ("", ", DefaultProvider", false),
    ] {
        let mut index = binary_inventory();
        replace(
            &mut index,
            "Contract.java",
            "package app; public interface Contract { int required(); }",
        );
        replace(&mut index, "DefaultProvider.java", "package app; public interface DefaultProvider { default int required() { return 1; } }");
        replace(
            &mut index,
            "Impl.java",
            &format!("package app; public class Impl implements Contract{extra} {{ {body} }}"),
        );
        let owner = index.index.lookup_type("Impl")[0];
        let status = required_interface_methods(
            &index,
            owner,
            &index.classes["Object.java"],
            &CompletenessFacts::complete(),
            &|_| true,
        );
        if provided {
            assert!(matches!(status, ResolutionStatus::Found(ref methods) if methods.is_empty()));
        } else if body.is_empty() && extra.is_empty() {
            assert!(matches!(status, ResolutionStatus::Found(ref methods) if methods.len() == 1));
        } else {
            assert!(
                matches!(status, ResolutionStatus::Unknown(_)),
                "{body} {extra}"
            );
        }
    }
    for (contract, providers, expected_empty) in [
        ("default int required() { return 1; }", "Contract", true),
        (
            "default int required() { return 1; }",
            "Contract, DefaultProvider",
            false,
        ),
        ("int required();", "Contract, WrongReturn", false),
    ] {
        let mut index = binary_inventory();
        replace(
            &mut index,
            "Contract.java",
            &format!("package app; public interface Contract {{ {contract} }}"),
        );
        replace(&mut index, "DefaultProvider.java", "package app; public interface DefaultProvider { default int required() { return 1; } }");
        replace(
            &mut index,
            "WrongReturn.java",
            "package app; public interface WrongReturn { long required(); }",
        );
        replace(
            &mut index,
            "Impl.java",
            &format!("package app; public class Impl implements {providers} {{}}"),
        );
        let status = required_interface_methods(
            &index,
            index.index.lookup_type("Impl")[0],
            &index.classes["Object.java"],
            &CompletenessFacts::complete(),
            &|_| true,
        );
        assert_eq!(
            matches!(status, ResolutionStatus::Found(ref methods) if methods.is_empty()),
            expected_empty
        );
        if !expected_empty {
            assert!(matches!(status, ResolutionStatus::Unknown(_)));
        }
    }
}

#[test]
fn required_interface_methods_use_complete_object_facts_instead_of_a_name_blacklist() {
    use ktcore::members::required_interface_methods;
    use ktlsp::resolve::{CompletenessFacts, ResolutionStatus};
    let mut index = binary_inventory();
    for (file, source) in [
        (
            "Contract.java",
            "package app; public interface Contract { int required(); }",
        ),
        (
            "Impl.java",
            "package app; public class Impl implements Contract {}",
        ),
    ] {
        let tree = ktlsp::java::JavaParser::new().parse(source);
        index.index.replace_file(
            file,
            ktlsp::java::extract_symbols(&tree, source),
            Tier::Volatile,
        );
    }
    for flags in [0x0001, 0x0011] {
        let object = std::sync::Arc::make_mut(index.classes.get_mut("Object.java").unwrap());
        object.methods[0].name = "required".into();
        object.methods[0].access_flags = flags;
        let owner = index.index.lookup_type("Impl")[0];
        assert!(matches!(
            required_interface_methods(
                &index,
                owner,
                &index.classes["Object.java"],
                &CompletenessFacts::complete(),
                &|_| true
            ),
            ResolutionStatus::Unknown(_)
        ));
    }
    std::sync::Arc::make_mut(index.classes.get_mut("Object.java").unwrap()).methods[0].name =
        "toString".into();
    let mut facts = CompletenessFacts::complete();
    let owner = index.index.lookup_type("Impl")[0];
    let object = index.classes["Object.java"].clone();
    assert!(
        matches!(required_interface_methods(&index, owner, &object, &facts, &|_| true),
        ResolutionStatus::Found(ref methods) if methods.len() == 1)
    );
    assert!(matches!(
        required_interface_methods(&index, owner, &object, &facts, &|_| false),
        ResolutionStatus::Unknown(_)
    ));
    facts.member_surfaces_unknown.insert(ktlsp::symbol::Def {
        file: owner.path.to_string(),
        start_byte: owner.sym.start_byte,
        end_byte: owner.sym.end_byte,
    });
    assert!(matches!(
        required_interface_methods(&index, owner, &object, &facts, &|_| true),
        ResolutionStatus::Unknown(_)
    ));
}

#[test]
fn required_interface_methods_do_not_confuse_source_primitive_spellings_or_java_unit_objects() {
    use ktcore::members::required_interface_methods;
    use ktlsp::resolve::{CompletenessFacts, ResolutionStatus};
    for (file, source) in [
        (
            "Contract.java",
            "package app; public interface Contract { kotlin.Unit required(); }",
        ),
        (
            "Contract.java",
            "package app; public interface Contract { Int required(); }",
        ),
        (
            "Contract.java",
            "package app; public interface Contract { int required()[]; }",
        ),
        (
            "Contract.java",
            "package app; public interface Contract { final int required(); }",
        ),
        (
            "Contract.java",
            "package app; public interface Contract { synchronized int required(); }",
        ),
        (
            "Contract.java",
            "package app; public interface Contract { strictfp int required(); }",
        ),
        (
            "Contract.kt",
            "package app\ninterface Contract { fun required(): int }",
        ),
        (
            "Contract.kt",
            "package app\ninterface Contract { fun required(): void }",
        ),
        (
            "Contract.kt",
            "package app\ninterface Contract { final fun required(): Int }",
        ),
    ] {
        let mut index = binary_inventory();
        let language = ktlsp::language::SourceLanguage::for_key(file);
        let tree = ktlsp::language::LanguageParsers::new().parse(language, source);
        let facts = ktlsp::language::symbol_facts(language, &tree, source);
        assert!(facts.clean);
        index
            .index
            .replace_file(file, facts.symbols, Tier::Volatile);
        let implementation = "package app; public class Impl implements Contract {}";
        let tree = ktlsp::java::JavaParser::new().parse(implementation);
        index.index.replace_file(
            "Impl.java",
            ktlsp::java::extract_symbols(&tree, implementation),
            Tier::Volatile,
        );
        assert!(
            matches!(
                required_interface_methods(
                    &index,
                    index.index.lookup_type("Impl")[0],
                    &index.classes["Object.java"],
                    &CompletenessFacts::complete(),
                    &|_| true
                ),
                ResolutionStatus::Unknown(_)
            ),
            "{source}"
        );
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

fn source_member_inventory() -> BinaryInventoryIndex {
    let mut index = binary_inventory();
    let source = "package app; public class Current { public int current() { return 2; } }";
    let tree = ktlsp::java::JavaParser::new().parse(source);
    index.index.replace_file(
        "Current.java",
        ktlsp::java::extract_symbols(&tree, source),
        Tier::Volatile,
    );
    index.source_owners.insert("Current.java".into());
    index
}

fn source_member_status(
    index: &BinaryInventoryIndex,
    caller: &str,
    owner: &str,
    name: &str,
    facts: &ktlsp::resolve::CompletenessFacts,
) -> ktlsp::resolve::ResolutionStatus<()> {
    ktlsp::resolve::member_name_status_for_receiver(
        index,
        caller,
        &ktlsp::types::Type::class(owner, Some("app".into())),
        name,
        facts,
        &ktlsp::infer::FileCtx::new("app".into(), Vec::new()),
    )
}

#[test]
fn plain_java_source_authority_diagnoses_only_its_owner_without_global_producer_closure() {
    let index = source_member_inventory();
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    facts.project_packages_complete.insert("app".into());
    assert!(
        source_member_status(&index, "Use.java", "Current", "removed", &facts)
            .is_definitely_absent()
    );
    assert!(source_member_status(&index, "Use.java", "Current", "current", &facts).is_found());
    assert!(
        !source_member_status(&index, "Use.java", "Current", "toString", &facts)
            .is_definitely_absent()
    );
    assert!(
        !source_member_status(&index, "Use.kt", "Current", "removed", &facts)
            .is_definitely_absent()
    );
    let text =
        "package app; class Use { void run(Current value) { value.current(); value.removed(); } }";
    let tree = ktlsp::java::JavaParser::new().parse(text);
    let diagnostics = ktlsp::java::diagnostics(&index, "Use.java", &tree, text, &facts);
    assert!(
        diagnostics
            .iter()
            .any(|item| item.start_byte == text.find("removed").unwrap()
                && item.code == Some(DiagnosticCode::UnresolvedReference)),
        "{diagnostics:?}"
    );
    assert!(!diagnostics
        .iter()
        .any(|item| item.start_byte == text.find("current").unwrap()));
    let import = ktlsp::parser::Import {
        path: "app.Missing".into(),
        alias: None,
        wildcard: false,
    };
    assert!(!ktlsp::resolve::import_target_status(
        &index,
        "Use.java",
        &import,
        &facts,
        &ktlsp::infer::FileCtx::new("app".into(), Vec::new())
    )
    .is_definitely_absent());
    assert!(!facts.compiler_producers_complete);
}

#[test]
fn plain_java_source_authority_keeps_dirty_unknown_sdk_annotation_and_hierarchy_reasons() {
    for gap in [
        "authority",
        "sdk",
        "jdk",
        "library",
        "dirty",
        "annotation",
        "hierarchy",
        "kotlin",
    ] {
        let mut index = source_member_inventory();
        let mut facts = ktlsp::resolve::CompletenessFacts::complete();
        facts.compiler_producers_complete = false;
        facts.project_packages_complete.insert("app".into());
        match gap {
            "authority" => index.source_owners.clear(),
            "sdk" => {
                index.classes.remove("Object.java");
            }
            "jdk" => facts.jdk_index_complete = false,
            "library" => facts.library_index_complete = false,
            "dirty" => {
                facts.project_scan_complete = false;
                facts.project_packages_complete.clear();
            }
            "annotation" => {
                let owner = index.index.lookup_type("Current")[0];
                facts.member_surfaces_unknown.insert(ktlsp::symbol::Def {
                    file: owner.path.to_string(),
                    start_byte: owner.sym.start_byte,
                    end_byte: owner.sym.end_byte,
                });
            }
            "hierarchy" => {
                let source = "package app; public class Current extends Missing {}";
                let tree = ktlsp::java::JavaParser::new().parse(source);
                index.index.replace_file(
                    "Current.java",
                    ktlsp::java::extract_symbols(&tree, source),
                    Tier::Volatile,
                );
            }
            "kotlin" => {
                index.index.remove_file("Current.java");
                let source = "package app\nclass Current";
                let tree = ktlsp::parser::KotlinParser::new().parse(source);
                index.index.replace_file(
                    "Current.kt",
                    ktlsp::indexer::extract_symbols(&tree, source, "app"),
                    Tier::Volatile,
                );
                index.source_owners.insert("Current.kt".into()); // Even an over-broad adapter cannot close a Kotlin owner.
            }
            _ => unreachable!(),
        }
        assert!(
            !source_member_status(&index, "Use.java", "Current", "removed", &facts)
                .is_definitely_absent(),
            "{gap}"
        );
    }
}

const PLAIN_OWNER: &str =
    include_str!("../../../dev/corpus/fixtures/java-diagnostics/PlainOwner.java");
const PLAIN_USE: &str = include_str!("../../../dev/corpus/fixtures/java-diagnostics/PlainUse.java");

fn plain_java_call_shape_inventory() -> BinaryInventoryIndex {
    let mut index = source_member_inventory();
    index.index.remove_file("Current.java");
    index.source_owners.clear();
    index.source_owners.insert("PlainOwner.java".into());
    for (file, source) in [
        ("PlainOwner.java", PLAIN_OWNER),
        ("PlainUse.java", PLAIN_USE),
    ] {
        let tree = ktlsp::java::JavaParser::new().parse(source);
        index.index.replace_file(
            file,
            ktlsp::java::extract_symbols(&tree, source),
            Tier::Volatile,
        );
    }
    let object = std::sync::Arc::make_mut(index.classes.get_mut("Object.java").unwrap());
    let mut wait = object.methods[0].clone();
    wait.name = "wait".into();
    wait.descriptor = "(J)V".into();
    object.methods.push(wait);
    index
}

#[test]
fn plain_java_call_shape_authority_tracks_unsaved_owner_and_caller_edits() {
    let mut index = plain_java_call_shape_inventory();
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    facts.project_packages_complete.insert("app".into());
    let tree = ktlsp::java::JavaParser::new().parse(PLAIN_USE);
    let definitions = ktlsp::java::goto_definition(
        &index,
        "PlainUse.java",
        PLAIN_USE,
        &tree,
        PLAIN_USE.find("accept(\"Ada\")").unwrap(),
    );
    assert_eq!(definitions.len(), 1, "{definitions:?}");
    let definition = &definitions[0];
    let declaration = ktlsp::hierarchy::entry_for_name_range(
        &index,
        &definition.file,
        definition.start_byte,
        definition.end_byte,
    )
    .expect("a selected Java call must retain its indexed callable metadata");
    assert_eq!(declaration.path.as_ref(), "PlainOwner.java");
    assert_eq!(
        (declaration.sym.arity, declaration.sym.min_arity),
        (Some(1), Some(1))
    );
    let status = ktlsp::resolve::declaration_surface_status(&index, &declaration, &facts);
    assert!(status.is_found(), "{status:?}");
    let boolean_owner = PLAIN_OWNER.replace("accept(String value)", "accept(boolean value)");
    let broken = PLAIN_USE
        .replace("accept(\"Ada\")", "accept()")
        .replace("accept(1, 2)", "accept(true)");
    let repaired = PLAIN_USE.replace("accept(\"Ada\")", "accept(true)");
    let arity_error = (
        "value.accept()",
        "No overload of accept accepts 0 arguments",
    );
    let type_error = (
        "value.accept(true)",
        "No overload of accept accepts argument type (boolean)",
    );
    let both_errors = [arity_error, type_error];
    let arity_only = [arity_error];
    let type_only = [type_error];
    for (phase, owner, caller, expected_calls) in [
        ("original", PLAIN_OWNER, PLAIN_USE, &[][..]),
        (
            "caller broken",
            PLAIN_OWNER,
            broken.as_str(),
            &both_errors[..],
        ),
        (
            "owner accepts boolean",
            boolean_owner.as_str(),
            broken.as_str(),
            &arity_only[..],
        ),
        (
            "caller repaired",
            boolean_owner.as_str(),
            repaired.as_str(),
            &[][..],
        ),
        (
            "owner restored",
            PLAIN_OWNER,
            repaired.as_str(),
            &type_only[..],
        ),
        ("caller restored", PLAIN_OWNER, PLAIN_USE, &[][..]),
    ] {
        for (file, source) in [("PlainOwner.java", owner), ("PlainUse.java", caller)] {
            let tree = ktlsp::java::JavaParser::new().parse(source);
            index.index.replace_file(
                file,
                ktlsp::java::extract_symbols(&tree, source),
                Tier::Volatile,
            );
        }
        let tree = ktlsp::java::JavaParser::new().parse(caller);
        let diagnostics = ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, caller, &facts);
        let actual: Vec<_> = diagnostics
            .iter()
            .map(|item| {
                (
                    item.start_byte,
                    item.end_byte,
                    item.code,
                    item.message.as_str(),
                )
            })
            .collect();
        let expected: Vec<_> = expected_calls
            .iter()
            .map(|(call, message)| {
                let start = caller.find(*call).unwrap();
                (
                    start,
                    start + call.len(),
                    Some(DiagnosticCode::CallShapeMismatch),
                    *message,
                )
            })
            .collect();
        assert_eq!(actual, expected, "{phase}: {diagnostics:?}");
        assert!(!facts.compiler_producers_complete);
    }
}

#[test]
fn plain_java_call_shape_authority_preserves_incomplete_world_guards() {
    let caller = PLAIN_USE
        .replace("accept(\"Ada\")", "accept()")
        .replace("accept(1, 2)", "accept(true)");
    for gap in [
        "authority",
        "sdk",
        "caller",
        "jdk",
        "dirty",
        "annotation",
        "generic",
        "unknown-super",
        "explicit-super",
    ] {
        let mut index = plain_java_call_shape_inventory();
        let mut facts = ktlsp::resolve::CompletenessFacts::complete();
        facts.compiler_producers_complete = false;
        facts.project_packages_complete.insert("app".into());
        match gap {
            "authority" => index.source_owners.clear(),
            "sdk" => {
                index.classes.remove("Object.java");
            }
            "caller" => index.caller_known = false,
            "jdk" => facts.jdk_index_complete = false,
            "dirty" => {
                facts.project_scan_complete = false;
                facts.project_packages_complete.clear();
            }
            "annotation" => {
                let owner = index.index.lookup_type("PlainOwner")[0];
                facts.member_surfaces_unknown.insert(ktlsp::symbol::Def {
                    file: owner.path.to_string(),
                    start_byte: owner.sym.start_byte,
                    end_byte: owner.sym.end_byte,
                });
            }
            "generic" | "unknown-super" | "explicit-super" => {
                let declaration = match gap {
                    "generic" => "class PlainOwner<T>",
                    "unknown-super" => "class PlainOwner extends Missing",
                    "explicit-super" => "class PlainOwner extends java.lang.Object",
                    _ => unreachable!(),
                };
                let source = PLAIN_OWNER.replace("class PlainOwner", declaration);
                let tree = ktlsp::java::JavaParser::new().parse(&source);
                index.index.replace_file(
                    "PlainOwner.java",
                    ktlsp::java::extract_symbols(&tree, &source),
                    Tier::Volatile,
                );
            }
            _ => unreachable!(),
        }
        assert_eq!(
            index
                .index
                .members_of("PlainOwner")
                .iter()
                .filter(|entry| entry.sym.name == "accept")
                .count(),
            2,
            "{gap}"
        );
        let tree = ktlsp::java::JavaParser::new().parse(&caller);
        let diagnostics = ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, &caller, &facts);
        assert!(
            diagnostics
                .iter()
                .all(|item| item.code != Some(DiagnosticCode::CallShapeMismatch)),
            "{gap}: {diagnostics:?}"
        );
        assert!(!facts.compiler_producers_complete);
    }
}

#[test]
fn plain_java_call_shape_authority_does_not_close_an_unproven_receiver() {
    for (receiver, source) in [
        ("Missing", None),
        (
            "Child",
            Some("package app; public class Child extends PlainOwner {}"),
        ),
        (
            "AnnotatedChild",
            Some("package app; @Generate public class AnnotatedChild extends PlainOwner {} @interface Generate {}"),
        ),
    ] {
        let mut index = plain_java_call_shape_inventory();
        let mut facts = ktlsp::resolve::CompletenessFacts::complete();
        facts.compiler_producers_complete = false;
        facts.project_packages_complete.insert("app".into());
        if let Some(source) = source {
            let tree = ktlsp::java::JavaParser::new().parse(source);
            let file = format!("{receiver}.java");
            index.index.replace_file(
                &file,
                ktlsp::java::extract_symbols(&tree, source),
                Tier::Volatile,
            );
            if receiver == "AnnotatedChild" {
                let owner = index.index.lookup_type(receiver)[0];
                facts.member_surfaces_unknown.insert(ktlsp::symbol::Def {
                    file: owner.path.to_string(),
                    start_byte: owner.sym.start_byte,
                    end_byte: owner.sym.end_byte,
                });
            }
        }
        let valid = PLAIN_USE.replace("PlainOwner value", &format!("{receiver} value"));
        if source.is_some() {
            let tree = ktlsp::java::JavaParser::new().parse(&valid);
            let definitions = ktlsp::java::goto_definition(
                &index,
                "PlainUse.java",
                &valid,
                &tree,
                valid.find("accept(\"Ada\")").unwrap(),
            );
            assert!(!definitions.is_empty(), "{receiver}");
            assert!(
                definitions
                    .iter()
                    .all(|definition| definition.file == "PlainOwner.java"),
                "{receiver}: {definitions:?}"
            );
        }
        let caller = valid
            .replace("accept(\"Ada\")", "accept()")
            .replace("accept(1, 2)", "accept(true)");
        let tree = ktlsp::java::JavaParser::new().parse(&caller);
        let diagnostics =
            ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, &caller, &facts);
        assert!(
            diagnostics
                .iter()
                .all(|item| item.code != Some(DiagnosticCode::CallShapeMismatch)),
            "{receiver}: {diagnostics:?}"
        );
        assert!(!facts.compiler_producers_complete);
    }
}

#[test]
fn plain_java_call_shape_authority_does_not_expand_constructor_diagnostics() {
    let mut index = plain_java_call_shape_inventory();
    let owner = PLAIN_OWNER.replace(
        "public class PlainOwner {",
        "public class PlainOwner {\n    public PlainOwner(String value) {}\n    public void construct() { new PlainOwner(); }",
    );
    let tree = ktlsp::java::JavaParser::new().parse(&owner);
    index.index.replace_file(
        "PlainOwner.java",
        ktlsp::java::extract_symbols(&tree, &owner),
        Tier::Volatile,
    );
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.project_packages_complete.insert("app".into());
    let diagnostics = ktlsp::java::diagnostics(&index, "PlainOwner.java", &tree, &owner, &facts);
    let call = "new PlainOwner()";
    let start = owner.find(call).unwrap();
    assert!(
        diagnostics.iter().any(|item| {
            item.code == Some(DiagnosticCode::CallShapeMismatch)
                && item.start_byte == start
                && item.end_byte == start + call.len()
        }),
        "closed-world constructor control: {diagnostics:?}"
    );
    facts.compiler_producers_complete = false;
    let diagnostics = ktlsp::java::diagnostics(&index, "PlainOwner.java", &tree, &owner, &facts);
    assert!(
        diagnostics
            .iter()
            .all(|item| item.code != Some(DiagnosticCode::CallShapeMismatch)),
        "{diagnostics:?}"
    );
}

#[test]
fn plain_java_owner_call_shape_does_not_require_unrelated_library_source_closure() {
    let index = plain_java_call_shape_inventory();
    let mut facts = ktlsp::resolve::CompletenessFacts::complete();
    facts.compiler_producers_complete = false;
    facts.library_index_complete = false;
    facts.project_packages_complete.insert("app".into());
    let caller = PLAIN_USE
        .replace("accept(\"Ada\")", "accept()")
        .replace("accept(1, 2)", "accept(true)");
    let tree = ktlsp::java::JavaParser::new().parse(&caller);
    let expected = ["value.accept()", "value.accept(true)"].map(|call| {
        let start = caller.find(call).unwrap();
        (
            start,
            start + call.len(),
            Some(DiagnosticCode::CallShapeMismatch),
        )
    });
    let diagnostics = ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, &caller, &facts);
    let actual = diagnostics
        .iter()
        .map(|item| (item.start_byte, item.end_byte, item.code))
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "an unrelated binary-only dependency cannot add methods to this certified owner: {diagnostics:?}");
    // Relevant owner-world and SDK uncertainty still block the same negative proof.
    facts.project_scan_complete = false;
    facts.project_packages_complete.clear();
    assert!(ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, &caller, &facts).is_empty());
    facts.project_scan_complete = true;
    facts.project_packages_complete.insert("app".into());
    facts.jdk_index_complete = false;
    assert!(ktlsp::java::diagnostics(&index, "PlainUse.java", &tree, &caller, &facts).is_empty());
}
