//! End-to-end test of library goto-definition: locate a `-sources.jar`, extract it, index its
//! Kotlin + Java sources, and resolve goto from a user Kotlin file into the extracted library
//! source. The hermetic test builds a fake Gradle cache (no network); the `#[ignore]`d test
//! exercises the real Maven Central download path.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use ktlsp::artifacts::{self, Repos};
use ktlsp::coords::Coordinate;
use ktlsp::deps;
use ktlsp::java::JavaParser;
use ktlsp::parser::KotlinParser;
use ktlsp::workspace::Workspace;

use zip::write::SimpleFileOptions;

fn unique_tmp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ktlsp_{tag}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a sources jar containing the given (entry-name, body) pairs.
fn write_sources_jar(path: &Path, entries: &[(&str, &str)]) {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p).unwrap();
    }
    let mut zip = zip::ZipWriter::new(fs::File::create(path).unwrap());
    for (name, body) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();
}

/// Index a coordinate's sources into a fresh workspace; return it plus the extraction root.
fn index_into_workspace(coord: &Coordinate, repos: &Repos, extract_root: &Path) -> Workspace {
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let batches = deps::resolve_coordinate(coord, repos, extract_root, &mut kotlin, &mut java);
    assert!(
        !batches.is_empty(),
        "expected indexed library files for {}",
        coord.label()
    );
    let mut ws = Workspace::new();
    for batch in batches {
        ws.index
            .replace_file(&batch.file, batch.symbols, ktlsp::index::Tier::Durable);
    }
    ws
}

fn index_coordinate_closure_into_workspace(
    coord: &Coordinate,
    repos: &Repos,
    extract_root: &Path,
) -> Workspace {
    let mut queue = VecDeque::from([coord.clone()]);
    let mut seen = BTreeSet::new();
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let mut ws = Workspace::new();
    while let Some(next) = queue.pop_front() {
        if !seen.insert(next.clone()) {
            continue;
        }
        for batch in deps::resolve_coordinate(&next, repos, extract_root, &mut kotlin, &mut java) {
            ws.index
                .replace_file(&batch.file, batch.symbols, ktlsp::index::Tier::Durable);
        }
        for dep in artifacts::dependency_coordinates(repos, &next) {
            if !seen.contains(&dep) && !queue.iter().any(|queued| queued == &dep) {
                queue.push_back(dep);
            }
        }
    }
    ws
}

fn index_coordinates_with_selection(
    coords: &[Coordinate],
    repos: &Repos,
    extract_root: &Path,
) -> Workspace {
    let mut selector = deps::CoordinateSelector::new();
    let mut indexed_files: BTreeMap<Coordinate, Vec<String>> = BTreeMap::new();
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let mut ws = Workspace::new();

    for coord in coords {
        match selector.consider(coord.clone()) {
            deps::CoordinateDecision::Selected => {}
            deps::CoordinateDecision::Replaces(previous) => {
                if let Some(files) = indexed_files.remove(&previous) {
                    for file in files {
                        ws.index.remove_file(&file);
                    }
                }
            }
            deps::CoordinateDecision::ShadowedBy(_) => continue,
        }

        let mut files = Vec::new();
        for batch in deps::resolve_coordinate(coord, repos, extract_root, &mut kotlin, &mut java) {
            files.push(batch.file.clone());
            ws.index
                .replace_file(&batch.file, batch.symbols, ktlsp::index::Tier::Durable);
        }
        indexed_files.insert(coord.clone(), files);
    }

    ws
}

/// Assert goto at the (last) usage of `token` lands on `token` in a file ending with `suffix`.
fn assert_goto_into_library(ws: &mut Workspace, key: &str, src: &str, token: &str, suffix: &str) {
    let offset = src.rfind(token).expect("token present in source");
    let defs = ws.goto_definition(key, offset);
    assert!(!defs.is_empty(), "goto on `{token}` returned nothing");
    let d = &defs[0];
    assert!(
        std::path::Path::new(&d.file).ends_with(suffix),
        "goto on `{token}` -> {} (expected a file ending in {suffix})",
        d.file
    );
    let target = fs::read_to_string(&d.file).expect("target file readable");
    assert_eq!(
        &target[d.start_byte..d.end_byte],
        token,
        "goto on `{token}` landed on the wrong identifier in {}",
        d.file
    );
}

const SNAPSHOT_V1: &str =
    "package api;\npublic class Useful {\n    public int versionOne() { return 1; }\n}\n";
const SNAPSHOT_V2: &str = "package api;\n// Snapshot version two moves every real source anchor and declares a different public member.\npublic class Useful {\n    public int versionTwoLong() { return 222; }\n}\n";

fn snapshot_source(tmp: &Path) -> (Coordinate, Repos, PathBuf) {
    let coordinate = Coordinate::parse("fixture:snapshot:1-SNAPSHOT").unwrap();
    let repos = Repos {
        gradle_cache: tmp.join("gradle"),
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".into(),
        download_dir: tmp.join("download"),
        allow_download: false,
    };
    let jar = repos
        .m2
        .join("fixture/snapshot/1-SNAPSHOT/snapshot-1-SNAPSHOT-sources.jar");
    (coordinate, repos, jar)
}

#[test]
fn snapshot_source_revision_refreshes_materialized_goto_cold_and_warm() {
    let tmp = unique_tmp("snapshot_goto_revision");
    let (coordinate, repos, jar) = snapshot_source(&tmp);
    let extract = tmp.join("extracted");
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let mut ws = Workspace::new();
    let mut previous_identity = None;
    for (text, member, old_member) in [
        (SNAPSHOT_V1, "versionOne", "versionTwoLong"),
        (SNAPSHOT_V2, "versionTwoLong", "versionOne"),
    ] {
        write_sources_jar(&jar, &[("api/Useful.java", text)]);
        let source = deps::coordinate_source(&coordinate, &repos, &extract).unwrap();
        assert_ne!(previous_identity.as_ref(), Some(&source.identity()));
        previous_identity = Some(source.identity());
        for _ in 0..2 {
            let batches = deps::resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
            assert_eq!(batches.len(), 1);
            let file = batches[0].file.clone();
            assert!(batches[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == member));
            assert!(!batches[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == old_member));
            for batch in batches {
                assert!(ws.index_dependency_batch(batch));
            }
            ws.bump_index_revision();
            assert_eq!(ws.doc_text(&file).as_deref(), Some(text));
            for (name, client) in [
                ("Use.kt", format!("import api.Useful\nfun use(helper: Useful) = helper.{member}()\n")),
                ("Use.java", format!("import api.Useful; class Use {{ int use(Useful helper) {{ return helper.{member}(); }} }}")),
            ] {
                let key = ktlsp::paths::file_key(&tmp.join(name));
                ws.open(key.clone(), client.clone());
                assert_goto_into_library(&mut ws, &key, &client, member, "api/Useful.java");
            }
            let definitions = ws.goto_definition(&file, text.find(member).unwrap());
            assert_eq!(
                definitions.len(),
                1,
                "closed source parsing must follow the current text"
            );
            assert_eq!(definitions[0].start_byte, text.find(member).unwrap());
        }
    }
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn snapshot_source_addition_and_removal_follow_model_refresh() {
    let tmp = unique_tmp("snapshot_source_delta");
    let (coordinate, repos, jar) = snapshot_source(&tmp);
    let binary = jar.with_file_name("snapshot-1-SNAPSHOT.jar");
    write_sources_jar(&binary, &[("api/Useful.class", "class-name fixture")]);
    let model = || {
        std::sync::Arc::new(ktlsp::classpath::ProjectSnapshot::new(
            ktlsp::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tfixture\tsnapshot\t1-SNAPSHOT\nEND\t:app\n",
                tmp.join("app").display(), tmp.join("app/src").display(), binary.display(), binary.display(),
            )),
        ))
    };
    let mut ws = Workspace::new();
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    write_sources_jar(
        &jar,
        &[
            ("api/Useful.java", SNAPSHOT_V1),
            ("api/Gone.java", "package api; public class Gone {}"),
        ],
    );
    ws.set_project_model(model());
    let first = deps::coordinate_source(&coordinate, &repos, &tmp.join("extracted")).unwrap();
    let batches = deps::resolve_library_source(&first, &mut kotlin, &mut java).unwrap();
    let gone = batches
        .iter()
        .find(|batch| Path::new(&batch.file).ends_with("api/Gone.java"))
        .unwrap()
        .file
        .clone();
    assert!(deps::materialize_source_file(Path::new(&gone)));
    for batch in batches {
        assert!(ws.index_dependency_batch(batch));
    }
    ws.bump_index_revision();
    let key = ktlsp::paths::file_key(&tmp.join("app/src/Use.java"));
    let client = "import api.Gone; class Use { Gone gone; }";
    ws.open(key.clone(), client.into());
    assert_goto_into_library(&mut ws, &key, client, "Gone", "api/Gone.java");

    write_sources_jar(
        &jar,
        &[
            ("api/Useful.java", SNAPSHOT_V2),
            ("api/Added.java", "package api; public class Added {}"),
        ],
    );
    ws.set_project_model(model());
    assert!(
        ws.index.entries_for_file(&gone).is_empty(),
        "model refresh must remove closed obsolete facts"
    );
    let second = deps::coordinate_source(&coordinate, &repos, &tmp.join("extracted")).unwrap();
    assert_ne!(first.identity(), second.identity());
    for _ in 0..2 {
        for batch in deps::resolve_library_source(&second, &mut kotlin, &mut java).unwrap() {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.bump_index_revision();
        assert!(
            !Path::new(&gone).exists(),
            "removed source entries must not survive on disk"
        );
        assert_eq!(ws.doc_text(&gone), None);
        assert!(ws
            .goto_definition(&key, client.rfind("Gone").unwrap())
            .is_empty());
        let added_client = "import api.Added; class Use { Added added; }";
        ws.change(&key, added_client.into());
        assert_goto_into_library(&mut ws, &key, added_client, "Added", "api/Added.java");
        ws.change(&key, client.into());
    }
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn snapshot_dirty_source_close_rematerializes_current_archive() {
    for navigate_before_close in [false, true] {
        let tmp = unique_tmp(&format!("snapshot_dirty_close_{navigate_before_close}"));
        let (coordinate, repos, jar) = snapshot_source(&tmp);
        let extract = tmp.join("extracted");
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        write_sources_jar(&jar, &[("api/Useful.java", SNAPSHOT_V1)]);
        let first = deps::coordinate_source(&coordinate, &repos, &extract).unwrap();
        let batches = deps::resolve_library_source(&first, &mut kotlin, &mut java).unwrap();
        let file = batches[0].file.clone();
        assert!(deps::materialize_source_file(Path::new(&file)));
        let mut ws = Workspace::new();
        for batch in batches {
            assert!(ws.index_dependency_batch(batch));
        }
        let dirty =
            "package api; public class Useful { public int editorUnsaved() { return 7; } }\n";
        ws.open(file.clone(), SNAPSHOT_V1.into());
        ws.change(&file, dirty.into());
        // A materialized cache entry may be evicted while its editor buffer stays open.
        fs::remove_file(&file).unwrap();
        write_sources_jar(&jar, &[("api/Useful.java", SNAPSHOT_V2)]);
        let second = deps::coordinate_source(&coordinate, &repos, &extract).unwrap();
        for batch in deps::resolve_library_source(&second, &mut kotlin, &mut java).unwrap() {
            assert!(!ws.index_dependency_batch(batch));
        }
        assert_eq!(ws.doc_text(&file).as_deref(), Some(dirty));
        assert!(ws
            .index
            .entries_for_file(&file)
            .iter()
            .any(|entry| entry.sym.name == "editorUnsaved"));
        assert!(
            !Path::new(&file).exists(),
            "reading an open buffer must not materialize over it"
        );
        if navigate_before_close {
            let client = "import api.Useful; class Use { int use(Useful helper) { return helper.editorUnsaved(); } }";
            let key = ktlsp::paths::file_key(&tmp.join("Use.java"));
            ws.open(key.clone(), client.into());
            let definitions = ws.goto_definition(&key, client.find("editorUnsaved").unwrap());
            assert_eq!(definitions.len(), 1);
            assert_eq!(ws.doc_text(&file).as_deref(), Some(dirty));
            assert_eq!(fs::read_to_string(&file).unwrap(), SNAPSHOT_V2);
        }
        ws.close(&file);
        let entries = ws.index.entries_for_file(&file);
        assert!(
            entries
                .iter()
                .any(|entry| entry.sym.name == "versionTwoLong"),
            "close must restore current archive facts before any later request"
        );
        assert!(!entries
            .iter()
            .any(|entry| entry.sym.name == "editorUnsaved"));
        assert_eq!(ws.doc_text(&file).as_deref(), Some(SNAPSHOT_V2));
        ws.open(file.clone(), ws.doc_text(&file).unwrap());
        assert!(ws
            .index
            .entries_for_file(&file)
            .iter()
            .any(|entry| entry.sym.name == "versionTwoLong"));
        fs::remove_dir_all(tmp).unwrap();
    }
}

#[test]
fn local_jar_source_refresh_preserves_scope_and_replaces_member_targets() {
    let tmp = unique_tmp("local_sources_scoped_refresh");
    let jar = tmp.join("libs/api.jar");
    let sources = tmp.join("libs/api-sources.jar");
    write_sources_jar(&jar, &[("api/Only.class", "")]);
    let model = || {
        std::sync::Arc::new(ktlsp::classpath::ProjectSnapshot::new(
            ktlsp::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nEND\t:app\nPROJECT\t:other\t{}\nSOURCE\tmain\t{}\nEND\t:other\n",
                tmp.join("app").display(), tmp.join("app/src").display(), jar.display(),
                tmp.join("other").display(), tmp.join("other/src").display(),
            )),
        ))
    };
    let mut ws = Workspace::new();
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let mut previous_source_files: Vec<String> = Vec::new();
    for member in [Some("first"), Some("other"), None, Some("first")] {
        if let Some(member) = member {
            write_sources_jar(
                &sources,
                &[(
                    "api/Only.java",
                    &format!("package api; public class Only {{ public void {member}() {{}} }}"),
                )],
            );
        } else {
            fs::remove_file(&sources).unwrap();
        }
        ws.set_project_model(model());
        for previous in &previous_source_files {
            assert!(
                ws.index.entries_for_file(previous).is_empty(),
                "old source facts survived refresh"
            );
        }
        let batches =
            deps::resolve_local_jar_stubs(&jar, &tmp.join("extracted"), &mut kotlin, &mut java)
                .unwrap();
        assert!(!batches.is_empty());
        previous_source_files = batches.iter().map(|batch| batch.file.clone()).collect();
        for batch in batches {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.bump_index_revision();
        ws.assume_index_complete_for_tests();
        for (name, text) in [
            ("Use.kt", "package app\nimport api.Only\nfun use(value: Only) { value.MEMBER() }\n"),
            ("Use.java", "package app; import api.Only; class Use { void use(Only value) { value.MEMBER(); } }"),
        ] {
            let text = text.replace("MEMBER", member.unwrap_or("first"));
            let key = ktlsp::paths::file_key(&tmp.join("app/src").join(name));
            ws.open(key.clone(), text.clone());
            let offset = text.find("value.").unwrap() + "value.".len();
            let items = ws.complete(&key, offset, false).map(|result| result.items).unwrap_or_default();
            for candidate in ["first", "other"] {
                assert_eq!(items.iter().any(|item| item.label == candidate), member == Some(candidate),
                    "{name} has the wrong {candidate} completion after {member:?}");
            }
            if let Some(member) = member {
                assert_goto_into_library(&mut ws, &key, &text, member, "api/Only.java");
            } else {
                assert!(ws.goto_definition(&key, offset).is_empty(), "removed source member still resolves");
            }
            let other = ktlsp::paths::file_key(&tmp.join("other/src").join(name));
            ws.open(other.clone(), text.clone());
            assert!(ws.goto_definition(&other, text.rfind("Only").unwrap()).is_empty(),
                "a module without the jar resolved its class");
        }
    }
    fs::remove_dir_all(tmp).unwrap();
}

#[test]
fn goto_uses_newest_selected_library_version() {
    let tmp = unique_tmp("libgoto_version_conflict");
    let older = Coordinate::parse("acme:demo:1.10.2").unwrap();
    let newer = Coordinate::parse("acme:demo:1.11.0").unwrap();

    let gradle_cache = tmp.join("gradle/caches");
    for (coord, value) in [(&older, "10"), (&newer, "11")] {
        let jar = gradle_cache
            .join("modules-2/files-2.1")
            .join(&coord.group)
            .join(&coord.artifact)
            .join(&coord.version)
            .join("deadbeef")
            .join(coord.sources_jar_name());
        write_sources_jar(
            &jar,
            &[(
                "acme/lib/Lib.kt",
                &format!("package acme.lib\n\nfun shared(): Int = {value}\n"),
            )],
        );
    }

    let repos = Repos {
        gradle_cache,
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: false,
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_coordinates_with_selection(&[older, newer], &repos, &extract_root);

    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    let src = "package app\n\
               import acme.lib.shared\n\
               \n\
               fun main() {\n\
               \x20\x20\x20\x20shared()\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    let offset = src.rfind("shared").expect("token present");
    let defs = ws.goto_definition(&key, offset);
    assert_eq!(
        defs.len(),
        1,
        "older library version should be shadowed: {defs:?}"
    );
    assert!(
        std::path::Path::new(&defs[0].file).starts_with(extract_root.join("acme/demo/1.11.0")),
        "goto should land in the newer source tree, got {}",
        defs[0].file
    );

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn goto_into_indexed_library_kotlin_and_java() {
    let tmp = unique_tmp("libgoto");
    let coord = Coordinate::parse("acme:demo:1.0").unwrap();

    // Fake Gradle cache: modules-2/files-2.1/{group}/{artifact}/{version}/{sha1}/{name}
    let gradle_cache = tmp.join("gradle/caches");
    let jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&coord.group)
        .join(&coord.artifact)
        .join(&coord.version)
        .join("deadbeef")
        .join(coord.sources_jar_name());
    write_sources_jar(
        &jar,
        &[
            (
                "acme/lib/Lib.kt",
                "package acme.lib\n\nfun libFunc(): Int = 1\n\nclass Widget(val size: Int)\n",
            ),
            (
                "acme/jlib/JThing.java",
                "package acme.jlib;\n\npublic class JThing {\n    public void run() {}\n}\n",
            ),
        ],
    );

    let repos = Repos {
        gradle_cache,
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: false, // cache-only: no network in this test
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_into_workspace(&coord, &repos, &extract_root);

    // A user Kotlin file referencing the library symbols.
    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    let src = "package app\n\
               import acme.lib.Widget\n\
               import acme.lib.libFunc\n\
               import acme.jlib.JThing\n\
               \n\
               fun main() {\n\
               \x20\x20\x20\x20val w = Widget(3)\n\
               \x20\x20\x20\x20libFunc()\n\
               \x20\x20\x20\x20val j = JThing()\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    // Kotlin class (type via constructor call), Kotlin top-level function, and Java class.
    assert_goto_into_library(&mut ws, &key, src, "Widget", "Lib.kt");
    assert_goto_into_library(&mut ws, &key, src, "libFunc", "Lib.kt");
    assert_goto_into_library(&mut ws, &key, src, "JThing", "JThing.java");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn goto_into_jvm_variant_sources_for_multiplatform_coordinate() {
    let tmp = unique_tmp("libgoto_jvm_variant");
    let coord = Coordinate::parse("io.ktor:ktor-client-apache5:3.5.0").unwrap();
    let jvm_coord = Coordinate::parse("io.ktor:ktor-client-apache5-jvm:3.5.0").unwrap();

    let gradle_cache = tmp.join("gradle/caches");
    let empty_root_jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&coord.group)
        .join(&coord.artifact)
        .join(&coord.version)
        .join("feedface")
        .join(coord.sources_jar_name());
    write_sources_jar(
        &empty_root_jar,
        &[("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\n")],
    );

    let jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&jvm_coord.group)
        .join(&jvm_coord.artifact)
        .join(&jvm_coord.version)
        .join("deadbeef")
        .join(jvm_coord.sources_jar_name());
    write_sources_jar(
        &jar,
        &[(
            "io/ktor/client/engine/apache5/Apache5.kt",
            "package io.ktor.client.engine.apache5\n\nobject Apache5\n",
        )],
    );

    let repos = Repos {
        gradle_cache,
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: false,
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_into_workspace(&coord, &repos, &extract_root);

    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    let src = "package app\n\
               import io.ktor.client.engine.apache5.Apache5\n\
               \n\
               fun main() {\n\
               \x20\x20\x20\x20val engine = Apache5\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    assert_goto_into_library(&mut ws, &key, src, "Apache5", "Apache5.kt");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn goto_into_transitive_java_sources_discovered_from_metadata() {
    let tmp = unique_tmp("libgoto_transitive_java");
    let root_coord = Coordinate::parse("io.ktor:ktor-client-apache5:3.5.0").unwrap();
    let jvm_coord = Coordinate::parse("io.ktor:ktor-client-apache5-jvm:3.5.0").unwrap();
    let httpclient =
        Coordinate::parse("org.apache.httpcomponents.client5:httpclient5:5.5.1").unwrap();
    let httpcore = Coordinate::parse("org.apache.httpcomponents.core5:httpcore5:5.3.6").unwrap();

    let gradle_cache = tmp.join("gradle/caches");
    let module_dir = gradle_cache
        .join("modules-2/files-2.1")
        .join(&jvm_coord.group)
        .join(&jvm_coord.artifact)
        .join(&jvm_coord.version)
        .join("module");
    fs::create_dir_all(&module_dir).unwrap();
    fs::write(
        module_dir.join(format!("{}-{}.module", jvm_coord.artifact, jvm_coord.version)),
        r#"{
  "variants": [
    { "name": "jvmApiElements", "dependencies": [
      { "group": "org.apache.httpcomponents.client5", "module": "httpclient5", "version": { "requires": "5.5.1" } }
    ] }
  ]
}"#,
    )
    .unwrap();

    let client_jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&httpclient.group)
        .join(&httpclient.artifact)
        .join(&httpclient.version)
        .join("client")
        .join(httpclient.sources_jar_name());
    write_sources_jar(
        &client_jar,
        &[
            (
                "org/apache/hc/client5/http/config/ConnectionConfig.java",
                "package org.apache.hc.client5.http.config;\n\npublic class ConnectionConfig {}\n",
            ),
            (
                "META-INF/maven/org.apache.httpcomponents.client5/httpclient5/pom.xml",
                "<project><dependencies><dependency><groupId>org.apache.httpcomponents.core5</groupId><artifactId>httpcore5</artifactId></dependency></dependencies></project>",
            ),
        ],
    );

    let core_jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&httpcore.group)
        .join(&httpcore.artifact)
        .join(&httpcore.version)
        .join("core")
        .join(httpcore.sources_jar_name());
    write_sources_jar(
        &core_jar,
        &[(
            "org/apache/hc/core5/util/TimeValue.java",
            "package org.apache.hc.core5.util;\n\npublic class TimeValue {}\n",
        )],
    );

    let repos = Repos {
        gradle_cache,
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: false,
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_coordinate_closure_into_workspace(&root_coord, &repos, &extract_root);

    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    let src = "package app\n\
               import org.apache.hc.client5.http.config.ConnectionConfig\n\
               import org.apache.hc.core5.util.TimeValue\n\
               \n\
               fun main(config: ConnectionConfig, ttl: TimeValue) {}\n";
    ws.open(key.clone(), src.to_string());

    assert_goto_into_library(
        &mut ws,
        &key,
        src,
        "ConnectionConfig",
        "ConnectionConfig.java",
    );
    assert_goto_into_library(&mut ws, &key, src, "TimeValue", "TimeValue.java");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn goto_into_indexed_jdk_source() {
    let tmp = unique_tmp("jdkgoto");
    let src_zip = tmp.join("jdk/lib/src.zip");
    write_sources_jar(
        &src_zip,
        &[(
            "java.sql/java/sql/Connection.java",
            "package java.sql;\n\npublic interface Connection {\n    void close();\n}\n",
        )],
    );

    let extract_root = tmp.join("extracted");
    let mut kotlin = KotlinParser::new();
    let mut java = JavaParser::new();
    let batches = deps::resolve_jdk_sources(&src_zip, &extract_root, &mut kotlin, &mut java);
    assert!(!batches.is_empty(), "expected indexed JDK source files");

    let mut ws = Workspace::new();
    for batch in batches {
        ws.index
            .replace_file(&batch.file, batch.symbols, ktlsp::index::Tier::Durable);
    }

    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    let src = "package app\n\
               import java.sql.Connection\n\
               \n\
               class UsesConnection(private val conn: Connection)\n";
    ws.open(key.clone(), src.to_string());

    assert_goto_into_library(&mut ws, &key, src, "Connection", "Connection.java");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn goto_into_android_sdk_sources_from_kotlin_import() {
    let tmp = unique_tmp("androidgoto");
    let gradle = tmp.join("apps/mobile/android");
    let catalog = gradle.join("gradle/libs.versions.toml");
    let sdk = tmp.join("fake-android-sdk");
    let bitmap = sdk.join("sources/android-36/android/graphics/Bitmap.java");
    fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    fs::create_dir_all(bitmap.parent().unwrap()).unwrap();
    fs::write(gradle.join("settings.gradle"), "").unwrap();
    fs::write(gradle.join("gradlew"), "").unwrap();
    fs::write(&catalog, "[versions]\ncompileSdk = \"36\"\n").unwrap();
    fs::write(
        gradle.join("local.properties"),
        format!("sdk.dir={}\n", sdk.display()),
    )
    .unwrap();
    fs::write(
        &bitmap,
        "package android.graphics; public final class Bitmap { public int getWidth() { return 0; } }",
    )
    .unwrap();

    let mut java = JavaParser::new();
    let batches = deps::resolve_android_imports(
        &tmp,
        &["android.graphics.Bitmap".to_string()],
        &tmp.join("extracted"),
        &mut java,
    );
    assert_eq!(batches.len(), 1);

    let mut ws = Workspace::new();
    for batch in batches {
        ws.index
            .replace_file(&batch.file, batch.symbols, ktlsp::index::Tier::Durable);
    }
    let key = tmp.join("Main.kt").to_string_lossy().into_owned();
    let src = "package app\nimport android.graphics.Bitmap\nfun use(value: Bitmap) = value\n";
    ws.open(key.clone(), src.to_string());

    let import_offset = src.find("Bitmap").unwrap();
    let import_defs = ws.goto_definition(&key, import_offset);
    assert_eq!(import_defs.len(), 1);
    assert!(std::path::Path::new(&import_defs[0].file).ends_with("android/graphics/Bitmap.java"));
    assert_goto_into_library(&mut ws, &key, src, "Bitmap", "android/graphics/Bitmap.java");

    let _ = fs::remove_dir_all(&tmp);
}

/// Stage B: supertype + extension data recorded by the indexer must survive into the **Durable**
/// tier, so member completion on a user-file receiver of a library type offers inherited members
/// and library extensions. This proves the supertype/extension index is populated for libraries
/// (parsed through the same `extract_symbols` as project files).
#[test]
fn member_completion_into_indexed_library() {
    let tmp = unique_tmp("libcomplete");
    let coord = Coordinate::parse("acme:widgets:2.0").unwrap();

    let gradle_cache = tmp.join("gradle/caches");
    let jar = gradle_cache
        .join("modules-2/files-2.1")
        .join(&coord.group)
        .join(&coord.artifact)
        .join(&coord.version)
        .join("cafebabe")
        .join(coord.sources_jar_name());
    // A base class + a subclass + a top-level extension on the base, all in the library.
    write_sources_jar(
        &jar,
        &[(
            "acme/ui/Widgets.kt",
            "package acme.ui\n\
             \n\
             open class View {\n\
             \x20\x20\x20\x20fun render() {}\n\
             }\n\
             \n\
             class Button : View() {\n\
             \x20\x20\x20\x20fun click() {}\n\
             }\n\
             \n\
             fun View.highlight() {}\n",
        )],
    );

    let repos = Repos {
        gradle_cache,
        m2: tmp.join("m2"),
        central_base: "http://127.0.0.1:0/unused".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: false,
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_into_workspace(&coord, &repos, &extract_root);

    // A user file constructing the library `Button` and completing after a dot.
    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    // `b.` is a bare trailing dot — exercises the placeholder-recovery path.
    let src = "package app\n\
               import acme.ui.Button\n\
               \n\
               fun main() {\n\
               \x20\x20\x20\x20val b = Button()\n\
               \x20\x20\x20\x20b.\n\
               }\n";
    ws.open(key.clone(), src.to_string());

    let dot = src.rfind("b.").unwrap() + "b.".len();
    let labels: std::collections::HashSet<String> = ws
        .complete(&key, dot, true)
        .expect("member completion should resolve the library type")
        .items
        .into_iter()
        .map(|c| c.label)
        .collect();
    assert!(labels.contains("click"), "own member: {labels:?}");
    assert!(
        labels.contains("render"),
        "inherited from View (Durable supertype walk): {labels:?}"
    );
    assert!(
        labels.contains("highlight"),
        "library extension on View: {labels:?}"
    );

    let _ = fs::remove_dir_all(&tmp);
}

/// Real download path: fetch a small sources jar from Maven Central, index it, and resolve goto.
/// Network-dependent, so ignored by default. Run with: `cargo test -- --ignored download`.
#[test]
#[ignore]
fn download_from_maven_central_and_goto() {
    let tmp = unique_tmp("libdownload");
    // org.jetbrains:annotations publishes a small sources jar containing Java sources.
    let coord = Coordinate::parse("org.jetbrains:annotations:24.1.0").unwrap();
    let repos = Repos {
        gradle_cache: tmp.join("empty-gradle"), // force a cache miss
        m2: tmp.join("empty-m2"),
        central_base: "https://repo1.maven.org/maven2".to_string(),
        download_dir: tmp.join("dl"),
        allow_download: true,
    };
    let extract_root = tmp.join("extracted");
    let mut ws = index_into_workspace(&coord, &repos, &extract_root);

    let key = tmp.join("app/Main.kt").to_string_lossy().into_owned();
    // `@NotNull` is the canonical annotation in this artifact (package org.jetbrains.annotations).
    let src = "package app\n\
               import org.jetbrains.annotations.NotNull\n\
               \n\
               fun use(x: NotNull) {}\n";
    ws.open(key.clone(), src.to_string());
    assert_goto_into_library(&mut ws, &key, src, "NotNull", ".java");

    let _ = fs::remove_dir_all(&tmp);
}

#[test]
fn source_encoding_policy_refresh_preserves_dirty_source_then_restores_utf8_goto_spans() {
    for navigate_before_close in [false, true] {
        let tmp = unique_tmp(&format!("encoding_dirty_close_{navigate_before_close}"));
        let (coordinate, repos, jar) = snapshot_source(&tmp);
        let pom = jar.parent().unwrap().join("snapshot-1-SNAPSHOT.pom");
        let metadata = |encoding| {
            format!("<project><groupId>fixture</groupId><artifactId>snapshot</artifactId><version>1-SNAPSHOT</version><properties><project.build.sourceEncoding>{encoding}</project.build.sourceEncoding></properties></project>")
        };
        let raw = "package api;\n// Bj\u{f6}rn\npublic class Useful { public int target() { return 1; } }\n";
        write_sources_jar(&jar, &[("api/Useful.java", raw)]);
        fs::write(&pom, metadata("UTF-8")).unwrap();
        let extract = tmp.join("extracted");
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let first = deps::coordinate_source(&coordinate, &repos, &extract).unwrap();
        let batches = deps::resolve_library_source(&first, &mut kotlin, &mut java).unwrap();
        let file = batches[0].file.clone();
        assert!(deps::materialize_source_file(Path::new(&file)));
        let mut ws = Workspace::new();
        for batch in batches {
            assert!(ws.index_dependency_batch(batch));
        }
        let dirty =
            "package api; public class Useful { public int editorUnsaved() { return 7; } }\n";
        ws.open(file.clone(), raw.into());
        ws.change(&file, dirty.into());
        fs::write(&pom, metadata("ISO-8859-1")).unwrap();
        let second = deps::coordinate_source(&coordinate, &repos, &extract).unwrap();
        assert_ne!(first.identity(), second.identity());
        for _ in 0..2 {
            for batch in deps::resolve_library_source(&second, &mut kotlin, &mut java).unwrap() {
                assert!(!ws.index_dependency_batch(batch));
            }
            assert_eq!(ws.doc_text(&file).as_deref(), Some(dirty));
            assert!(!Path::new(&file).exists());
        }
        let expected = raw
            .as_bytes()
            .iter()
            .copied()
            .map(char::from)
            .collect::<String>();
        if navigate_before_close {
            let client = "import api.Useful; class Use { int use(Useful helper) { return helper.editorUnsaved(); } }";
            let key = ktlsp::paths::file_key(&tmp.join("Use.java"));
            ws.open(key.clone(), client.into());
            assert_eq!(
                ws.goto_definition(&key, client.find("editorUnsaved").unwrap())
                    .len(),
                1
            );
            assert_eq!(ws.doc_text(&file).as_deref(), Some(dirty));
            assert_eq!(fs::read_to_string(&file).unwrap(), expected);
        }
        ws.close(&file);
        assert_eq!(ws.doc_text(&file).as_deref(), Some(expected.as_str()));
        assert!(!ws
            .index
            .entries_for_file(&file)
            .iter()
            .any(|entry| entry.sym.name == "editorUnsaved"));
        let client =
            "import api.Useful; class Use { int use(Useful helper) { return helper.target(); } }";
        let key = ktlsp::paths::file_key(&tmp.join("Use.java"));
        ws.open(key.clone(), client.into());
        assert_goto_into_library(&mut ws, &key, client, "target", "api/Useful.java");
        fs::remove_dir_all(tmp).unwrap();
    }
}
