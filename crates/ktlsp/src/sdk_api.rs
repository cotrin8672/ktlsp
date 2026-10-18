//! Compiler-selected SDK signatures. API inventories and navigation archives have separate IDs.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{ensure, Context, Result};
use sha2::{Digest, Sha256};

use crate::classpath::SdkApiSelection;
use crate::deps::FileSymbols;
use crate::java::JavaParser;
use crate::language::{self, SourceLanguage};
use crate::project_model::{FileOrigin, SourceRepresentation};

/// An owner occurring in two release-matching entries has no selected definition.
pub(crate) fn release_entries(
    bytes: &[u8],
    release: u16,
) -> Result<BTreeMap<String, Option<String>>> {
    let marker = char::from_digit(release.into(), 36)
        .context("unsupported SDK release marker")?
        .to_ascii_uppercase();
    let archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    ensure!(
        archive.len() <= crate::jar::MAX_SOURCE_FILES,
        "too many SDK signature entries"
    );
    let selected = archive
        .file_names()
        .filter_map(|name| {
            let (releases, rest) = name.split_once('/')?;
            (releases.contains(marker)
                && releases
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || byte.is_ascii_uppercase())
                && rest.ends_with(".sig"))
            .then_some((name, rest))
        })
        .collect::<Vec<_>>();
    // Object's canonical location proves the archive layout; a dotted component alone does not.
    let legacy = selected
        .iter()
        .any(|(_, rest)| *rest == "java/lang/Object.sig");
    let modular = selected
        .iter()
        .any(|(_, rest)| *rest == "java.base/java/lang/Object.sig");
    ensure!(legacy != modular, "mixed or unproved SDK signature layout");
    let mut entries = BTreeMap::new();
    for (name, rest) in selected {
        let owner = if legacy {
            rest
        } else {
            let (module, owner) = rest
                .split_once('/')
                .context("selected SDK signature has no module")?;
            ensure!(
                !module.is_empty() && module.split('.').all(crate::jar::is_java_identifier),
                "invalid SDK signature module"
            );
            owner
        };
        let owner = owner.strip_suffix(".sig").unwrap();
        if owner
            .split('/')
            .any(|component| matches!(component, "." | ".."))
            || matches!(
                owner.rsplit('/').next(),
                Some("module-info" | "package-info")
            )
        {
            continue;
        }
        ensure!(
            owner.split('/').all(crate::jar::is_java_identifier),
            "selected SDK owner differs from proved layout"
        );
        entries
            .entry(owner.to_string())
            .and_modify(|selected| *selected = None)
            .or_insert_with(|| Some(name.to_string()));
    }
    ensure!(
        !entries.is_empty(),
        "selected SDK release has no signatures"
    );
    Ok(entries)
}

pub(crate) struct SdkApiIndex {
    pub batches: Vec<FileSymbols>,
    /// Unsupported declarations remain coverage gaps, never complete empty classes.
    pub complete: bool,
}

pub(crate) fn index(selection: &SdkApiSelection, extract_root: &Path) -> Result<SdkApiIndex> {
    ensure!(
        selection.release.is_some(),
        "SDK API projection requires a release inventory"
    );
    let bytes = std::fs::read(&selection.archive)?;
    ensure!(
        format!("{:x}", Sha256::digest(&bytes)) == selection.archive_sha256,
        "selected SDK archive changed"
    );
    ensure!(
        crate::classpath::file_origin_id(&selection.archive) == selection.archive_revision,
        "selected SDK archive revision changed"
    );
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let origin = selection.origin();
    let dest = extract_root.join("jdk-api").join(format!(
        "{:x}",
        Sha256::digest(selection.identity().as_bytes())
    ));
    let mut java = JavaParser::new();
    let mut batches = Vec::new();
    let mut complete = true;
    let mut classes = BTreeMap::new();
    for (owner, entry_name) in selection.entries.iter() {
        let Some(entry_name) = entry_name else {
            complete = false;
            continue;
        };
        let mut entry = archive.by_name(entry_name)?;
        let size = entry.size();
        let limit = ktcore::classfile::MAX_CLASS_FILE_BYTES;
        ensure!(size <= limit as u64, "oversized SDK signature");
        let mut class_bytes = Vec::with_capacity(size as usize);
        entry
            .by_ref()
            .take(limit as u64 + 1)
            .read_to_end(&mut class_bytes)?;
        ensure!(
            class_bytes.len() as u64 == size && class_bytes.len() <= limit,
            "truncated SDK signature"
        );
        let class = ktcore::classfile::decode(&origin, &class_bytes)?;
        ensure!(
            class.identity.internal_name == *owner,
            "SDK signature owner differs from entry"
        );
        classes.insert(owner.clone(), class);
    }
    let mut projections = BTreeMap::new();
    for (owner, class) in &classes {
        if owner.contains('$') {
            continue;
        }
        let Some((source, unavailable)) =
            crate::jar::selected_sdk_class_stub_facts(selection, class)
        else {
            complete = false;
            continue;
        };
        projections.insert(
            owner.clone(),
            (
                source,
                vec![(
                    owner.rsplit('/').next().unwrap().to_string(),
                    None,
                    unavailable,
                )],
            ),
        );
    }
    for (owner, class) in &classes {
        if !owner.contains('$') {
            continue;
        }
        let Some((outer, name)) = static_nested_identity(class, &classes) else {
            complete = false;
            continue;
        };
        let Some((source, unavailable)) =
            crate::jar::selected_sdk_nested_stub_facts(selection, class, &name)
        else {
            complete = false;
            continue;
        };
        let Some((parent_source, owners)) = projections.get_mut(&outer) else {
            complete = false;
            continue;
        };
        let declaration = source
            .lines()
            .filter(|line| !line.starts_with("package ") && !line.starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let end = parent_source
            .rfind('}')
            .context("SDK parent declaration missing close")?;
        parent_source.insert_str(end, &format!("\n{declaration}\n"));
        owners.push((
            name,
            Some(outer.rsplit('/').next().unwrap().to_string()),
            unavailable,
        ));
    }
    for (owner, (source, owners)) in projections {
        let path = dest.join(format!("{owner}.java"));
        ensure!(
            path.starts_with(&dest),
            "SDK declaration path escapes cache"
        );
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::jar::write_source_text(&path, &source)?;
        let tree = java.parse(&source);
        ensure!(
            !tree.root_node().has_error(),
            "SDK declaration projection is not parseable Java"
        );
        let mut facts = language::symbol_facts(SourceLanguage::Java, &tree, &source);
        for (name, container, unavailable) in owners {
            let declaration = facts
                .symbols
                .iter_mut()
                .find(|symbol| {
                    matches!(
                        symbol.kind,
                        ktcore::symbol::SymbolKind::Class | ktcore::symbol::SymbolKind::Interface
                    ) && symbol.name == name
                        && symbol.container.as_deref() == container.as_deref()
                })
                .context("SDK declaration projection has no exact owner")?;
            declaration.unavailable_members = Some(Box::new(unavailable));
        }
        batches.push(FileSymbols {
            file: crate::paths::file_key(&path),
            symbols: facts.symbols,
            jvm: facts.jvm,
            library_source: None,
            origin: Some(FileOrigin {
                origin_id: origin.clone(),
                representation: SourceRepresentation::Stub,
                project_scope: None,
            }),
        });
    }
    Ok(SdkApiIndex { batches, complete })
}

/// One-level public static interfaces only: no parameterized outer or instance capture.
fn static_nested_identity(
    class: &ktcore::classfile::ClassFileFacts,
    classes: &BTreeMap<String, ktcore::classfile::ClassFileFacts>,
) -> Option<(String, String)> {
    let row = class
        .inner_classes
        .iter()
        .find(|row| row.inner.as_ref() == class.identity.internal_name)?;
    let outer = row.outer.as_deref()?;
    let name = row.name.as_deref()?;
    if outer.contains('$')
        || name.contains('$')
        || !crate::jar::is_java_identifier(name)
        || row.access_flags != 0x0609
        || class.access_flags != 0x0601
        || class
            .attribute_names
            .iter()
            .any(|a| a.as_ref() == "EnclosingMethod")
        || class.identity.internal_name != format!("{outer}${name}")
    {
        return None;
    }
    let parent = classes.get(outer)?;
    if parent.identity.origin_id != class.identity.origin_id
        || parent.access_flags & 1 == 0
        || !parent.inner_classes.iter().any(|other| other == row)
    {
        return None;
    }
    Some((outer.to_string(), name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entry_selection() -> (std::path::PathBuf, SdkApiSelection) {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-static-sdk-entry-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("ct.sym");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        for (owner, bytes) in [
            (
                "java/lang/Object",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Object.sig")
                    .as_slice(),
            ),
            (
                "java/lang/Number",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Number.sig")
                    .as_slice(),
            ),
            (
                "java/lang/String",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/String.sig")
                    .as_slice(),
            ),
            (
                "java/util/concurrent/atomic/AtomicInteger",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/AtomicInteger.sig"
                )
                .as_slice(),
            ),
            (
                "java/io/Serializable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Serializable.sig"
                )
                .as_slice(),
            ),
            (
                "java/lang/Comparable",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Comparable.sig"
                )
                .as_slice(),
            ),
            (
                "java/lang/CharSequence",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/CharSequence.sig"
                )
                .as_slice(),
            ),
            (
                "java/util/Map",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Map.sig")
                    .as_slice(),
            ),
            (
                "java/util/Map$Entry",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk17-release8/Map$Entry.sig"
                )
                .as_slice(),
            ),
        ] {
            zip.start_file(
                format!("8/java.base/{owner}.sig"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let bytes = std::fs::read(&archive).unwrap();
        let selection = SdkApiSelection {
            home: root.clone(),
            source_zip: None,
            release: Some(8),
            archive_revision: crate::classpath::file_origin_id(&archive),
            archive,
            archive_sha256: format!("{:x}", Sha256::digest(&bytes)),
            entries: std::sync::Arc::new(release_entries(&bytes, 8).unwrap()),
        };
        (root, selection)
    }

    #[test]
    fn sdk_static_nested_entry_has_useful_completion_and_exact_nested_navigation() {
        let (root, selection) = entry_selection();
        let indexed = index(&selection, &root.join("extract")).unwrap();
        let mut ws = crate::workspace::Workspace::new();
        for batch in indexed.batches {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.assume_index_complete_for_tests();
        for (name, text) in [
            ("Qualified.java", "class Qualified { String f(java.util.Map.Entry<String,String> e) { return e.getKey().substring(0); } }"),
            ("Imported.java", "import java.util.Map.Entry; class Imported { String f(Entry<String,String> e) { return e.getKey().substring(0); } }"),
        ] {
            let key = crate::paths::file_key(&root.join(name));
            ws.open(&key, text.into());
            let offset = text.find("e.get").unwrap()+"e.get".len();
            let result = ws.complete(&key, offset, true);
            assert!(result.as_ref().is_some_and(|r| r.items.iter().any(|i| i.label == "getKey")), "actual selected release8 static nested SDK API must be useful: {result:?}");
            let chain_offset = text.find(".substring").unwrap() + ".sub".len();
            let chain = ws.complete(&key, chain_offset, true);
            assert!(chain.as_ref().is_some_and(|r| r.items.iter().any(|i| i.label == "substring")), "actual nested K must substitute String for chained member completion: {chain:?}");
            let defs = ws.goto_definition(&key,text.find("getKey()").unwrap());
            assert_eq!(defs.len(),1,"exact nested method navigation: {defs:?}");
            assert!(defs[0].file.ends_with("java/util/Map.java"));
            let target = std::fs::read_to_string(&defs[0].file).unwrap();
            assert_eq!(&target[defs[0].start_byte..defs[0].end_byte],"getKey");
            assert!(target[..defs[0].start_byte].contains("interface Entry"));
            assert!(!target.contains("interface Map$Entry"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdk_static_nested_identity_requires_reciprocal_public_static_owner_facts() {
        let parent = ktcore::classfile::decode(
            "selected",
            include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Map.sig"),
        )
        .unwrap();
        let child = ktcore::classfile::decode(
            "selected",
            include_bytes!("../../ktcore/tests/fixtures/classfile/sdk17-release8/Map$Entry.sig"),
        )
        .unwrap();
        let mut classes = BTreeMap::from([(parent.identity.internal_name.clone(), parent.clone())]);
        assert_eq!(
            static_nested_identity(&child, &classes),
            Some(("java/util/Map".into(), "Entry".into()))
        );
        for kind in [
            "nonstatic",
            "anonymous",
            "wrong-name",
            "different-origin",
            "missing-parent-proof",
        ] {
            let mut child = child.clone();
            let mut parent = parent.clone();
            let row = child
                .inner_classes
                .iter_mut()
                .find(|r| r.inner.as_ref() == child.identity.internal_name)
                .unwrap();
            match kind {
                "nonstatic" => row.access_flags &= !8,
                "anonymous" => row.name = None,
                "wrong-name" => row.name = Some("Other".into()),
                "different-origin" => parent.identity.origin_id = "other".into(),
                "missing-parent-proof" => parent.inner_classes.clear(),
                _ => unreachable!(),
            }
            classes.insert(parent.identity.internal_name.clone(), parent);
            assert!(static_nested_identity(&child, &classes).is_none(), "{kind}");
        }
    }

    #[test]
    fn release_catalog_uses_markers_and_refuses_duplicate_or_unsafe_owners() {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for name in [
            "78/java.base/java/lang/String.sig",
            "9/java.base/java/lang/OnlyNew.sig",
            "8/java.base/java/lang/Object.sig",
            "89/other.module/java/lang/Object.sig",
            "AB8/java.base/java/lang/Combined.sig",
            "8/java.base/../Escapes.sig",
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(b"catalog-only").unwrap();
        }
        let bytes = archive.finish().unwrap().into_inner();
        let entries = release_entries(&bytes, 8).unwrap();
        assert_eq!(
            entries["java/lang/String"].as_deref(),
            Some("78/java.base/java/lang/String.sig")
        );
        assert_eq!(entries["java/lang/Object"], None);
        assert!(entries.contains_key("java/lang/Combined"));
        assert!(!entries.contains_key("java/lang/OnlyNew"));
        assert!(!entries.contains_key("../Escapes"));
        assert!(release_entries(&bytes, 17).is_err());
    }
    fn sdk11_test_archive(entries: &[(&str, &[u8])]) -> (std::path::PathBuf, Vec<u8>) {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-sdk11-layout-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut zip = zip::ZipWriter::new(std::fs::File::create(root.join("ct.sym")).unwrap());
        for (name, bytes) in entries {
            zip.start_file(*name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
        let bytes = std::fs::read(root.join("ct.sym")).unwrap();
        (root, bytes)
    }

    fn sdk11_test_selection(root: &Path, bytes: &[u8]) -> Result<SdkApiSelection> {
        let archive = root.join("ct.sym");
        Ok(SdkApiSelection {
            home: root.to_path_buf(),
            source_zip: None,
            release: Some(8),
            archive_revision: crate::classpath::file_origin_id(&archive),
            archive,
            archive_sha256: format!("{:x}", Sha256::digest(bytes)),
            entries: std::sync::Arc::new(release_entries(bytes, 8)?),
        })
    }

    fn sdk11_actual_entries() -> [(&'static str, &'static [u8]); 5] {
        [
            (
                "876/java/lang/Object.sig",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk11-release8/Object.sig")
                    .as_slice(),
            ),
            (
                "8/java/lang/String.sig",
                include_bytes!("../../ktcore/tests/fixtures/classfile/sdk11-release8/String.sig")
                    .as_slice(),
            ),
            (
                "8769A/java/io/Serializable.sig",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk11-release8/Serializable.sig"
                )
                .as_slice(),
            ),
            (
                "8769A/java/lang/Comparable.sig",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk11-release8/Comparable.sig"
                )
                .as_slice(),
            ),
            (
                "8/java/lang/CharSequence.sig",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/sdk11-release8/CharSequence.sig"
                )
                .as_slice(),
            ),
        ]
    }

    #[test]
    fn sdk11_actual_release8_catalogue_keeps_complete_owner_paths() {
        let (root, bytes) = sdk11_test_archive(&sdk11_actual_entries());
        let entries = release_entries(&bytes, 8).unwrap();
        assert_eq!(
            entries["java/lang/Object"].as_deref(),
            Some("876/java/lang/Object.sig")
        );
        assert_eq!(
            entries["java/lang/String"].as_deref(),
            Some("8/java/lang/String.sig")
        );
        assert!(!entries.contains_key("lang/Object") && !entries.contains_key("lang/String"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdk11_actual_release8_string_has_useful_members_and_exact_navigation() {
        let (root, bytes) = sdk11_test_archive(&sdk11_actual_entries());
        let selection = sdk11_test_selection(&root, &bytes).unwrap();
        let indexed = index(&selection, &root.join("extract")).unwrap();
        assert!(indexed
            .batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "substring"
                && symbol.container.as_deref() == Some("String")));
        let mut ws = crate::workspace::Workspace::new();
        for batch in indexed.batches {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.assume_index_complete_for_tests();
        let key = crate::paths::file_key(&root.join("Use.java"));
        let source = "class Use { String f(String value) { return value.substring(1); } }";
        ws.open(&key, source.into());
        let completion = ws.complete(&key, source.find("value.").unwrap() + "value.".len(), true);
        assert!(
            completion
                .as_ref()
                .is_some_and(|result| result.items.iter().any(|item| item.label == "substring")),
            "actual SDK11 release8 String members must be useful: {completion:?}"
        );
        assert!(
            !completion
                .as_ref()
                .unwrap()
                .items
                .iter()
                .any(|item| item.label == "strip"),
            "Java8 must not expose Java11 String.strip"
        );
        let definitions = ws.goto_definition(&key, source.find("substring").unwrap());
        assert_eq!(
            definitions.len(),
            1,
            "exact actual SDK11 signature navigation: {definitions:?}"
        );
        assert!(definitions[0].file.ends_with("java/lang/String.java"));
        let target = std::fs::read_to_string(&definitions[0].file).unwrap();
        assert_eq!(
            &target[definitions[0].start_byte..definitions[0].end_byte],
            "substring"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sdk11_layout_selection_rejects_mixed_and_missing_canonical_anchor() {
        let object = sdk11_actual_entries()[0].1;
        let string = sdk11_actual_entries()[1].1;
        for (name, entries) in [
            (
                "both-layout-anchors",
                vec![
                    ("8/java/lang/Object.sig", object),
                    ("8/java.base/java/lang/Object.sig", object),
                ],
            ),
            (
                "legacy-anchor-modular-member",
                vec![
                    ("8/java/lang/Object.sig", object),
                    ("8/java.base/java/lang/String.sig", string),
                ],
            ),
            (
                "modular-anchor-legacy-member",
                vec![
                    ("8/java.base/java/lang/Object.sig", object),
                    ("8/java/lang/String.sig", string),
                ],
            ),
            (
                "identifier-prefix-is-not-an-owner-proof",
                vec![
                    ("8/java/lang/Object.sig", object),
                    ("8/unproven/java/lang/String.sig", string),
                ],
            ),
            (
                "no-canonical-anchor",
                vec![("8/java/lang/String.sig", string)],
            ),
        ] {
            let (root, bytes) = sdk11_test_archive(&entries);
            let result = sdk11_test_selection(&root, &bytes)
                .and_then(|selection| index(&selection, &root.join("extract")));
            assert!(
                result.is_err(),
                "mixed or unproved layout must not publish any SDK facts: {name}"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn sdk11_same_layout_duplicate_owner_remains_ambiguous() {
        let object = sdk11_actual_entries()[0].1;
        let string = sdk11_actual_entries()[1].1;
        let (root, bytes) = sdk11_test_archive(&[
            ("8/java/lang/Object.sig", object),
            ("89/java/lang/Object.sig", object),
            ("8/java/lang/String.sig", string),
        ]);
        let entries = release_entries(&bytes, 8).unwrap();
        assert_eq!(entries["java/lang/Object"], None);
        assert!(entries["java/lang/String"].is_some());
        std::fs::remove_dir_all(root).unwrap();
    }
}
