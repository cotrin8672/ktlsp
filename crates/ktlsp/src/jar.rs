//! Extract Kotlin/Java sources from a `-sources.jar` (a JAR is a ZIP).

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use anyhow::Context;

/// Cap on source files extracted from a single jar — a defensive bound against a pathological or
/// malicious archive with an absurd number of entries.
const MAX_SOURCE_FILES: usize = 50_000;

/// Cap on the per-entry capacity hint taken from the archive's declared uncompressed size, so a
/// forged header cannot trigger an absurd upfront allocation.
const MAX_ENTRY_SIZE_HINT: u64 = 64 << 20;

/// Open `jar` as a `ZipArchive` over its full in-memory bytes. Indexing touches every entry, so
/// one upfront read beats the seek-per-entry storm a raw `File` produces (dominant cost in
/// cold-index flamegraphs).
fn open_archive(jar: &Path) -> anyhow::Result<zip::ZipArchive<io::Cursor<Vec<u8>>>> {
    let bytes = fs::read(jar).with_context(|| format!("open jar {}", jar.display()))?;
    zip::ZipArchive::new(io::Cursor::new(bytes))
        .with_context(|| format!("read jar {}", jar.display()))
}

/// Empty compatibility artifacts need no source definitions. Metadata and nested archives do not
/// prove emptiness, and unreadable archives stay errors.
/// Proof work is bounded to 50,000 entries and one 16 MiB descriptor; the compressed archive is
/// still loaded in full by the shared archive reader.
pub fn may_contain_definitions(jar: &Path) -> anyhow::Result<bool> {
    let mut archive = open_archive(jar)?;
    if archive.len() > MAX_SOURCE_FILES {
        return Ok(true);
    }
    let mut module = false;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name();
        if name.ends_with(".class") {
            if module
                || !module_descriptor_path(name)
                || entry.size() > ktcore::classfile::MAX_CLASS_FILE_BYTES as u64
            {
                return Ok(true);
            }
            let mut bytes = Vec::with_capacity(entry.size().min(8192) as usize);
            entry
                .by_ref()
                .take(ktcore::classfile::MAX_CLASS_FILE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .with_context(|| {
                    format!(
                        "read module descriptor {} from {}",
                        entry.name(),
                        jar.display()
                    )
                })?;
            if bytes.len() > ktcore::classfile::MAX_CLASS_FILE_BYTES {
                return Ok(true);
            }
            anyhow::ensure!(
                bytes.len() as u64 == entry.size(),
                "module descriptor size mismatch"
            );
            if !matches!(ktcore::classfile::is_requires_only_module(&bytes), Ok(true)) {
                return Ok(true);
            }
            module = true;
            continue;
        }
        if [
            ".kotlin_metadata",
            ".kotlin_module",
            ".kotlin_builtins",
            ".knm",
            ".klib",
            ".jar",
            ".zip",
        ]
        .iter()
        .any(|extension| name.ends_with(extension))
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn module_descriptor_path(name: &str) -> bool {
    if name == "module-info.class" {
        return true;
    }
    name.strip_prefix("META-INF/versions/")
        .and_then(|name| name.strip_suffix("/module-info.class"))
        .is_some_and(|version| {
            !version.starts_with('0')
                && version.bytes().all(|ch| ch.is_ascii_digit())
                && version.parse::<u16>().is_ok_and(|version| version >= 9)
        })
}

/// Open an archive over bytes shared across shard workers — one allocation feeds every shard's
/// own `ZipArchive` state (central-directory cursors are per-worker, the bytes are not copied).
pub fn archive_shared(
    bytes: std::sync::Arc<[u8]>,
    jar: &Path,
) -> anyhow::Result<zip::ZipArchive<io::Cursor<std::sync::Arc<[u8]>>>> {
    zip::ZipArchive::new(io::Cursor::new(bytes))
        .with_context(|| format!("read jar {}", jar.display()))
}

/// Extract every `.kt`/`.java` entry from `jar` into `out_dir`, preserving the in-jar directory
/// layout. Returns the extracted file paths. Safe against zip-slip: entries whose normalized path
/// would escape `out_dir` (via `..`, absolute paths, or symlink relays) are skipped.
pub fn extract_sources(jar: &Path, out_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut archive = open_archive(jar)?;

    let mut extracted = Vec::new();
    for i in 0..archive.len() {
        if extracted.len() >= MAX_SOURCE_FILES {
            tracing::warn!(
                "jar {} has more than {MAX_SOURCE_FILES} source files; truncating",
                jar.display()
            );
            break;
        }
        let mut entry = archive.by_index(i)?;

        // `enclosed_name` strips a leading `/`, collapses `..`, and returns None for anything that
        // would escape the root. zip >= 2.3 also fixes the symlink-relay zip-slip (CVE-2025-29787).
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if !matches!(
            rel.extension().and_then(|e| e.to_str()),
            Some("kt") | Some("java")
        ) {
            continue;
        }

        let out_path = out_dir.join(&rel);
        // Belt-and-suspenders: confirm the resolved path stays under out_dir.
        if !out_path.starts_with(out_dir) {
            continue;
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out)?;
        extracted.push(out_path);
    }
    Ok(extracted)
}

/// Visit every Kotlin/Java source entry in `jar` without first materializing the whole archive on
/// disk. `out_dir` determines the stable path reported to callers for each entry; that path can
/// later be materialized with [`extract_source_file`] when an editor needs to open it.
pub fn visit_sources(
    jar: &Path,
    out_dir: &Path,
    mut visit: impl FnMut(&Path, &str),
) -> anyhow::Result<usize> {
    let mut archive = open_archive(jar)?;

    let mut visited = 0;
    for i in 0..archive.len() {
        if visited >= MAX_SOURCE_FILES {
            tracing::warn!(
                "jar {} has more than {MAX_SOURCE_FILES} source files; truncating",
                jar.display()
            );
            break;
        }
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if !matches!(
            rel.extension().and_then(|e| e.to_str()),
            Some("kt") | Some("java")
        ) {
            continue;
        }

        let path = out_dir.join(&rel);
        if !path.starts_with(out_dir) {
            continue;
        }
        let mut text = String::with_capacity(entry.size().min(MAX_ENTRY_SIZE_HINT) as usize);
        entry
            .read_to_string(&mut text)
            .with_context(|| format!("read source {} from {}", rel.display(), jar.display()))?;
        visit(&path, &text);
        visited += 1;
    }
    Ok(visited)
}

/// Like [`visit_sources`], but visits only entries whose index satisfies `index % shards ==
/// shard`, so several workers can index one large archive (the JDK `src.zip`) in parallel over
/// shared, already-read bytes.
pub fn visit_sources_shard(
    bytes: std::sync::Arc<[u8]>,
    jar: &Path,
    out_dir: &Path,
    shard: usize,
    shards: usize,
    mut visit: impl FnMut(&Path, &str),
) -> anyhow::Result<usize> {
    let mut archive = archive_shared(bytes, jar)?;

    let mut visited = 0;
    for i in (shard..archive.len()).step_by(shards) {
        if visited >= MAX_SOURCE_FILES {
            tracing::warn!(
                "jar {} shard {shard} has more than {MAX_SOURCE_FILES} source files; truncating",
                jar.display()
            );
            break;
        }
        let mut entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if !matches!(
            rel.extension().and_then(|e| e.to_str()),
            Some("kt") | Some("java")
        ) {
            continue;
        }

        let path = out_dir.join(&rel);
        if !path.starts_with(out_dir) {
            continue;
        }
        let mut text = String::with_capacity(entry.size().min(MAX_ENTRY_SIZE_HINT) as usize);
        entry
            .read_to_string(&mut text)
            .with_context(|| format!("read source {} from {}", rel.display(), jar.display()))?;
        visit(&path, &text);
        visited += 1;
    }
    Ok(visited)
}

/// Materialize one source entry from `jar` at its stable path below `out_dir`.
///
/// Returns `Ok(false)` when no matching source entry exists. Both the requested path and archive
/// entry are validated so an on-disk marker cannot be used to write outside the cache root.
pub fn extract_source_file(jar: &Path, out_dir: &Path, path: &Path) -> anyhow::Result<bool> {
    let rel = match path.strip_prefix(out_dir) {
        Ok(rel)
            if matches!(
                rel.extension().and_then(|e| e.to_str()),
                Some("kt") | Some("java")
            ) =>
        {
            rel
        }
        _ => return Ok(false),
    };
    let mut archive = open_archive(jar)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let Some(entry_rel) = entry.enclosed_name() else {
            continue;
        };
        if entry_rel != rel {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(path)?;
        io::copy(&mut entry, &mut out)?;
        return Ok(true);
    }
    Ok(false)
}

/// Collect already-extracted `.kt`/`.java` files under a directory (for reusing a prior extraction).
pub fn collect_sources(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in walkdir::WalkDir::new(dir) {
        let entry = entry.with_context(|| format!("enumerate sources {}", dir.display()))?;
        if entry.file_type().is_file()
            && matches!(
                entry.path().extension().and_then(|e| e.to_str()),
                Some("kt") | Some("java")
            )
        {
            paths.push(entry.into_path());
        }
    }
    Ok(paths)
}

/// Synthesize minimal Java source stubs for top-level `.class` entries in a binary jar.
///
/// This is intentionally shallow: it gives ktlsp parseable class declarations for goto/import
/// targets when a local Gradle jar has no source artifact. Members are not decompiled.
pub fn extract_class_stubs(jar: &Path, out_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut archive = open_archive(jar)?;
    let mut classes = BTreeSet::new();

    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if rel.extension().and_then(|e| e.to_str()) != Some("class") {
            continue;
        }
        let normalized = rel.to_string_lossy().replace('\\', "/");
        let Some(class_path) = normalized.strip_suffix(".class") else {
            continue;
        };
        if class_path == "module-info" || class_path.ends_with("/module-info") {
            continue;
        }
        if class_path == "package-info" || class_path.ends_with("/package-info") {
            continue;
        }
        let top_level = class_path.split('$').next().unwrap_or(class_path);
        if top_level.rsplit('/').next().is_some_and(is_java_identifier) {
            classes.insert(top_level.to_string());
        }
    }

    fs::create_dir_all(out_dir)?;
    let mut out = Vec::new();
    for class_path in classes {
        let mut parts = class_path.rsplitn(2, '/');
        let name = parts.next().unwrap_or_default();
        let package_path = parts.next().unwrap_or_default();
        let package = package_path.replace('/', ".");
        let rel_path = format!("{}.java", class_path);
        let path = out_dir.join(rel_path);
        if !path.starts_with(out_dir) {
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let src = if package.is_empty() {
            format!("public class {name} {{\n}}\n")
        } else {
            format!("package {package};\n\npublic class {name} {{\n}}\n")
        };
        fs::write(&path, src)?;
        out.push(path);
    }
    Ok(out)
}

fn is_java_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    fn write_jar(path: &Path, entries: &[(&str, &str)]) {
        let entries: Vec<_> = entries
            .iter()
            .map(|(name, body)| (*name, body.as_bytes()))
            .collect();
        write_binary_jar(path, &entries);
    }

    fn write_binary_jar(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, body) in entries {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(body).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn requires_only_selected_binaries_have_no_ordinary_definition_surface() {
        let fixtures: &[(&str, &[u8])] = &[
            (
                "kotlin-stdlib-jdk7-1.8.21",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk7-1.8.21.jar"),
            ),
            (
                "kotlin-stdlib-jdk8-1.8.21",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk8-1.8.21.jar"),
            ),
            (
                "kotlin-stdlib-jdk7-2.4.0",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk7-2.4.0.jar"),
            ),
            (
                "kotlin-stdlib-jdk8-2.4.0",
                include_bytes!("../tests/fixtures/module-only/kotlin-stdlib-jdk8-2.4.0.jar"),
            ),
            (
                "kotlinx-coroutines-jdk8-1.11.0",
                include_bytes!("../tests/fixtures/module-only/kotlinx-coroutines-jdk8-1.11.0.jar"),
            ),
        ];
        let root =
            std::env::temp_dir().join(format!("ktlsp_module_only_selected_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let mut possible = Vec::new();
        for (name, bytes) in fixtures {
            let path = root.join(format!("{name}.jar"));
            fs::write(&path, bytes).unwrap();
            if may_contain_definitions(&path).unwrap() {
                possible.push(*name);
            }
        }
        fs::remove_dir_all(root).unwrap();
        assert!(
            possible.is_empty(),
            "requires-only selected artifacts still treated as ordinary API: {possible:?}"
        );
    }

    #[test]
    fn module_only_classification_preserves_possible_definition_inputs() {
        let module = include_bytes!(
            "../../ktcore/tests/fixtures/classfile/module-only/javac-empty-module.class"
        )
        .as_slice();
        let plain = include_bytes!(
            "../../ktcore/tests/fixtures/classfile/module-only/ordinary-as-module-info.class"
        )
        .as_slice();
        let root = std::env::temp_dir().join(format!("ktlsp_module_shapes_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("test.jar");
        for name in ["module-info.class", "META-INF/versions/9/module-info.class"] {
            write_binary_jar(&path, &[(name, module)]);
            assert!(!may_contain_definitions(&path).unwrap(), "{name}");
        }
        for name in [
            "demo/module-info.class",
            "META-INF/versions/8/module-info.class",
            "META-INF/versions/09/module-info.class",
            "META-INF/versions/9/extra/module-info.class",
        ] {
            write_binary_jar(&path, &[(name, module)]);
            assert!(may_contain_definitions(&path).unwrap(), "{name}");
        }
        for (name, bytes) in [
            ("Plain.class", plain),
            ("module-info.class", plain),
            ("module-info.class", b"not a descriptor".as_slice()),
        ] {
            write_binary_jar(&path, &[(name, bytes)]);
            assert!(may_contain_definitions(&path).unwrap(), "{name}");
        }
        for name in [
            "Plain.class",
            "META-INF/versions/9/module-info.class",
            "native.knm",
            "native.klib",
            "code.kotlin_metadata",
            "code.kotlin_module",
            "code.kotlin_builtins",
            "nested.jar",
            "nested.zip",
        ] {
            write_binary_jar(&path, &[("module-info.class", module), (name, plain)]);
            assert!(may_contain_definitions(&path).unwrap(), "{name}");
        }
        write_binary_jar(
            &path,
            &[
                ("module-info.class", module),
                ("META-INF/versions/9/module-info.class", module),
            ],
        );
        assert!(
            may_contain_definitions(&path).unwrap(),
            "multiple descriptors stay unsupported"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn module_descriptor_io_errors_and_size_limits_are_conservative() {
        let module = include_bytes!(
            "../../ktcore/tests/fixtures/classfile/module-only/javac-empty-module.class"
        )
        .as_slice();
        let root = std::env::temp_dir().join(format!("ktlsp_module_io_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("test.jar");
        write_binary_jar(&path, &[("module-info.class", module)]);
        let valid = fs::read(&path).unwrap();
        let start = valid
            .windows(module.len())
            .position(|bytes| bytes == module)
            .unwrap();
        let mut bad_crc = valid.clone();
        bad_crc[start + module.len() - 1] ^= 1;
        fs::write(&path, bad_crc).unwrap();
        let error = may_contain_definitions(&path).unwrap_err();
        assert!(
            format!("{error:#}").to_lowercase().contains("checksum"),
            "{error:#}"
        );
        fs::write(&path, &valid[..valid.len() - 20]).unwrap();
        assert!(may_contain_definitions(&path).is_err());
        let central = valid
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        let mut forged_size = valid;
        forged_size[central + 24..central + 28]
            .copy_from_slice(&(ktcore::classfile::MAX_CLASS_FILE_BYTES as u32 + 1).to_le_bytes());
        fs::write(&path, forged_size).unwrap();
        assert!(may_contain_definitions(&path).unwrap());
        write_binary_jar(
            &path,
            &[(
                "module-info.class",
                &vec![0; ktcore::classfile::MAX_CLASS_FILE_BYTES + 1],
            )],
        );
        assert!(may_contain_definitions(&path).unwrap());
        let mut false_hint = fs::read(&path).unwrap();
        let central = false_hint
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        false_hint[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        fs::write(&path, false_hint).unwrap();
        assert!(
            may_contain_definitions(&path).unwrap(),
            "observed size is bounded even when the declared size is small"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn excessive_archive_entries_cannot_prove_emptiness() {
        let root = std::env::temp_dir().join(format!("ktlsp_module_work_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("test.jar");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        for index in 0..=MAX_SOURCE_FILES {
            zip.start_file(format!("res/{index}"), SimpleFileOptions::default())
                .unwrap();
        }
        zip.finish().unwrap();
        assert!(may_contain_definitions(&path).unwrap());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_enumeration_distinguishes_empty_and_missing_directories() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_source_enumeration_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        assert!(collect_sources(&root.join("missing")).is_err());
        assert!(collect_sources(&root).unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_sources_and_blocks_zip_slip() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_jar_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let jar = tmp.join("lib-sources.jar");
        let out = tmp.join("out");
        fs::create_dir_all(&tmp).unwrap();

        write_jar(
            &jar,
            &[
                ("demo/Lib.kt", "package demo\nfun libFunc() {}\n"),
                (
                    "demo/JThing.java",
                    "package demo;\npublic class JThing {}\n",
                ),
                ("META-INF/MANIFEST.MF", "ignored\n"), // non-source: skipped
                ("../escape.kt", "package evil\nfun bad() {}\n"), // zip-slip: skipped
            ],
        );

        let files = extract_sources(&jar, &out).unwrap();
        let names: Vec<String> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into())
            .collect();
        assert!(names.contains(&"Lib.kt".to_string()));
        assert!(names.contains(&"JThing.java".to_string()));
        assert_eq!(
            files.len(),
            2,
            "only the two source files, manifest + escape excluded"
        );
        // every extracted path stays under out_dir
        assert!(files.iter().all(|p| p.starts_with(&out)));
        // nothing escaped to the parent
        assert!(!tmp.join("escape.kt").exists());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn shard_visits_partition_entries() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_jar_shard_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let jar = tmp.join("lib-sources.jar");
        let out = tmp.join("out");
        fs::create_dir_all(&tmp).unwrap();

        let entries: Vec<(String, String)> = (0..7)
            .map(|i| {
                (
                    format!("demo/F{i}.kt"),
                    format!("package demo\nfun f{i}() {{}}\n"),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = entries
            .iter()
            .map(|(name, body)| (name.as_str(), body.as_str()))
            .collect();
        write_jar(&jar, &refs);

        let bytes: std::sync::Arc<[u8]> = std::sync::Arc::from(fs::read(&jar).unwrap());
        let mut seen = Vec::new();
        for shard in 0..3 {
            let mut shard_seen = Vec::new();
            let visited = visit_sources_shard(bytes.clone(), &jar, &out, shard, 3, |path, _| {
                shard_seen.push(path.file_name().unwrap().to_string_lossy().into_owned());
            })
            .unwrap();
            assert_eq!(visited, shard_seen.len());
            seen.extend(shard_seen);
        }
        seen.sort();
        let expected: Vec<String> = (0..7).map(|i| format!("F{i}.kt")).collect();
        assert_eq!(
            seen, expected,
            "shards partition the entry set without overlap"
        );

        let _ = fs::remove_dir_all(&tmp);
    }
}
