//! Kotlin/JVM facade controls independently checked with Kotlin 1.9.23 and javac --release 17.
use ktlsp::edit::apply_to_text;
use ktlsp::index::InferenceIndex;
use ktlsp::symbol::Def;
use ktlsp::workspace::Workspace;
use std::collections::BTreeSet;

const API: &str = "/jvm/interop/KotlinExtensions.kt";
const KT: &str = "/jvm/interop/KotlinUse.kt";
const JAVA: &str = "/jvm/interop/JavaUse.java";
const SOURCE: &str = r#"@file:JvmName("KotlinExtensions")
package interop

class Payload {
    fun label(): String = "payload"
}
class Call<T>(val value: T) {
    fun regular(): String = "ordinary"
}
@JvmName("awaitNullable")
suspend fun <T : Any> Call<T?>.await(): T? = value
"#;
const KOTLIN_USE: &str = "package interop\nsuspend fun run(call: Call<Payload?>): Payload? { call.regular(); return call.await() }\n";
const JAVA_USE: &str = "package interop; import kotlin.coroutines.Continuation; class JavaUse { Object run(Call<Payload> call, Continuation<? super Payload> continuation) { call.regular(); return KotlinExtensions.awaitNullable(call, continuation); } }";
const DECOY: &str = "package interop; class Decoy { static Object awaitNullable(Call<Payload> call, kotlin.coroutines.Continuation<? super Payload> c) { return null; } }";

fn workspace(decoy: bool) -> Workspace {
    let mut ws = Workspace::new();
    ws.open(API, SOURCE.into());
    ws.open(KT, KOTLIN_USE.into());
    ws.open(JAVA, JAVA_USE.into());
    if decoy {
        ws.open("/jvm/interop/Decoy.java", DECOY.into());
    }
    ws
}
fn definition(name: &str) -> Def {
    let start = if name == "await" {
        SOURCE.find(".await()").unwrap() + 1
    } else {
        SOURCE.find(name).unwrap()
    };
    Def {
        file: API.into(),
        start_byte: start,
        end_byte: start + name.len(),
    }
}
fn offset(text: &str, owner: &str, name: &str) -> usize {
    text.find(&format!("{owner}.{name}")).unwrap() + owner.len() + 1
}
fn goto(ws: &mut Workspace, file: &str, owner: &str, name: &str) -> Vec<Def> {
    let text = ws.doc_text(file).unwrap();
    ws.goto_definition(file, offset(&text, owner, name))
}
fn labels(ws: &mut Workspace, file: &str, owner: &str, prefix: &str) -> BTreeSet<String> {
    let text = ws.doc_text(file).unwrap();
    ws.complete(file, offset(&text, owner, prefix) + prefix.len(), true)
        .map(|c| c.items.into_iter().map(|i| i.label).collect())
        .unwrap_or_default()
}
#[test]
fn ordinary_java_member_control() {
    let mut ws = workspace(false);
    assert!(labels(&mut ws, JAVA, "call", "regular").contains("regular"));
    assert_eq!(
        goto(&mut ws, JAVA, "call", "regular"),
        vec![definition("regular")]
    );
}
#[test]
fn ordinary_kotlin_member_control() {
    let mut ws = workspace(false);
    assert!(labels(&mut ws, KT, "call", "regular").contains("regular"));
    assert_eq!(
        goto(&mut ws, KT, "call", "regular"),
        vec![definition("regular")]
    );
}
#[test]
fn canonical_kotlin_extension_control() {
    let mut ws = workspace(false);
    assert!(labels(&mut ws, KT, "call", "await").contains("await"));
    assert_eq!(
        goto(&mut ws, KT, "call", "await"),
        vec![definition("await")]
    );
}
#[test]
fn java_fixed_jvm_name_has_useful_completion_and_exact_goto() {
    let mut ws = workspace(false);
    assert!(labels(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable"),
        vec![definition("await")]
    );
}
#[test]
fn java_fixed_jvm_name_excludes_unrelated_owner() {
    let mut ws = workspace(true);
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable"),
        vec![definition("await")]
    );
}
#[test]
fn authored_kotlin_name_is_not_exported_to_java() {
    let mut ws = workspace(true);
    let text = JAVA_USE.replace("KotlinExtensions.awaitNullable", "KotlinExtensions.await");
    ws.change(JAVA, text);
    assert!(!labels(&mut ws, JAVA, "KotlinExtensions", "await").contains("await"));
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "await").is_empty());
}
#[test]
fn unknown_qualified_owner_never_borrows_visible_method() {
    let mut ws = workspace(true);
    ws.change(JAVA, JAVA_USE.replace("KotlinExtensions", "MissingFacade"));
    assert!(labels(&mut ws, JAVA, "MissingFacade", "awaitNullable").is_empty());
    assert!(goto(&mut ws, JAVA, "MissingFacade", "awaitNullable").is_empty());
}
#[test]
fn jvm_alias_is_not_kotlin_extension_name() {
    let mut ws = workspace(true);
    ws.change(
        KT,
        KOTLIN_USE.replace("call.await()", "call.awaitNullable()"),
    );
    assert!(!labels(&mut ws, KT, "call", "awaitNullable").contains("awaitNullable"));
    assert!(goto(&mut ws, KT, "call", "awaitNullable").is_empty());
}
#[test]
fn authored_rename_preserves_fixed_jvm_alias_and_java_buffer() {
    let mut ws = workspace(true);
    let target = definition("await");
    let edits = ws.rename(API, target.start_byte, "awaitPayload").unwrap();
    let changed = apply_to_text(API, SOURCE, &edits).unwrap();
    assert!(changed.contains("Call<T?>.awaitPayload()"));
    assert!(changed.contains("@JvmName(\"awaitNullable\")"));
    assert_eq!(apply_to_text(JAVA, JAVA_USE, &edits).unwrap(), JAVA_USE);
    assert!(apply_to_text(KT, KOTLIN_USE, &edits)
        .unwrap()
        .contains("call.awaitPayload()"));
}
#[test]
fn exported_alias_rename_edits_validated_literal_and_java_uses_only() {
    let mut ws = workspace(true);
    let at = offset(JAVA_USE, "KotlinExtensions", "awaitNullable");
    let edits = ws
        .rename(JAVA, at, "awaitJava")
        .expect("editable source-backed JVM alias");
    let changed = apply_to_text(API, SOURCE, &edits).unwrap();
    assert!(changed.contains("@JvmName(\"awaitJava\")"));
    assert!(changed.contains("Call<T?>.await()"));
    assert_eq!(apply_to_text(KT, KOTLIN_USE, &edits).unwrap(), KOTLIN_USE);
    assert!(apply_to_text(JAVA, JAVA_USE, &edits)
        .unwrap()
        .contains("KotlinExtensions.awaitJava("));
    assert_eq!(
        apply_to_text("/jvm/interop/Decoy.java", DECOY, &edits).unwrap(),
        DECOY
    );
}
#[test]
fn exported_and_authored_references_include_both_views_without_decoy() {
    let mut ws = workspace(true);
    let refs = ws.references(
        JAVA,
        offset(JAVA_USE, "KotlinExtensions", "awaitNullable"),
        true,
    );
    assert!(refs.contains(&definition("await")), "{refs:?}");
    assert!(
        refs.iter()
            .any(|r| r.file == JAVA && &JAVA_USE[r.start_byte..r.end_byte] == "awaitNullable"),
        "{refs:?}"
    );
    assert!(
        refs.iter()
            .any(|r| r.file == KT && &KOTLIN_USE[r.start_byte..r.end_byte] == "await"),
        "{refs:?}"
    );
    assert!(
        !refs.iter().any(|r| r.file.ends_with("Decoy.java")),
        "{refs:?}"
    );
}

#[test]
fn dirty_annotation_change_replaces_alias_and_identity() {
    let mut ws = workspace(true);
    let before = ws.index.scoped_for_file(JAVA);
    let old_source = before.jvm_sources("KotlinExtensions")[0];
    let old_id = ktcore::jvm::CallableCandidate::Jvm {
        owner: ktcore::jvm::JvmOwner::Source(old_source),
        facts: &old_source.facts.callables[0],
    }
    .identity(&before)
    .unwrap();
    ws.change(API, SOURCE.replace("awaitNullable", "awaitUpdated"));
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
    assert!(!labels(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    ws.change(JAVA, JAVA_USE.replace("awaitNullable", "awaitUpdated"));
    assert!(labels(&mut ws, JAVA, "KotlinExtensions", "awaitUpdated").contains("awaitUpdated"));
    let current_source = SOURCE.replace("awaitNullable", "awaitUpdated");
    let at = current_source.find(".await()").unwrap() + 1;
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitUpdated"),
        vec![Def {
            file: API.into(),
            start_byte: at,
            end_byte: at + 5
        }]
    );
    let view = ws.index.scoped_for_file(JAVA);
    let source = view.jvm_sources("KotlinExtensions")[0];
    let new_id = ktcore::jvm::CallableCandidate::Jvm {
        owner: ktcore::jvm::JvmOwner::Source(source),
        facts: &source.facts.callables[0],
    }
    .identity(&view)
    .unwrap();
    assert_ne!(old_id.class.input, new_id.class.input);
}
#[test]
fn dirty_facade_change_removes_old_owner_without_global_fallback() {
    let mut ws = workspace(true);
    ws.change(
        API,
        SOURCE.replace(
            "@file:JvmName(\"KotlinExtensions\")",
            "@file:JvmName(\"ChangedFacade\")",
        ),
    );
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
    ws.change(JAVA, JAVA_USE.replace("KotlinExtensions", "ChangedFacade"));
    assert!(labels(&mut ws, JAVA, "ChangedFacade", "awaitNullable").contains("awaitNullable"));
}
#[test]
fn malformed_or_escaped_alias_never_reuses_previous_projection() {
    let mut ws = workspace(true);
    for value in [
        "@JvmName(\"await\\u004eullable\")",
        "@JvmName(\"awaitNullable\", \"other\")",
        "@JvmName(\"awaitNullable",
    ] {
        ws.change(API, SOURCE.replace("@JvmName(\"awaitNullable\")", value));
        assert!(
            goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty(),
            "{value}"
        );
    }
}
#[test]
fn incomplete_java_selectors_keep_useful_lowered_callable() {
    let mut ws = workspace(true);
    for selector in ["awaitN", ""] {
        let text = JAVA_USE.replace("awaitNullable(call, continuation)", selector);
        ws.change(JAVA, text);
        assert!(
            labels(&mut ws, JAVA, "KotlinExtensions", selector).contains("awaitNullable"),
            "{selector}"
        );
    }
}
#[test]
fn java_signature_and_descriptor_use_verified_suspend_lowering() {
    use ktlsp::index::InferenceIndex;
    let mut ws = workspace(false);
    let signature = ws
        .signature_help(JAVA, JAVA_USE.find("call, continuation").unwrap() + 6)
        .unwrap();
    assert_eq!(signature.signatures.len(), 1, "{signature:?}");
    let signature = &signature.signatures[0];
    assert_eq!(signature.parameters.len(), 2);
    assert!(
        signature.label.starts_with("awaitNullable("),
        "{signature:?}"
    );
    assert!(signature.label.ends_with(": Object"), "{signature:?}");
    let view = ws.index.scoped_for_file(JAVA);
    let source = view.jvm_sources("KotlinExtensions")[0];
    let candidate = ktcore::jvm::CallableCandidate::Jvm {
        owner: ktcore::jvm::JvmOwner::Source(source),
        facts: &source.facts.callables[0],
    };
    assert_eq!(
        candidate.identity(&view).unwrap().descriptor,
        "(Linterop/Call;Lkotlin/coroutines/Continuation;)Ljava/lang/Object;"
    );
    assert_eq!(
        ktcore::jvm::projected_return_type(candidate)
            .unwrap()
            .package(),
        Some("java.lang")
    );
    assert_eq!(
        ktcore::jvm::projected_return_type(candidate)
            .unwrap()
            .name(),
        Some("Object")
    );
}
#[test]
fn jvm_projection_is_not_a_fake_type_or_mutated_kotlin_symbol() {
    let ws = workspace(false);
    assert!(ws.index.lookup_type("KotlinExtensions").is_empty());
    assert!(ws.index.lookup_by_name("awaitNullable").is_empty());
    let authored = &ws.index.lookup_by_name("await")[0].sym;
    assert_eq!(authored.arity, Some(0));
    assert_eq!(authored.ext_receiver.as_deref(), Some("Call"));
    assert_eq!(authored.return_type.as_ref().unwrap().name, "T");
    assert!(authored.return_type.as_ref().unwrap().nullable);
}
#[test]
fn applied_java_export_rename_can_start_on_annotation_literal() {
    let mut ws = workspace(true);
    let edits = ws
        .rename(API, SOURCE.find("awaitNullable").unwrap() + 2, "javaAwait")
        .unwrap();
    assert!(apply_to_text(API, SOURCE, &edits)
        .unwrap()
        .contains("@JvmName(\"javaAwait\")"));
    assert!(apply_to_text(JAVA, JAVA_USE, &edits)
        .unwrap()
        .contains("KotlinExtensions.javaAwait("));
    assert_eq!(apply_to_text(KT, KOTLIN_USE, &edits).unwrap(), KOTLIN_USE);
}

#[test]
fn shadowed_jvm_name_annotation_does_not_create_false_alias() {
    let mut ws = workspace(true);
    ws.open(
        "/jvm/interop/JvmName.kt",
        "package interop\nannotation class JvmName(val name: String) {}\n".into(),
    );
    ws.change(
        API,
        SOURCE.replace("@file:JvmName", "@file:kotlin.jvm.JvmName"),
    );
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
    assert!(!labels(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    let at = ws.doc_text(API).unwrap().find("awaitNullable").unwrap();
    assert!(ws.rename(API, at, "changed").is_none());
}
#[test]
fn explicitly_imported_other_annotation_is_not_builtin_jvm_name() {
    let mut ws = workspace(true);
    let source = SOURCE
        .replace("package interop", "package interop\nimport other.JvmName")
        .replace("@file:JvmName", "@file:kotlin.jvm.JvmName");
    ws.change(API, source);
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
}
#[test]
fn inferred_suspend_return_is_not_fabricated_as_unit() {
    let mut ws = workspace(false);
    ws.change(
        API,
        SOURCE.replace("Call<T?>.await(): T? = value", "Call<T?>.await() = 1"),
    );
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
}
#[test]
fn imported_facade_has_the_same_selected_projection() {
    let mut ws = workspace(true);
    let source = JAVA_USE.replace("package interop;", "package app; import interop.Call; import interop.Payload; import interop.KotlinExtensions;");
    ws.change(JAVA, source);
    assert!(labels(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable"),
        vec![definition("await")]
    );
}
#[test]
fn two_modules_with_same_facade_never_share_identity_refs_or_rename() {
    use ktlsp::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
    fn origin(id: &str, module: &str) -> FileOrigin {
        FileOrigin {
            origin_id: id.into(),
            representation: SourceRepresentation::Source,
            project_scope: Some(ProjectScope {
                module: module.into(),
                source_set: "main".into(),
            }),
        }
    }
    let mut ws = workspace(false);
    let old_api = "/jvm/old/KotlinExtensions.kt";
    let other_java = "/jvm/other/JavaUse.java";
    let old_source = format!("// other version\n{SOURCE}");
    ws.open(old_api, old_source.clone());
    ws.open(other_java, JAVA_USE.into());
    for (file, id, module) in [
        (API, "new-api", "api"),
        (old_api, "old-api", "old"),
        (JAVA, "a", "a"),
        (KT, "a", "a"),
        (other_java, "b", "b"),
    ] {
        ws.index.set_file_origin(file, origin(id, module));
    }
    ws.index.replace_scope_visibilities(
        [
            ("api", vec!["new-api"]),
            ("old", vec!["old-api"]),
            ("a", vec!["a", "new-api", "old-api"]),
            ("b", vec!["b", "old-api", "new-api"]),
        ]
        .into_iter()
        .map(|(module, origins)| ScopeVisibility {
            scope: origin("", module).project_scope.unwrap(),
            ordered_origins: origins.into_iter().map(str::to_string).collect(),
        })
        .collect(),
    );
    ws.bump_index_revision();
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable"),
        vec![definition("await")]
    );
    let start = old_source.find(".await()").unwrap() + 1;
    assert_eq!(
        goto(&mut ws, other_java, "KotlinExtensions", "awaitNullable"),
        vec![Def {
            file: old_api.into(),
            start_byte: start,
            end_byte: start + 5
        }]
    );
    let refs = ws.references(
        JAVA,
        offset(JAVA_USE, "KotlinExtensions", "awaitNullable"),
        true,
    );
    assert!(
        !refs
            .iter()
            .any(|r| r.file == other_java || r.file == old_api),
        "{refs:?}"
    );
    let edits = ws
        .rename(
            JAVA,
            offset(JAVA_USE, "KotlinExtensions", "awaitNullable"),
            "onlyA",
        )
        .unwrap();
    assert!(apply_to_text(JAVA, JAVA_USE, &edits)
        .unwrap()
        .contains("KotlinExtensions.onlyA("));
    assert_eq!(
        apply_to_text(other_java, JAVA_USE, &edits).unwrap(),
        JAVA_USE
    );
    assert_eq!(
        apply_to_text(old_api, &old_source, &edits).unwrap(),
        old_source
    );
}
#[test]
fn accepted_projection_snapshot_shares_facts_and_survives_replacement() {
    let mut ws = workspace(false);
    let snapshot = ws.index.inference_snapshot();
    assert!(std::ptr::eq(
        ws.index.jvm_sources("KotlinExtensions")[0],
        snapshot.jvm_sources("KotlinExtensions")[0]
    ));
    let previous_revision = snapshot.jvm_sources("KotlinExtensions")[0].revision;
    ws.change(API, SOURCE.replace("awaitNullable", "updatedAwait"));
    assert_eq!(
        snapshot.jvm_sources("KotlinExtensions")[0].facts.callables[0].name,
        "awaitNullable"
    );
    let current = ws.index.jvm_sources("KotlinExtensions")[0];
    assert_ne!(previous_revision, current.revision);
    assert_eq!(current.facts.callables[0].name, "updatedAwait");
    ws.index.remove_file(API);
    assert!(ws.index.jvm_sources("KotlinExtensions").is_empty());
    assert_eq!(snapshot.jvm_sources("KotlinExtensions").len(), 1);
}

#[test]
fn unresolved_erasure_does_not_enable_alias_rename() {
    let mut ws = workspace(true);
    ws.change(API, SOURCE.replace("Call<T?>.await", "Missing<T?>.await"));
    let view = ws.index.scoped_for_file(JAVA);
    let source = view.jvm_sources("KotlinExtensions")[0];
    let candidate = ktcore::jvm::CallableCandidate::Jvm {
        owner: ktcore::jvm::JvmOwner::Source(source),
        facts: &source.facts.callables[0],
    };
    assert!(candidate.identity(&view).is_none());
    assert!(ws
        .rename(
            JAVA,
            offset(JAVA_USE, "KotlinExtensions", "awaitNullable"),
            "unknownAlias"
        )
        .is_none());
}

#[test]
fn serialized_dependency_facts_preserve_projection_and_open_source_priority() {
    use ktlsp::language::{LanguageParsers, SourceLanguage};
    let mut parsers = LanguageParsers::new();
    let tree = parsers.parse(SourceLanguage::Kotlin, SOURCE);
    let facts = ktlsp::language::file_facts(SourceLanguage::Kotlin, &tree, SOURCE);
    let bytes = bincode::serialize(&ktlsp::deps::FileSymbols {
        file: API.into(),
        symbols: facts.symbols,
        jvm: facts.jvm,
        library_source: None,
        origin: None,
    })
    .unwrap();
    let batch = || bincode::deserialize::<ktlsp::deps::FileSymbols>(&bytes).unwrap();
    let mut ws = Workspace::new();
    assert!(ws.index_dependency_batch(batch()));
    ws.open(JAVA, JAVA_USE.into());
    assert!(labels(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    assert_eq!(
        goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable"),
        vec![definition("await")]
    );
    assert!(ws
        .rename(
            JAVA,
            offset(JAVA_USE, "KotlinExtensions", "awaitNullable"),
            "readonlyAlias"
        )
        .is_none());
    ws.open(API, SOURCE.replace("awaitNullable", "dirtyAlias"));
    assert!(!ws.index_dependency_batch(batch()));
    assert!(goto(&mut ws, JAVA, "KotlinExtensions", "awaitNullable").is_empty());
    ws.change(JAVA, JAVA_USE.replace("awaitNullable", "dirtyAlias"));
    assert!(labels(&mut ws, JAVA, "KotlinExtensions", "dirtyAlias").contains("dirtyAlias"));
}

#[test]
fn closed_project_scan_publishes_and_retracts_facade_without_opening_it() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("ktlsp-jvm-scan-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let api_path = root.join("KotlinExtensions.kt");
    let java_path = root.join("JavaUse.java");
    std::fs::write(&api_path, SOURCE).unwrap();
    std::fs::write(&java_path, JAVA_USE).unwrap();
    let api = ktlsp::paths::file_key(&api_path);
    let java = ktlsp::paths::file_key(&java_path);
    let mut ws = Workspace::new();
    ws.open(&java, JAVA_USE.into());
    assert_eq!(ws.scan(&root), 1);
    assert!(!ws.open_doc_keys().contains(&api));
    assert!(labels(&mut ws, &java, "KotlinExtensions", "awaitNullable").contains("awaitNullable"));
    let mut expected = definition("await");
    expected.file = api.clone();
    assert_eq!(
        goto(&mut ws, &java, "KotlinExtensions", "awaitNullable"),
        vec![expected]
    );
    std::fs::remove_file(&api_path).unwrap();
    assert_eq!(ws.scan(&root), 0);
    assert!(ws.index.jvm_sources("KotlinExtensions").is_empty());
    assert!(goto(&mut ws, &java, "KotlinExtensions", "awaitNullable").is_empty());
    assert!(labels(&mut ws, &java, "KotlinExtensions", "awaitNullable").is_empty());
    let _ = std::fs::remove_dir_all(root);
}
