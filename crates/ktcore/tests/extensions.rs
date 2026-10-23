//! Public-API tests of the pure contribution foundation, not an installed Mixin provider or LSP.
//! Signatures/modifiers below are host facts established separately by compiler/runtime controls.

use std::ops::Range;
use std::sync::Arc;

use ktcore::diagnostics::{Diagnostic, Severity};
use ktcore::extensions::*;
use ktcore::index::{Index, InferenceIndex, Tier};
use ktcore::indexer::extract_symbols;
use ktcore::knowledge::Knowledge;
use ktcore::parser::KotlinParser;
use ktcore::project_model::{FileOrigin, ProjectScope, ScopeVisibility, SourceRepresentation};
use ktcore::symbol::{Def, IndexedSymbol, SymbolKind};

const TARGET_FILE: &str = "/oracle/Target.kt";
const MIXIN_FILE: &str = "/oracle/CaseMixin.kt";
const TARGET: &str = r#"package oracle
class Target {
    private var secret: Int = 41
    private val label: String = "fortyone"
    fun twice(value: Int): Int = value * 2
    fun twice(value: String): String = value + value
    fun numbers(values: IntArray): Int = values.size
}
"#;
const MIXIN: &str = r#"package oracle.mixin
import oracle.Target
import org.spongepowered.asm.mixin.Mixin
import org.spongepowered.asm.mixin.Shadow
import org.spongepowered.asm.mixin.injection.At
import org.spongepowered.asm.mixin.injection.Inject
import org.spongepowered.asm.mixin.injection.callback.CallbackInfoReturnable
@Mixin(value = [Target::class], remap = false)
abstract class CaseMixin {
    @Shadow(prefix = "shadow$") abstract fun `shadow$twice`(value: Int): Int
    @field:Shadow private lateinit var label: String
    @Inject(method = ["twice(I)I"], at = [At("HEAD")], cancellable = true)
    private fun `oracle$probe`(cir: CallbackInfoReturnable<Int>) {
        cir.returnValue = `shadow$twice`(4) + label.length
    }
}
"#;

fn index_source(index: &mut Index, file: &str, text: &str, package: &str) -> Vec<IndexedSymbol> {
    let mut parser = KotlinParser::new();
    let tree = parser.parse(text);
    assert!(!tree.root_node().has_error(), "{text}");
    let symbols = extract_symbols(&tree, text, package);
    index.replace_file(file, symbols.clone(), Tier::Volatile);
    symbols
}

fn method(parameters: &[JvmType], returns: &JvmType) -> MemberSignature {
    MemberSignature::method(parameters, returns).unwrap()
}

fn def(file: &str, symbol: &IndexedSymbol) -> Def {
    Def {
        file: file.into(),
        start_byte: symbol.start_byte,
        end_byte: symbol.end_byte,
    }
}

fn facts(file: &str, symbols: &[IndexedSymbol]) -> Vec<DeclaredMemberFacts> {
    symbols
        .iter()
        .filter_map(|symbol| {
            let (signature, visibility) = match (symbol.kind, symbol.name.as_str()) {
                (SymbolKind::Function, "twice")
                    if symbol.params.first().is_some_and(|ty| ty.name == "Int") =>
                {
                    (method(&[JvmType::Int], &JvmType::Int), Visibility::Public)
                }
                (SymbolKind::Function, "twice") => (
                    method(&[JvmType::String], &JvmType::String),
                    Visibility::Public,
                ),
                (SymbolKind::Function, "numbers") => (
                    method(&[JvmType::Array(Box::new(JvmType::Int))], &JvmType::Int),
                    Visibility::Public,
                ),
                (SymbolKind::Property, "secret") => (
                    MemberSignature::field(&JvmType::Int).unwrap(),
                    Visibility::Private,
                ),
                (SymbolKind::Property, "label") => (
                    MemberSignature::field(&JvmType::String).unwrap(),
                    Visibility::Private,
                ),
                _ => return None,
            };
            Some(DeclaredMemberFacts {
                definition: def(file, symbol),
                signature: Knowledge::Found(signature),
                dispatch: Knowledge::Found(Dispatch::Instance),
                visibility: Knowledge::Found(visibility),
                synthetic: Knowledge::Found(false),
            })
        })
        .collect()
}

fn fixture() -> (Index, ExtensionStore) {
    let mut index = Index::new();
    let symbols = index_source(&mut index, TARGET_FILE, TARGET, "oracle");
    index_source(&mut index, MIXIN_FILE, MIXIN, "oracle.mixin");
    let mut store = ExtensionStore::default();
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Found(()),
            facts(TARGET_FILE, &symbols),
        )
        .unwrap();
    store
        .replace_source(
            &index,
            MIXIN_FILE,
            Arc::from(MIXIN),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    store.configure_provider("mixin-consumer", true, Arc::from("{}"));
    store.set_namespace_coverage(Knowledge::Found(()));
    (index, store)
}

fn occurrence(text: &str, name: &str, occurrence: usize) -> Range<usize> {
    let start = text.match_indices(name).nth(occurrence).unwrap().0;
    start..start + name.len()
}

fn requirement(
    read: &mut ReadSession<'_>,
    name: &str,
    signature: Fact<MemberSignature>,
    dispatch: Dispatch,
) -> MemberRequirement {
    MemberRequirement {
        target: read.resolve_type("oracle.Target"),
        authored: None,
        names: MemberNames::Exact(vec![name.into()]),
        kind: MemberKind::Method,
        signature,
        dispatch: Knowledge::Found(dispatch),
        name_link: NameLink::Direct,
    }
}

#[test]
fn normalized_source_rejects_foreign_owners_and_invalid_nested_ranges_atomically() {
    use ktcore::extensions::source::{
        AnnotationArgument, AnnotationValue, SourceAnnotation, SourceDeclaration,
    };
    let (index, mut store) = fixture();
    let entries = index.entries_for_file(MIXIN_FILE);
    let method = entries
        .iter()
        .find(|entry| entry.sym.name.contains("shadow$twice"))
        .unwrap();
    let owner = entries
        .iter()
        .find(|entry| entry.sym.name == "CaseMixin")
        .unwrap();
    let quoted = occurrence(MIXIN, "\"shadow$\"", 0);
    let annotation = occurrence(MIXIN, "@Shadow(prefix = \"shadow$\")", 0);
    let valid = SourceDeclaration {
        definition: def(MIXIN_FILE, &method.sym),
        owner: Some(def(MIXIN_FILE, &owner.sym)),
        member: None,
        jvm_property: None,
        annotations: vec![SourceAnnotation {
            ty: ktcore::types::TypeRef::simple("Shadow"),
            use_site_target: None,
            range: annotation,
            arguments: vec![AnnotationArgument {
                name: Some("prefix".into()),
                range: quoted.clone(),
                value: Knowledge::Found(AnnotationValue::String {
                    value: "shadow$".into(),
                    content_range: quoted.start + 1..quoted.end - 1,
                }),
            }],
        }],
    };
    store
        .replace_source_declarations(
            &index,
            MIXIN_FILE,
            Arc::from(MIXIN),
            Knowledge::Found(()),
            vec![valid.clone()],
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert_eq!(read.declarations(), &[valid.clone()]);
    let request = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method_signature()),
        Dispatch::Instance,
    );
    read.require(request).unwrap();
    let pending = read.finish();
    let mut invalid = Vec::new();
    let mut foreign = valid.clone();
    foreign.owner = Some(Def {
        file: TARGET_FILE.into(),
        start_byte: 0,
        end_byte: 1,
    });
    invalid.push(foreign);
    let mut outside = valid.clone();
    outside.annotations[0].range = 0..MIXIN.len() + 1;
    invalid.push(outside);
    let mut escaped = valid.clone();
    escaped.annotations[0].arguments[0].range = 0..1;
    invalid.push(escaped);
    let mut wrong_text = valid.clone();
    if let Knowledge::Found(AnnotationValue::String { value, .. }) =
        &mut wrong_text.annotations[0].arguments[0].value
    {
        *value = "wrong".into();
    }
    invalid.push(wrong_text);
    for invalid in invalid {
        assert!(store
            .replace_source_declarations(
                &index,
                MIXIN_FILE,
                Arc::from(MIXIN),
                Knowledge::Found(()),
                vec![invalid]
            )
            .is_err());
        assert_eq!(
            store.source_declarations(MIXIN_FILE).unwrap(),
            &[valid.clone()]
        );
    }
    store.commit(pending).unwrap();
}

fn method_signature() -> MemberSignature {
    method(&[JvmType::Int], &JvmType::Int)
}

#[test]
fn exact_signature_static_and_owner_are_required_for_a_binding() {
    let (index, mut store) = fixture();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let exact = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(exact.clone()).unwrap();
    let resolution = read.resolution(id).unwrap();
    let Knowledge::Found(binding) = &resolution.binding else {
        panic!("{resolution:?}")
    };
    assert_eq!(binding.target.definition().file, TARGET_FILE);
    assert_eq!(
        &TARGET[binding.target.definition().start_byte..binding.target.definition().end_byte],
        "twice"
    );
    assert_eq!(binding.target.signature().unwrap().descriptor(), "(I)I");
    assert_eq!(resolution.candidates.len(), 2);
    assert_eq!(
        resolution
            .candidates
            .iter()
            .filter(|candidate| !candidate.mismatches.is_empty())
            .count(),
        1
    );
    let mut wrong_signature = exact.clone();
    wrong_signature.signature = Knowledge::Found(method(&[JvmType::Long], &JvmType::Int));
    let id = read.require(wrong_signature).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_definitely_absent());
    assert!(read
        .resolution(id)
        .unwrap()
        .candidates
        .iter()
        .all(|candidate| candidate.mismatches.contains(&MemberMismatch::Signature)));
    let mut wrong_static = exact;
    wrong_static.dispatch = Knowledge::Found(Dispatch::Static);
    let id = read.require(wrong_static).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_definitely_absent());
    assert!(read
        .resolution(id)
        .unwrap()
        .candidates
        .iter()
        .all(|candidate| candidate.mismatches.contains(&MemberMismatch::Dispatch)));
}

#[test]
fn incomplete_selectors_keep_candidates_and_unknown_instead_of_a_false_binding() {
    let (index, mut store) = fixture();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut partial = requirement(
        &mut read,
        "twice",
        Knowledge::Unknown(vec![IncompleteReason::IncompleteSelector]),
        Dispatch::Instance,
    );
    partial.names = MemberNames::Prefix("twi".into());
    let id = read.require(partial).unwrap();
    let result = read.resolution(id).unwrap();
    assert!(matches!(result.binding, Knowledge::Unknown(_)));
    assert_eq!(result.candidates.len(), 2);
    assert_eq!(
        result
            .candidates
            .iter()
            .filter_map(|candidate| candidate.id.signature())
            .map(MemberSignature::descriptor)
            .collect::<Vec<_>>(),
        vec!["(I)I", "(Ljava/lang/String;)Ljava/lang/String;"]
    );
}

#[test]
fn missing_metadata_and_generated_target_coverage_stay_unknown() {
    let (index, mut store) = fixture();
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    let result = read.resolution(id).unwrap();
    assert!(matches!(result.binding, Knowledge::Unknown(_)));
    assert!(result.candidates.iter().all(|candidate| matches!(
        candidate.signature,
        Knowledge::Unknown(_)
    ) && matches!(
        candidate.dispatch,
        Knowledge::Unknown(_)
    )));
    let mut synthetic = requirement(
        &mut read,
        "this$0",
        Knowledge::Found(MemberSignature::field(&JvmType::String).unwrap()),
        Dispatch::Instance,
    );
    synthetic.kind = MemberKind::Field;
    let id = read.require(synthetic).unwrap();
    assert!(matches!(
        read.resolution(id).unwrap().binding,
        Knowledge::Unknown(_)
    ));
}

#[test]
fn known_source_member_binding_does_not_certify_complete_generated_member_coverage() {
    let (index, mut store) = fixture();
    let symbols: Vec<_> = index
        .entries_for_file(TARGET_FILE)
        .iter()
        .map(|entry| (*entry.sym).clone())
        .collect();
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
            facts(TARGET_FILE, &symbols),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let target = read.resolve_type("oracle.Target");
    let Knowledge::Found(owner) = target else {
        panic!("actual source owner")
    };
    let (_, coverage) = read.declared_members(&owner).unwrap();
    assert!(matches!(coverage, Knowledge::Unknown(_)));
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    let Knowledge::Found(binding) = &read.resolution(id).unwrap().binding else {
        panic!("exact authored JVM member must remain usable")
    };
    assert_eq!(binding.target.definition().file, TARGET_FILE);
    assert_eq!(binding.target.signature().unwrap().descriptor(), "(I)I");
    let candidate = read
        .resolution(id)
        .unwrap()
        .candidates
        .iter()
        .find(|candidate| candidate.id == binding.target)
        .unwrap();
    assert_eq!(candidate.visibility, Knowledge::Found(Visibility::Public));
    assert_eq!(candidate.synthetic, Knowledge::Found(false));
}

#[test]
fn unknown_competing_signature_still_prevents_an_exact_binding() {
    let (index, mut store) = fixture();
    let symbols: Vec<_> = index
        .entries_for_file(TARGET_FILE)
        .iter()
        .map(|entry| (*entry.sym).clone())
        .collect();
    let mut members = facts(TARGET_FILE, &symbols);
    members.iter_mut().find(|fact| matches!(&fact.signature, Knowledge::Found(signature) if signature.descriptor() == "(Ljava/lang/String;)Ljava/lang/String;"))
        .unwrap().signature = Knowledge::Unknown(vec![IncompleteReason::UnsupportedType("unresolved overload".into())]);
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
            members,
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    assert!(matches!(
        read.resolution(id).unwrap().binding,
        Knowledge::Unknown(_)
    ));
}

#[test]
fn incomplete_owner_cannot_prove_missing_wrong_descriptor_or_wrong_static_absent() {
    let (index, mut store) = fixture();
    let symbols: Vec<_> = index
        .entries_for_file(TARGET_FILE)
        .iter()
        .map(|entry| (*entry.sym).clone())
        .collect();
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Unknown(vec![IncompleteReason::IncompleteTarget]),
            facts(TARGET_FILE, &symbols),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for (name, parameter, dispatch) in [
        ("generatedMissing", JvmType::Int, Dispatch::Instance),
        ("twice", JvmType::Long, Dispatch::Instance),
        ("twice", JvmType::Int, Dispatch::Static),
    ] {
        let req = requirement(
            &mut read,
            name,
            Knowledge::Found(method(&[parameter], &JvmType::Int)),
            dispatch,
        );
        let id = read.require(req).unwrap();
        assert!(
            matches!(read.resolution(id).unwrap().binding, Knowledge::Unknown(_)),
            "{name}"
        );
    }
}

#[test]
fn inherited_members_are_not_declared_target_members() {
    let (mut index, mut store) = fixture();
    let text = "package oracle\nopen class Base {\n fun inherited(value: Int): Int = value\n}\nclass Derived : Base()\n";
    index_source(&mut index, "/oracle/Derived.kt", text, "oracle");
    store
        .replace_source(
            &index,
            "/oracle/Derived.kt",
            Arc::from(text),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "inherited",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.target = read.resolve_type("oracle.Derived");
    let id = read.require(req).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_definitely_absent());
    assert!(read.resolution(id).unwrap().candidates.is_empty());
}

#[test]
fn scoped_lookup_cannot_merge_two_versions_of_one_target() {
    let (mut index, mut store) = fixture();
    let scope = ProjectScope {
        module: "app".into(),
        source_set: "main".into(),
    };
    for (file, origin, project_scope) in [
        (MIXIN_FILE, "app", Some(scope.clone())),
        (TARGET_FILE, "library-v1", None),
        ("/library-v2/Target.kt", "library-v2", None),
    ] {
        index.set_file_origin(
            file,
            FileOrigin {
                origin_id: origin.into(),
                representation: SourceRepresentation::Source,
                project_scope,
            },
        );
    }
    index.replace_scope_visibilities(vec![ScopeVisibility {
        scope,
        ordered_origins: vec!["app".into(), "library-v1".into(), "library-v2".into()],
    }]);
    let v2 = "package oracle\nclass Target {\n fun newOnly(value: Int): Int = value\n}\n";
    index_source(&mut index, "/library-v2/Target.kt", v2, "oracle");
    store
        .replace_source(
            &index,
            "/library-v2/Target.kt",
            Arc::from(v2),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    store.invalidate_model();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "newOnly",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    let result = read.resolution(id).unwrap();
    assert!(result.binding.is_definitely_absent());
    assert_eq!(
        match &result.requirement.target {
            Knowledge::Found(target) => &target.definition().file,
            other => panic!("{other:?}"),
        },
        TARGET_FILE
    );
}

fn prefixed_pending(
    index: &Index,
    store: &mut ExtensionStore,
    provider: &str,
) -> PendingContributions {
    let mut read = store.begin(index, provider, MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.authored = Some(
        read.declaration(occurrence(MIXIN, "`shadow$twice`", 0))
            .unwrap(),
    );
    req.name_link = NameLink::Prefixed("shadow$".into());
    let id = read.require(req).unwrap();
    for occurrence_index in 0..2 {
        let full = occurrence(MIXIN, "`shadow$twice`", occurrence_index);
        read.bind_site(
            id,
            full.clone(),
            full.start + "`shadow$".len()..full.end - 1,
            RenameSubject::Target,
        )
        .unwrap();
    }
    let full = occurrence(MIXIN, "twice(I)I", 0);
    read.bind_site(
        id,
        full.clone(),
        full.start..full.start + "twice".len(),
        RenameSubject::Target,
    )
    .unwrap();
    read.finish()
}

fn apply_edits(source: &str, edits: &[Replacement]) -> String {
    let mut output = source.to_string();
    for edit in edits.iter().rev() {
        output.replace_range(edit.range.clone(), &edit.text);
    }
    output
}

#[test]
fn detached_evaluation_resumes_requirements_and_commits_normally() {
    let (index, mut store) = fixture();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    let pending = read.finish();
    let mut read = store.resume(&index, MIXIN_FILE, pending).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_found());
    let name = occurrence(MIXIN, "twice(I)I", 0);
    read.bind_site(
        id,
        name.clone(),
        name.start..name.start + 5,
        RenameSubject::Target,
    )
    .unwrap();
    store.commit(read.finish()).unwrap();
    let set = store.contributions("mixin-consumer", MIXIN_FILE).unwrap();
    assert_eq!(set.sites().len(), 1);
    assert!(set.binding_at(name.start).is_some());
}

#[test]
fn detached_evaluation_rejects_prior_read_changes_before_another_batch() {
    for change in [
        "source",
        "owner",
        "config",
        "model",
        "namespace",
        "newer-evaluation",
        "foreign-owner",
    ] {
        let (mut index, mut store) = fixture();
        let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
        assert!(read.resolve_type("oracle.Target").is_found());
        let pending = read.finish();
        match change {
            "source" => {
                let text = TARGET.replace("41", "42");
                let symbols = index_source(&mut index, TARGET_FILE, &text, "oracle");
                store
                    .replace_source(
                        &index,
                        TARGET_FILE,
                        Arc::from(text),
                        Knowledge::Found(()),
                        facts(TARGET_FILE, &symbols),
                    )
                    .unwrap();
            }
            "owner" => {
                store
                    .replace_source(
                        &index,
                        MIXIN_FILE,
                        Arc::from(MIXIN),
                        Knowledge::Found(()),
                        Vec::new(),
                    )
                    .unwrap();
            }
            "config" => store.configure_provider("mixin-consumer", true, Arc::from("changed")),
            "model" => store.invalidate_model(),
            "namespace" => store.set_namespace_coverage(Knowledge::Unknown(vec![
                IncompleteReason::IncompleteScope,
            ])),
            "newer-evaluation" => {
                drop(
                    store
                        .begin(&index, "mixin-consumer", MIXIN_FILE)
                        .unwrap()
                        .finish(),
                );
            }
            "foreign-owner" => {}
            _ => unreachable!(),
        }
        let owner = if change == "foreign-owner" {
            TARGET_FILE
        } else {
            MIXIN_FILE
        };
        assert!(store.resume(&index, owner, pending).is_err(), "{change}");
        assert!(store.contributions("mixin-consumer", MIXIN_FILE).is_none());
    }
}

#[test]
fn prefix_and_selector_rename_share_binding_and_edit_only_the_name_bytes() {
    let (index, mut store) = fixture();
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.commit(pending).unwrap();
    let set = store.contributions("mixin-consumer", MIXIN_FILE).unwrap();
    let cursor = occurrence(MIXIN, "`shadow$twice`", 0).start + 2;
    let binding = set.binding_at(cursor).unwrap();
    assert_ne!(
        binding.target.definition(),
        binding.authored.as_ref().unwrap().definition()
    );
    assert_eq!(set.references(&binding.target).len(), 3);
    assert!(set.diagnostics().is_empty());
    let subject = set.rename_subject_at(cursor).unwrap();
    assert_eq!(subject, RenameTarget::Member(binding.target.clone()));
    let edits = set.rename_edits(&subject, "rotate").unwrap();
    assert_eq!(edits.len(), 3);
    assert!(edits
        .iter()
        .all(|edit| &MIXIN[edit.range.clone()] == "twice"));
    let renamed = apply_edits(MIXIN, &edits);
    assert_eq!(renamed.matches("`shadow$rotate`").count(), 2);
    assert!(renamed.contains("\"rotate(I)I\""));
    assert!(renamed.contains("prefix = \"shadow$\""));
    assert_eq!(set.sites()[0].projection.apply("rotate"), "`shadow$rotate`");
    assert_eq!(set.sites()[2].projection.apply("rotate"), "rotate(I)I");
    let mut parser = KotlinParser::new();
    assert!(!parser.parse(&renamed).root_node().has_error());
}

#[test]
fn independent_alias_target_rename_preserves_authored_name_and_its_calls() {
    let (mut index, mut store) = fixture();
    let alias = "package oracle.mixin\nimport org.spongepowered.asm.mixin.Shadow\nabstract class CaseMixin {\n @Shadow(aliases = [\"twice\"]) abstract fun renamed(value: Int): Int\n fun use(): Int = renamed(4)\n}\n";
    index_source(&mut index, MIXIN_FILE, alias, "oracle.mixin");
    store
        .replace_source(
            &index,
            MIXIN_FILE,
            Arc::from(alias),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.authored = Some(read.declaration(occurrence(alias, "renamed", 0)).unwrap());
    req.name_link = NameLink::IndependentAlias;
    let id = read.require(req).unwrap();
    for i in 0..2 {
        let range = occurrence(alias, "renamed", i);
        read.bind_site(id, range.clone(), range, RenameSubject::Authored)
            .unwrap();
    }
    let range = occurrence(alias, "twice", 0);
    read.bind_site(id, range.clone(), range, RenameSubject::Target)
        .unwrap();
    let pending = read.finish();
    store.commit(pending).unwrap();
    let set = store.contributions("mixin-consumer", MIXIN_FILE).unwrap();
    let binding = set
        .binding_at(occurrence(alias, "renamed", 0).start)
        .unwrap();
    assert_eq!(set.references(&binding.target).len(), 3);
    let edits = set
        .rename_edits(&RenameTarget::Member(binding.target.clone()), "rotate")
        .unwrap();
    let target_renamed = apply_edits(alias, &edits);
    assert_eq!(edits.len(), 1);
    assert!(target_renamed.contains("aliases = [\"rotate\"]"));
    assert_eq!(target_renamed.matches("renamed").count(), 2);
    let own_subject = set
        .rename_subject_at(occurrence(alias, "renamed", 0).start)
        .unwrap();
    let own_edits = set.rename_edits(&own_subject, "localName").unwrap();
    let own_renamed = apply_edits(alias, &own_edits);
    assert_eq!(own_edits.len(), 2);
    assert!(own_renamed.contains("aliases = [\"twice\"]"));
    assert_eq!(own_renamed.matches("localName").count(), 2);
}

#[test]
fn source_model_config_and_negative_reads_reject_old_results() {
    let (mut index, mut store) = fixture();
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.invalidate_model();
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.configure_provider("mixin-consumer", true, Arc::from("{\"prefix\":\"other$\"}"));
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    let symbols = index
        .entries_for_file(TARGET_FILE)
        .into_iter()
        .map(|entry| (*entry.sym).clone())
        .collect::<Vec<_>>();
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Found(()),
            facts(TARGET_FILE, &symbols),
        )
        .unwrap();
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    store.set_namespace_coverage(Knowledge::Found(()));
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert!(read
        .resolve_type("oracle.AddedLater")
        .is_definitely_absent());
    let pending = read.finish();
    let text = "package oracle\nclass AddedLater\n";
    index_source(&mut index, "/oracle/AddedLater.kt", text, "oracle");
    store
        .replace_source(
            &index,
            "/oracle/AddedLater.kt",
            Arc::from(text),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
}

#[test]
fn atomic_replacement_disable_and_failure_are_owned_and_do_not_self_invalidate() {
    let (index, mut store) = fixture();
    store.configure_provider("other", true, Arc::from("{}"));
    let first = prefixed_pending(&index, &mut store, "mixin-consumer");
    let other = prefixed_pending(&index, &mut store, "other");
    store.commit(first).unwrap();
    store.commit(other).unwrap();
    assert!(store.contributions("mixin-consumer", MIXIN_FILE).is_some());
    assert!(store.contributions("other", MIXIN_FILE).is_some());
    let revision = store.publication_revision();
    let empty = store
        .begin(&index, "mixin-consumer", MIXIN_FILE)
        .unwrap()
        .finish();
    store.commit(empty).unwrap();
    assert!(store.publication_revision() > revision);
    assert!(store
        .contributions("mixin-consumer", MIXIN_FILE)
        .unwrap()
        .sites()
        .is_empty());
    assert_eq!(
        store
            .contributions("other", MIXIN_FILE)
            .unwrap()
            .sites()
            .len(),
        3
    );
    let failed = store.begin(&index, "other", MIXIN_FILE).unwrap().finish();
    store.fail(failed).unwrap();
    assert!(store.contributions("other", MIXIN_FILE).is_none());
    assert!(store.contributions("mixin-consumer", MIXIN_FILE).is_some());
    store.configure_provider("mixin-consumer", false, Arc::from("{}"));
    assert!(store.contributions("mixin-consumer", MIXIN_FILE).is_none());
    assert!(matches!(
        store.begin(&index, "mixin-consumer", MIXIN_FILE),
        Err(ContractError::DisabledProvider)
    ));
}

#[test]
fn newer_evaluation_wins_and_a_late_failure_cannot_erase_it() {
    let (index, mut store) = fixture();
    let old = prefixed_pending(&index, &mut store, "mixin-consumer");
    let newer = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.commit(newer).unwrap();
    assert_eq!(store.commit(old), Err(ContractError::Stale));
    let old_failure = store
        .begin(&index, "mixin-consumer", MIXIN_FILE)
        .unwrap()
        .finish();
    let newer = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.commit(newer).unwrap();
    assert_eq!(store.fail(old_failure), Err(ContractError::Stale));
    assert_eq!(
        store
            .contributions("mixin-consumer", MIXIN_FILE)
            .unwrap()
            .sites()
            .len(),
        3
    );
}

#[test]
fn invalid_owner_ranges_utf8_and_foreign_identities_cannot_be_published() {
    let (mut index, mut store) = fixture();
    let (other_index, mut other_store) = fixture();
    let mut other = other_store
        .begin(&other_index, "mixin-consumer", MIXIN_FILE)
        .unwrap();
    let foreign = other.resolve_type("oracle.Target");
    let foreign_pending = other.finish();
    assert_eq!(
        store.commit(foreign_pending),
        Err(ContractError::ForeignIdentity)
    );
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.target = foreign;
    assert_eq!(read.require(req), Err(ContractError::ForeignIdentity));
    drop(read);
    let unicode = format!("// 日本語\n{MIXIN}");
    index_source(&mut index, MIXIN_FILE, &unicode, "oracle.mixin");
    store
        .replace_source(
            &index,
            MIXIN_FILE,
            Arc::from(unicode.as_str()),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    assert_eq!(
        read.bind_site(id, 3..5, 3..4, RenameSubject::Target),
        Err(ContractError::InvalidRange)
    );
    let range = occurrence(&unicode, "twice(I)I", 0);
    assert_eq!(
        read.bind_site(
            id,
            range.clone(),
            range.start..range.end + 1,
            RenameSubject::Target
        ),
        Err(ContractError::InvalidRange)
    );
    assert_eq!(
        read.bind_site(
            id,
            range.clone(),
            range.start..range.start + 1,
            RenameSubject::Target
        ),
        Err(ContractError::InvalidNameLink)
    );
    assert_eq!(
        read.diagnostic(Diagnostic {
            start_byte: 3,
            end_byte: 4,
            severity: Severity::Error,
            code: None,
            message: "bad range".into()
        }),
        Err(ContractError::InvalidRange)
    );
}

#[test]
fn known_signatures_support_primitive_string_and_arrays_and_reject_void_values() {
    assert_eq!(
        method(
            &[JvmType::String, JvmType::Array(Box::new(JvmType::Int))],
            &JvmType::Void
        )
        .descriptor(),
        "(Ljava/lang/String;[I)V"
    );
    assert_eq!(
        MemberSignature::field(&JvmType::Array(Box::new(JvmType::String)))
            .unwrap()
            .descriptor(),
        "[Ljava/lang/String;"
    );
    assert_eq!(
        MemberSignature::method(&[JvmType::Void], &JvmType::Int),
        Err(ContractError::InvalidSignature)
    );
    assert_eq!(
        MemberSignature::field(&JvmType::Array(Box::new(JvmType::Void))),
        Err(ContractError::InvalidSignature)
    );
}

#[test]
fn unrelated_source_edits_do_not_expire_a_scoped_read_but_target_removal_does() {
    let (mut index, mut store) = fixture();
    let file = "/oracle/Unrelated.kt";
    let first = "package oracle\nclass Unrelated {\n val value: Int = 1\n}\n";
    index_source(&mut index, file, first, "oracle");
    store
        .replace_source(
            &index,
            file,
            Arc::from(first),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    let second = first.replace("= 1", "= 2");
    index_source(&mut index, file, &second, "oracle");
    store
        .replace_source(
            &index,
            file,
            Arc::from(second.as_str()),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    store.commit(pending).unwrap();
    let revision = store.publication_revision();
    store.remove_source(TARGET_FILE);
    assert!(store.publication_revision() > revision);
    assert!(store.contributions("mixin-consumer", MIXIN_FILE).is_none());
}

#[test]
fn namespace_incompleteness_never_proves_a_missing_type() {
    let (index, mut store) = fixture();
    store.set_namespace_coverage(Knowledge::Unknown(vec![IncompleteReason::IncompleteScope]));
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert!(matches!(
        read.resolve_type("oracle.NotLoaded"),
        Knowledge::Unknown(_)
    ));
    let pending = read.finish();
    store.set_namespace_coverage(Knowledge::Found(()));
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert!(read.resolve_type("oracle.NotLoaded").is_definitely_absent());
}

#[test]
fn unknown_flags_on_rejected_overloads_do_not_hide_a_proved_signature_match() {
    let (index, mut store) = fixture();
    let symbols = index
        .entries_for_file(TARGET_FILE)
        .into_iter()
        .map(|entry| (*entry.sym).clone())
        .collect::<Vec<_>>();
    let mut normalized = facts(TARGET_FILE, &symbols);
    for fact in &mut normalized {
        if matches!(&fact.signature, Knowledge::Found(signature) if signature.descriptor().starts_with("(Ljava/lang/String;"))
        {
            fact.dispatch = Knowledge::Unknown(vec![IncompleteReason::MissingFacts]);
        }
    }
    store
        .replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Found(()),
            normalized,
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    let id = read.require(req).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_found());
}

#[test]
fn alias_fallback_sites_only_rename_the_member_that_actually_bound() {
    let (mut index, mut store) = fixture();
    let alias = "package oracle.mixin\nabstract class CaseMixin {\n abstract fun renamed(value: Int): Int\n val aliases: String = \"missing,twice\"\n}\n";
    index_source(&mut index, MIXIN_FILE, alias, "oracle.mixin");
    store
        .replace_source(
            &index,
            MIXIN_FILE,
            Arc::from(alias),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.names = MemberNames::Exact(vec!["missing".into(), "twice".into()]);
    req.authored = Some(read.declaration(occurrence(alias, "renamed", 0)).unwrap());
    req.name_link = NameLink::IndependentAlias;
    let id = read.require(req).unwrap();
    let missing = occurrence(alias, "missing", 0);
    assert_eq!(
        read.bind_site(id, missing.clone(), missing, RenameSubject::Target),
        Err(ContractError::InvalidNameLink)
    );
    let twice = occurrence(alias, "twice", 0);
    read.bind_site(id, twice.clone(), twice.clone(), RenameSubject::Target)
        .unwrap();
    assert_eq!(
        read.bind_site(id, twice.clone(), twice, RenameSubject::Target),
        Err(ContractError::OverlappingEdits)
    );
}

#[test]
fn old_target_identity_cannot_cross_an_accepted_model_change() {
    let (index, mut store) = fixture();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let target = read.resolve_type("oracle.Target");
    drop(read);
    store.invalidate_model();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let mut req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[JvmType::Int], &JvmType::Int)),
        Dispatch::Instance,
    );
    req.target = target;
    assert_eq!(read.require(req), Err(ContractError::Stale));
}

#[test]
fn invalid_normalized_input_does_not_replace_a_valid_published_set() {
    let (index, mut store) = fixture();
    let pending = prefixed_pending(&index, &mut store, "mixin-consumer");
    store.commit(pending).unwrap();
    let revision = store.publication_revision();
    let foreign = DeclaredMemberFacts {
        definition: Def {
            file: "/foreign/Target.kt".into(),
            start_byte: 0,
            end_byte: 6,
        },
        signature: Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
        dispatch: Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
        visibility: Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
        synthetic: Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
    };
    assert_eq!(
        store.replace_source(
            &index,
            TARGET_FILE,
            Arc::from(TARGET),
            Knowledge::Found(()),
            vec![foreign]
        ),
        Err(ContractError::ForeignFile)
    );
    assert_eq!(store.publication_revision(), revision);
    assert_eq!(
        store
            .contributions("mixin-consumer", MIXIN_FILE)
            .unwrap()
            .sites()
            .len(),
        3
    );
}

#[test]
fn private_field_facts_are_readable_and_the_field_binding_uses_the_real_target_def() {
    let (index, mut store) = fixture();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let target = read.resolve_type("oracle.Target");
    let authored = read.declaration(occurrence(MIXIN, "label", 0)).unwrap();
    let req = MemberRequirement {
        target,
        authored: Some(authored),
        names: MemberNames::Exact(vec!["label".into()]),
        kind: MemberKind::Field,
        signature: Knowledge::Found(MemberSignature::field(&JvmType::String).unwrap()),
        dispatch: Knowledge::Found(Dispatch::Instance),
        name_link: NameLink::Direct,
    };
    let id = read.require(req.clone()).unwrap();
    let result = read.resolution(id).unwrap();
    let Knowledge::Found(binding) = &result.binding else {
        panic!("{result:?}")
    };
    assert_eq!(binding.target.definition().file, TARGET_FILE);
    assert_eq!(
        binding.target.signature().unwrap().descriptor(),
        "Ljava/lang/String;"
    );
    assert_eq!(
        result.candidates[0].visibility,
        Knowledge::Found(Visibility::Private)
    );
    assert_eq!(result.candidates[0].synthetic, Knowledge::Found(false));
    assert_ne!(
        binding.target.definition(),
        binding.authored.as_ref().unwrap().definition()
    );
    let mut wrong_static = req;
    wrong_static.dispatch = Knowledge::Found(Dispatch::Static);
    let id = read.require(wrong_static).unwrap();
    assert!(read.resolution(id).unwrap().binding.is_definitely_absent());
}

// These are public semantic-host contract tests. The immutable Person.class is decoded here;
// no test fabricates a successful Gradle/compiler invocation or a filesystem freshness receipt.
// The host indexes the real class-name range only and leaves source-member coverage Unknown.
const COMPILED_HOST_A: &str = "/compiled/app-a/Person.java";
const COMPILED_HOST_B: &str = "/compiled/app-b/Person.java";
const COMPILED_HOST_PROVIDER: &str = "compiled-contract";
const COMPILED_HOST_SOURCE: &str =
    include_str!("fixtures/classfile/sources/pluginfixture/Person.java");
const COMPILED_HOST_BYTES: &[u8] = include_bytes!("fixtures/classfile/pluginfixture/Person.class");

fn compiled_host_fixture() -> (
    Index,
    ExtensionStore,
    Arc<ktcore::classfile::ClassFileFacts>,
) {
    let mut index = Index::new();
    let mut store = ExtensionStore::default();
    let start = COMPILED_HOST_SOURCE.find("Person").unwrap();
    for (file, origin) in [(COMPILED_HOST_A, "app-a"), (COMPILED_HOST_B, "app-b")] {
        index.replace_file(
            file,
            vec![IndexedSymbol::new(
                "Person",
                SymbolKind::Class,
                "pluginfixture",
                None,
                start,
                start + "Person".len(),
            )],
            Tier::Volatile,
        );
        index.set_file_origin(
            file,
            FileOrigin {
                origin_id: origin.into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(ProjectScope {
                    module: origin.into(),
                    source_set: "main".into(),
                }),
            },
        );
        store
            .replace_source(
                &index,
                file,
                Arc::from(COMPILED_HOST_SOURCE),
                Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
                Vec::new(),
            )
            .unwrap();
    }
    index.replace_scope_visibilities(
        ["app-a", "app-b"]
            .into_iter()
            .map(|origin| ScopeVisibility {
                scope: ProjectScope {
                    module: origin.into(),
                    source_set: "main".into(),
                },
                ordered_origins: vec![origin.into()],
            })
            .collect(),
    );
    store.configure_provider(COMPILED_HOST_PROVIDER, true, Arc::from("{}"));
    let class = Arc::new(ktcore::classfile::decode("app-a", COMPILED_HOST_BYTES).unwrap());
    assert!(!index.may_have_compiled_callables());
    (index, store, class)
}

fn compiled_host_type(index: &Index, store: &mut ExtensionStore, file: &str) -> TypeId {
    let mut read = store.begin(index, COMPILED_HOST_PROVIDER, file).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("pluginfixture.Person") else {
        panic!("exact source owner")
    };
    owner
}

fn compiled_host_getter_pending(
    index: &Index,
    store: &mut ExtensionStore,
    file: &str,
    getter: usize,
) -> PendingContributions {
    let mut read = store.begin(index, COMPILED_HOST_PROVIDER, file).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("pluginfixture.Person") else {
        panic!("source owner")
    };
    let Knowledge::Found(class) = read.compiled_class(&owner).unwrap() else {
        panic!("attached class")
    };
    let methods = read.compiled_methods(&class).unwrap();
    assert!(read.admit_java_method(&methods[getter]).unwrap().is_found());
    read.finish()
}

fn compiled_host_published_members(
    index: &mut Index,
    store: &ExtensionStore,
    file: &str,
) -> Vec<ktcore::jvm::MemberIdentity> {
    let mut sets = std::collections::BTreeMap::<String, Vec<ContributionSet>>::new();
    for (_, owner, set) in store.contribution_sets() {
        sets.entry(owner.into()).or_default().push(set.clone());
    }
    index.replace_contributions(sets);
    let scoped = index.scoped_for_file(file);
    ktcore::jvm::member_candidates(
        &scoped,
        &ktcore::types::Type::class("Person", Some("pluginfixture".into())),
        "getName",
    )
    .into_iter()
    .map(|candidate| {
        assert!(
            candidate.definition().is_none(),
            "a binary method has no invented source range"
        );
        candidate.identity(&scoped).unwrap()
    })
    .collect()
}

#[test]
fn compiled_host_exact_origin_scope_and_store_are_not_interchangeable() {
    let (mut index, mut store, class_a) = compiled_host_fixture();
    let owner_a = compiled_host_type(&index, &mut store, COMPILED_HOST_A);
    let owner_b = compiled_host_type(&index, &mut store, COMPILED_HOST_B);
    assert_eq!(owner_a.qualified_name(), owner_b.qualified_name());
    assert_ne!(owner_a.definition(), owner_b.definition());
    assert_eq!(
        store.replace_compiled_input(&index, &owner_b, Some(class_a.clone())),
        Err(ContractError::ForeignIdentity)
    );
    let mut foreign_store = ExtensionStore::default();
    assert_eq!(
        foreign_store.replace_compiled_input(&index, &owner_a, Some(class_a.clone())),
        Err(ContractError::ForeignIdentity)
    );
    {
        let mut read = store
            .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_B)
            .unwrap();
        assert_eq!(
            read.compiled_class(&owner_a).err(),
            Some(ContractError::ForeignIdentity)
        );
        assert!(matches!(
            read.compiled_class(&owner_b).unwrap(),
            Knowledge::Unknown(_)
        ));
    }
    store
        .replace_compiled_input(&index, &owner_a, Some(class_a.clone()))
        .unwrap();
    let class_b = Arc::new(ktcore::classfile::decode("app-b", COMPILED_HOST_BYTES).unwrap());
    assert_eq!(
        class_a.identity.input, class_b.identity.input,
        "same actual bytes"
    );
    store
        .replace_compiled_input(&index, &owner_b, Some(class_b))
        .unwrap();
    let getter = class_a
        .methods
        .iter()
        .position(|method| method.name.as_ref() == "getName")
        .unwrap();
    let a = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_A, getter);
    store.commit(a).unwrap();
    let b = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_B, getter);
    store.commit(b).unwrap();
    let a = compiled_host_published_members(&mut index, &store, COMPILED_HOST_A);
    let b = compiled_host_published_members(&mut index, &store, COMPILED_HOST_B);
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    assert_eq!(a[0].class.origin_id, "app-a");
    assert_eq!(b[0].class.origin_id, "app-b");
    assert_ne!(
        a[0], b[0],
        "same FQN/descriptor/bytes in two modules are different members"
    );
}

#[test]
fn compiled_host_negative_arrival_and_cross_session_capabilities_cannot_publish_old_work() {
    let (mut index, mut store, class) = compiled_host_fixture();
    let owner = compiled_host_type(&index, &mut store, COMPILED_HOST_A);
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert!(matches!(
        read.compiled_class(&owner).unwrap(),
        Knowledge::Unknown(_)
    ));
    let negative = read.finish();
    store.commit(negative).unwrap();
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert!(matches!(
        read.compiled_class(&owner).unwrap(),
        Knowledge::Unknown(_)
    ));
    let old_negative = read.finish();
    store
        .replace_compiled_input(&index, &owner, Some(class.clone()))
        .unwrap();
    assert!(store
        .contributions(COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .is_none());
    assert_eq!(store.commit(old_negative), Err(ContractError::Stale));

    let getter = class
        .methods
        .iter()
        .position(|method| method.name.as_ref() == "getName")
        .unwrap();
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    let Knowledge::Found(old_class) = read.compiled_class(&owner).unwrap() else {
        panic!("class")
    };
    let old_method = read.compiled_methods(&old_class).unwrap()[getter].clone();
    assert!(read.admit_java_method(&old_method).unwrap().is_found());
    let old_pending = read.finish();
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert_eq!(
        read.compiled_methods(&old_class).err(),
        Some(ContractError::ForeignIdentity)
    );
    assert_eq!(
        read.admit_java_method(&old_method),
        Err(ContractError::ForeignIdentity)
    );
    let Knowledge::Found(current) = read.compiled_class(&owner).unwrap() else {
        panic!("current class")
    };
    let current_method = read.compiled_methods(&current).unwrap()[getter].clone();
    assert!(read.admit_java_method(&current_method).unwrap().is_found());
    let current_pending = read.finish();
    assert_eq!(store.commit(old_pending), Err(ContractError::Stale));
    store.commit(current_pending).unwrap();
    assert_eq!(
        compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).len(),
        1
    );
}

#[test]
fn compiled_host_revisions_source_model_and_clear_retract_sets_and_pending_but_noop_is_current() {
    for change in ["class", "source", "model", "clear"] {
        let (mut index, mut store, class) = compiled_host_fixture();
        let owner = compiled_host_type(&index, &mut store, COMPILED_HOST_A);
        let other = compiled_host_type(&index, &mut store, COMPILED_HOST_B);
        store
            .replace_compiled_input(&index, &owner, Some(class.clone()))
            .unwrap();
        let getter = class
            .methods
            .iter()
            .position(|method| method.name.as_ref() == "getName")
            .unwrap();
        let mut read = store
            .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_B)
            .unwrap();
        assert!(matches!(
            read.compiled_class(&other).unwrap(),
            Knowledge::Unknown(_)
        ));
        let independent = read.finish();
        store.commit(independent).unwrap();
        let first = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_A, getter);
        store.commit(first).unwrap();
        assert_eq!(
            compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).len(),
            1
        );
        assert!(index.may_have_compiled_callables());
        assert!(index
            .scoped_for_file(COMPILED_HOST_A)
            .may_have_compiled_callables());
        let published_snapshot = index.inference_snapshot();
        let noop = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_A, getter);
        // Equal facts from a separate decode are a no-op, not an artificial new revision.
        let equal = Arc::new(ktcore::classfile::decode("app-a", COMPILED_HOST_BYTES).unwrap());
        store
            .replace_compiled_input(&index, &owner, Some(equal))
            .unwrap();
        store.commit(noop).unwrap();
        let old = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_A, getter);
        let published_before = store.publication_revision();
        match change {
            "class" => {
                // A valid changed SourceFile hint changes the decoded content SHA. This is an
                // input-revision control, not evidence of a successful compilation of new text.
                let mut bytes = COMPILED_HOST_BYTES.to_vec();
                let hint = bytes
                    .windows(b"Person.java".len())
                    .position(|part| part == b"Person.java")
                    .unwrap();
                bytes[hint] = b'Q';
                let changed = Arc::new(ktcore::classfile::decode("app-a", &bytes).unwrap());
                assert_ne!(changed.identity.input, class.identity.input);
                store
                    .replace_compiled_input(&index, &owner, Some(changed))
                    .unwrap();
            }
            "source" => store
                .replace_source(
                    &index,
                    COMPILED_HOST_A,
                    Arc::from(format!("{COMPILED_HOST_SOURCE}\n// unsaved change\n")),
                    Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
                    Vec::new(),
                )
                .unwrap(),
            "model" => store.invalidate_model(),
            "clear" => store.clear_compiled_inputs(),
            _ => unreachable!(),
        }
        assert!(
            store
                .contributions(COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
                .is_none(),
            "{change}"
        );
        assert!(store.publication_revision() > published_before, "{change}");
        assert_eq!(store.commit(old), Err(ContractError::Stale), "{change}");
        if change != "model" {
            assert!(
                store
                    .contributions(COMPILED_HOST_PROVIDER, COMPILED_HOST_B)
                    .is_some(),
                "unread module remains current: {change}"
            );
        }
        if matches!(change, "source" | "model") {
            assert_eq!(
                store.replace_compiled_input(&index, &owner, Some(class.clone())),
                Err(ContractError::Stale),
                "{change}"
            );
        }
        assert!(compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).is_empty());
        assert!(!index.may_have_compiled_callables(), "retracted: {change}");
        assert!(
            published_snapshot.may_have_compiled_callables(),
            "immutable snapshot: {change}"
        );
        if change == "class" {
            let current = compiled_host_getter_pending(&index, &mut store, COMPILED_HOST_A, getter);
            store.commit(current).unwrap();
            let members = compiled_host_published_members(&mut index, &store, COMPILED_HOST_A);
            assert_eq!(members.len(), 1);
            assert_ne!(members[0].class.input, class.identity.input);
        } else {
            let fresh_owner = compiled_host_type(&index, &mut store, COMPILED_HOST_A);
            let mut read = store
                .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
                .unwrap();
            assert!(
                matches!(
                    read.compiled_class(&fresh_owner).unwrap(),
                    Knowledge::Unknown(_)
                ),
                "{change}"
            );
            drop(read);
            assert!(
                compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).is_empty()
            );
        }
    }
}

#[test]
fn compiled_host_unsupported_shape_is_unknown_and_does_not_certify_source_or_invent_definitions() {
    let (mut index, mut store, class) = compiled_host_fixture();
    let owner = compiled_host_type(&index, &mut store, COMPILED_HOST_A);
    store
        .replace_compiled_input(&index, &owner, Some(class.clone()))
        .unwrap();
    let getter = class
        .methods
        .iter()
        .position(|method| method.name.as_ref() == "getName")
        .unwrap();
    let constructor = class
        .methods
        .iter()
        .position(|method| method.name.as_ref() == "<init>")
        .unwrap();
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    let Knowledge::Found(handle) = read.compiled_class(&owner).unwrap() else {
        panic!("actual decoded class")
    };
    let handles = read.compiled_methods(&handle).unwrap();
    assert_eq!(
        read.admit_java_method(&handles[constructor]).unwrap(),
        Knowledge::Unknown(vec![IncompleteReason::UnsupportedSource])
    );
    assert!(read.admit_java_method(&handles[getter]).unwrap().is_found());
    assert!(
        matches!(
            read.declared_members(&owner).unwrap().1,
            Knowledge::Unknown(_)
        ),
        "positive binary method is not source completeness"
    );
    let accepted = read.finish();
    store.commit(accepted).unwrap();
    assert_eq!(
        compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).len(),
        1
    );

    // Explicit normalized-metadata fault injection tests the admission gate only. These cloned
    // negative facts are not passed off as independently decoded or freshly compiled outputs.
    for shape in [
        "nonpublic-class",
        "generic-class",
        "static",
        "synthetic",
        "generic-method",
        "descriptor",
    ] {
        let mut unsupported = (*class).clone();
        match shape {
            "nonpublic-class" => unsupported.access_flags &= !0x0001,
            "generic-class" => {
                unsupported.signature = Some(Arc::from("<T:Ljava/lang/Object;>Ljava/lang/Object;"))
            }
            "static" => unsupported.methods[getter].access_flags |= 0x0008,
            "synthetic" => unsupported.methods[getter].access_flags |= 0x1000,
            "generic-method" => {
                unsupported.methods[getter].signature = Some(Arc::from("()Ljava/lang/String;"))
            }
            "descriptor" => unsupported.methods[getter].descriptor = Arc::from("()I"),
            _ => unreachable!(),
        }
        store
            .replace_compiled_input(&index, &owner, Some(Arc::new(unsupported)))
            .unwrap();
        let mut read = store
            .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
            .unwrap();
        let Knowledge::Found(handle) = read.compiled_class(&owner).unwrap() else {
            panic!("host attached metadata")
        };
        let method = read.compiled_methods(&handle).unwrap()[getter].clone();
        assert_eq!(
            read.admit_java_method(&method).unwrap(),
            Knowledge::Unknown(vec![IncompleteReason::UnsupportedSource]),
            "{shape}"
        );
        let pending = read.finish();
        store.commit(pending).unwrap();
        assert!(
            compiled_host_published_members(&mut index, &store, COMPILED_HOST_A).is_empty(),
            "{shape}"
        );
    }
}

#[test]
fn selected_compiled_java_array_factories_keep_exact_descriptor_rank_origin_and_no_fake_source() {
    use ktcore::classfile::decode;
    use ktcore::infer::FileCtx;
    use ktcore::jvm::{self, CallableCandidate};
    use ktcore::types::Type;
    use std::collections::BTreeMap;
    const FILE: &str = "/compiled/array/Factory.java";
    const ITEM_FILE: &str = "/compiled/array/Item.java";
    const PROVIDER: &str = "compiled-array-contract";
    const SOURCE: &str = include_str!("fixtures/classfile/sources/arrayfixture/Factory.java");
    const ITEM_SOURCE: &str = include_str!("fixtures/classfile/sources/arrayfixture/Item.java");
    // Root must first compile the paired ordinary Java source and preserve its actual class bytes.
    const BYTES: &[u8] = include_bytes!("fixtures/classfile/arrayfixture/Factory.class");
    let mut index = Index::new();
    let scope = ProjectScope {
        module: "array-owner".into(),
        source_set: "main".into(),
    };
    for (file, text, name) in [(FILE, SOURCE, "Factory"), (ITEM_FILE, ITEM_SOURCE, "Item")] {
        let start = text.find(name).unwrap();
        // A class-only index isolates the real binary method surface. Coverage stays Unknown.
        index.replace_file(
            file,
            vec![IndexedSymbol::new(
                name,
                SymbolKind::Class,
                "arrayfixture",
                None,
                start,
                start + name.len(),
            )],
            Tier::Volatile,
        );
        index.set_file_origin(
            file,
            FileOrigin {
                origin_id: "array-owner".into(),
                representation: SourceRepresentation::Source,
                project_scope: Some(scope.clone()),
            },
        );
    }
    index.replace_scope_visibilities(vec![ScopeVisibility {
        scope,
        ordered_origins: vec!["array-owner".into()],
    }]);
    // A valid Kotlin class named int must not rebind the descriptor's primitive component.
    const POISON: &str = "package consumer\nclass int\n";
    let poison_at = POISON.find("int").unwrap();
    index.replace_file(
        "/compiled/array/Poison.kt",
        vec![IndexedSymbol::new(
            "int",
            SymbolKind::Class,
            "consumer",
            None,
            poison_at,
            poison_at + 3,
        )],
        Tier::Volatile,
    );
    index.set_file_origin(
        "/compiled/array/Poison.kt",
        FileOrigin {
            origin_id: "array-owner".into(),
            representation: SourceRepresentation::Source,
            project_scope: Some(ProjectScope {
                module: "array-owner".into(),
                source_set: "main".into(),
            }),
        },
    );
    let mut store = ExtensionStore::default();
    store.configure_provider(PROVIDER, true, Arc::from("{}"));
    store
        .replace_source(
            &index,
            FILE,
            Arc::from(SOURCE),
            Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
            Vec::new(),
        )
        .unwrap();
    let owner = {
        let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
        let Knowledge::Found(owner) = read.resolve_type("arrayfixture.Factory") else {
            panic!("source owner")
        };
        owner
    };
    let class = Arc::new(decode("array-owner", BYTES).unwrap());
    assert_eq!(class.identity.internal_name, "arrayfixture/Factory");
    assert!(class.signature.is_none());
    store
        .replace_compiled_input(&index, &owner, Some(class.clone()))
        .unwrap();
    let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
    let Knowledge::Found(handle) = read.compiled_class(&owner).unwrap() else {
        panic!("exact compiled input")
    };
    let handles = read.compiled_methods(&handle).unwrap();
    for (name, descriptor) in [
        ("name", "()Ljava/lang/String;"),
        ("rows", "()[[Larrayfixture/Item;"),
        ("counts", "()[I"),
    ] {
        let position = class
            .methods
            .iter()
            .position(|method| method.name.as_ref() == name)
            .unwrap();
        assert_eq!(class.methods[position].descriptor.as_ref(), descriptor);
        assert!(
            read.admit_java_method(&handles[position])
                .unwrap()
                .is_found(),
            "{name}: {descriptor}"
        );
    }
    for name in [
        "generic",
        "hidden",
        "annotated",
        "staticRows",
        "withArgument",
    ] {
        let position = class
            .methods
            .iter()
            .position(|method| method.name.as_ref() == name)
            .unwrap();
        assert!(matches!(
            read.admit_java_method(&handles[position]).unwrap(),
            Knowledge::Unknown(_)
        ));
    }
    let pending = read.finish();
    store.commit(pending).unwrap();
    let mut sets = BTreeMap::new();
    sets.insert(
        FILE.into(),
        vec![store.contributions(PROVIDER, FILE).unwrap().clone()],
    );
    index.replace_contributions(sets);
    let scoped = index.scoped_for_file(FILE);
    let receiver = Type::class("Factory", Some("arrayfixture".into()));
    for (name, descriptor, rank, component, package) in [
        (
            "rows",
            "()[[Larrayfixture/Item;",
            2,
            "Item",
            Some("arrayfixture"),
        ),
        ("counts", "()[I", 1, "int", None),
    ] {
        let candidates = jvm::member_candidates(&scoped, &receiver, name);
        let [candidate @ CallableCandidate::Jvm { .. }] = candidates.as_slice() else {
            panic!("one exact compiled candidate")
        };
        assert!(
            candidate.definition().is_none(),
            "binary method has no authored Def"
        );
        let identity = candidate.identity(&scoped).unwrap();
        assert_eq!(identity.class, class.identity);
        assert_eq!(identity.descriptor, descriptor);
        let mut result = jvm::projected_return_type_in_context(
            &scoped,
            &receiver,
            *candidate,
            &FileCtx::new("consumer".into(), Vec::new()),
        )
        .expect("positive array result");
        for _ in 0..rank {
            let Type::Array { component, .. } = result else {
                panic!("lost array rank: {name}")
            };
            result = *component;
        }
        assert_eq!(result.name(), Some(component));
        assert_eq!(result.package(), package);
    }

    let factory = (*index.entries_for_file(FILE)[0].sym).clone();
    let at = SOURCE.find("rows()").unwrap();
    let mut rows = IndexedSymbol::new(
        "rows",
        SymbolKind::Function,
        "arrayfixture",
        Some("Factory".into()),
        at,
        at + 4,
    );
    rows.arity = Some(0);
    rows.return_type = Some(ktcore::types::TypeRef::simple("Item[][]"));
    let mut parser = tree_sitter::Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .unwrap();
    let tree = parser.parse(SOURCE, None).unwrap();
    let node = tree
        .root_node()
        .descendant_for_byte_range(at, at + 4)
        .unwrap()
        .parent()
        .unwrap();
    rows.source_member = ktcore::indexer::source_member_flags(node, SOURCE, true);
    index.replace_file(FILE, vec![factory, rows], Tier::Volatile);
    store
        .replace_source(
            &index,
            ITEM_FILE,
            Arc::from(ITEM_SOURCE),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let type_scope = ktcore::indexer::TypeScope::with_default_imports(
        "arrayfixture",
        Vec::new(),
        &["java.lang"],
    );
    let declarations =
        ktcore::extensions::source::extract(&index, FILE, &tree, SOURCE, &type_scope);
    store
        .replace_source_declarations(
            &index,
            FILE,
            Arc::from(SOURCE),
            Knowledge::Unknown(vec![IncompleteReason::MissingFacts]),
            declarations,
        )
        .unwrap();
    let owner = {
        let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
        let Knowledge::Found(owner) = read.resolve_type("arrayfixture.Factory") else {
            panic!("authored owner")
        };
        let (members, _) = read.declared_members(&owner).unwrap();
        assert!(
            matches!(&members[0].signature, Knowledge::Found(signature) if signature.descriptor() == "()[[Larrayfixture/Item;")
        );
        owner
    };
    store
        .replace_compiled_input(&index, &owner, Some(class.clone()))
        .unwrap();
    let mut read = store.begin(&index, PROVIDER, FILE).unwrap();
    let Knowledge::Found(handle) = read.compiled_class(&owner).unwrap() else {
        panic!("authored compiled input")
    };
    let position = class
        .methods
        .iter()
        .position(|method| method.name.as_ref() == "rows")
        .unwrap();
    let handles = read.compiled_methods(&handle).unwrap();
    assert!(read
        .admit_java_method(&handles[position])
        .unwrap()
        .is_found());
    let pending = read.finish();
    store.commit(pending).unwrap();
    assert_eq!(
        store.contributions(PROVIDER, FILE).unwrap(),
        &ContributionSet::default(),
        "an authored nominal-array method needs no duplicate compiled contribution"
    );
}

#[test]
fn nominal_signature_provenance_does_not_change_canonical_string_descriptor_equality() {
    let (mut index, mut store) = fixture();
    let file = "/sdk/String.kt";
    let text = "package java.lang\nclass String\n";
    index_source(&mut index, file, text, "java.lang");
    store
        .replace_source(
            &index,
            file,
            Arc::from(text),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("java.lang.String") else {
        panic!("String owner")
    };
    let Knowledge::Found(reference) = read.reference_type(&owner).unwrap() else {
        panic!("String reference")
    };
    let expected = method(&[], &JvmType::String);
    let nominal = method(&[], &reference);
    assert_eq!(expected.descriptor(), nominal.descriptor());
    assert_eq!(
        expected, nominal,
        "provenance validates handles, not JVM descriptor identity"
    );
    assert_eq!(
        MemberSignature::field(&JvmType::Array(Box::new(JvmType::String))).unwrap(),
        MemberSignature::field(&JvmType::Array(Box::new(reference))).unwrap()
    );
}

#[test]
fn nominal_reference_identities_are_canonical_and_cannot_cross_hosts_or_revisions() {
    let (mut index, mut store) = fixture();
    let nested_file = "/oracle/Nested.kt";
    let nested = "package oracle\nclass Outer { class Inner }\n";
    index_source(&mut index, nested_file, nested, "oracle");
    store
        .replace_source(
            &index,
            nested_file,
            Arc::from(nested),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("oracle.Outer.Inner") else {
        panic!("nested owner");
    };
    let Knowledge::Found(reference) = read.reference_type(&owner).unwrap() else {
        panic!("nested reference");
    };
    let signature = method(&[JvmType::Array(Box::new(reference.clone()))], &reference);
    assert_eq!(
        signature.descriptor(),
        "([Loracle/Outer$Inner;)Loracle/Outer$Inner;"
    );
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(signature.clone()),
        Dispatch::Instance,
    );
    read.require(req).unwrap();
    let pending = read.finish();
    let changed = nested.replace("class Inner", "class Inner { fun added(): Int = 1 }");
    index_source(&mut index, nested_file, &changed, "oracle");
    store
        .replace_source(
            &index,
            nested_file,
            Arc::from(changed),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(signature.clone()),
        Dispatch::Instance,
    );
    assert_eq!(read.require(req), Err(ContractError::Stale));
    drop(read);
    let (foreign_index, mut foreign_store) = fixture();
    let mut foreign = foreign_store
        .begin(&foreign_index, "mixin-consumer", MIXIN_FILE)
        .unwrap();
    let Knowledge::Found(owner) = foreign.resolve_type("oracle.Target") else {
        panic!("owner");
    };
    let Knowledge::Found(reference) = foreign.reference_type(&owner).unwrap() else {
        panic!("reference");
    };
    drop(foreign);
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let req = requirement(
        &mut read,
        "twice",
        Knowledge::Found(method(&[reference], &JvmType::Int)),
        Dispatch::Instance,
    );
    assert_eq!(read.require(req), Err(ContractError::ForeignIdentity));
}

#[test]
fn nominal_source_signatures_resolve_references_and_keep_boxing_generics_and_missing_types_unknown()
{
    let (mut index, mut store) = fixture();
    let file = "/oracle/References.kt";
    let text = "package oracle\nclass References {\n fun exact(value: Target): Target = value\n fun refs(value: Array<Target>): Array<Target> = value\n fun boxed(value: Array<Int>): Array<Int> = value\n fun <T> generic(value: T): T = value\n fun missing(value: Missing): Missing = value\n}\n";
    let tree = KotlinParser::new().parse(text);
    assert!(
        !tree.root_node().has_error(),
        "valid nominal fixture: {text}"
    );
    let symbols = extract_symbols(&tree, text, "oracle");
    index.replace_file(file, symbols, Tier::Volatile);
    let scope = ktcore::indexer::TypeScope::with_default_imports(
        "oracle",
        Vec::new(),
        &["kotlin", "java.lang"],
    );
    let declarations = ktcore::extensions::source::extract(&index, file, &tree, text, &scope);
    store
        .replace_source_declarations(
            &index,
            file,
            Arc::from(text),
            Knowledge::Found(()),
            declarations,
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("oracle.References") else {
        panic!("owner");
    };
    let (members, _) = read.declared_members(&owner).unwrap();
    for (name, descriptor) in [
        ("exact", "(Loracle/Target;)Loracle/Target;"),
        ("refs", "([Loracle/Target;)[Loracle/Target;"),
    ] {
        let member = members.iter().find(|member| member.name == name).unwrap();
        assert!(
            matches!(&member.signature, Knowledge::Found(signature) if signature.descriptor() == descriptor),
            "{name}: {:?}",
            member.signature
        );
    }
    for name in ["boxed", "generic", "missing"] {
        assert!(
            matches!(
                &members
                    .iter()
                    .find(|member| member.name == name)
                    .unwrap()
                    .signature,
                Knowledge::Unknown(_)
            ),
            "{name}"
        );
    }
    drop(read);
    let shadow_file = "/oracle/Array.kt";
    let shadow = "package oracle\nclass Array<T>\n";
    index_source(&mut index, shadow_file, shadow, "oracle");
    store
        .replace_source(
            &index,
            shadow_file,
            Arc::from(shadow),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("oracle.References") else {
        panic!("owner");
    };
    let (members, _) = read.declared_members(&owner).unwrap();
    assert!(
        matches!(
            &members
                .iter()
                .find(|member| member.name == "refs")
                .unwrap()
                .signature,
            Knowledge::Unknown(_)
        ),
        "user Array must not become a JVM array"
    );
}

#[test]
fn nominal_value_generic_and_ambiguous_reference_owners_stay_unknown() {
    let (mut index, mut store) = fixture();
    for (file, text) in [
        ("/oracle/Generic.kt", "package oracle\nclass Generic<T>\n"),
        (
            "/oracle/Value.kt",
            "package oracle\n@JvmInline value class Inline(val raw: Int)\n",
        ),
    ] {
        index_source(&mut index, file, text, "oracle");
        store
            .replace_source(
                &index,
                file,
                Arc::from(text),
                Knowledge::Found(()),
                Vec::new(),
            )
            .unwrap();
    }
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for name in ["oracle.Generic", "oracle.Inline"] {
        let Knowledge::Found(owner) = read.resolve_type(name) else {
            panic!("{name}");
        };
        assert!(
            matches!(read.reference_type(&owner).unwrap(), Knowledge::Unknown(_)),
            "{name}"
        );
    }
    let Knowledge::Found(owner) = read.resolve_type("oracle.Target") else {
        panic!("Target");
    };
    drop(read);
    let duplicate = "package oracle\nclass Target\n";
    index_source(&mut index, "/oracle/Duplicate.kt", duplicate, "oracle");
    store
        .replace_source(
            &index,
            "/oracle/Duplicate.kt",
            Arc::from(duplicate),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert!(
        matches!(read.reference_type(&owner).unwrap(), Knowledge::Unknown(reasons) if reasons.contains(&IncompleteReason::AmbiguousType))
    );
}

#[test]
fn nominal_enum_record_and_named_object_owners_have_canonical_descriptors() {
    let (mut index, mut store) = fixture();
    let java = "package oracle; class JavaKinds { enum Axis { X, Y } record Point(int value) {} }";
    let symbols = [
        ("JavaKinds", SymbolKind::Class, None),
        ("Axis", SymbolKind::EnumClass, Some("JavaKinds")),
        ("Point", SymbolKind::Class, Some("JavaKinds")),
    ]
    .into_iter()
    .map(|(name, kind, container)| {
        let at = java.find(name).unwrap();
        IndexedSymbol::new(
            name,
            kind,
            "oracle",
            container.map(str::to_string),
            at,
            at + name.len(),
        )
    })
    .collect();
    index.replace_file("/oracle/JavaKinds.java", symbols, Tier::Volatile);
    let kotlin =
        "package oracle\nclass KotlinKinds {\n enum class Axis { X, Y }\n object Marker\n}\n";
    index_source(&mut index, "/oracle/KotlinKinds.kt", kotlin, "oracle");
    for (file, text) in [
        ("/oracle/JavaKinds.java", java),
        ("/oracle/KotlinKinds.kt", kotlin),
    ] {
        store
            .replace_source(
                &index,
                file,
                Arc::from(text),
                Knowledge::Found(()),
                Vec::new(),
            )
            .unwrap();
    }
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for name in [
        "JavaKinds.Axis",
        "JavaKinds.Point",
        "KotlinKinds.Axis",
        "KotlinKinds.Marker",
    ] {
        let qualified = format!("oracle.{name}");
        let Knowledge::Found(owner) = read.resolve_type(&qualified) else {
            panic!("{qualified}")
        };
        let Knowledge::Found(reference) = read.reference_type(&owner).unwrap() else {
            panic!("reference {qualified}")
        };
        let internal = format!("oracle/{}", name.replace('.', "$"));
        assert_eq!(
            method(&[JvmType::Array(Box::new(reference.clone()))], &reference).descriptor(),
            format!("([L{internal};)L{internal};")
        );
    }
}

fn install_nominal_kotlin(
    index: &mut Index,
    store: &mut ExtensionStore,
    file: &str,
    text: &str,
    package: &str,
) {
    let tree = KotlinParser::new().parse(text);
    assert!(!tree.root_node().has_error(), "{text}");
    index.replace_file(file, extract_symbols(&tree, text, package), Tier::Volatile);
    let scope = ktcore::indexer::TypeScope::with_default_imports(
        package,
        Vec::new(),
        &["kotlin", "java.lang"],
    );
    let declarations = ktcore::extensions::source::extract(index, file, &tree, text, &scope);
    store
        .replace_source_declarations(
            index,
            file,
            Arc::from(text),
            Knowledge::Found(()),
            declarations,
        )
        .unwrap();
}

#[test]
fn declaration_type_provider_keeps_exact_nested_owners_and_jvm_signatures() {
    struct Probe;
    impl AnalysisProvider for Probe {
        fn id(&self) -> &str {
            "mixin-consumer"
        }
        fn analyze(&self, read: &mut ReadSession<'_>) -> Result<(), ContractError> {
            let methods: Vec<_> = read
                .declarations()
                .iter()
                .filter(|d| d.member.is_some())
                .cloned()
                .collect();
            assert_eq!(methods.len(), 2);
            for authored in methods {
                let owner = authored.owner.unwrap();
                let declaration = read.declaration(owner.start_byte..owner.end_byte)?;
                assert!(
                    !matches!(read.resolve_type_ref(&ktcore::types::TypeRef::simple("Axis")), Knowledge::Found(ref ty) if ty.definition() == &owner),
                    "simple-name resolution cannot identify this nested owner"
                );
                let Knowledge::Found(ty) = read.declaration_type(&declaration)? else {
                    panic!("exact type declaration")
                };
                assert_eq!(ty.definition(), &owner);
                let members = read.declared_members(&ty)?.0;
                assert_eq!(members.len(), 1);
                assert_eq!(members[0].id.definition(), &authored.definition);
                assert!(
                    matches!(&members[0].signature, Knowledge::Found(signature) if signature.descriptor() == "()I")
                );
                let Knowledge::Found(reference) = read.reference_type(&ty)? else {
                    panic!("nested JVM reference")
                };
                let expected = match ty.qualified_name() {
                    "oracle.First.Axis" => "(Loracle/First$Axis;)V",
                    "oracle.Second.Axis" => "(Loracle/Second$Axis;)V",
                    other => panic!("wrong owner: {other}"),
                };
                assert_eq!(method(&[reference], &JvmType::Void).descriptor(), expected);
                let member =
                    read.declaration(authored.definition.start_byte..authored.definition.end_byte)?;
                assert!(
                    matches!(read.declaration_type(&member)?, Knowledge::Unknown(reasons) if reasons == vec![IncompleteReason::UnsupportedType("value".into())])
                );
            }
            Ok(())
        }
    }
    for java in [false, true] {
        let (mut index, mut store) = fixture();
        install_nominal_kotlin(
            &mut index,
            &mut store,
            "/other/Axis.kt",
            "package other\nclass Axis\n",
            "other",
        );
        let file = if java {
            "/oracle/Nested.java"
        } else {
            "/oracle/Nested.kt"
        };
        let text = if java {
            "package oracle; class First { static class Axis { public int value() { return 1; } } } class Second { static class Axis { public int value() { return 2; } } }"
        } else {
            "package oracle\nclass First { class Axis { fun value(): Int = 1 } }\nclass Second { class Axis { fun value(): Int = 2 } }\n"
        };
        if java {
            let mut parser = tree_sitter::Parser::new();
            parser
                .set_language(&tree_sitter_java::LANGUAGE.into())
                .unwrap();
            let tree = parser.parse(text, None).unwrap();
            assert!(!tree.root_node().has_error());
            let mut symbols = Vec::new();
            for (ordinal, outer) in ["First", "Second"].into_iter().enumerate() {
                for (name, kind, container) in [
                    (outer, SymbolKind::Class, None),
                    ("Axis", SymbolKind::Class, Some(outer.to_string())),
                    ("value", SymbolKind::Function, Some(format!("{outer}.Axis"))),
                ] {
                    let range = occurrence(text, name, if name == outer { 0 } else { ordinal });
                    let mut symbol =
                        IndexedSymbol::new(name, kind, "oracle", container, range.start, range.end);
                    if kind == SymbolKind::Function {
                        symbol.arity = Some(0);
                        symbol.return_type = Some(ktcore::types::TypeRef::simple("int"));
                        let node = tree
                            .root_node()
                            .descendant_for_byte_range(range.start, range.end)
                            .unwrap()
                            .parent()
                            .unwrap();
                        symbol.source_member =
                            ktcore::indexer::source_member_flags(node, text, true);
                    }
                    symbols.push(symbol);
                }
            }
            index.replace_file(file, symbols, Tier::Volatile);
            let scope = ktcore::indexer::TypeScope::with_default_imports(
                "oracle",
                Vec::new(),
                &["java.lang"],
            );
            let declarations = source::extract(&index, file, &tree, text, &scope);
            store
                .replace_source_declarations(
                    &index,
                    file,
                    Arc::from(text),
                    Knowledge::Found(()),
                    declarations,
                )
                .unwrap();
        } else {
            install_nominal_kotlin(&mut index, &mut store, file, text, "oracle");
        }
        let mut read = store.begin(&index, Probe.id(), file).unwrap();
        Probe.analyze(&mut read).unwrap();
        let pending = read.finish();
        store.commit(pending).unwrap();
    }
}

#[test]
fn declaration_type_validates_identity_scope_and_read_dependencies() {
    let (mut index, mut store, _) = compiled_host_fixture();
    let range = occurrence(COMPILED_HOST_SOURCE, "Person", 0);
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert_eq!(
        read.declaration(0..1),
        Err(ContractError::NotIndexedDeclaration)
    );
    let declaration = read.declaration(range.clone()).unwrap();
    assert!(read.declaration_type(&declaration).unwrap().is_found());
    let pending = read.finish();
    store.invalidate_model();
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert!(
        read.declaration_type(&declaration).unwrap().is_found(),
        "unchanged declaration in a current scope remains reusable"
    );
    let pending = read.finish();
    store.set_namespace_coverage(Knowledge::Found(()));
    assert_eq!(store.commit(pending), Err(ContractError::Stale));
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_B)
        .unwrap();
    assert_eq!(
        read.declaration_type(&declaration),
        Err(ContractError::ForeignIdentity)
    );
    drop(read);
    let (foreign_index, mut foreign_store, _) = compiled_host_fixture();
    let mut foreign = foreign_store
        .begin(&foreign_index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert_eq!(
        foreign.declaration_type(&declaration),
        Err(ContractError::ForeignIdentity)
    );
    drop(foreign);
    index.replace_scope_visibilities(Vec::new());
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert!(
        matches!(read.declaration_type(&declaration).unwrap(), Knowledge::Unknown(reasons) if reasons == vec![IncompleteReason::IncompleteScope])
    );
    drop(read);
    store
        .replace_source(
            &index,
            COMPILED_HOST_A,
            Arc::from(format!("{COMPILED_HOST_SOURCE}\n")),
            Knowledge::Found(()),
            Vec::new(),
        )
        .unwrap();
    let mut read = store
        .begin(&index, COMPILED_HOST_PROVIDER, COMPILED_HOST_A)
        .unwrap();
    assert_eq!(
        read.declaration_type(&declaration),
        Err(ContractError::Stale)
    );
}

#[test]
fn nominal_enum_and_object_source_signatures_are_useful() {
    let (mut index, mut store) = fixture();
    install_nominal_kotlin(&mut index, &mut store, "/oracle/KotlinKinds.kt", "package oracle\nclass KotlinKinds {\n enum class Axis { X, Y }\n object Marker\n fun axis(value: oracle.KotlinKinds.Axis): oracle.KotlinKinds.Axis = value\n fun marker(value: oracle.KotlinKinds.Marker): oracle.KotlinKinds.Marker = value\n}\n", "oracle");
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("oracle.KotlinKinds") else {
        panic!("owner")
    };
    let (members, _) = read.declared_members(&owner).unwrap();
    for (name, descriptor) in [
        (
            "axis",
            "(Loracle/KotlinKinds$Axis;)Loracle/KotlinKinds$Axis;",
        ),
        (
            "marker",
            "(Loracle/KotlinKinds$Marker;)Loracle/KotlinKinds$Marker;",
        ),
    ] {
        let member = members.iter().find(|member| member.name == name).unwrap();
        assert!(
            matches!(&member.signature, Knowledge::Found(signature) if signature.descriptor() == descriptor),
            "{name}: {:?}",
            member.signature
        );
    }
}

#[test]
fn nominal_kotlin_any_and_string_references_use_platform_names() {
    let (mut index, mut store) = fixture();
    install_nominal_kotlin(
        &mut index,
        &mut store,
        "/kotlin/Builtins.kt",
        "package kotlin\nclass Any\nclass String\n",
        "kotlin",
    );
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for (name, descriptor) in [
        ("Any", "(Ljava/lang/Object;)Ljava/lang/Object;"),
        ("String", "(Ljava/lang/String;)Ljava/lang/String;"),
    ] {
        let Knowledge::Found(owner) = read.resolve_type(&format!("kotlin.{name}")) else {
            panic!("{name}")
        };
        let Knowledge::Found(reference) = read.reference_type(&owner).unwrap() else {
            panic!("reference {name}")
        };
        assert_eq!(
            method(&[reference.clone()], &reference).descriptor(),
            descriptor
        );
    }
}

#[test]
fn nominal_kotlin_any_source_signatures_and_arrays_are_useful() {
    let (mut index, mut store) = fixture();
    install_nominal_kotlin(
        &mut index,
        &mut store,
        "/kotlin/Builtins.kt",
        "package kotlin\nclass Any\nclass String\nclass Int\n",
        "kotlin",
    );
    install_nominal_kotlin(&mut index, &mut store, "/oracle/Mapped.kt", "package oracle\nclass Mapped {\n fun any(value: Any): Any = value\n fun text(value: String): String = value\n fun refs(value: Array<Any>): Array<Any> = value\n fun boxed(value: Array<Int>): Array<Int> = value\n fun nullable(value: Any?): Any? = value\n}\n", "oracle");
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("oracle.Mapped") else {
        panic!("Mapped")
    };
    let (members, _) = read.declared_members(&owner).unwrap();
    for (name, descriptor) in [
        ("any", "(Ljava/lang/Object;)Ljava/lang/Object;"),
        ("text", "(Ljava/lang/String;)Ljava/lang/String;"),
        ("refs", "([Ljava/lang/Object;)[Ljava/lang/Object;"),
    ] {
        let member = members.iter().find(|member| member.name == name).unwrap();
        assert!(
            matches!(&member.signature, Knowledge::Found(signature) if signature.descriptor() == descriptor),
            "{name}: {:?}",
            member.signature
        );
    }
    for name in ["boxed", "nullable"] {
        assert!(
            matches!(
                &members
                    .iter()
                    .find(|member| member.name == name)
                    .unwrap()
                    .signature,
                Knowledge::Unknown(_)
            ),
            "{name}"
        );
    }
}

#[test]
fn nominal_kotlin_builtin_boxing_without_a_signature_context_stays_unknown() {
    let (mut index, mut store) = fixture();
    install_nominal_kotlin(
        &mut index,
        &mut store,
        "/kotlin/Builtins.kt",
        "package kotlin\nclass Int\nclass Boolean\nclass Unit\n",
        "kotlin",
    );
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for name in ["Int", "Boolean", "Unit"] {
        let Knowledge::Found(owner) = read.resolve_type(&format!("kotlin.{name}")) else {
            panic!("{name}")
        };
        assert!(
            matches!(read.reference_type(&owner).unwrap(), Knowledge::Unknown(_)),
            "{name} cannot be issued using its source name"
        );
    }
}

#[test]
fn nominal_new_declaration_kinds_keep_local_generic_and_malformed_headers_unknown() {
    let (mut index, mut store) = fixture();
    for (file, text, name, kind, container) in [
        (
            "/oracle/Local.java",
            "package oracle; class Local { void f() { enum Axis { X } } }",
            "Axis",
            SymbolKind::EnumClass,
            Some("Local"),
        ),
        (
            "/oracle/GenericRecord.java",
            "package oracle; record Point<T>(T value) {}",
            "Point",
            SymbolKind::Class,
            None,
        ),
        (
            "/oracle/Broken.java",
            "package oracle; enum Broken implements {}",
            "Broken",
            SymbolKind::EnumClass,
            None,
        ),
    ] {
        let at = text.find(name).unwrap();
        let mut symbol = IndexedSymbol::new(
            name,
            kind,
            "oracle",
            container.map(str::to_string),
            at,
            at + name.len(),
        );
        if name == "Point" {
            symbol.type_params.push("T".into());
        }
        index.replace_file(file, vec![symbol], Tier::Volatile);
        store
            .replace_source(
                &index,
                file,
                Arc::from(text),
                Knowledge::Found(()),
                Vec::new(),
            )
            .unwrap();
    }
    install_nominal_kotlin(
        &mut index,
        &mut store,
        "/oracle/Expected.kt",
        "package oracle\nexpect enum class Expected { X }\n",
        "oracle",
    );
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for name in [
        "oracle.Local.Axis",
        "oracle.Point",
        "oracle.Broken",
        "oracle.Expected",
    ] {
        let Knowledge::Found(owner) = read.resolve_type(name) else {
            panic!("{name}")
        };
        assert!(
            matches!(read.reference_type(&owner).unwrap(), Knowledge::Unknown(_)),
            "{name}"
        );
    }
}

#[test]
fn nominal_nested_owner_scope_selects_its_own_enum_and_keeps_generic_array_shadow_unknown() {
    let (mut index, mut store) = fixture();
    install_nominal_kotlin(&mut index, &mut store, "/oracle/ScopedKinds.kt", "package oracle\nclass First {\n enum class Axis { X }\n class Array<T>\n fun axis(value: Axis): Axis = value\n fun rows(value: Array<Axis>): Array<Axis> = value\n}\nclass Second {\n enum class Axis { Y }\n fun axis(value: Axis): Axis = value\n}\n", "oracle");
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    for name in ["First", "Second"] {
        let Knowledge::Found(owner) = read.resolve_type(&format!("oracle.{name}")) else {
            panic!("{name}")
        };
        let (members, _) = read.declared_members(&owner).unwrap();
        let signature = &members
            .iter()
            .find(|member| member.name == "axis")
            .unwrap()
            .signature;
        let expected = format!("(Loracle/{name}$Axis;)Loracle/{name}$Axis;");
        assert!(
            matches!(signature, Knowledge::Found(signature) if signature.descriptor() == expected),
            "{name}: {signature:?}"
        );
        if name == "First" {
            assert!(
                matches!(
                    &members
                        .iter()
                        .find(|member| member.name == "rows")
                        .unwrap()
                        .signature,
                    Knowledge::Unknown(_)
                ),
                "an authored generic Array must not become a JVM array"
            );
        }
    }
}

#[test]
fn nominal_archive_jvm_actual_any_is_useful_and_common_expect_is_not_selected() {
    let (mut index, mut store) = fixture();
    let common = "/cache/stdlib/2.1.20/commonMain/kotlin/Any.kt";
    let actual = "/cache/stdlib/2.1.20/jvmMain/kotlin/Any.kt";
    for (file, text) in [
        (common, "package kotlin\npublic expect open class Any()\n"),
        (actual, "package kotlin\npublic actual open class Any {}\n"),
    ] {
        install_nominal_kotlin(&mut index, &mut store, file, text, "kotlin");
        index.set_file_origin(
            file,
            ktcore::project_model::FileOrigin {
                origin_id: "maven:stdlib:2.1.20".into(),
                representation: ktcore::project_model::SourceRepresentation::Source,
                project_scope: None,
            },
        );
    }
    install_nominal_kotlin(
        &mut index,
        &mut store,
        "/oracle/Mapped.kt",
        "package oracle\nclass Mapped { fun any(value: Any): Any = value }\n",
        "oracle",
    );
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    let Knowledge::Found(owner) = read.resolve_type("kotlin.Any") else {
        panic!("actual Any");
    };
    assert_eq!(owner.definition().file, actual);
    assert!(read.reference_type(&owner).unwrap().is_found());
    let Knowledge::Found(mapped) = read.resolve_type("oracle.Mapped") else {
        panic!("Mapped");
    };
    let member = read
        .declared_members(&mapped)
        .unwrap()
        .0
        .into_iter()
        .find(|member| member.name == "any")
        .unwrap();
    assert!(matches!(member.signature, Knowledge::Found(signature)
        if signature.descriptor() == "(Ljava/lang/Object;)Ljava/lang/Object;"));
}

#[test]
fn nominal_archive_source_view_narrowing_preserves_genuine_ambiguity() {
    for (other, different_origin, authority) in [
        ("/cache/one/jvmMain/oracle/Value.kt", true, true),
        ("/cache/two/jvmMain/oracle/Value.kt", false, true),
        ("/cache/one/jvmMain/oracle/Value.kt", false, false),
        ("/cache/one/jsMain/oracle/Value.kt", false, true),
    ] {
        let (mut index, mut store) = fixture();
        let common = "/cache/one/commonMain/oracle/Value.kt";
        for (file, text) in [
            (common, "package oracle\nexpect class Value\n"),
            (other, "package oracle\nactual class Value {}\n"),
        ] {
            install_nominal_kotlin(&mut index, &mut store, file, text, "oracle");
            if authority {
                index.set_file_origin(
                    file,
                    ktcore::project_model::FileOrigin {
                        origin_id: if file == other && different_origin {
                            "maven:other:1"
                        } else {
                            "maven:one:1"
                        }
                        .into(),
                        representation: ktcore::project_model::SourceRepresentation::Source,
                        project_scope: None,
                    },
                );
            }
        }
        let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
        assert!(
            matches!(read.resolve_type("oracle.Value"), Knowledge::Unknown(_)),
            "{other}"
        );
    }
}

#[test]
fn nominal_archive_narrowing_leaves_single_declarations_unchanged() {
    for text in [
        "package oracle\nclass Single {}\n",
        "package oracle\nexpect class Single()\n",
    ] {
        let (mut index, mut store) = fixture();
        let file = "/cache/only/commonMain/oracle/Single.kt";
        install_nominal_kotlin(&mut index, &mut store, file, text, "oracle");
        let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
        let Knowledge::Found(owner) = read.resolve_type("oracle.Single") else {
            panic!("single declaration");
        };
        assert_eq!(owner.definition().file, file);
    }
}

#[test]
fn nominal_archive_multiple_jvm_actual_declarations_stay_ambiguous() {
    let (mut index, mut store) = fixture();
    for (file, text) in [
        (
            "/cache/one/commonMain/oracle/Value.kt",
            "package oracle\nexpect class Value\n",
        ),
        (
            "/cache/one/jvmMain/oracle/Value.kt",
            "package oracle\nactual class Value {}\n",
        ),
        (
            "/cache/one/jvmMain/oracle/Other.kt",
            "package oracle\nactual class Value {}\n",
        ),
    ] {
        install_nominal_kotlin(&mut index, &mut store, file, text, "oracle");
        index.set_file_origin(
            file,
            ktcore::project_model::FileOrigin {
                origin_id: "maven:one:1".into(),
                representation: ktcore::project_model::SourceRepresentation::Source,
                project_scope: None,
            },
        );
    }
    let mut read = store.begin(&index, "mixin-consumer", MIXIN_FILE).unwrap();
    assert!(matches!(
        read.resolve_type("oracle.Value"),
        Knowledge::Unknown(_)
    ));
}
