//! Host lifecycle tests. Actual framework semantics and editor acceptance have independent oracles.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use ktcore::diagnostics::{Diagnostic, Severity};
use ktcore::extensions::*;
use ktlsp::workspace::Workspace;

const TARGET: &str = "package oracle\nclass Target { fun twice(value: Int): Int = value * 2 }";
const OWNER: &str = "package oracle; abstract class Owner { abstract int mirror(int value); }";

struct Consumer {
    calls: Arc<AtomicUsize>,
}
impl AnalysisProvider for Consumer {
    fn id(&self) -> &str {
        "test-consumer"
    }
    fn analyze(&self, read: &mut ReadSession<'_>) -> Result<(), ContractError> {
        let declarations = read.declarations().to_vec();
        let Some(authored) = declarations.iter().find(|declaration| {
            read.source()
                .get(declaration.definition.start_byte..declaration.definition.end_byte)
                .is_some_and(|name| name == "mirror" || name.starts_with("bridge$"))
        }) else {
            return Ok(());
        };
        self.calls.fetch_add(1, Ordering::SeqCst);
        let configuration = read.configuration().to_string();
        if configuration == "failure" {
            return Err(ContractError::InvalidName);
        }
        if configuration == "panic" {
            panic!("test consumer panic");
        }
        let target_name = if configuration == "{}" {
            "oracle.Target"
        } else {
            &configuration
        };
        let member = authored.member.as_ref().unwrap();
        let authored_name =
            &read.source()[authored.definition.start_byte..authored.definition.end_byte];
        let prefixed = authored_name.strip_prefix("bridge$").map(str::to_string);
        let names = match &prefixed {
            Some(name) if name != "twice" => MemberNames::Prefix(name.clone()),
            _ => MemberNames::Exact(vec!["twice".into()]),
        };
        let target = read.resolve_type(target_name);
        let declaration =
            read.declaration(authored.definition.start_byte..authored.definition.end_byte)?;
        let id = read.require(MemberRequirement {
            target,
            authored: Some(declaration),
            names,
            kind: MemberKind::Method,
            signature: member.signature.clone(),
            dispatch: member.dispatch.clone(),
            name_link: if prefixed.is_some() {
                NameLink::Prefixed("bridge$".into())
            } else {
                NameLink::IndependentAlias
            },
        })?;
        let status = if read.resolution(id)?.binding.is_found() {
            "bound"
        } else {
            "unknown"
        };
        read.bind_site(
            id,
            authored.definition.start_byte..authored.definition.end_byte,
            authored.definition.start_byte
                + if prefixed.is_some() {
                    "bridge$".len()
                } else {
                    0
                }..authored.definition.end_byte,
            if prefixed.is_some() {
                RenameSubject::Target
            } else {
                RenameSubject::Authored
            },
        )?;
        if prefixed.is_none() {
            if let Some(start) = read.source().find("\"twice\"").map(|start| start + 1) {
                read.bind_site(
                    id,
                    start..start + 5,
                    start..start + 5,
                    RenameSubject::Target,
                )?;
            }
        }
        read.diagnostic(Diagnostic {
            start_byte: authored.definition.start_byte,
            end_byte: authored.definition.end_byte,
            severity: Severity::Hint,
            code: None,
            message: format!("test-consumer:{status}"),
        })
    }
}

fn fixture() -> (Workspace, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Consumer {
        calls: calls.clone(),
    })]);
    ws.open("Owner.java", OWNER.into());
    (ws, calls)
}

fn messages(ws: &mut Workspace) -> Vec<String> {
    ws.diagnostics("Owner.java")
        .into_iter()
        .filter(|diagnostic| diagnostic.message.starts_with("test-consumer:"))
        .map(|diagnostic| diagnostic.message)
        .collect()
}

#[test]
fn accepted_source_and_negative_reads_update_the_same_provider_set() {
    let (mut ws, _) = fixture();
    assert_eq!(messages(&mut ws), ["test-consumer:unknown"]);
    ws.open("Target.kt", TARGET.into());
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    ws.change("Target.kt", TARGET.replace("value: Int", "value: String"));
    assert_eq!(messages(&mut ws), ["test-consumer:unknown"]);
    ws.change("Target.kt", TARGET.into());
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    ws.close("Target.kt");
    assert_eq!(messages(&mut ws), ["test-consumer:unknown"]);
}

#[test]
fn configuration_disable_failure_and_panic_remove_only_current_results() {
    let (mut ws, _) = fixture();
    ws.open("Target.kt", TARGET.into());
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    assert!(ws.configure_analysis_provider("test-consumer", false, Arc::from("{}")));
    assert!(messages(&mut ws).is_empty());
    assert!(ws.configure_analysis_provider("test-consumer", true, Arc::from("oracle.Missing")));
    assert_eq!(messages(&mut ws), ["test-consumer:unknown"]);
    ws.configure_analysis_provider("test-consumer", true, Arc::from("{}"));
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    for failure in ["failure", "panic"] {
        ws.configure_analysis_provider("test-consumer", true, Arc::from(failure));
        assert!(messages(&mut ws).is_empty());
        ws.configure_analysis_provider("test-consumer", true, Arc::from("{}"));
        assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    }
    assert!(!ws.configure_analysis_provider("unregistered", true, Arc::from("{}")));
}

#[test]
fn unrelated_body_edits_preserve_read_dependencies_and_refresh_suspends_old_results() {
    let (mut ws, calls) = fixture();
    ws.open("Target.kt", TARGET.into());
    ws.open(
        "Other.kt",
        "package oracle\nclass Other { fun value(): Int = 1 }".into(),
    );
    let before = calls.load(Ordering::SeqCst);
    ws.change(
        "Other.kt",
        "package oracle\nclass Other { fun value(): Int = 2 }".into(),
    );
    assert_eq!(calls.load(Ordering::SeqCst), before);
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    ws.begin_analysis_generation();
    assert!(messages(&mut ws).is_empty());
    ws.change("Target.kt", TARGET.replace("value * 2", "value * 3"));
    assert!(messages(&mut ws).is_empty());
}

#[test]
fn shared_relation_drives_goto_and_incomplete_name_completion_with_exact_subrange() {
    let (mut ws, _) = fixture();
    ws.open("Target.kt", TARGET.into());
    let expected = TARGET.find("twice").unwrap();
    assert_eq!(
        ws.goto_definition("Owner.java", OWNER.find("mirror").unwrap()),
        vec![ktcore::symbol::Def {
            file: "Target.kt".into(),
            start_byte: expected,
            end_byte: expected + 5
        }]
    );
    let partial = OWNER.replace("mirror", "bridge$tw");
    ws.change("Owner.java", partial.clone());
    let end = partial.find("bridge$tw").unwrap() + "bridge$tw".len();
    let completions = ws.complete("Owner.java", end, true).unwrap();
    assert_eq!(completions.items.len(), 1);
    let item = &completions.items[0];
    assert_eq!(item.label, "bridge$twice");
    assert_eq!(item.insert_text, "twice");
    let mut accepted = partial;
    accepted.replace_range(item.replace_range.clone().unwrap(), &item.insert_text);
    assert!(accepted.contains("int bridge$twice(int value)"));
    ws.change("Owner.java", accepted.clone());
    assert_eq!(
        ws.goto_definition("Owner.java", accepted.find("bridge$twice").unwrap())[0].file,
        "Target.kt"
    );
    ws.configure_analysis_provider("test-consumer", false, Arc::from("{}"));
    assert_eq!(
        ws.goto_definition("Owner.java", accepted.find("bridge$twice").unwrap())[0].file,
        "Owner.java"
    );
}

#[test]
fn kotlin_completion_preserves_provider_relations_declared_after_the_cursor() {
    use ktlsp::index::{Index, InferenceIndex, LookupEntries};
    use std::cell::Cell;
    struct BindingReads<'a> {
        index: &'a Index,
        target_reads: Cell<usize>,
    }
    impl InferenceIndex for BindingReads<'_> {
        fn lookup_by_name(&self, name: &str) -> LookupEntries<'_> {
            if name == "twice" {
                self.target_reads.set(self.target_reads.get() + 1);
            }
            InferenceIndex::lookup_by_name(self.index, name)
        }
        fn members_of(&self, name: &str) -> LookupEntries<'_> {
            InferenceIndex::members_of(self.index, name)
        }
        fn extensions_for(&self, name: &str) -> LookupEntries<'_> {
            InferenceIndex::extensions_for(self.index, name)
        }
        fn contributions_for_file(&self, file: &str) -> &[ContributionSet] {
            InferenceIndex::contributions_for_file(self.index, file)
        }
    }
    let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Consumer {
        calls: Arc::new(AtomicUsize::new(0)),
    })]);
    let source = "package oracle\n// 補完\nclass Owner {\n    fun use() = this.mirror(3).toString()\n    fun mirror(value: Int): Int = value\n}\n";
    ws.open("Target.kt", TARGET.into());
    ws.open("Owner.kt", source.into());
    let expected = ktcore::symbol::Def {
        file: "Target.kt".into(),
        start_byte: TARGET.find("twice").unwrap(),
        end_byte: TARGET.find("twice").unwrap() + "twice".len(),
    };
    assert_eq!(
        ws.goto_definition("Owner.kt", source.rfind("mirror").unwrap()),
        vec![expected]
    );
    let offset = source.find(".toString").unwrap() + ".to".len();
    for enabled in [true, false] {
        ws.configure_analysis_provider("test-consumer", enabled, Arc::from("{}"));
        let index = BindingReads {
            index: &ws.index,
            target_reads: Cell::new(0),
        };
        let query = ktlsp::semantic_query::after_dot_query(
            &index,
            &mut ktlsp::parser::KotlinParser::new(),
            "Owner.kt",
            source,
            offset,
            1000,
        );
        assert!(query.is_some());
        assert_eq!(
            index.target_reads.get() > 0,
            enabled,
            "receiver inference must follow the authored declaration's accepted target"
        );
    }
}

fn apply(ws: &Workspace, file: &str, edits: &[ktlsp::edit::TextEdit]) -> String {
    ktlsp::edit::apply_to_text(file, &ws.doc_text(file).unwrap(), edits).unwrap()
}

#[test]
fn shared_references_and_applied_target_rename_preserve_prefix_at_declaration_and_calls() {
    let (mut ws, _) = fixture();
    ws.open("Target.kt", TARGET.into());
    let owner = "package oracle; abstract class Owner { abstract int bridge$twice(int value); int use() { return bridge$twice(3); } }";
    let caller = "package oracle\nfun call(target: Target): Int = target.twice(2)";
    ws.change("Owner.java", owner.into());
    ws.open("Caller.kt", caller.into());
    let offset = owner.find("bridge$twice").unwrap();
    let refs = ws.references("Owner.java", offset, true);
    assert_eq!(refs.len(), 4, "{refs:?}");
    assert_eq!(ws.references("Owner.java", offset, false).len(), 2);
    let prepared = ws.prepare_rename("Owner.java", offset).unwrap();
    assert_eq!(prepared.placeholder, "twice");
    assert_eq!(
        &owner[prepared.range.start_byte..prepared.range.end_byte],
        "twice"
    );
    let edits = ws.rename("Owner.java", offset, "doubleValue").unwrap();
    assert_eq!(edits.len(), 4, "{edits:?}");
    let owner_after = apply(&ws, "Owner.java", &edits);
    assert_eq!(
        owner_after,
        owner.replace("bridge$twice", "bridge$doubleValue")
    );
    assert_eq!(
        apply(&ws, "Target.kt", &edits),
        TARGET.replace("twice", "doubleValue")
    );
    assert_eq!(
        apply(&ws, "Caller.kt", &edits),
        caller.replace("twice", "doubleValue")
    );
    ws.change("Owner.java", owner_after);
    ws.change("Target.kt", apply(&ws, "Target.kt", &edits));
    ws.change("Caller.kt", apply(&ws, "Caller.kt", &edits));
    // The fixture provider recognizes only the independently expected old name, so its old
    // exact target binding must disappear after the edits are accepted.
    assert_eq!(messages(&mut ws), ["test-consumer:unknown"]);
}

#[test]
fn independent_alias_rename_keeps_authored_and_target_names_distinct() {
    let (mut ws, _) = fixture();
    ws.open("Target.kt", TARGET.into());
    let owner = "package oracle; abstract class Owner { static final String LINK = \"twice\"; abstract int mirror(int value); int use() { return mirror(3); } }";
    ws.change("Owner.java", owner.into());
    let alias_offset = owner.find("\"twice\"").unwrap() + 1;
    assert_eq!(
        ws.goto_definition("Owner.java", alias_offset)[0].file,
        "Target.kt"
    );
    assert_eq!(ws.references("Owner.java", alias_offset, true).len(), 4);
    let target_edits = ws
        .rename("Owner.java", alias_offset, "doubleValue")
        .unwrap();
    assert_eq!(target_edits.len(), 2, "{target_edits:?}");
    assert_eq!(
        apply(&ws, "Owner.java", &target_edits),
        owner.replace("\"twice\"", "\"doubleValue\"")
    );
    assert_eq!(
        apply(&ws, "Target.kt", &target_edits),
        TARGET.replace("twice", "doubleValue")
    );
    for offset in [
        owner.find("mirror").unwrap(),
        owner.rfind("mirror").unwrap(),
    ] {
        let authored_edits = ws.rename("Owner.java", offset, "surface").unwrap();
        assert_eq!(authored_edits.len(), 2, "{authored_edits:?}");
        assert_eq!(
            apply(&ws, "Owner.java", &authored_edits),
            owner.replace("mirror", "surface")
        );
        assert_eq!(apply(&ws, "Target.kt", &authored_edits), TARGET);
    }
}

#[test]
fn failed_or_disabled_provider_does_not_remove_another_providers_diagnostics() {
    struct Observer;
    impl AnalysisProvider for Observer {
        fn id(&self) -> &str {
            "z-observer"
        }
        fn analyze(&self, read: &mut ReadSession<'_>) -> Result<(), ContractError> {
            read.diagnostic(Diagnostic {
                start_byte: 0,
                end_byte: 1,
                severity: Severity::Hint,
                code: None,
                message: "observer:current".into(),
            })
        }
    }
    let mut ws = Workspace::with_analysis_providers(vec![
        Arc::new(Consumer {
            calls: Arc::new(AtomicUsize::new(0)),
        }),
        Arc::new(Observer),
    ]);
    ws.open("Owner.java", OWNER.into());
    ws.open("Target.kt", TARGET.into());
    for (enabled, configuration) in [(false, "{}"), (true, "failure"), (true, "panic")] {
        ws.configure_analysis_provider("test-consumer", enabled, Arc::from(configuration));
        assert!(messages(&mut ws).is_empty());
        assert!(ws
            .diagnostics("Owner.java")
            .iter()
            .any(|diagnostic| diagnostic.message == "observer:current"));
    }
    ws.configure_analysis_provider("test-consumer", true, Arc::from("{}"));
    ws.configure_analysis_provider("z-observer", false, Arc::from("{}"));
    assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
    assert!(ws
        .diagnostics("Owner.java")
        .iter()
        .all(|diagnostic| diagnostic.message != "observer:current"));
}

#[test]
fn target_rename_excludes_an_ambiguous_overload_usage_and_the_other_known_overload() {
    let (mut ws, _) = fixture();
    let target = "package oracle\nclass Target { fun twice(value: Int): Int = value * 2\nfun twice(value: String): String = value }";
    let owner = "package oracle; abstract class Owner { abstract int bridge$twice(int value); int use() { return bridge$twice(3); } }";
    let caller = "package oracle; class Caller { Object unknown(Target target) { return target.twice(missing); } String known(Target target) { return target.twice(\"x\"); } }";
    ws.open("Target.kt", target.into());
    ws.change("Owner.java", owner.into());
    ws.open("Caller.java", caller.into());
    let candidates = ws.goto_definition("Caller.java", caller.find("twice").unwrap());
    assert_eq!(
        candidates.len(),
        2,
        "ambiguous navigation candidates must not become an exact reference: {candidates:?}"
    );
    let edits = ws
        .rename(
            "Owner.java",
            owner.find("bridge$twice").unwrap(),
            "doubleValue",
        )
        .unwrap();
    assert_eq!(edits.len(), 3, "{edits:?}");
    assert_eq!(apply(&ws, "Caller.java", &edits), caller);
    assert_eq!(
        apply(&ws, "Owner.java", &edits),
        owner.replace("bridge$twice", "bridge$doubleValue")
    );
    assert_eq!(
        apply(&ws, "Target.kt", &edits),
        target.replacen("fun twice(value: Int)", "fun doubleValue(value: Int)", 1)
    );
}

#[test]
fn skipped_late_dependency_batch_preserves_open_source_bindings_and_provider_diagnostics() {
    for (file, source) in [("Target.kt", TARGET), ("Target.java", "package oracle; public class Target { public int twice(int value) { return value * 2; } }")] {
        let (mut ws, calls) = fixture();
        ws.open(file, source.into());
        let expected = ws.goto_definition("Owner.java", OWNER.find("mirror").unwrap());
        assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
        let language = ktlsp::language::SourceLanguage::for_key(file);
        let mut parsers = ktlsp::language::LanguageParsers::new();
        let stale = source.replace("twice", "oldName");
        let tree = parsers.parse(language, &stale);
        let facts = ktlsp::language::file_facts(language, &tree, &stale);
        let before = calls.load(Ordering::SeqCst);
        assert!(!ws.index_dependency_file(file, facts.symbols));
        assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
        assert_eq!(ws.goto_definition("Owner.java", OWNER.find("mirror").unwrap()), expected);
        assert_eq!(calls.load(Ordering::SeqCst), before);
        ws.bump_index_revision();
        assert_eq!(messages(&mut ws), ["test-consumer:bound"]);
        assert_eq!(calls.load(Ordering::SeqCst), before);
    }
}

#[test]
fn late_open_kotlin_dependency_ownership_refreshes_provider_results() {
    late_open_dependency_ownership("Target.kt", TARGET);
}

#[test]
fn late_open_java_dependency_ownership_refreshes_provider_results() {
    late_open_dependency_ownership(
        "Target.java",
        "package oracle; public class Target { public int twice(int value) { return value * 2; } }",
    );
}

fn late_open_dependency_ownership(file: &str, source: &str) {
    use ktlsp::project_model::{FileOrigin, SourceRepresentation};
    for change in ["both", "file", "scope", "maven"] {
        let root = std::env::temp_dir().join("ktlsp-extension-late-ownership");
        let source_root = root.join("src");
        let owner_key = ktlsp::paths::file_key(&source_root.join("Owner.java"));
        let target_key = ktlsp::paths::file_key(&root.join("cache").join(file));
        let calls = Arc::new(AtomicUsize::new(0));
        let mut ws = Workspace::with_analysis_providers(vec![Arc::new(Consumer {
            calls: calls.clone(),
        })]);
        let jar = root.join("api.jar");
        let local = root.join("local.jar");
        let dump = format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tvendor\tapi\t1\nSCP\tmain\t{}\nEND\t:app\n",
                root.display(), source_root.display(), jar.display(), jar.display(), local.display()
            );
        let accepted_model = Arc::new(ktlsp::classpath::ProjectSnapshot::new(
            ktlsp::classpath::parse_dump(&dump),
        ));
        let initial_model = if change == "scope" {
            Arc::new(ktlsp::classpath::ProjectSnapshot::new(
                ktlsp::classpath::parse_dump(
                    &dump.replace(&format!("SCP\tmain\t{}\n", local.display()), ""),
                ),
            ))
        } else {
            accepted_model.clone()
        };
        ws.set_project_model(initial_model);
        let stale_generation = ws.analysis_generation();
        ws.begin_analysis_generation();
        ws.scan_model_sources();
        let origin = FileOrigin {
            origin_id: if change == "maven" {
                "maven:vendor:api:1".to_string()
            } else {
                accepted_model.artifact_origin(&local).unwrap().to_string()
            },
            representation: SourceRepresentation::Source,
            project_scope: None,
        };
        if change == "file" {
            assert!(ws.index_dependency_batch(ktlsp::deps::FileSymbols {
                jvm: None,
                file: ktlsp::paths::file_key(&root.join("cache/Seed.java")),
                symbols: Vec::new(),
                library_source: None,
                origin: Some(origin.clone()),
            }));
            ws.bump_index_revision();
        } else if change == "scope" {
            assert!(ws.index.set_file_origin(&target_key, origin.clone()));
        }
        let owner = OWNER.replace("abstract int mirror", "Target target; abstract int mirror");
        ws.open(owner_key.clone(), owner.clone());
        ws.open(target_key.clone(), source.into());
        let status = |ws: &mut Workspace| {
            ws.diagnostics(&owner_key)
                .into_iter()
                .filter(|diagnostic| diagnostic.message.starts_with("test-consumer:"))
                .map(|diagnostic| diagnostic.message)
                .collect::<Vec<_>>()
        };
        assert_eq!(status(&mut ws), ["test-consumer:unknown"]);
        assert!(ws
            .goto_definition(&owner_key, owner.find("Target target").unwrap())
            .is_empty());
        let before = calls.load(Ordering::SeqCst);
        let language = ktlsp::language::SourceLanguage::for_key(file);
        let stale = source.replace("twice", "oldName");
        let mut parsers = ktlsp::language::LanguageParsers::new();
        let tree = parsers.parse(language, &stale);
        let symbols = ktlsp::language::file_facts(language, &tree, &stale).symbols;
        let batch = || ktlsp::deps::FileSymbols {
            jvm: None,
            file: target_key.clone(),
            symbols: symbols.clone(),
            library_source: None,
            origin: Some(origin.clone()),
        };
        let owner_scope = ws
            .index
            .file_origin(&owner_key)
            .unwrap()
            .project_scope
            .clone()
            .unwrap();
        let visibility = ws.index.scope_visibility(&owner_scope).cloned();
        let old_origin = ws.index.file_origin(&target_key).cloned();
        let generation = ws.analysis_generation();
        assert!(!ws.index_dependency_batch_for_generation(stale_generation, batch()));
        ws.bump_index_revision();
        assert_eq!(ws.index.file_origin(&target_key), old_origin.as_ref());
        assert_eq!(ws.index.scope_visibility(&owner_scope), visibility.as_ref());
        assert_eq!(status(&mut ws), ["test-consumer:unknown"]);
        assert_eq!(calls.load(Ordering::SeqCst), before);
        if change == "scope" {
            ws.set_project_model(accepted_model.clone());
            assert!(!ws.index_dependency_batch_for_generation(generation, batch()));
            assert_eq!(ws.doc_text(&target_key).as_deref(), Some(source));
            assert_eq!(ws.index.file_origin(&target_key), old_origin.as_ref());
        }
        let generation = ws.analysis_generation();
        assert!(!ws.index_dependency_batch_for_generation(generation, batch()));
        ws.bump_index_revision();
        assert_eq!(ws.doc_text(&target_key).as_deref(), Some(source));
        assert_eq!(ws.index.file_origin(&target_key), Some(&origin));
        assert_eq!(
            ws.goto_definition(&owner_key, owner.find("Target target").unwrap())
                .len(),
            1
        );
        assert_eq!(status(&mut ws), ["test-consumer:bound"], "{file} {change}");
        let definition = ws.goto_definition(&owner_key, owner.find("mirror").unwrap());
        assert_eq!(definition[0].file, target_key);
        assert_eq!(definition[0].start_byte, source.find("twice").unwrap());
        assert!(calls.load(Ordering::SeqCst) > before);
        let before_repeat = calls.load(Ordering::SeqCst);
        assert!(!ws.index_dependency_batch(batch()));
        ws.bump_index_revision();
        assert_eq!(status(&mut ws), ["test-consumer:bound"]);
        assert_eq!(calls.load(Ordering::SeqCst), before_repeat);
        assert_eq!(
            ws.goto_definition(&owner_key, owner.find("mirror").unwrap()),
            definition
        );
    }
}
