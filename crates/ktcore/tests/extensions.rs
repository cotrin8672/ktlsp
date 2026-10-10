//! Public-API tests of the pure contribution foundation, not an installed Mixin provider or LSP.
//! Signatures/modifiers below are host facts established separately by compiler/runtime controls.

use std::ops::Range;
use std::sync::Arc;

use ktcore::diagnostics::{Diagnostic, Severity};
use ktcore::extensions::*;
use ktcore::index::{Index, Tier};
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
