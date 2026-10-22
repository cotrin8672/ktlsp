//! Salsa-backed per-file queries.
//!
//! The mutable `Workspace` still owns the cross-file `Index` and open-buffer incremental trees.
//! Salsa owns pure, file-local derivations from source text, plus an immutable inference index
//! snapshot. Closed-file requests and explicit reindexing can reuse parsed trees and
//! declaration/usage facts across repeated queries without letting Salsa observe the workspace's
//! mutable index mid-update.

use std::collections::HashMap;

use salsa::{Durability, Setter};
use tree_sitter::{Node, Tree};

use crate::index::{Index, InferenceIndex, InferenceIndexSnapshot, ScopedIndex};
use crate::infer;
use crate::language::{self, FileFacts, SourceLanguage};
use crate::parser;
use crate::types::Type;

#[salsa::db]
trait KtlspDb: salsa::Database {
    fn index(&self) -> &InferenceIndexSnapshot;
    fn source(&self, key: &str) -> Option<SourceFile>;
}

#[salsa::db]
#[derive(Clone)]
struct KtlspDatabase {
    storage: salsa::Storage<Self>,
    index: InferenceIndexSnapshot,
    sources: HashMap<String, SourceFile>,
    source_stamp: Option<IndexStamp>,
}

impl Default for KtlspDatabase {
    fn default() -> Self {
        let mut db = KtlspDatabase {
            storage: salsa::Storage::new(None),
            index: InferenceIndexSnapshot::default(),
            sources: HashMap::new(),
            source_stamp: None,
        };
        db.source_stamp = Some(IndexStamp::new(&db, 0));
        db
    }
}

impl KtlspDatabase {
    fn set_index(&mut self, index: &Index) {
        self.index = index.inference_snapshot();
    }
}

#[salsa::db]
impl salsa::Database for KtlspDatabase {}

#[salsa::db]
impl KtlspDb for KtlspDatabase {
    fn index(&self) -> &InferenceIndexSnapshot {
        &self.index
    }
    fn source(&self, key: &str) -> Option<SourceFile> {
        let _ = self.source_stamp.unwrap().revision(self);
        self.sources.get(key).copied()
    }
}

#[salsa::input(debug)]
pub struct SourceFile {
    #[returns(ref)]
    key: String,
    #[returns(ref)]
    scope_key: String,
    #[returns(copy)]
    language: SourceLanguage,
    #[returns(ref)]
    text: String,
}

#[salsa::input(debug)]
pub struct IndexStamp {
    #[returns(copy)]
    revision: u64,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct ExprKey {
    pub start_byte: usize,
    pub end_byte: usize,
    pub kind_id: u16,
}

impl ExprKey {
    pub fn from_node(node: Node<'_>) -> Self {
        ExprKey {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            kind_id: node.kind_id(),
        }
    }

    fn matches(self, node: Node<'_>) -> bool {
        self.start_byte == node.start_byte()
            && self.end_byte == node.end_byte()
            && self.kind_id == node.kind_id()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCtxFacts {
    pub package: String,
    pub imports: Vec<parser::Import>,
    pub local_extensions: Option<std::sync::Arc<Vec<crate::indexer::LocalExtension>>>,
}

#[salsa::tracked(returns(ref), no_eq)]
pub fn parsed_tree(db: &dyn salsa::Database, file: SourceFile) -> Tree {
    let mut parsers = language::LanguageParsers::new();
    parsers.parse(file.language(db), file.text(db))
}

#[salsa::tracked(returns(ref))]
pub fn source_file_facts(db: &dyn salsa::Database, file: SourceFile) -> FileFacts {
    language::file_facts(file.language(db), parsed_tree(db, file), file.text(db))
}

#[salsa::tracked(returns(ref))]
pub fn file_ctx_facts(db: &dyn salsa::Database, file: SourceFile) -> FileCtxFacts {
    let tree = parsed_tree(db, file);
    let text = file.text(db);
    // TODO(language-facade): FileCtxFacts is intentionally still Kotlin-shaped because expression
    // inference consumes ktcore::parser::Import. Making this fully language-neutral requires a
    // shared import fact type first; do not feed Java imports into Kotlin inference as a shortcut.
    let ctx = infer::FileCtx::from_tree_in_file(tree, text, file.scope_key(db));
    FileCtxFacts {
        package: ctx.package,
        imports: ctx.imports,
        local_extensions: ctx.local_extensions,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct InferredType {
    pub value: Type,
    pub missing_sources: std::collections::BTreeSet<String>,
}

impl InferredType {
    fn known(value: Type) -> Self {
        Self {
            value,
            missing_sources: Default::default(),
        }
    }
    fn unknown() -> Self {
        Self::known(Type::Unknown)
    }
    pub fn into_type(self) -> Option<Type> {
        (!matches!(self.value, Type::Unknown)).then_some(self.value)
    }
    fn consume(self, missing: &std::cell::RefCell<std::collections::BTreeSet<String>>) -> Type {
        missing.borrow_mut().extend(self.missing_sources);
        self.value
    }
}

#[salsa::tracked(returns(clone))]
fn expr_type(db: &dyn KtlspDb, file: SourceFile, index: IndexStamp, key: ExprKey) -> InferredType {
    expr_type_at_depth(db, file, index, key, 0)
}

#[salsa::tracked(returns(clone))]
fn expr_type_at_depth(
    db: &dyn KtlspDb,
    file: SourceFile,
    index: IndexStamp,
    key: ExprKey,
    depth: u8,
) -> InferredType {
    let _ = index.revision(db);
    if usize::from(depth) > infer::MAX_DEPTH {
        return InferredType::unknown();
    }
    let tree = parsed_tree(db, file);
    let Some(node) = find_expr_node(tree.root_node(), key) else {
        return InferredType::unknown();
    };
    let ctx = infer::FileCtx::from_tree_in_file(tree, file.text(db), file.scope_key(db));
    let missing = std::cell::RefCell::new(Default::default());
    let declaration = |entry: &crate::index::Entry| {
        let value = source_declaration(db, index, entry, depth + 1).consume(&missing);
        (!matches!(value, Type::Unknown)).then_some(value)
    };
    let scoped_index =
        ScopedIndex::new(db.index(), file.scope_key(db)).with_declaration_types(&declaration);
    let mut child_infer = SalsaChildInfer {
        db,
        file,
        index,
        missing: &missing,
    };
    let value = infer::infer_with_child_infer(
        &scoped_index,
        node,
        file.text(db),
        &ctx,
        usize::from(depth),
        &mut child_infer,
    );
    InferredType {
        value,
        missing_sources: missing.into_inner(),
    }
}

fn source_declaration(
    db: &dyn KtlspDb,
    index: IndexStamp,
    entry: &crate::index::Entry,
    depth: u8,
) -> InferredType {
    let _ = index.revision(db);
    if usize::from(depth) > infer::MAX_DEPTH {
        return InferredType::unknown();
    }
    let Some(file) = db.source(&entry.path) else {
        return InferredType {
            value: Type::Unknown,
            missing_sources: [entry.path.to_string()].into_iter().collect(),
        };
    };
    let facts = source_file_facts(db, file);
    // An input registered for an older index/source revision cannot certify this declaration.
    if !facts
        .symbols
        .iter()
        .any(|symbol| symbol == entry.sym.as_ref())
    {
        return InferredType::unknown();
    }
    if let Some(owner) = entry.sym.container.as_deref() {
        let accepted = db
            .index()
            .jvm_sources(owner)
            .into_iter()
            .find(|source| source.file == entry.path.as_ref());
        if let Some(accepted) = accepted {
            if facts
                .jvm
                .as_ref()
                .is_none_or(|current| current.input_hash != accepted.facts.input_hash)
            {
                return InferredType::unknown();
            }
        }
    }
    let tree = parsed_tree(db, file);
    let Some(node) = tree
        .root_node()
        .descendant_for_byte_range(entry.sym.start_byte, entry.sym.end_byte)
    else {
        return InferredType::unknown();
    };
    if node.kind() != "identifier"
        || node.byte_range() != (entry.sym.start_byte..entry.sym.end_byte)
    {
        return InferredType::unknown();
    }
    declaration_type_at_depth(db, file, index, ExprKey::from_node(node), depth)
}

#[salsa::tracked(returns(clone))]
fn declaration_type_at_depth(
    db: &dyn KtlspDb,
    file: SourceFile,
    index: IndexStamp,
    key: ExprKey,
    depth: u8,
) -> InferredType {
    let _ = index.revision(db);
    if usize::from(depth) > infer::MAX_DEPTH {
        return InferredType::unknown();
    }
    let Some(node) = find_expr_node(parsed_tree(db, file).root_node(), key) else {
        return InferredType::unknown();
    };
    let ctx =
        infer::FileCtx::from_tree_in_file(parsed_tree(db, file), file.text(db), file.scope_key(db));
    let missing = std::cell::RefCell::new(Default::default());
    let declaration = |entry: &crate::index::Entry| {
        let value = source_declaration(db, index, entry, depth + 1).consume(&missing);
        (!matches!(value, Type::Unknown)).then_some(value)
    };
    let scoped =
        ScopedIndex::new(db.index(), file.scope_key(db)).with_declaration_types(&declaration);
    let mut children = SalsaChildInfer {
        db,
        file,
        index,
        missing: &missing,
    };
    let value = infer::decl_type(
        &scoped,
        node,
        file.text(db),
        &ctx,
        usize::from(depth),
        &mut children,
    );
    InferredType {
        value,
        missing_sources: missing.into_inner(),
    }
}

struct SalsaChildInfer<'db> {
    db: &'db dyn KtlspDb,
    file: SourceFile,
    index: IndexStamp,
    missing: &'db std::cell::RefCell<std::collections::BTreeSet<String>>,
}

impl infer::ChildInfer for SalsaChildInfer<'_> {
    fn infer_child(
        &mut self,
        index: &dyn InferenceIndex,
        node: Node<'_>,
        src: &str,
        ctx: &infer::FileCtx,
        depth: usize,
    ) -> Type {
        if depth > infer::MAX_DEPTH {
            return Type::Unknown;
        }
        if supports_compositional_expr(node) {
            return expr_type_at_depth(
                self.db,
                self.file,
                self.index,
                ExprKey::from_node(node),
                depth.min(u8::MAX as usize) as u8,
            )
            .consume(self.missing);
        }
        infer::infer_with_child_infer(index, node, src, ctx, depth, self)
    }
}

fn supports_compositional_expr(node: Node<'_>) -> bool {
    match node.kind() {
        "string_literal" | "character_literal" | "number_literal" | "float_literal"
        | "boolean_literal" | "identifier" => true,
        "navigation_expression" => node.named_child_count() >= 2,
        "call_expression" => {
            !has_named_child_kind(node, "annotated_lambda")
                && node.named_child(0).is_some_and(|callee| {
                    matches!(callee.kind(), "identifier" | "navigation_expression")
                })
        }
        _ => false,
    }
}

fn has_named_child_kind(node: Node<'_>, kind: &str) -> bool {
    let mut cursor = node.walk();
    let found = node
        .named_children(&mut cursor)
        .any(|child| child.kind() == kind);
    found
}

fn find_expr_node(root: Node<'_>, key: ExprKey) -> Option<Node<'_>> {
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        if key.matches(node) {
            return Some(node);
        }
        if node.start_byte() > key.start_byte || node.end_byte() < key.end_byte {
            continue;
        }
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() <= key.start_byte && child.end_byte() >= key.end_byte {
                stack.push(child);
            }
        }
    }
    None
}

pub struct SalsaState {
    db: KtlspDatabase,
    index: IndexStamp,
    indexed_revision: u64,
}

impl Default for SalsaState {
    fn default() -> Self {
        let db = KtlspDatabase::default();
        let index = IndexStamp::new(&db, 0);
        SalsaState {
            db,
            index,
            indexed_revision: 0,
        }
    }
}

impl SalsaState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn forget(&mut self, key: &str) {
        if self.db.sources.remove(key).is_some() {
            self.sources_changed();
        }
    }

    pub fn record_text(&mut self, key: &str, text: &str, durability: Durability) {
        self.upsert_file(key, text, durability);
    }

    pub fn prepare_declaration(&mut self, key: &str, text: &str, index: &Index, revision: u64) {
        self.sync_index(index, revision);
        self.upsert_file(key, text, Durability::LOW);
    }

    pub fn declaration_type(&self, entry: &crate::index::Entry) -> Option<Type> {
        self.declaration_query(entry).into_type()
    }

    pub fn declaration_query(&self, entry: &crate::index::Entry) -> InferredType {
        source_declaration(&self.db, self.index, entry, 0)
    }

    pub fn parsed_tree(&mut self, key: &str, text: &str, durability: Durability) -> Tree {
        let file = self.upsert_file(key, text, durability);
        parsed_tree(&self.db, file).clone()
    }

    pub fn file_facts(&mut self, key: &str, text: &str, durability: Durability) -> FileFacts {
        let file = self.upsert_file(key, text, durability);
        source_file_facts(&self.db, file).clone()
    }

    pub fn expr_type(
        &mut self,
        key: &str,
        text: &str,
        durability: Durability,
        index: &Index,
        index_revision: u64,
        expr: ExprKey,
    ) -> Type {
        self.expr_type_in_file(key, key, text, durability, index, index_revision, expr)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn expr_type_in_file(
        &mut self,
        key: &str,
        scope_key: &str,
        text: &str,
        durability: Durability,
        index: &Index,
        index_revision: u64,
        expr: ExprKey,
    ) -> Type {
        self.expr_query_in_file(
            key,
            scope_key,
            text,
            durability,
            index,
            index_revision,
            expr,
        )
        .value
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn expr_query_in_file(
        &mut self,
        key: &str,
        scope_key: &str,
        text: &str,
        durability: Durability,
        index: &Index,
        index_revision: u64,
        expr: ExprKey,
    ) -> InferredType {
        self.sync_index(index, index_revision);
        let file = self.upsert_file(key, text, durability);
        if file.scope_key(&self.db) != scope_key {
            file.set_scope_key(&mut self.db)
                .with_durability(durability)
                .to(scope_key.to_string());
        }
        expr_type(&self.db, file, self.index, expr)
    }

    fn sync_index(&mut self, index: &Index, revision: u64) {
        if self.indexed_revision == revision {
            return;
        }
        let started = crate::trace::enabled().then(std::time::Instant::now);
        self.db.set_index(index);
        if let Some(started) = started {
            crate::trace::span(
                "inference_snapshot",
                "index",
                started,
                serde_json::json!({ "revision": revision }),
            );
        }
        self.index
            .set_revision(&mut self.db)
            .with_durability(Durability::LOW)
            .to(revision);
        self.indexed_revision = revision;
    }

    fn upsert_file(&mut self, key: &str, text: &str, durability: Durability) -> SourceFile {
        if let Some(file) = self.db.sources.get(key).copied() {
            if file.text(&self.db) != text {
                file.set_text(&mut self.db)
                    .with_durability(durability)
                    .to(text.to_string());
            }
            return file;
        }

        let file = SourceFile::builder(
            key.to_string(),
            key.to_string(),
            SourceLanguage::for_key(key),
            text.to_string(),
        )
        .durability(durability)
        .new(&self.db);
        self.db.sources.insert(key.to_string(), file);
        self.sources_changed();
        file
    }

    fn sources_changed(&mut self) {
        let stamp = self.db.source_stamp.unwrap();
        let revision = stamp.revision(&self.db).wrapping_add(1);
        stamp.set_revision(&mut self.db).to(revision);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inferred_noncompositional_children_keep_the_callers_depth_budget() {
        use infer::ChildInfer;
        let text = "package probe\nval value = (\"x\")\n";
        let mut state = SalsaState::new();
        let file = state.upsert_file("Depth.kt", text, Durability::LOW);
        let tree = parsed_tree(&state.db, file);
        let start = text.find("(\"x\")").unwrap();
        let node = tree
            .root_node()
            .descendant_for_byte_range(start, start + 5)
            .unwrap();
        assert!(!supports_compositional_expr(node), "{}", node.kind());
        let missing = Default::default();
        let mut child = SalsaChildInfer {
            db: &state.db,
            file,
            index: state.index,
            missing: &missing,
        };
        let index = Index::new();
        let ctx = infer::FileCtx::from_tree_in_file(tree, text, "Depth.kt");
        assert_eq!(
            child.infer_child(&index, node, text, &ctx, infer::MAX_DEPTH),
            Type::Unknown
        );
        // A fresh context avoids reusing the deliberately exhausted local expression cache.
        let ctx = infer::FileCtx::from_tree_in_file(tree, text, "Depth.kt");
        assert_eq!(
            child.infer_child(&index, node, text, &ctx, 0),
            Type::class("String", Some("kotlin".into()))
        );
    }

    #[test]
    fn inferred_declaration_inputs_track_late_registration_initializer_edits_and_cycles() {
        let mut state = SalsaState::new();
        let mut index = Index::new();
        let a = "package probe\nclass A { var value = B().value }\n";
        let b = "package probe\nclass B { val value = \"x\" }\n";
        for (file, text) in [("A.kt", a), ("B.kt", b)] {
            let tree = parser::KotlinParser::new().parse(text);
            let symbols = crate::indexer::extract_symbols(&tree, text, "probe");
            let jvm = ktcore::jvm::source_facts(file, &tree, text, &symbols);
            index.replace_file(file, symbols, crate::index::Tier::Volatile);
            index.replace_jvm_source(file, jvm, crate::index::Tier::Volatile);
        }
        let entry = index
            .lookup_by_name("value")
            .into_iter()
            .find(|e| e.path.as_ref() == "A.kt")
            .unwrap()
            .clone();
        state.prepare_declaration("A.kt", a, &index, 1);
        assert_eq!(state.declaration_type(&entry), None);
        state.prepare_declaration("B.kt", b, &index, 1);
        assert_eq!(
            state.declaration_type(&entry),
            Some(Type::class("String", Some("kotlin".into())))
        );
        {
            use ktcore::{jvm, members};
            let query = |entry: &crate::index::Entry| state.declaration_type(entry);
            let scoped = index
                .scoped_for_file("Use.java")
                .with_declaration_types(&query);
            let receiver = Type::class("A", Some("probe".into()));
            let ctx = infer::FileCtx::new("probe".into(), Vec::new());
            for (name, expected) in [
                ("String", members::Applicability::Applicable),
                ("Int", members::Applicability::Rejected),
            ] {
                let selected = jvm::select_call(
                    &scoped,
                    &receiver,
                    jvm::member_candidates(&scoped, &receiver, "setValue"),
                    "setValue",
                    &members::CallArguments {
                        count: Some(1),
                        types: vec![Type::class(name, Some("kotlin".into()))],
                        names: None,
                        has_spread: false,
                        trailing_lambda: false,
                    },
                    &ctx,
                );
                assert_eq!(selected.applicability, expected);
                let [setter] = selected.consumable_candidates() else {
                    panic!("one known setter")
                };
                assert_eq!(setter.arity(), Some(1));
                assert_eq!(
                    setter.identity(&scoped).unwrap().descriptor,
                    "(Ljava/lang/String;)V"
                );
                assert_eq!(
                    jvm::projected_return_type_in_context(&scoped, &receiver, *setter, &ctx),
                    Some(Type::class("Unit", Some("kotlin".into())))
                );
            }
        }
        // Equal symbol facts/byte positions cannot certify text from another accepted input.
        let changed = b.replace("\"x\"", "123");
        state.prepare_declaration("B.kt", &changed, &index, 1);
        assert_eq!(state.declaration_type(&entry), None);
        let tree = parser::KotlinParser::new().parse(&changed);
        let symbols = crate::indexer::extract_symbols(&tree, &changed, "probe");
        index.replace_jvm_source(
            "B.kt",
            ktcore::jvm::source_facts("B.kt", &tree, &changed, &symbols),
            crate::index::Tier::Volatile,
        );
        state.prepare_declaration("B.kt", &changed, &index, 2);
        assert_eq!(
            state.declaration_type(&entry),
            Some(Type::class("Int", Some("kotlin".into())))
        );
        let cyclic = b.replace("\"x\"", "A().value");
        let tree = parser::KotlinParser::new().parse(&cyclic);
        let symbols = crate::indexer::extract_symbols(&tree, &cyclic, "probe");
        index.replace_jvm_source(
            "B.kt",
            ktcore::jvm::source_facts("B.kt", &tree, &cyclic, &symbols),
            crate::index::Tier::Volatile,
        );
        state.prepare_declaration("B.kt", &cyclic, &index, 3);
        assert_eq!(state.declaration_type(&entry), None);
        let tree = parser::KotlinParser::new().parse(b);
        let symbols = crate::indexer::extract_symbols(&tree, b, "probe");
        index.replace_jvm_source(
            "B.kt",
            ktcore::jvm::source_facts("B.kt", &tree, b, &symbols),
            crate::index::Tier::Volatile,
        );
        state.prepare_declaration("B.kt", b, &index, 4);
        assert_eq!(
            state.declaration_type(&entry),
            Some(Type::class("String", Some("kotlin".into())))
        );
    }

    #[test]
    fn file_facts_update_when_text_changes() {
        let mut state = SalsaState::new();
        let key = "Main.kt";

        let first = state.file_facts(key, "package a\nfun one() {}\n", Durability::LOW);
        assert_eq!(first.package, "a");
        assert_eq!(first.symbols[0].name, "one");

        let second = state.file_facts(key, "package b\nfun two() {}\n", Durability::LOW);
        assert_eq!(second.package, "b");
        assert_eq!(second.symbols[0].name, "two");
    }

    #[test]
    fn file_facts_are_language_aware() {
        let mut state = SalsaState::new();

        let facts = state.file_facts(
            "Main.java",
            "package demo;\nclass Main { void run() {} }\n",
            Durability::LOW,
        );

        assert_eq!(facts.package, "demo");
        assert!(facts.symbols.iter().any(|symbol| symbol.name == "Main"));
        assert!(facts.symbols.iter().any(|symbol| symbol.name == "run"));
    }

    #[test]
    fn parameter_bounds_track_source_scope_text_and_index_revisions() {
        fn query(
            state: &mut SalsaState,
            index: &Index,
            scope: &str,
            source: &str,
            expression: &str,
            revision: u64,
        ) -> Type {
            let tree = parser::KotlinParser::new().parse(source);
            let start = source.rfind(expression).unwrap();
            let node = tree
                .root_node()
                .descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            assert_eq!(&source[node.byte_range()], expression);
            state.expr_type_in_file(
                "synthetic.kt",
                scope,
                source,
                Durability::LOW,
                index,
                revision,
                ExprKey::from_node(node),
            )
        }
        let mut state = SalsaState::new();
        let mut index = Index::new();
        let level = "package app\ninterface Level { fun getEnchantmentLevel(): Int }\n";
        let wrong = "package app\ninterface Wrong { fun wrongOnly(): Int }\n";
        for (file, source) in [("Level.kt", level), ("Wrong.kt", wrong)] {
            let tree = parser::KotlinParser::new().parse(source);
            index.replace_file(
                file,
                crate::indexer::extract_symbols(&tree, source, "app"),
                crate::index::Tier::Volatile,
            );
        }
        let source = "package app\nfun <T : Level> run(value: T) { value.getEnchantmentLevel() }\n";
        for scope in ["actual/Use.kt", "other/Use.kt"] {
            let Type::Parameter { definition, .. } =
                query(&mut state, &index, scope, source, "value", 1)
            else {
                panic!("parameter provenance missing");
            };
            assert_eq!(definition.file, scope);
            assert_eq!(
                query(
                    &mut state,
                    &index,
                    scope,
                    source,
                    "value.getEnchantmentLevel()",
                    1
                ),
                Type::class("Int", Some("kotlin".into()))
            );
        }
        let changed = source.replace("T : Level", "T : Wrong");
        assert_eq!(
            query(
                &mut state,
                &index,
                "other/Use.kt",
                &changed,
                "value.getEnchantmentLevel()",
                1
            ),
            Type::Unknown
        );
        assert_eq!(
            query(
                &mut state,
                &index,
                "other/Use.kt",
                source,
                "value.getEnchantmentLevel()",
                1
            ),
            Type::class("Int", Some("kotlin".into()))
        );
        let changed = level.replace(": Int", ": String");
        let tree = parser::KotlinParser::new().parse(&changed);
        index.replace_file(
            "Level.kt",
            crate::indexer::extract_symbols(&tree, &changed, "app"),
            crate::index::Tier::Volatile,
        );
        assert_eq!(
            query(
                &mut state,
                &index,
                "other/Use.kt",
                source,
                "value.getEnchantmentLevel()",
                2
            ),
            Type::class("String", Some("kotlin".into()))
        );
    }
}
