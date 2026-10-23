//! Extract Kotlin/Java sources from a `-sources.jar` (a JAR is a ZIP).

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::source_encoding::JavaSourceEncoding;
use anyhow::Context;

/// Cap on source files extracted from a single jar — a defensive bound against a pathological or
/// malicious archive with an absurd number of entries.
pub(crate) const MAX_SOURCE_FILES: usize = 50_000;

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

pub(crate) fn read_class_file(
    path: &Path,
    entry_name: &str,
    origin_id: &str,
) -> Result<Option<ktcore::classfile::ClassFileFacts>, String> {
    let read = || -> anyhow::Result<Option<ktcore::classfile::ClassFileFacts>> {
        anyhow::ensure!(
            !entry_name.is_empty()
                && Path::new(entry_name)
                    .components()
                    .all(|part| matches!(part, std::path::Component::Normal(_))),
            "class entry must be a relative path without parents"
        );
        let limit = ktcore::classfile::MAX_CLASS_FILE_BYTES;
        let decode = |reader: &mut dyn Read, size: u64| {
            anyhow::ensure!(size <= limit as u64, "class entry too large");
            let mut bytes = Vec::with_capacity(size.min(8192) as usize);
            reader.take(limit as u64 + 1).read_to_end(&mut bytes)?;
            anyhow::ensure!(
                bytes.len() <= limit && bytes.len() as u64 == size,
                "class entry size mismatch"
            );
            ktcore::classfile::decode(origin_id, &bytes).map(Some)
        };
        let metadata = fs::metadata(path)?;
        if metadata.is_dir() {
            let mut file = match fs::File::open(path.join(entry_name)) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.into()),
            };
            let metadata = file.metadata()?;
            anyhow::ensure!(metadata.is_file(), "class entry is not a file");
            return decode(&mut file, metadata.len());
        }
        anyhow::ensure!(
            metadata.is_file(),
            "classpath entry is not a file or directory"
        );
        let mut archive = open_archive(path)?;
        // shortcut: Version-specific class selection is unsupported; reject only the affected owner.
        anyhow::ensure!(
            !archive.file_names().any(|name| {
                name.strip_prefix("META-INF/versions/")
                    .and_then(|name| name.split_once('/'))
                    .is_some_and(|(version, name)| {
                        !version.is_empty()
                            && version.bytes().all(|byte| byte.is_ascii_digit())
                            && name == entry_name
                    })
            }),
            "versioned class entry is unsupported"
        );
        let mut entry = match archive.by_name(entry_name) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        anyhow::ensure!(!entry.is_dir(), "class entry is not a file");
        let size = entry.size();
        decode(&mut entry, size)
    };
    read().map_err(|error| format!("read class {entry_name} from {}: {error:#}", path.display()))
}

pub(crate) fn plain_java_classpath_entry(path: &Path, processors_disabled: bool) -> bool {
    const SERVICES: [&str; 2] = [
        "META-INF/services/javax.annotation.processing.Processor",
        "META-INF/services/com.sun.source.util.Plugin",
    ];
    const MANIFEST: &str = "META-INF/MANIFEST.MF";
    if path.is_dir() {
        for service in SERVICES {
            if processors_disabled && service == SERVICES[0] {
                continue;
            }
            match fs::symlink_metadata(path.join(service)) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                _ => return false,
            }
        }
        let manifest = path.join(MANIFEST);
        return match fs::symlink_metadata(&manifest) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Ok(_) => fs::File::open(manifest).is_ok_and(manifest_has_no_classpath),
            Err(_) => false,
        };
    }
    if !path.is_file()
        || !path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("jar"))
    {
        return false;
    }
    let Ok(mut archive) = open_archive(path) else {
        return false;
    };
    if archive
        .file_names()
        .any(|name| name != MANIFEST && name.eq_ignore_ascii_case(MANIFEST))
    {
        return false;
    }
    for service in SERVICES {
        if processors_disabled && service == SERVICES[0] {
            continue;
        }
        if !matches!(
            archive.by_name(service),
            Err(zip::result::ZipError::FileNotFound)
        ) {
            return false;
        }
    }
    let plain = match archive.by_name(MANIFEST) {
        Ok(entry) => manifest_has_no_classpath(entry),
        Err(zip::result::ZipError::FileNotFound) => true,
        Err(_) => false,
    };
    plain
}

fn manifest_has_no_classpath(reader: impl Read) -> bool {
    const LIMIT: u64 = 64 * 1024;
    let mut bytes = Vec::new();
    if reader.take(LIMIT + 1).read_to_end(&mut bytes).is_err() || bytes.len() as u64 > LIMIT {
        return false;
    }
    let Ok(manifest) = std::str::from_utf8(&bytes) else {
        return false;
    };
    !manifest.lines().any(|line| {
        line.split_once(':')
            .is_some_and(|(name, _)| name.trim().eq_ignore_ascii_case("Class-Path"))
    })
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
    extract_sources_with_encoding(jar, out_dir, JavaSourceEncoding::Utf8)
}

pub(crate) fn extract_sources_with_encoding(
    jar: &Path,
    out_dir: &Path,
    java_encoding: JavaSourceEncoding,
) -> anyhow::Result<Vec<PathBuf>> {
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
        let capacity = entry.size().min(MAX_ENTRY_SIZE_HINT) as usize;
        let text = read_source_text(&mut entry, capacity, &rel, java_encoding)
            .with_context(|| format!("read source {} from {}", rel.display(), jar.display()))?;
        write_source_text(&out_path, &text)?;
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
    visit: impl FnMut(&Path, &str),
) -> anyhow::Result<usize> {
    visit_sources_with_encoding(jar, out_dir, JavaSourceEncoding::Utf8, visit)
}

pub(crate) fn visit_sources_with_encoding(
    jar: &Path,
    out_dir: &Path,
    java_encoding: JavaSourceEncoding,
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
        let capacity = entry.size().min(MAX_ENTRY_SIZE_HINT) as usize;
        let text = read_source_text(&mut entry, capacity, &rel, java_encoding)
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
    visit: impl FnMut(&Path, &str),
) -> anyhow::Result<usize> {
    visit_sources_shard_with_encoding(
        bytes,
        jar,
        out_dir,
        shard,
        shards,
        JavaSourceEncoding::Utf8,
        visit,
    )
}

pub(crate) fn visit_sources_shard_with_encoding(
    bytes: std::sync::Arc<[u8]>,
    jar: &Path,
    out_dir: &Path,
    shard: usize,
    shards: usize,
    java_encoding: JavaSourceEncoding,
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
        let capacity = entry.size().min(MAX_ENTRY_SIZE_HINT) as usize;
        let text = read_source_text(&mut entry, capacity, &rel, java_encoding)
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
    extract_source_file_with_encoding(jar, out_dir, path, JavaSourceEncoding::Utf8)
}

pub(crate) fn extract_source_file_with_encoding(
    jar: &Path,
    out_dir: &Path,
    path: &Path,
    java_encoding: JavaSourceEncoding,
) -> anyhow::Result<bool> {
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
        let capacity = entry.size().min(MAX_ENTRY_SIZE_HINT) as usize;
        let text = read_source_text(&mut entry, capacity, rel, java_encoding)
            .with_context(|| format!("read source {} from {}", rel.display(), jar.display()))?;
        write_source_text(path, &text)?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn read_source_text(
    entry: &mut impl Read,
    capacity: usize,
    relative: &Path,
    java_encoding: JavaSourceEncoding,
) -> anyhow::Result<String> {
    let mut bytes = Vec::with_capacity(capacity);
    // Consume the complete ZIP entry, including CRC validation, before decoding or writing.
    entry.read_to_end(&mut bytes)?;
    let encoding = if relative.extension().and_then(|value| value.to_str()) == Some("java") {
        java_encoding
    } else {
        JavaSourceEncoding::Utf8
    };
    encoding.decode(bytes)
}

static SOURCE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub(crate) fn write_source_text(path: &Path, text: &str) -> anyhow::Result<()> {
    let sequence = SOURCE_WRITE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_extension(format!("ktlsp.{}.{sequence}.tmp", std::process::id()));
    // Failed create_new never owns the temporary and must not remove another writer's file.
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| -> io::Result<()> {
        file.write_all(text.as_bytes())?;
        file.flush()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    Ok(result?)
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

/// Materialize declaration stubs for top-level binary classes without a source artifact.
///
/// Ordinary Java methods are positive API facts only. Unsupported declarations keep the old
/// name-only fallback; these stubs never certify a complete source/API surface.
pub fn extract_class_stubs(
    jar: &Path,
    out_dir: &Path,
) -> anyhow::Result<
    Vec<(
        PathBuf,
        Option<Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>>,
    )>,
> {
    Ok(extract_class_stub_facts(jar, out_dir)?
        .into_iter()
        .map(|(path, owners)| {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            let members = owners.and_then(|mut owners| owners.remove(name));
            (path, members)
        })
        .collect())
}

pub(crate) fn extract_class_stub_facts(
    jar: &Path,
    out_dir: &Path,
) -> anyhow::Result<Vec<(PathBuf, Option<BinaryOwnerMembers>)>> {
    let mut out = Vec::new();
    visit_class_stub_facts(jar, out_dir, |path, source, owners| {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_source_text(path, source)?;
        out.push((path.to_path_buf(), owners));
        Ok(())
    })?;
    Ok(out)
}

/// Render the same bounded binary declarations without an eager disk round trip.
pub(crate) fn visit_class_stub_facts(
    jar: &Path,
    out_dir: &Path,
    visit: impl FnMut(&Path, &str, Option<BinaryOwnerMembers>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    visit_class_stub_facts_selected(jar, out_dir, None, visit)
}

pub(crate) fn extract_class_stub_file(
    jar: &Path,
    out_dir: &Path,
    path: &Path,
) -> anyhow::Result<bool> {
    let Ok(relative) = path.strip_prefix(out_dir) else {
        return Ok(false);
    };
    if relative.extension().and_then(|ext| ext.to_str()) != Some("java")
        || !relative
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Ok(false);
    }
    let mut found = false;
    visit_class_stub_facts_selected(jar, out_dir, Some(path), |path, source, _| {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_source_text(path, source)?;
        found = true;
        Ok(())
    })?;
    Ok(found)
}

fn visit_class_stub_facts_selected(
    jar: &Path,
    out_dir: &Path,
    selected: Option<&Path>,
    mut visit: impl FnMut(&Path, &str, Option<BinaryOwnerMembers>) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let bytes = fs::read(jar).with_context(|| format!("open jar {}", jar.display()))?;
    // zip's indexed map may coalesce duplicate central names. A non-ZIP64 EOCD count
    // must agree before it can prove a unique enclosing class; otherwise nested
    // members remain unavailable while existing top-level extraction is preserved.
    let declared_entries = bytes
        .windows(4)
        .rposition(|s| s == b"PK\x05\x06")
        .and_then(|at| {
            let header = bytes.get(at..at + 22)?;
            let comment = u16::from_le_bytes([header[20], header[21]]) as usize;
            if at + 22 + comment != bytes.len() || header[4..8] != [0, 0, 0, 0] {
                return None;
            }
            let disk = u16::from_le_bytes([header[8], header[9]]);
            let total = u16::from_le_bytes([header[10], header[11]]);
            (disk == total && total != u16::MAX).then_some(total as usize)
        });
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes))
        .with_context(|| format!("read jar {}", jar.display()))?;
    let unique_entries = declared_entries == Some(archive.len());
    let no_nested = std::collections::BTreeMap::new();
    anyhow::ensure!(
        archive.len() <= MAX_SOURCE_FILES,
        "too many binary jar entries"
    );
    let versioned: BTreeSet<_> = archive
        .file_names()
        .filter_map(|name| {
            let normalized = name.replace('\\', "/");
            let (version, owner) = normalized
                .strip_prefix("META-INF/versions/")?
                .split_once('/')?;
            (!version.is_empty() && version.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| owner.strip_suffix(".class").map(str::to_owned))
                .flatten()
        })
        .collect();
    let origin = crate::classpath::file_origin_id(jar);
    let mut classes = BTreeSet::new();
    let mut declarations = std::collections::BTreeMap::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i)?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        if entry.is_dir() || rel.extension().and_then(|e| e.to_str()) != Some("class") {
            continue;
        }
        let normalized = rel.to_string_lossy().replace('\\', "/");
        let Some(class_path) = normalized.strip_suffix(".class") else {
            continue;
        };
        if class_path.starts_with("META-INF/") || !class_path.split('/').all(is_java_identifier) {
            continue;
        }
        let leaf = class_path.rsplit('/').next().unwrap_or(class_path);
        let top_level = &class_path
            [..class_path.len() - leaf.len() + leaf.split('$').next().unwrap_or(leaf).len()];
        if !top_level.rsplit('/').next().is_some_and(is_java_identifier) {
            continue;
        }
        classes.insert(top_level.to_string());
        if versioned.contains(class_path) {
            continue;
        }
        // Normalized duplicate base entries cannot select one member surface by archive order.
        match declarations.entry(class_path.to_string()) {
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Some(i));
            }
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                entry.insert(None);
            }
        }
    }
    for class_path in classes {
        let mut parts = class_path.rsplitn(2, '/');
        let name = parts.next().unwrap_or_default();
        let package_path = parts.next().unwrap_or_default();
        let package = package_path.replace('/', ".");
        let path = out_dir.join(format!("{class_path}.java"));
        if !path.starts_with(out_dir) || selected.is_some_and(|selected| selected != path) {
            continue;
        }
        // Decode one bounded class at a time; a later error prevents the caller publishing/caching
        // partial facts. Atomic stub files can be repaired by the existing cache miss.
        let declaration = if let Some(index) = declarations.get(&class_path).copied().flatten() {
            match decoded_binary_entry(&mut archive, index, &origin, &class_path)? {
                Some(facts) => binary_owner_source(
                    &mut archive,
                    if unique_entries {
                        &declarations
                    } else {
                        &no_nested
                    },
                    &versioned,
                    &facts,
                    name,
                    None,
                    &[],
                    0,
                )?
                .map(|(source, owners)| (source, Some(owners))),
                None => None,
            }
        } else {
            None
        };
        let (src, unavailable) = declaration.unwrap_or_else(|| {
            if package.is_empty() {
                (format!("public class {name} {{\n}}\n"), None)
            } else {
                (
                    format!("package {package};\n\npublic class {name} {{\n}}\n"),
                    None,
                )
            }
        });
        visit(&path, &src, unavailable)?;
    }
    Ok(())
}

pub(crate) type BinaryOwnerMembers =
    std::collections::BTreeMap<String, Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>>;

fn decoded_binary_entry<R: Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    index: usize,
    origin: &str,
    expected: &str,
) -> anyhow::Result<Option<ktcore::classfile::ClassFileFacts>> {
    let mut entry = archive.by_index(index)?;
    let limit = ktcore::classfile::MAX_CLASS_FILE_BYTES;
    if entry.size() > limit as u64 {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(entry.size().min(8192) as usize);
    entry
        .by_ref()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit || bytes.len() as u64 != entry.size() {
        return Ok(None);
    }
    Ok(ktcore::classfile::decode(origin, &bytes)
        .ok()
        .filter(|facts| facts.identity.internal_name == expected))
}

/// Only exact reciprocal InnerClasses rows introduce lexical scopes. Archive paths narrow
/// lookup but never invent a containing class by splitting a dollar-bearing name.
fn binary_owner_source<R: Read + io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    indices: &std::collections::BTreeMap<String, Option<usize>>,
    versioned: &BTreeSet<String>,
    class: &ktcore::classfile::ClassFileFacts,
    owner_name: &str,
    source_file: Option<&str>,
    enclosing: &[&[ktcore::classfile::signature::SignatureParameter]],
    depth: usize,
) -> anyhow::Result<Option<(String, BinaryOwnerMembers)>> {
    if depth >= 64 {
        return Ok(None);
    }
    let own_name = owner_name.rsplit('.').next().unwrap();
    let Some((mut source, unavailable)) =
        java_declaration_stub_scoped(class, false, Some(own_name), enclosing, source_file)
    else {
        return Ok(None);
    };
    let signature = match &class.signature {
        Some(_) => match ktcore::classfile::signature::class_signature_scoped(class, enclosing) {
            Some(s) => Some(s),
            None => return Ok(None),
        },
        None => None,
    };
    let own_parameters = signature
        .as_ref()
        .map_or(&[][..], |s| s.type_parameters.as_slice());
    let layers: Vec<_> = std::iter::once(own_parameters)
        .chain(enclosing.iter().copied())
        .collect();
    let mut owners = BinaryOwnerMembers::from([(owner_name.to_owned(), unavailable)]);
    let mut names = std::collections::BTreeMap::new();
    for row in class
        .inner_classes
        .iter()
        .filter(|row| row.outer.as_deref() == Some(class.identity.internal_name.as_str()))
    {
        if let Some(name) = row.name.as_deref() {
            *names.entry(name).or_insert(0usize) += 1;
        }
    }
    let mut nested = Vec::new();
    for row in class
        .inner_classes
        .iter()
        .filter(|row| row.outer.as_deref() == Some(class.identity.internal_name.as_str()))
    {
        let Some(name) = row.name.as_deref() else {
            continue;
        };
        // Public nonstatic classes may be plain, final or abstract. Other InnerClasses kinds
        // cannot establish an enclosing-instance binding; the decoded class must agree below.
        if !is_java_identifier(name)
            || row.access_flags & 0x0001 == 0
            || row.access_flags & !(0x0001 | 0x0010 | 0x0400) != 0
            || row.access_flags & 0x0410 == 0x0410
            || names.get(name) != Some(&1)
        {
            continue;
        }
        if row.inner.as_ref() != format!("{}${name}", class.identity.internal_name)
            || versioned.contains(row.inner.as_ref())
        {
            continue;
        }
        let Some(index) = indices.get(row.inner.as_ref()).copied().flatten() else {
            continue;
        };
        let Some(child) =
            decoded_binary_entry(archive, index, &class.identity.origin_id, &row.inner)?
        else {
            continue;
        };
        if child.source_file != class.source_file
            || child.access_flags & !0x0020 != row.access_flags
        {
            continue;
        }
        let reciprocal: Vec<_> = child
            .inner_classes
            .iter()
            .filter(|r| r.inner == row.inner)
            .collect();
        if reciprocal.as_slice() != [row] {
            continue;
        }
        let Some((child_source, child_owners)) = binary_owner_source(
            archive,
            indices,
            versioned,
            &child,
            &format!("{owner_name}.{name}"),
            class.source_file.as_deref(),
            &layers,
            depth + 1,
        )?
        else {
            continue;
        };
        // Every binary declaration remains tagged by its exact owner, including omitted members.
        let Some(begin) = child_source.find("public ") else {
            continue;
        };
        nested.push(child_source[begin..].to_string());
        owners.extend(child_owners);
    }
    let Some(end) = source.rfind('}') else {
        return Ok(None);
    };
    for child in nested.into_iter().rev() {
        source.insert_str(end, &child);
    }
    Ok(Some((source, owners)))
}

#[cfg(test)]
fn java_class_stub(class: &ktcore::classfile::ClassFileFacts) -> Option<String> {
    java_class_stub_facts(class).map(|(source, _)| source)
}

fn java_class_stub_facts(
    class: &ktcore::classfile::ClassFileFacts,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    java_declaration_stub(class, false)
}

/// Only a pinned compiler SDK archive establishes that absent SourceFile and API annotations
/// describe Java declarations rather than an unknown source-language lowering.
pub(crate) fn selected_sdk_class_stub_facts(
    selection: &crate::classpath::SdkApiSelection,
    class: &ktcore::classfile::ClassFileFacts,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    (selection.release.is_some()
        && class.identity.origin_id == selection.origin()
        && selection
            .entries
            .get(class.identity.internal_name.as_str())
            .is_some_and(Option::is_some))
    .then(|| java_declaration_stub(class, true))
    .flatten()
}

fn java_declaration_stub(
    class: &ktcore::classfile::ClassFileFacts,
    selected_sdk: bool,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    java_declaration_stub_named(class, selected_sdk, None)
}

/// Only an already-proven nested identity may supply a source simple name.
pub(crate) fn selected_sdk_nested_stub_facts(
    selection: &crate::classpath::SdkApiSelection,
    class: &ktcore::classfile::ClassFileFacts,
    name: &str,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    if selection.release.is_none()
        || class.identity.origin_id != selection.origin()
        || !selection
            .entries
            .get(class.identity.internal_name.as_str())
            .is_some_and(Option::is_some)
    {
        return None;
    }
    let row = class
        .inner_classes
        .iter()
        .find(|row| row.inner.as_ref() == class.identity.internal_name)?;
    if row.name.as_deref() != Some(name)
        || !matches!(
            (row.access_flags, class.access_flags),
            (0x0609, 0x0601) | (0x4019, 0x4031)
        )
        || !is_java_identifier(name)
    {
        return None;
    }
    if class.access_flags & 0x4000 != 0 {
        return selected_sdk_enum_stub(class, name);
    }
    java_declaration_stub_named(class, true, Some(name))
}

/// A selected final enum has an exact implicit superclass and self-typed constants.
/// Reuse the ordinary bounded member renderer; enum syntax restores these implicit facts
/// through the same Java adapter used for authored source.
fn selected_sdk_enum_stub(
    class: &ktcore::classfile::ClassFileFacts,
    name: &str,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    if class.super_name.as_deref() != Some("java/lang/Enum")
        || !class.interfaces.is_empty()
        || class.signature.as_deref()
            != Some(format!("Ljava/lang/Enum<L{};>;", class.identity.internal_name).as_str())
    {
        return None;
    }
    let mut constants = Vec::new();
    let mut unique = std::collections::BTreeSet::new();
    for field in &class.fields {
        if field.access_flags & 0x4000 == 0 {
            continue;
        }
        if field.access_flags != 0x4019
            || field.descriptor.as_ref() != format!("L{};", class.identity.internal_name)
            || field.signature.is_some()
            || !field.attribute_names.is_empty()
            || !is_java_identifier(&field.name)
            || !unique.insert(field.name.as_ref())
        {
            return None;
        }
        constants.push(field.name.as_ref());
    }
    let mut declaration = class.clone();
    declaration.access_flags &= !0x4000;
    declaration.super_name = Some("java/lang/Object".into());
    declaration.signature = None;
    declaration
        .fields
        .retain(|field| field.access_flags & 0x4000 == 0);
    let (source, unavailable) = java_declaration_stub_named(&declaration, true, Some(name))?;
    let head = format!("public final class {name}");
    let source = source.replacen(&head, &format!("public static enum {name}"), 1);
    let opening = source.find("{\n")? + 2;
    let mut source = source;
    source.insert_str(opening, &format!("    {};\n", constants.join(", ")));
    Some((source, unavailable))
}

fn java_declaration_stub_named(
    class: &ktcore::classfile::ClassFileFacts,
    selected_sdk: bool,
    source_name: Option<&str>,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    java_declaration_stub_scoped(class, selected_sdk, source_name, &[], None)
}

fn java_declaration_stub_scoped(
    class: &ktcore::classfile::ClassFileFacts,
    selected_sdk: bool,
    source_name: Option<&str>,
    enclosing: &[&[ktcore::classfile::signature::SignatureParameter]],
    expected_source: Option<&str>,
) -> Option<(
    String,
    Vec<(String, ktcore::symbol::SymbolKind, Option<u8>)>,
)> {
    const PUBLIC: u16 = 0x0001;
    const INTERFACE: u16 = 0x0200;
    const ABSTRACT: u16 = 0x0400;
    let is_interface = class.access_flags & INTERFACE != 0;
    let (package, name) = class
        .identity
        .internal_name
        .rsplit_once('/')
        .unwrap_or(("", &class.identity.internal_name));
    if class.access_flags & PUBLIC == 0
        || class.access_flags & !(PUBLIC | 0x0010 | 0x0020 | INTERFACE | ABSTRACT) != 0
        || class.access_flags & 0x0410 == 0x0410
        || (class.source_file.is_some() || !selected_sdk)
            && class.source_file.as_deref()
                != Some(expected_source.unwrap_or(format!("{name}.java").as_str()))
        || class.attribute_names.iter().any(|attribute| {
            (!selected_sdk
                && matches!(
                    attribute.as_ref(),
                    "RuntimeVisibleAnnotations"
                        | "RuntimeInvisibleAnnotations"
                        | "RuntimeVisibleTypeAnnotations"
                        | "RuntimeInvisibleTypeAnnotations"
                ))
                || matches!(
                    attribute.as_ref(),
                    "Synthetic" | "Record" | "EnclosingMethod"
                )
        })
    {
        return None;
    }
    if is_interface
        && (class.access_flags != PUBLIC | INTERFACE | ABSTRACT
            || class.super_name.as_deref() != Some("java/lang/Object"))
    {
        return None;
    }
    let generic = match &class.signature {
        Some(_) => Some(ktcore::classfile::signature::class_signature_scoped(
            class, enclosing,
        )?),
        None => None,
    };
    let class_parameters = generic
        .as_ref()
        .map_or(&[][..], |signature| signature.type_parameters.as_slice());
    let lexical_parameters: Vec<_> = class_parameters
        .iter()
        .chain(enclosing.iter().flat_map(|s| s.iter()))
        .cloned()
        .collect();
    let type_parameters =
        java_signature_parameters(class_parameters, &lexical_parameters, !package.is_empty())?;
    // A Signature must agree with the erased hierarchy before its arguments become facts.
    let hierarchy_type = |internal: &str| -> Option<String> {
        if internal == class.identity.internal_name.as_str() || internal.contains('$') {
            return None;
        }
        let ty = ktcore::classfile::field_type(&format!("L{internal};"))?;
        if !package.is_empty() && ty.qualification.as_deref() == Some("") {
            return None;
        }
        java_descriptor_type(&ty, class_parameters, !package.is_empty())
    };
    let superclass = match class.super_name.as_deref() {
        Some("java/lang/Object") | None => None,
        Some(parent) => Some(match &generic {
            Some(signature) => {
                if parent == class.identity.internal_name || parent.contains('$') {
                    return None;
                }
                java_descriptor_type(&signature.superclass, class_parameters, !package.is_empty())?
            }
            None => hierarchy_type(parent)?,
        }),
    };
    let interfaces = match &generic {
        Some(signature) => {
            if class
                .interfaces
                .iter()
                .any(|internal| internal.as_ref() == class.identity.internal_name)
            {
                return None;
            }
            signature
                .interfaces
                .iter()
                .map(|ty| java_descriptor_type(ty, class_parameters, !package.is_empty()))
                .collect::<Option<Vec<_>>>()?
        }
        None => class
            .interfaces
            .iter()
            .map(|interface| hierarchy_type(interface))
            .collect::<Option<Vec<_>>>()?,
    };
    let mut unique_interfaces = std::collections::BTreeSet::new();
    if class
        .interfaces
        .iter()
        .any(|interface| !unique_interfaces.insert(interface))
    {
        return None;
    }
    // Reject public return-only overloads before unsupported descriptors/signatures are filtered.
    // A normal Java covariant bridge is not an additional source-callable overload.
    let mut shapes = std::collections::BTreeMap::new();
    for method in &class.methods {
        if method.access_flags & (PUBLIC | 0x0004) != 0
            && method.access_flags & (0x0040 | 0x1000) == 0
            && !method
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Synthetic")
        {
            let key = (method.name.as_ref(), method.descriptor.split_once(')')?.0);
            *shapes.entry(key).or_insert(0usize) += 1;
        }
    }
    let mut field_names = std::collections::BTreeMap::new();
    for field in &class.fields {
        if field.access_flags & (PUBLIC | 0x0004) != 0
            && field.access_flags & 0x1000 == 0
            && !field
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Synthetic")
        {
            *field_names.entry(field.name.as_ref()).or_insert(0usize) += 1;
        }
    }
    let mut fields = std::collections::BTreeMap::new();
    for field in &class.fields {
        if field.access_flags & (PUBLIC | 0x0004) == 0
            || is_interface && field.access_flags != PUBLIC | 0x0008 | 0x0010
            || is_interface
                && !selected_sdk
                && field.attribute_names.iter().any(|attribute| {
                    !matches!(
                        attribute.as_ref(),
                        "ConstantValue" | "Signature" | "Deprecated"
                    )
                })
            || field.access_flags & 0x0005 == 0x0005
            || field.access_flags & !(PUBLIC | 0x0004 | 0x0008 | 0x0010 | 0x0040 | 0x0080) != 0
            || field.access_flags & 0x0050 == 0x0050
            || !is_java_identifier(&field.name)
            || field_names.get(field.name.as_ref()) != Some(&1)
            || field
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Synthetic")
        {
            continue;
        }
        let scope = if field.access_flags & 0x0008 != 0 {
            &[][..]
        } else {
            lexical_parameters.as_slice()
        };
        let field_enclosing = if field.access_flags & 0x0008 != 0 {
            &[][..]
        } else {
            enclosing
        };
        let field_class = if field.access_flags & 0x0008 != 0 {
            &[][..]
        } else {
            class_parameters
        };
        let Some(ty) = (match &field.signature {
            Some(signature) => ktcore::classfile::signature::field_signature_scoped(
                signature,
                &field.descriptor,
                field_class,
                field_enclosing,
            ),
            None => ktcore::classfile::field_type(&field.descriptor),
        }) else {
            continue;
        };
        if !package.is_empty() && ty.qualification.as_deref() == Some("") {
            continue;
        }
        let Some(ty) = java_descriptor_type(&ty, scope, !package.is_empty()) else {
            continue;
        };
        let static_ = if field.access_flags & 0x0008 != 0 {
            "static "
        } else {
            ""
        };
        let final_ = if field.access_flags & 0x0010 != 0 {
            "final "
        } else {
            ""
        };
        let volatile = if field.access_flags & 0x0040 != 0 {
            "volatile "
        } else {
            ""
        };
        let transient = if field.access_flags & 0x0080 != 0 {
            "transient "
        } else {
            ""
        };
        fields.insert(
            field.name.as_ref(),
            format!(
                "    {} {static_}{final_}{volatile}{transient}{ty} {};\n",
                if field.access_flags & PUBLIC != 0 {
                    "public"
                } else {
                    "protected"
                },
                field.name
            ),
        );
    }
    let mut methods = std::collections::BTreeMap::new();
    for method in &class.methods {
        if method.access_flags & (PUBLIC | 0x0004) == 0
            || is_interface && !matches!(method.access_flags, 0x0401 | 0x0001 | 0x0009)
            || is_interface
                && !selected_sdk
                && method.attribute_names.iter().any(|attribute| {
                    !matches!(
                        attribute.as_ref(),
                        "Code" | "Signature" | "Exceptions" | "MethodParameters" | "Deprecated"
                    )
                })
            || method.access_flags & 0x0005 == 0x0005
            || method.access_flags
                & !(PUBLIC
                    | 0x0004
                    | 0x0008
                    | 0x0010
                    | 0x0020
                    | 0x0800
                    | ABSTRACT
                    | if selected_sdk { 0x0100 } else { 0 })
                != 0
            || method.access_flags & ABSTRACT != 0
                && (class.access_flags & ABSTRACT == 0 || method.access_flags & 0x0938 != 0)
            || !is_java_identifier(&method.name)
            || method
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Synthetic")
        {
            continue;
        }
        let key = (method.name.as_ref(), method.descriptor.split_once(')')?.0);
        if shapes.get(&key) != Some(&1) {
            continue;
        }
        let class_scope = if method.access_flags & 0x0008 != 0 {
            &[][..]
        } else {
            class_parameters
        };
        let method_enclosing = if method.access_flags & 0x0008 != 0 {
            &[][..]
        } else {
            enclosing
        };
        let generic_method = match &method.signature {
            Some(signature) => match ktcore::classfile::signature::method_signature_scoped(
                signature,
                &method.descriptor,
                class_scope,
                method_enclosing,
            ) {
                Some(signature) => Some(signature),
                None => continue,
            },
            None => None,
        };
        let method_parameters = generic_method
            .as_ref()
            .map_or(&[][..], |signature| signature.type_parameters.as_slice());
        let scope: Vec<_> = method_parameters
            .iter()
            .chain(class_scope)
            .chain(method_enclosing.iter().flat_map(|s| s.iter()))
            .cloned()
            .collect();
        let Some(generic_declaration) =
            java_signature_parameters(method_parameters, &scope, !package.is_empty())
        else {
            continue;
        };
        let (parameters, returns) = match generic_method {
            Some(signature) => (signature.parameters, signature.returns),
            None => match ktcore::classfile::method_types_with_inner_classes(
                &method.descriptor,
                &class.inner_classes,
            ) {
                Some(types) => types,
                None => continue,
            },
        };
        // A named Java package cannot spell an unnamed-package nominal type without rebinding it.
        if !package.is_empty()
            && parameters
                .iter()
                .chain(returns.iter())
                .any(|ty| ty.qualification.as_deref() == Some(""))
        {
            continue;
        }
        let Some(parameters) = parameters
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                Some(format!(
                    "{} p{i}",
                    java_descriptor_type(ty, &scope, !package.is_empty())?
                ))
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let returns = match returns {
            Some(ty) => match java_descriptor_type(&ty, &scope, !package.is_empty()) {
                Some(ty) => ty,
                None => continue,
            },
            None => "void".to_string(),
        };
        let static_ = if method.access_flags & 0x0008 != 0 {
            "static "
        } else {
            ""
        };
        let final_ = if method.access_flags & 0x0010 != 0 {
            "final "
        } else {
            ""
        };
        let access = if method.access_flags & PUBLIC != 0 {
            "public"
        } else {
            "protected"
        };
        let default_ = if is_interface && method.access_flags & (0x0008 | ABSTRACT) == 0 {
            "default "
        } else {
            ""
        };
        let declaration = if method.access_flags & ABSTRACT != 0 {
            format!(
                "    {access} abstract {generic_declaration}{returns} {}({});\n",
                method.name,
                parameters.join(", ")
            )
        } else {
            format!(
                "    {access} {default_}{static_}{final_}{generic_declaration}{returns} {}({}) {{ throw null; }}\n",
                method.name,
                parameters.join(", ")
            )
        };
        methods.insert(key, declaration);
    }
    // Keep the interface identity and hierarchy; unsupported members block only their own names.
    let mut unavailable_fields = std::collections::BTreeSet::new();
    for field in &class.fields {
        if is_java_identifier(&field.name)
            && field.access_flags & 0x1000 == 0
            && !fields.contains_key(field.name.as_ref())
        {
            unavailable_fields.insert(field.name.to_string());
        }
    }
    let mut unavailable_methods = std::collections::BTreeSet::new();
    for method in &class.methods {
        if is_java_identifier(&method.name)
            && method.access_flags & (0x0002 | 0x0040 | 0x1000) == 0
            && !method
                .attribute_names
                .iter()
                .any(|name| name.as_ref() == "Synthetic")
            && !method
                .descriptor
                .split_once(')')
                .is_some_and(|(parameters, _)| {
                    methods.contains_key(&(method.name.as_ref(), parameters))
                })
        {
            let arity = (method.access_flags & 0x0080 == 0)
                .then(|| ktcore::classfile::method_parameter_count(&method.descriptor))
                .flatten();
            unavailable_methods.insert((method.name.to_string(), arity));
        }
    }
    let unavailable = unavailable_methods
        .into_iter()
        .map(|(name, arity)| (name, ktcore::symbol::SymbolKind::Function, arity))
        .chain(
            unavailable_fields
                .into_iter()
                .map(|name| (name, ktcore::symbol::SymbolKind::Property, None)),
        )
        .collect();
    let name = source_name.unwrap_or(name);
    let mut source =
        "// Binary declaration stub; initializers and method bodies are unavailable.\n".to_string();
    if !package.is_empty() {
        source.push_str(&format!("package {};\n\n", package.replace('/', ".")));
    }
    let modifier = if class.access_flags & 0x0010 != 0 {
        "final "
    } else if class.access_flags & 0x0400 != 0 {
        "abstract "
    } else {
        ""
    };
    if is_interface {
        source.push_str(&format!(
            "public interface {name}{}",
            type_parameters.trim_end()
        ));
    } else {
        source.push_str(&format!(
            "public {modifier}class {name}{}",
            type_parameters.trim_end()
        ));
    }
    if let Some(parent) = superclass {
        source.push_str(&format!(" extends {parent}"));
    }
    if !interfaces.is_empty() {
        let relation = if is_interface {
            "extends"
        } else {
            "implements"
        };
        source.push_str(&format!(" {relation} {}", interfaces.join(", ")));
    }
    source.push_str(" {\n");
    for field in fields.into_values() {
        source.push_str(&field);
    }
    for method in methods.into_values() {
        source.push_str(&method);
    }
    source.push_str("}\n");
    Some((source, unavailable))
}

fn java_signature_parameters(
    parameters: &[ktcore::classfile::signature::SignatureParameter],
    scope: &[ktcore::classfile::signature::SignatureParameter],
    named_package: bool,
) -> Option<String> {
    if parameters.is_empty() {
        return Some(String::new());
    }
    let declarations = parameters
        .iter()
        .map(|parameter| {
            if !is_java_identifier(&parameter.name) {
                return None;
            }
            let bounds = parameter
                .bounds
                .iter()
                .map(|bound| {
                    if bound.name.ends_with("[]") {
                        return None;
                    }
                    java_descriptor_type(bound, scope, named_package)
                })
                .collect::<Option<Vec<_>>>()?;
            Some(if bounds.is_empty() {
                parameter.name.clone()
            } else {
                format!("{} extends {}", parameter.name, bounds.join(" & "))
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(format!("<{}> ", declarations.join(", ")))
}

fn java_descriptor_type(
    ty: &ktcore::types::TypeRef,
    scope: &[ktcore::classfile::signature::SignatureParameter],
    named_package: bool,
) -> Option<String> {
    use ktcore::types::Projection;
    if ty.projection == Projection::Star {
        return Some("?".to_string());
    }
    let mut invariant = ty.clone();
    invariant.projection = Projection::Invariant;
    if ty.projection != Projection::Invariant {
        let bound = java_descriptor_type(&invariant, scope, named_package)?;
        return Some(format!(
            "? {} {bound}",
            if ty.projection == Projection::Out {
                "extends"
            } else {
                "super"
            }
        ));
    }
    let base = ty.name.trim_end_matches("[]");
    let nominal = if let Some(owner) = ty.owner.as_deref() {
        if !is_java_identifier(base) || owner.name.ends_with("[]") {
            return None;
        }
        format!(
            "{}.{}",
            java_descriptor_type(owner, scope, named_package)?,
            base
        )
    } else {
        match ty.qualification.as_deref() {
            None if matches!(
                base,
                "boolean" | "byte" | "char" | "short" | "int" | "long" | "float" | "double"
            ) =>
            {
                if !ty.args.is_empty() {
                    return None;
                }
                Some(base.to_string())
            }
            None if is_java_identifier(base)
                && scope.iter().any(|parameter| parameter.name == base)
                && ty.args.is_empty() =>
            {
                Some(base.to_string())
            }
            Some(package)
                if is_java_identifier(base)
                    && ((!named_package && package.is_empty())
                        || (!package.is_empty() && package.split('.').all(is_java_identifier))) =>
            {
                Some(if package.is_empty() {
                    base.to_string()
                } else {
                    format!("{package}.{base}")
                })
            }
            _ => None,
        }?
    };
    let arguments = if ty.args.is_empty() {
        String::new()
    } else {
        let arguments = ty
            .args
            .iter()
            .map(|argument| java_descriptor_type(argument, scope, named_package))
            .collect::<Option<Vec<_>>>()?;
        format!("<{}>", arguments.join(", "))
    };
    Some(format!("{nominal}{arguments}{}", &ty.name[base.len()..]))
}

pub(crate) fn is_java_identifier(name: &str) -> bool {
    if matches!(
        name,
        "_" | "abstract"
            | "assert"
            | "boolean"
            | "break"
            | "byte"
            | "case"
            | "catch"
            | "char"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extends"
            | "false"
            | "final"
            | "finally"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "implements"
            | "import"
            | "instanceof"
            | "int"
            | "interface"
            | "long"
            | "native"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "short"
            | "static"
            | "strictfp"
            | "super"
            | "switch"
            | "synchronized"
            | "this"
            | "throw"
            | "throws"
            | "transient"
            | "true"
            | "try"
            | "void"
            | "volatile"
            | "while"
    ) {
        return false;
    }
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
    fn binary_java_f_bounded_methods_preserve_result_members_in_both_languages() {
        let class = ktcore::classfile::decode(
            "javac:genericfixture",
            include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Box.class"),
        )
        .unwrap();
        let source = java_class_stub(&class).expect("binary method with an own-variable F-bound");
        let mut workspace = crate::workspace::Workspace::new();
        for (file, text) in [
            ("Item.java", include_str!("../../ktcore/tests/fixtures/classfile/sources/genericfixture/Item.java")),
            ("Other.java", "package genericfixture; public class Other { public void wrongResult() {} }"),
            ("Comparable.java", "package java.lang; public interface Comparable<T> { int compareTo(T other); }"),
            ("Value.java", "package genericfixture; public class Value implements Comparable<Value> { public int compareTo(Value other) { return 0; } public void exactResult() {} }"),
        ] {
            workspace.open(file, text.into());
        }
        workspace.open("Box.java", source);
        let method = &workspace.index.lookup_by_name("bounded")[0];
        let bounds = method
            .sym
            .method_type_bounds
            .as_deref()
            .expect("binary F-bound must survive source projection");
        assert_eq!(bounds[0].0, "V");
        assert_eq!(bounds[0].1.name, "Comparable");
        assert_eq!(bounds[0].1.args[0].name, "V");
        for (file, text) in [
            ("Use.java", "package genericfixture; class Use { void use(Box<Item> box, Item item, Value value, Other other) { box.bounded(value).exactResult(); box.bounded(other).wrongResult(); box.related(item).ping(); box.related(other).wrongResult(); } }"),
            ("Use.kt", "package genericfixture\nfun use(box:Box<Item>,item:Item,value:Value,other:Other) { box.bounded(value).exactResult(); box.bounded(other).wrongResult(); box.related(item).ping(); box.related(other).wrongResult() }\n"),
        ] {
            workspace.open(file, text.into());
            let prefix = "box.bounded(value).exactRes";
            let at = text.find(prefix).unwrap() + prefix.len();
            let completion = workspace.complete(file, at, true).expect("completion response");
            assert!(completion.items.iter().any(|item| item.label == "exactResult"), "{file}");
            assert!(!completion.items.iter().any(|item| item.label == "wrongResult"), "{file}");
            let at = text.find("exactResult").unwrap();
            let definitions = workspace.goto_definition(file, at);
            assert_eq!(definitions.len(), 1, "{file}: {definitions:?}");
            assert_eq!(definitions[0].file, "Value.java");
            let target = workspace.doc_text("Value.java").unwrap();
            assert_eq!(&target[definitions[0].start_byte..definitions[0].end_byte], "exactResult");
            let prefix = "box.bounded(other).wrongRes";
            let at = text.find(prefix).unwrap() + prefix.len();
            assert!(!workspace.complete(file, at, true).is_some_and(|completion| completion.items.iter().any(|item| item.label == "wrongResult")), "{file}: incompatible method bound");
            let prefix = "box.related(item).pi";
            let at = text.find(prefix).unwrap() + prefix.len();
            let completion = workspace.complete(file, at, true).expect("owner-dependent binary method completion");
            assert!(completion.items.iter().any(|item| item.label == "ping"), "{file}");
            assert!(!completion.items.iter().any(|item| item.label == "wrongResult"), "{file}");
            let definitions = workspace.goto_definition(file, text.find("box.related(item).ping").unwrap() + "box.related(item).".len());
            assert_eq!(definitions.len(), 1, "{file}: {definitions:?}");
            assert_eq!(definitions[0].file, "Item.java");
            let target = workspace.doc_text("Item.java").unwrap();
            assert_eq!(&target[definitions[0].start_byte..definitions[0].end_byte], "ping");
            let prefix = "box.related(other).wrongRes";
            let at = text.find(prefix).unwrap() + prefix.len();
            assert!(!workspace.complete(file, at, true).is_some_and(|completion| completion.items.iter().any(|item| item.label == "wrongResult")), "{file}: incompatible receiver-dependent bound");
        }
    }

    #[test]
    fn binary_java_generic_signatures_feed_shared_java_and_kotlin_queries() {
        let mut workspace = crate::workspace::Workspace::new();
        let root = std::env::temp_dir().join(format!(
            "ktlsp-binary-generic-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for (name, bytes) in [
            (
                "Item",
                include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Item.class")
                    .as_slice(),
            ),
            (
                "Other",
                include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Other.class")
                    .as_slice(),
            ),
            (
                "Box",
                include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Box.class")
                    .as_slice(),
            ),
            (
                "Child",
                include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Child.class")
                    .as_slice(),
            ),
            (
                "Holder",
                include_bytes!("../../ktcore/tests/fixtures/classfile/genericfixture/Holder.class")
                    .as_slice(),
            ),
        ] {
            let class = ktcore::classfile::decode("javac:genericfixture", bytes).unwrap();
            let source =
                java_class_stub(&class).unwrap_or_else(|| panic!("missing binary API for {name}"));
            assert!(
                !crate::java::JavaParser::new()
                    .parse(&source)
                    .root_node()
                    .has_error(),
                "{source}"
            );
            workspace.open(
                crate::paths::file_key(&root.join(format!("{name}.java"))),
                source,
            );
        }
        // Match the editor's authored buffer and its earlier blank-prefix query exactly:
        // an unfinished statement later in the body must not erase a proven parameter.
        let native_source = include_str!(
            "../../../dev/corpus/fixtures/binary-java-jar/binarygenerics/JavaGenerics.java"
        );
        let partial = native_source.replace("Holder.ITEM.get().ping();", "Holder.ITEM.get().");
        let control_key = crate::paths::file_key(&root.join("JavaGenerics.java"));
        workspace.open(&control_key, native_source.to_string());
        workspace.scan(&root);
        let baseline_at = native_source.find("child.get().ping").unwrap() + "child.get().".len();
        assert!(
            workspace
                .completion_query(&control_key, baseline_at)
                .is_some_and(|query| {
                    query
                        .candidates
                        .iter()
                        .any(|candidate| candidate.label == "ping")
                }),
            "the unchanged native Java buffer must resolve the same indexed binary API"
        );
        let mut missing_controls = Vec::new();
        for (context, text) in [
            ("later unfinished statement", partial.clone()),
            (
                "later malformed local and unfinished statement",
                partial.replace(
                    "        Holder.ITEM.get().",
                    "        Object Holder = ;\n        Holder.ITEM.get().",
                ),
            ),
        ] {
            workspace.change(&control_key, text.clone());
            workspace.scan(&root);
            let at = text.find("child.get().ping").unwrap() + "child.get().".len();
            let useful = workspace
                .completion_query(&control_key, at)
                .is_some_and(|query| {
                    query
                        .candidates
                        .iter()
                        .any(|candidate| candidate.label == "ping")
                });
            if !useful {
                missing_controls.push(context);
            }
        }
        for (context, text, receiver) in [
            (
                "unresolved foreign parameter",
                partial.replace("Child child", "foreign.Missing child"),
                "child.get().",
            ),
            (
                "unrelated nominal parameter",
                partial.replace("Child child", "Other child"),
                "child.get().",
            ),
            (
                "inapplicable invocation",
                partial.replace("child.get().ping", "child.get(item).ping"),
                "child.get(item).",
            ),
        ] {
            workspace.change(&control_key, text.clone());
            workspace.scan(&root);
            let at = text.find(receiver).unwrap() + receiver.len();
            if let Some(query) = workspace.completion_query(&control_key, at) {
                assert!(
                    query
                        .candidates
                        .iter()
                        .all(|candidate| candidate.label != "ping"),
                    "prefix recovery must preserve {context}: {:?}",
                    query.candidates
                );
            }
        }
        workspace.close(&control_key);
        assert!(
            missing_controls.is_empty(),
            "earlier native instance controls lost their parameter identity: {missing_controls:?}"
        );
        for (file, source) in [
            (
                "Use.java",
                "package genericfixture; class Use { void test(Child child, Item item, Other other) { child.get().ping(); child.value.ping(); child.array()[0].ping(); child.same(item).ping(); child.shadow(other).pong(); Holder.ITEM.get().ping(); var inferred = Holder.ITEM.get(); inferred.ping(); } }",
            ),
            (
                "Use.kt",
                "package genericfixture\nfun test(child: Child, item: Item, other: Other) { child.get().ping(); child.value.ping(); child.array()[0].ping(); child.same(item).ping(); child.shadow(other).pong(); Holder.ITEM.get().ping() }",
            ),
        ] {
            let key = crate::paths::file_key(&root.join(file));
            workspace.open(&key, source.to_string());
            workspace.scan(&root);
            let static_item = source.find("Holder.ITEM").unwrap() + "Holder.".len();
            let holder_key = crate::paths::file_key(&root.join("Holder.java"));
            let holder_source = workspace.doc_text(&holder_key).unwrap();
            let declaration = holder_source.find("ITEM").unwrap();
            assert_eq!(
                workspace.goto_definition(&key, static_item),
                vec![crate::symbol::Def {
                    file: holder_key,
                    start_byte: declaration,
                    end_byte: declaration + "ITEM".len(),
                }],
                "{file} exact static ITEM navigation"
            );
            for (expression, member, target) in [
                ("child.get().ping", "ping", "Item.java"),
                ("child.value.ping", "ping", "Item.java"),
                ("child.array()[0].ping", "ping", "Item.java"),
                ("child.same(item).ping", "ping", "Item.java"),
                ("child.shadow(other).pong", "pong", "Other.java"),
                ("Holder.ITEM.get().ping", "ping", "Item.java"),
            ] {
                let at = source.find(expression).unwrap() + expression.len();
                let query = workspace
                    .completion_query(&key, at)
                    .unwrap_or_else(|| panic!("no completion: {file} {expression}"));
                assert!(
                    query
                        .candidates
                        .iter()
                        .any(|candidate| candidate.label == member),
                    "{file} {expression}: {:?}",
                    query.candidates
                );
                let targets = workspace.goto_definition(&key, at - member.len());
                assert_eq!(targets.len(), 1, "{file} {expression}: {targets:?}");
                assert_eq!(
                    targets[0].file,
                    crate::paths::file_key(&root.join(target)),
                    "{file} {expression}"
                );
            }
            if file == "Use.java" {
                let hints = workspace.inlay_hints(&key, 0, source.len());
                assert!(
                    hints.iter().any(|hint| hint.label == ": Item"),
                    "static generic inference: {hints:?}"
                );
                let call = "Holder.ITEM.get().ping(";
                let at = source.find(call).unwrap() + call.len();
                let help = workspace
                    .signature_help(&key, at)
                    .expect("static field chain signature");
                assert!(
                    help.signatures
                        .iter()
                        .any(|signature| signature.label.contains("ping")
                            && signature.label.contains("String")),
                    "{help:?}"
                );
                let item_key = crate::paths::file_key(&root.join("Item.java"));
                let item_source = workspace.doc_text(&item_key).unwrap();
                let references =
                    workspace.references(&item_key, item_source.find("ping").unwrap(), false);
                assert!(
                    references.iter().any(|reference| reference.file == key
                        && reference.start_byte == at - "ping(".len()),
                    "static chain references: {references:?}"
                );
                for invalid in [
                    source.replace("Holder.ITEM.get()", "Holder.instance.get()"),
                    source.replace("Other other)", "Other other, Object Holder)"),
                    source.replace("class Use", "@UnknownProducer class Use"),
                    source.replace(
                        "Holder.ITEM.get().ping();",
                        "Object Holder = ; Holder.ITEM.get().ping();",
                    ),
                    source.replace(
                        "Holder.ITEM.get().ping();",
                        "Object Holder\nHolder.ITEM.get().ping();",
                    ),
                ] {
                    workspace.change(&key, invalid.clone());
                    workspace.scan(&root);
                    let expression = if invalid.contains("Holder.instance") {
                        "Holder.instance.get().ping"
                    } else {
                        "Holder.ITEM.get().ping"
                    };
                    let at = invalid.find(expression).unwrap() + expression.len();
                    if let Some(query) = workspace.completion_query(&key, at) {
                        assert!(
                            query
                                .candidates
                                .iter()
                                .all(|candidate| candidate.label != "ping"),
                            "unsafe static receiver: {invalid}"
                        );
                    }
                    assert!(
                        workspace
                            .goto_definition(&key, at - "ping".len())
                            .is_empty(),
                        "unsafe static receiver: {invalid}"
                    );
                }
            }
            let unfinished = if file == "Use.java" {
                "package genericfixture; class Use { void test() { Holder.ITEM.get(). } }"
            } else {
                "package genericfixture\nfun unfinished() { Holder.ITEM.get(). }"
            };
            workspace.change(&key, unfinished.into());
            workspace.scan(&root);
            let at = unfinished.find("Holder.ITEM.get().").unwrap() + "Holder.ITEM.get().".len();
            let query = workspace
                .completion_query(&key, at)
                .expect("unfinished static generic chain");
            assert!(
                query
                    .candidates
                    .iter()
                    .any(|candidate| candidate.label == "ping"),
                "{file} actual unfinished source: {:?}",
                query.candidates
            );
            let item = unfinished.find("Holder.ITEM").unwrap() + "Holder.".len();
            let holder_key = crate::paths::file_key(&root.join("Holder.java"));
            let holder_source = workspace.doc_text(&holder_key).unwrap();
            let declaration = holder_source.find("ITEM").unwrap();
            assert_eq!(
                workspace.goto_definition(&key, item),
                vec![crate::symbol::Def {
                    file: holder_key,
                    start_byte: declaration,
                    end_byte: declaration + "ITEM".len(),
                }],
                "{file} exact unfinished static ITEM navigation"
            );
            if file == "Use.java" {
                for invalid in [
                    unfinished.replace("class Use", "@UnknownProducer class Use"),
                    unfinished.replace("test()", "test(Object Holder)"),
                    unfinished.replace("test()", "test("),
                    unfinished.replace("Holder.ITEM", "Object Holder = ; Holder.ITEM"),
                    unfinished.replace("Holder.ITEM", "Object Holder\nHolder.ITEM"),
                    unfinished.trim_end_matches('}').to_string(),
                ] {
                    workspace.change(&key, invalid.clone());
                    workspace.scan(&root);
                    let at =
                        invalid.find("Holder.ITEM.get().").unwrap() + "Holder.ITEM.get().".len();
                    if let Some(query) = workspace.completion_query(&key, at) {
                        assert!(
                            query
                                .candidates
                                .iter()
                                .all(|candidate| candidate.label != "ping"),
                            "unsafe unfinished static receiver: {invalid}"
                        );
                    }
                    let item = invalid.find("Holder.ITEM").unwrap() + "Holder.".len();
                    assert!(
                        workspace.goto_definition(&key, item).is_empty(),
                        "unsafe unfinished static navigation: {invalid}"
                    );
                }
                let unrelated = "package genericfixture; class Use { void test() { Holder.ITEM.get().; Object Holder = null; Holder.ITEM.get(). } }";
                workspace.change(&key, unrelated.into());
                workspace.scan(&root);
                let shadowed_item = unrelated.rfind("Holder.ITEM").unwrap() + "Holder.".len();
                assert!(workspace.goto_definition(&key, shadowed_item).is_empty(), "an unrelated unfinished receiver cannot erase the later local shadow");
                let typed = "package genericfixture; @UnknownProducer class Use { void test(Child child) { child.value. } }";
                workspace.change(&key, typed.into());
                workspace.scan(&root);
                let at = typed.find("child.value.").unwrap() + "child.value.".len();
                let query = workspace.completion_query(&key, at).expect("annotated caller's explicit value receiver");
                assert!(query.candidates.iter().any(|candidate| candidate.label == "ping"), "annotated typed receiver must remain useful: {:?}", query.candidates);
                let value = typed.find("child.value").unwrap() + "child.".len();
                let box_key = crate::paths::file_key(&root.join("Box.java"));
                let box_source = workspace.doc_text(&box_key).unwrap();
                let declaration = box_source.find("value").unwrap();
                assert_eq!(workspace.goto_definition(&key, value), vec![crate::symbol::Def {
                    file: box_key, start_byte: declaration, end_byte: declaration + "value".len(),
                }], "annotated caller's typed field navigation");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn java_field_chain_withholds_inaccessible_hidden_and_kotlin_lowered_properties() {
        let mut workspace = crate::workspace::Workspace::new();
        workspace.open(
            "Item.java",
            "package fieldguard; public class Item { public void ping() {} }".into(),
        );
        workspace.open(
            "Parent.java",
            "package fieldguard; public class Parent { public Item field; }".into(),
        );
        workspace.open(
            "Child.java",
            "package fieldguard; public class Child extends Parent { private Item field; }".into(),
        );
        workspace.open(
            "Hidden.java",
            "package fieldguard; public class Hidden { private Item field; }".into(),
        );
        workspace.open(
            "Holder.kt",
            "package fieldguard\nclass Holder(val field: Item)".into(),
        );
        let source = "package fieldguard; class Use { void use(Child child, Hidden hidden, Holder holder) { child.field.ping(); hidden.field.ping(); holder.field.ping(); } }";
        workspace.open("Use.java", source.into());
        for expression in ["child.field.ping", "hidden.field.ping", "holder.field.ping"] {
            let at = source.find(expression).unwrap() + expression.len();
            if let Some(query) = workspace.completion_query("Use.java", at) {
                assert!(
                    query
                        .candidates
                        .iter()
                        .all(|candidate| candidate.label != "ping"),
                    "{expression}"
                );
            }
            assert!(
                workspace
                    .goto_definition("Use.java", at - "ping".len())
                    .is_empty(),
                "{expression}"
            );
        }
    }

    #[test]
    fn java_namespace_proof_keeps_body_edits_separate_from_declaration_errors() {
        let mut parser = crate::java::JavaParser::new();
        for (source, expected) in [
            // This grammar loses the class header; only validated completion recovery can prove it.
            ("class Use { void test() { Holder.ITEM.get(). } }", false),
            ("class Use { void test() { int n = ; } }", true),
            ("class Use { void test( { Holder.ITEM.get(). } }", false),
            ("class Use { Item ; void test() {} }", false),
            ("class Use { Item value = Holder.ITEM.get().; }", false),
            ("class Use { void test() { Holder.ITEM.get(). }", false),
        ] {
            let tree = parser.parse(source);
            assert!(
                tree.root_node().has_error(),
                "fixture must contain an actual edit error: {source}"
            );
            assert_eq!(
                crate::java::declaration_headers_complete(&tree, source),
                expected,
                "{source}"
            );
        }
    }

    #[test]
    fn binary_java_hierarchy_preserves_abstract_shadows_and_withholds_unknown_overrides() {
        // Injected metadata isolates shadowing; the Ponder fixture supplies a valid method shape.
        let mut class = ktcore::classfile::decode(
            "fixture",
            include_bytes!(
                "../../ktcore/tests/fixtures/classfile/ponder1087/VirtualBlockEntity.class"
            ),
        )
        .unwrap();
        class.identity.internal_name = "binaryhierarchy/Mid".into();
        class.source_file = Some("Mid.java".into());
        class.access_flags = 0x0421;
        class.super_name = Some("binaryhierarchy/Base".into());
        class.methods.truncate(1);
        class.methods[0].name = "f".into();
        class.methods[0].descriptor = "()Ljava/lang/String;".into();
        let source = java_class_stub(&class).unwrap();
        assert!(
            source.contains("public abstract java.lang.String f();"),
            "{source}"
        );
        let mut workspace = crate::workspace::Workspace::new();
        workspace.open(
            "Base.java",
            "package binaryhierarchy; public class Base { public Object f() { return null; } }"
                .to_string(),
        );
        workspace.open("Mid.java", source);
        let kotlin = "package binaryhierarchy\nclass Leaf : Mid() { override fun f(): String = \"\"; fun test() { super.f() } }\nfun ordinary(value: Mid) { value.f() }\n";
        workspace.open("Use.kt", kotlin.to_string());
        let query = workspace
            .completion_query("Use.kt", kotlin.find("value.f").unwrap() + "value.f".len())
            .unwrap();
        assert!(query.candidates.iter().any(|item| item.label == "f"));
        let targets =
            workspace.goto_definition("Use.kt", kotlin.find("value.f").unwrap() + "value.".len());
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file, "Mid.java");
        let query = workspace
            .completion_query("Use.kt", kotlin.find("super.f").unwrap() + "super.f".len())
            .unwrap();
        assert!(query.candidates.iter().all(|item| item.label != "f"));
        assert!(workspace
            .goto_definition("Use.kt", kotlin.find("super.f").unwrap() + "super.".len())
            .is_empty());
        for invalid in [0x0008, 0x0010, 0x0020, 0x0100, 0x0800] {
            class.methods[0].access_flags = 0x0401 | invalid;
            let (source, unavailable) = java_class_stub_facts(&class).unwrap();
            assert!(!source.contains(" f("), "{source}");
            assert!(unavailable.iter().any(|(name, kind, arity)| name == "f"
                && *kind == ktcore::symbol::SymbolKind::Function
                && *arity == Some(0)));
        }
        class.access_flags = 0x0021;
        class.methods[0].access_flags = 0x0001;
        class.methods[0].descriptor = "()Ljava/util/List;".into();
        class.methods[0].signature = Some("()Ljava/util/List<Ljava/lang/String;>;".into());
        let source = java_class_stub(&class).unwrap();
        assert!(
            source.contains("java.util.List<java.lang.String> f()"),
            "{source}"
        );
        // A present but contradictory Signature cannot fall back to the raw descriptor.
        class.methods[0].signature = Some("()Ljava/util/Set<Ljava/lang/String;>;".into());
        let (source, unavailable) = java_class_stub_facts(&class).unwrap();
        assert!(source.contains("extends binaryhierarchy.Base") && !source.contains(" f("));
        assert!(unavailable.iter().any(|(name, kind, arity)| name == "f"
            && *kind == ktcore::symbol::SymbolKind::Function
            && *arity == Some(0)));
        class.methods[0].access_flags |= 0x0080;
        class.methods[0].descriptor = "([Ljava/lang/Object;)Ljava/util/List;".into();
        let (_, unavailable) = java_class_stub_facts(&class).unwrap();
        assert!(unavailable.iter().any(|(name, kind, arity)| name == "f"
            && *kind == ktcore::symbol::SymbolKind::Function
            && arity.is_none()));
    }

    #[test]
    fn binary_java_actual_ponder_interface_keeps_abstract_methods() {
        let class = ktcore::classfile::decode(
            "fixture:ponder-neoforge:1.0.87+mc1.21.1",
            include_bytes!(
                "../../ktcore/tests/fixtures/classfile/ponder1087/VirtualBlockEntity.class"
            ),
        )
        .unwrap();
        assert_eq!(class.access_flags, 0x0601);
        assert_eq!(class.methods.len(), 2);
        for (name, descriptor) in [("markVirtual", "()V"), ("isVirtual", "()Z")] {
            let method = class
                .methods
                .iter()
                .find(|method| method.name.as_ref() == name)
                .unwrap();
            assert_eq!(method.descriptor.as_ref(), descriptor);
            assert_eq!(method.access_flags, 0x0401);
        }
        let source = java_class_stub(&class).expect("verified public Java interface");
        assert!(
            source.contains("public interface VirtualBlockEntity"),
            "{source}"
        );
        assert!(source.contains("void markVirtual();"), "{source}");
        assert!(source.contains("boolean isVirtual();"), "{source}");
        let tree = crate::java::JavaParser::new().parse(&source);
        assert!(!tree.root_node().has_error(), "{source}");
        let symbols = crate::java::extract_symbols(&tree, &source);
        assert!(symbols
            .iter()
            .any(|symbol| symbol.name == "VirtualBlockEntity"
                && symbol.kind == ktcore::symbol::SymbolKind::Interface));
        for name in ["markVirtual", "isVirtual"] {
            assert_eq!(
                symbols
                    .iter()
                    .find(|symbol| symbol.name == name)
                    .unwrap()
                    .source_member
                    .implementation(),
                Some(ktcore::symbol::MemberImplementation::Abstract)
            );
        }
        let mut inherited = class.clone();
        inherited.interfaces = vec!["fixture/Parent".into()];
        assert!(java_class_stub(&inherited)
            .unwrap()
            .contains("interface VirtualBlockEntity extends fixture.Parent"));
        for flags in [0x0201, 0x0611] {
            let mut invalid = class.clone();
            invalid.access_flags = flags;
            assert!(java_class_stub(&invalid).is_none());
        }
        let mut default = class.clone();
        default.methods[0].access_flags = 0x0001;
        assert!(java_class_stub(&default)
            .unwrap()
            .contains("default void markVirtual()"));
        let mut unsupported = class.clone();
        unsupported.methods[0].signature = Some("()TT;".into());
        let (source, unavailable) = java_class_stub_facts(&unsupported).unwrap();
        assert!(!source.contains(" markVirtual("));
        assert!(unavailable.iter().any(|(name, _, _)| name == "markVirtual"));
    }

    #[test]
    fn binary_java_default_interface_keeps_hierarchy_and_name_specific_unknowns() {
        use ktcore::extensions::{Dispatch, Visibility};
        let source = "package defaultfixture; interface Implicit { int VALUE = 1; }";
        let tree = crate::java::JavaParser::new().parse(source);
        let symbols = crate::java::extract_symbols(&tree, source);
        let constant = symbols
            .iter()
            .find(|symbol| symbol.name == "VALUE")
            .unwrap();
        assert_eq!(constant.source_member.dispatch(), Some(Dispatch::Static));
        assert_eq!(
            constant.source_member.visibility(),
            Some(Visibility::Public)
        );
        assert_eq!(constant.value_type.as_ref().unwrap().name, "int");
        let original = ktcore::classfile::decode(
            "javac:defaultfixture",
            include_bytes!("../../ktcore/tests/fixtures/classfile/defaultfixture/Overlay.class"),
        )
        .unwrap();
        let (complete, unavailable) = java_class_stub_facts(&original).unwrap();
        assert!(complete.contains("void writeSafe(java.lang.Thread.State p0)"));
        assert!(!unavailable.iter().any(|(name, _, _)| name == "writeSafe"));
        for blocked in [false, true] {
            let mut class = original.clone();
            // A missing identity, rather than the enum type itself, makes this name unavailable.
            class
                .inner_classes
                .retain(|row| row.inner.as_ref() != "java/lang/Thread$State");
            if blocked {
                class
                    .methods
                    .iter_mut()
                    .find(|method| method.name.as_ref() == "writeSafe")
                    .unwrap()
                    .name = "getBreakSpeed".into();
            }
            let (source, unavailable) = java_class_stub_facts(&class).unwrap();
            assert!(source.contains("public interface Overlay"), "{source}");
            assert!(
                source.contains("default <T extends java.lang.Object> T tooltip(T p0)"),
                "{source}"
            );
            assert!(
                source.contains("public static int fromStatic()"),
                "{source}"
            );
            assert!(!source.contains(" helper("));
            let mut java = crate::java::JavaParser::new();
            let tree = java.parse(&source);
            assert!(!tree.root_node().has_error(), "{source}");
            let mut symbols = crate::java::extract_symbols(&tree, &source);
            for (name, dispatch) in [
                ("tooltip", Dispatch::Instance),
                ("fromStatic", Dispatch::Static),
                ("MARKER", Dispatch::Static),
            ] {
                let symbol = symbols.iter().find(|symbol| symbol.name == name).unwrap();
                assert_eq!(symbol.source_member.dispatch(), Some(dispatch));
                assert_eq!(symbol.source_member.visibility(), Some(Visibility::Public));
            }
            assert!(
                source.contains("public static final int MARKER;"),
                "{source}"
            );
            let unknown = if blocked {
                "getBreakSpeed"
            } else {
                "writeSafe"
            };
            assert!(unavailable.iter().any(|(name, _, _)| name == unknown));
            symbols
                .iter_mut()
                .find(|symbol| symbol.name == "Overlay")
                .unwrap()
                .unavailable_members = Some(Box::new(unavailable));
            let mut ws = crate::workspace::Workspace::new();
            ws.index
                .replace_file("Overlay.java", symbols, crate::index::Tier::Durable);
            ws.open("Base.java", "package defaultfixture; public class Base { protected float getBreakSpeed() { return 1; } }".into());
            ws.open("Mid.java", "package defaultfixture; public abstract class Mid extends Base implements Overlay {}".into());
            let kotlin = "package defaultfixture\ninterface Other\nabstract class Leaf : Mid(), Other { fun use() { super.getBreakSpeed() } }";
            ws.open("Use.kt", kotlin.into());
            let at = kotlin.find("super.getBreakSpeed").unwrap() + "super.getBreakSpeed".len();
            let query = ws.completion_query("Use.kt", at).unwrap();
            assert_eq!(
                query
                    .candidates
                    .iter()
                    .any(|candidate| candidate.label == "getBreakSpeed"),
                !blocked,
                "{:?}",
                query.candidates
            );
            let targets = ws.goto_definition("Use.kt", at - 1);
            if blocked {
                assert!(targets.is_empty(), "{targets:?}");
            } else {
                assert_eq!(targets.len(), 1);
                assert_eq!(targets[0].file, "Base.java");
            }
            for name in ["fromStatic", "MARKER", "helper"] {
                let source = kotlin.replace("getBreakSpeed", name);
                ws.open("Use.kt", source.clone());
                let at =
                    source.find(&format!("super.{name}")).unwrap() + "super.".len() + name.len();
                assert!(ws
                    .completion_query("Use.kt", at)
                    .unwrap()
                    .candidates
                    .iter()
                    .all(|candidate| candidate.label != name));
            }
        }
        let mut annotated = original.clone();
        annotated
            .attribute_names
            .push("RuntimeVisibleAnnotations".into());
        assert!(java_class_stub(&annotated).is_none());
        for failure in ["annotation", "flags", "signature"] {
            let mut unsupported = original.clone();
            let method = unsupported
                .methods
                .iter_mut()
                .find(|method| method.name.as_ref() == "tooltip")
                .unwrap();
            match failure {
                "annotation" => method
                    .attribute_names
                    .push("RuntimeVisibleAnnotations".into()),
                "flags" => method.access_flags = 0x0409,
                _ => method.signature = Some("()TT;".into()),
            }
            let (source, unavailable) = java_class_stub_facts(&unsupported).unwrap();
            assert!(source.contains("public interface Overlay"));
            assert!(!source.contains(" tooltip("));
            assert!(unavailable.iter().any(|(name, _, _)| name == "tooltip"));
        }
    }

    #[test]
    fn binary_java_actual_create_nested_parameter_requires_inner_identity() {
        let original = ktcore::classfile::decode(
            "create:6.0.10-280",
            include_bytes!(
                "../../ktcore/tests/fixtures/classfile/create6010/KineticBlockEntity.class"
            ),
        )
        .unwrap();
        let internal = "net/minecraft/core/HolderLookup$Provider";
        let descriptor = &original
            .methods
            .iter()
            .find(|method| method.name.as_ref() == "read")
            .unwrap()
            .descriptor;
        assert!(
            ktcore::classfile::method_types(descriptor).is_none(),
            "unscoped descriptors must stay unknown"
        );
        let (parameters, returns) =
            ktcore::classfile::method_types_with_inner_classes(descriptor, &original.inner_classes)
                .unwrap();
        assert!(returns.is_none());
        assert_eq!(parameters[1].name, "Provider");
        assert_eq!(parameters[1].owner.as_ref().unwrap().name, "HolderLookup");
        assert_eq!(
            parameters[1]
                .owner
                .as_ref()
                .unwrap()
                .qualification
                .as_deref(),
            Some("net.minecraft.core")
        );
        for failure in [
            "known",
            "missing",
            "duplicate",
            "outer",
            "name",
            "nonstatic",
            "private",
        ] {
            let mut class = original.clone();
            let at = class
                .inner_classes
                .iter()
                .position(|row| row.inner.as_ref() == internal)
                .unwrap();
            match failure {
                "missing" => {
                    class.inner_classes.remove(at);
                }
                "duplicate" => class.inner_classes.push(class.inner_classes[at].clone()),
                "outer" => class.inner_classes[at].outer = Some("net/minecraft/core/Other".into()),
                "name" => class.inner_classes[at].name = Some("Other".into()),
                "nonstatic" => class.inner_classes[at].access_flags &= !0x0008,
                "private" => class.inner_classes[at].access_flags = 0x060a,
                _ => {}
            }
            let (source, unavailable) = java_class_stub_facts(&class).unwrap();
            for method in ["read", "write"] {
                let expected = format!("void {method}(net.minecraft.nbt.CompoundTag p0, net.minecraft.core.HolderLookup.Provider p1, boolean p2)");
                assert_eq!(
                    source.contains(&expected),
                    failure == "known",
                    "{failure} {method}: {source}"
                );
                assert_eq!(
                    unavailable.iter().any(|(name, _, _)| name == method),
                    failure != "known",
                    "{failure} {method}"
                );
            }
        }
    }

    #[test]
    fn binary_java_stub_keeps_actual_public_constants_and_withholds_unsupported_fields() {
        let class = ktcore::classfile::decode(
            "fixture",
            include_bytes!(
            "../../ktcore/tests/fixtures/classfile/gradle8142/JavaCompilerArgumentsBuilder.class"
        ),
        )
        .unwrap();
        let source = java_class_stub(&class).unwrap();
        let mut parser = crate::java::JavaParser::new();
        let tree = parser.parse(&source);
        assert!(!tree.root_node().has_error(), "{source}");
        let symbols = crate::java::extract_symbols(&tree, &source);
        assert!(!symbols
            .iter()
            .any(|symbol| symbol.kind == ktcore::symbol::SymbolKind::Property
                && matches!(
                    symbol.name.as_str(),
                    "spec" | "includeLauncherOptions" | "args"
                )));
        for name in [
            "USE_UNSHARED_COMPILER_TABLE_OPTION",
            "EMPTY_SOURCE_PATH_REF_DIR",
        ] {
            let field = symbols.iter().find(|symbol| symbol.name == name).unwrap();
            assert_eq!(field.kind, ktcore::symbol::SymbolKind::Property);
            assert_eq!(field.value_type.as_ref().unwrap().name, "String");
            assert_eq!(
                field.value_type.as_ref().unwrap().qualification.as_deref(),
                Some("java.lang")
            );
            assert_eq!(
                field.source_member.dispatch(),
                Some(ktcore::extensions::Dispatch::Static)
            );
            assert!(source.contains(&format!("public static final java.lang.String {name};")));
        }
        let prototype = class
            .fields
            .iter()
            .find(|field| field.name.as_ref() == "EMPTY_SOURCE_PATH_REF_DIR")
            .unwrap();
        let mut protected = class.clone();
        let mut field = prototype.clone();
        field.name = "retained".into();
        field.access_flags = 0x0004;
        protected.fields.push(field);
        let source = java_class_stub(&protected).unwrap();
        assert!(source.contains("protected java.lang.String retained;"));
        let tree = parser.parse(&source);
        let symbols = crate::java::extract_symbols(&tree, &source);
        let field = symbols
            .iter()
            .find(|symbol| symbol.name == "retained")
            .unwrap();
        assert_eq!(
            field.source_member.visibility(),
            Some(ktcore::extensions::Visibility::Protected)
        );
        assert!(field.source_member.protected_package_access());
        let owner = symbols
            .iter()
            .find(|symbol| symbol.kind == ktcore::symbol::SymbolKind::Class)
            .unwrap();
        let receiver = ktcore::types::Type::class(&owner.name, Some(owner.package.clone()));
        let mut index = ktcore::index::Index::new();
        index.replace_file("Gradle.java", symbols, ktcore::index::Tier::Durable);
        let unrelated = ktcore::infer::FileCtx::new("outside".into(), Vec::new());
        assert!(ktcore::members::completion_candidates_at(
            &index, &receiver, "retained", &unrelated, 0, false
        )
        .members
        .is_empty());
        for case in [
            "private",
            "synthetic-flag",
            "synthetic-attribute",
            "generic",
            "nested",
            "unnamed",
            "duplicate-name",
            "final-volatile",
        ] {
            // Injected malformed/unsupported metadata tests conservatism, not javac callability.
            let mut altered = class.clone();
            let mut field = prototype.clone();
            field.name = "withheld".into();
            match case {
                "private" => field.access_flags = 0x0002,
                "synthetic-flag" => field.access_flags |= 0x1000,
                "synthetic-attribute" => field.attribute_names.push("Synthetic".into()),
                "generic" => field.signature = Some("TT;".into()),
                "nested" => field.descriptor = "Lpkg/Outer$Inner;".into(),
                "unnamed" => field.descriptor = "LItem;".into(),
                "duplicate-name" => {
                    let mut duplicate = field.clone();
                    duplicate.descriptor = "I".into();
                    altered.fields.push(duplicate);
                }
                "final-volatile" => field.access_flags |= 0x0040,
                _ => unreachable!(),
            }
            altered.fields.push(field);
            assert!(
                !java_class_stub(&altered).unwrap().contains("withheld"),
                "{case}"
            );
        }
    }

    #[test]
    fn binary_java_stub_withholds_return_only_groups_before_unsupported_members_are_filtered() {
        // Deliberately injected JVM metadata tests rendering, not a javac-produced source API.
        let mut class = ktcore::classfile::decode(
            "fixture",
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class"),
        )
        .unwrap();
        let prototype = class
            .methods
            .iter()
            .find(|method| method.name.as_ref() == "getName")
            .unwrap()
            .clone();
        for (name, descriptor, signature, flags, synthetic) in [
            (
                "nestedCollision",
                "()Ljava/lang/String;",
                None,
                0x0001,
                false,
            ),
            (
                "nestedCollision",
                "()Lpkg/Outer$Inner;",
                None,
                0x0001,
                false,
            ),
            (
                "genericCollision",
                "()Ljava/lang/String;",
                None,
                0x0001,
                false,
            ),
            (
                "genericCollision",
                "()Ljava/lang/Object;",
                Some("<T:Ljava/lang/Object;>()TT;"),
                0x0001,
                false,
            ),
            ("bridgeSafe", "()Ljava/lang/String;", None, 0x0001, false),
            ("bridgeSafe", "()Ljava/lang/Object;", None, 0x1041, false),
            ("attributeSafe", "()Ljava/lang/String;", None, 0x0001, false),
            ("attributeSafe", "()Ljava/lang/Object;", None, 0x0001, true),
        ] {
            let mut method = prototype.clone();
            method.name = name.into();
            method.descriptor = descriptor.into();
            method.signature = signature.map(Into::into);
            method.access_flags = flags;
            if synthetic {
                method.attribute_names.push("Synthetic".into());
            }
            class.methods.push(method);
        }
        let source = java_class_stub(&class).unwrap();
        assert!(source.contains("java.lang.String getName("), "{source}");
        assert!(
            !source.contains("nestedCollision(") && !source.contains("genericCollision("),
            "{source}"
        );
        assert_eq!(
            source.matches("java.lang.String bridgeSafe(").count(),
            1,
            "{source}"
        );
        assert_eq!(
            source.matches("java.lang.String attributeSafe(").count(),
            1,
            "{source}"
        );
    }

    #[test]
    fn binary_java_stub_preserves_nominal_hierarchy_and_withholds_unspellable_parents() {
        let class = ktcore::classfile::decode(
            "fixture",
            include_bytes!("../../ktcore/tests/fixtures/classfile/create6010/BlockBreakingKineticBlockEntity.class"),
        )
        .unwrap();
        let source = java_class_stub(&class).unwrap();
        assert!(
            source.contains("extends com.simibubi.create.content.kinetics.base.KineticBlockEntity")
        );
        let mut parser = crate::java::JavaParser::new();
        assert!(!parser.parse(&source).root_node().has_error(), "{source}");

        // Injected descriptor metadata tests the rendering boundary, not a javac-produced API.
        let mut interfaces = class.clone();
        interfaces.interfaces = vec!["other/Readable".into(), "other/Writable".into()];
        assert!(java_class_stub(&interfaces)
            .unwrap()
            .contains("implements other.Readable, other.Writable"));
        interfaces.interfaces.push("other/Readable".into());
        assert!(java_class_stub(&interfaces).is_none());
        for parent in [
            class.identity.internal_name.as_ref(),
            "other/Outer$Nested",
            "Unnamed",
            "other/class",
        ] {
            let mut altered = class.clone();
            altered.super_name = Some(parent.into());
            assert!(java_class_stub(&altered).is_none(), "{parent}");
        }
        let mut generic = class;
        generic.signature = Some("Ljava/lang/Object;".into());
        assert!(java_class_stub(&generic).is_none());
    }

    #[test]
    fn binary_java_stub_preserves_unnamed_nominal_identity_at_the_rendering_boundary() {
        // Descriptor metadata is injected here to test spelling; this is not a javac-produced API.
        let mut class = ktcore::classfile::decode(
            "fixture",
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class"),
        )
        .unwrap();
        class.identity.internal_name = "named/Factory".into();
        class.source_file = Some("Factory.java".into());
        let prototype = class
            .methods
            .iter()
            .find(|method| method.name.as_ref() == "getName")
            .unwrap()
            .clone();
        for (name, descriptor) in [("take", "(LItem;)V"), ("give", "()[LItem;")] {
            let mut method = prototype.clone();
            method.name = name.into();
            method.descriptor = descriptor.into();
            class.methods.push(method);
        }
        let named = java_class_stub(&class).unwrap();
        assert!(named.contains("java.lang.String getName("), "{named}");
        assert!(
            !named.contains("take(") && !named.contains("give("),
            "{named}"
        );
        class.identity.internal_name = "Factory".into();
        let unnamed = java_class_stub(&class).unwrap();
        assert!(unnamed.contains("void take(Item p0)"), "{unnamed}");
        assert!(unnamed.contains("Item[] give()"), "{unnamed}");
    }

    #[test]
    fn binary_java_stubs_keep_public_method_types_without_lowered_kotlin_or_constructors() {
        let factory =
            include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class");
        let item = include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class");
        let person =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let model =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Model.class");
        let noarg =
            include_bytes!("../../ktcore/tests/fixtures/classfile/noarg-on/NeedsLabel.class");
        let root =
            std::env::temp_dir().join(format!("ktlsp_binary_method_stubs_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let jar = root.join("api.jar");
        write_binary_jar(
            &jar,
            &[
                ("arrayfixture/Factory.class", factory),
                ("arrayfixture/Item.class", item),
                ("pluginfixture/Person.class", person),
                ("pluginfixture/Model.class", model),
                ("noargfixture/NeedsLabel.class", noarg),
                ("other/Factory.class", factory),
                ("broken/Only.class", b"not a classfile"),
                ("../Escaped.class", factory),
                ("arrayfixture/Factory.java", b"public class Poison {}"),
            ],
        );
        let output = root.join("stubs");
        let files = extract_class_stubs(&jar, &output).unwrap();
        assert!(!root.join("Escaped.java").exists());
        let mut parser = crate::java::JavaParser::new();
        for (path, _) in files {
            let text = fs::read_to_string(&path).unwrap();
            let tree = parser.parse(&text);
            assert!(!tree.root_node().has_error(), "{}: {text}", path.display());
            let symbols = crate::java::extract_symbols(&tree, &text);
            let methods: Vec<_> = symbols
                .iter()
                .filter(|symbol| symbol.kind == ktcore::symbol::SymbolKind::Function)
                .collect();
            if path.ends_with("arrayfixture/Factory.java") {
                let rows = methods.iter().find(|method| method.name == "rows").unwrap();
                assert_eq!(rows.return_type.as_ref().unwrap().name, "Item[][]");
                assert_eq!(
                    rows.return_type.as_ref().unwrap().qualification.as_deref(),
                    Some("arrayfixture")
                );
                let counts = methods
                    .iter()
                    .find(|method| method.name == "counts")
                    .unwrap();
                assert_eq!(counts.return_type.as_ref().unwrap().name, "int[]");
                let annotated = methods
                    .iter()
                    .find(|method| method.name == "annotated")
                    .unwrap();
                assert_eq!(annotated.return_type.as_ref().unwrap().name, "Item[]");
                let static_rows = methods
                    .iter()
                    .find(|method| method.name == "staticRows")
                    .unwrap();
                assert_eq!(
                    static_rows.source_member.dispatch(),
                    Some(ktcore::extensions::Dispatch::Static)
                );
                let with_argument = methods
                    .iter()
                    .find(|method| method.name == "withArgument")
                    .unwrap();
                assert_eq!(with_argument.params[0].name, "int");
                assert_eq!(with_argument.min_arity, Some(1));
                let generic = methods
                    .iter()
                    .find(|method| method.name == "generic")
                    .unwrap();
                assert_eq!(generic.type_params, vec!["T"]);
                assert_eq!(generic.return_type.as_ref().unwrap().name, "T[]");
                assert!(!methods.iter().any(|method| method.name == "Factory"));
                let hidden = methods
                    .iter()
                    .find(|method| method.name == "hidden")
                    .unwrap();
                assert_eq!(
                    hidden.source_member.visibility(),
                    Some(ktcore::extensions::Visibility::Protected)
                );
                assert!(hidden.source_member.protected_package_access());
                let receiver = ktcore::types::Type::class("Factory", Some("arrayfixture".into()));
                let mut index = ktcore::index::Index::new();
                index.replace_file(
                    "Factory.java",
                    symbols.clone(),
                    ktcore::index::Tier::Durable,
                );
                let unrelated = ktcore::infer::FileCtx::new("outside".into(), Vec::new());
                assert!(ktcore::members::completion_candidates_at(
                    &index, &receiver, "hidden", &unrelated, 0, false
                )
                .members
                .is_empty());
                assert!(!ktcore::members::completion_candidates_at(
                    &index, &receiver, "rows", &unrelated, 0, false
                )
                .members
                .is_empty());
                assert!(!symbols.iter().any(|symbol| symbol.name == "Poison"));
            } else if path.ends_with("pluginfixture/Person.java") {
                assert_eq!(
                    methods
                        .iter()
                        .map(|method| method.name.as_str())
                        .collect::<Vec<_>>(),
                    ["getName"]
                );
            } else if path.ends_with("arrayfixture/Item.java") {
                assert_eq!(
                    methods
                        .iter()
                        .map(|method| method.name.as_str())
                        .collect::<Vec<_>>(),
                    ["useful"]
                );
            } else {
                assert!(methods.is_empty(), "{}: {text}", path.display());
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_java_stubs_withhold_versioned_alias_and_corrupt_member_inputs() {
        let factory =
            include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class")
                .as_slice();
        let item = include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class")
            .as_slice();
        let root = std::env::temp_dir().join(format!(
            "ktlsp_binary_stub_selection_{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let jar = root.join("api.jar");
        for (label, extra) in [
            (
                "versioned",
                ("META-INF/versions/17/arrayfixture/Factory.class", factory),
            ),
            ("aliased", ("arrayfixture\\Factory.class", factory)),
        ] {
            write_binary_jar(
                &jar,
                &[
                    ("arrayfixture/Factory.class", factory),
                    ("arrayfixture/Item.class", item),
                    extra,
                ],
            );
            let output = root.join(label);
            extract_class_stubs(&jar, &output).unwrap();
            assert!(
                !fs::read_to_string(output.join("arrayfixture/Factory.java"))
                    .unwrap()
                    .contains("rows(")
            );
            assert!(fs::read_to_string(output.join("arrayfixture/Item.java"))
                .unwrap()
                .contains("useful("));
        }
        write_binary_jar(
            &jar,
            &[
                ("arrayfixture/Item.class", item),
                ("arrayfixture/Factory.class", factory),
            ],
        );
        let mut bytes = fs::read(&jar).unwrap();
        let at = bytes
            .windows(factory.len())
            .position(|window| window == factory)
            .unwrap();
        bytes[at + factory.len() - 1] ^= 1;
        fs::write(&jar, bytes).unwrap();
        let output = root.join("corrupt");
        assert!(
            extract_class_stubs(&jar, &output).is_err(),
            "CRC failure cannot publish member facts"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_class_reader_decodes_exact_entry_and_origin() {
        let class =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let root =
            std::env::temp_dir().join(format!("ktlsp_selected_class_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("test.jar");
        for entry in [
            "pluginfixture/Person.class",
            "classes/pluginfixture/Person.class",
        ] {
            write_binary_jar(&path, &[("unselected.class", b"invalid"), (entry, class)]);
            let facts = read_class_file(&path, entry, "selected-origin")
                .unwrap()
                .unwrap();
            assert_eq!(facts.identity.origin_id, "selected-origin");
            assert_eq!(facts.identity.internal_name, "pluginfixture/Person");
            assert!(facts.methods.iter().any(|member| {
                member.name.as_ref() == "getName"
                    && member.descriptor.as_ref() == "()Ljava/lang/String;"
            }));
            assert!(read_class_file(&path, "missing.class", "selected-origin")
                .unwrap()
                .is_none());
            assert!(read_class_file(&path, "unselected.class", "selected-origin").is_err());
            assert!(read_class_file(&path, entry, "").is_err());
            let disk_entry = root.join(entry);
            fs::create_dir_all(disk_entry.parent().unwrap()).unwrap();
            fs::write(disk_entry, class).unwrap();
            let disk_facts = read_class_file(&root, entry, "selected-origin")
                .unwrap()
                .unwrap();
            assert_eq!(disk_facts, facts);
        }
        assert!(read_class_file(&root, "missing.class", "origin")
            .unwrap()
            .is_none());
        for unsafe_name in [
            "",
            "../Person.class",
            "/Person.class",
            "nested/../../Person.class",
        ] {
            assert!(
                read_class_file(&root, unsafe_name, "origin").is_err(),
                "{unsafe_name}"
            );
        }
        write_binary_jar(
            &path,
            &[("META-INF/versions/9/pluginfixture/Person.class", class)],
        );
        assert!(read_class_file(&path, "pluginfixture/Person.class", "origin").is_err());
        write_binary_jar(
            &path,
            &[
                ("META-INF/versions/9/module-info.class", b"unselected"),
                ("pluginfixture/Person.class", class),
            ],
        );
        assert!(
            read_class_file(&path, "pluginfixture/Person.class", "origin")
                .unwrap()
                .is_some()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selected_class_reader_rejects_io_checksum_and_size_errors() {
        let class =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class");
        let root =
            std::env::temp_dir().join(format!("ktlsp_selected_class_io_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("test.jar");
        assert!(read_class_file(&path, "Person.class", "origin").is_err());
        fs::write(&path, b"not a zip").unwrap();
        assert!(read_class_file(&path, "Person.class", "origin").is_err());
        write_binary_jar(&path, &[("Person.class", class)]);
        let mut corrupt = fs::read(&path).unwrap();
        let start = corrupt
            .windows(class.len())
            .position(|bytes| bytes == class)
            .unwrap();
        corrupt[start + class.len() - 1] ^= 1;
        fs::write(&path, corrupt).unwrap();
        assert!(read_class_file(&path, "Person.class", "origin").is_err());
        let oversized = vec![0; ktcore::classfile::MAX_CLASS_FILE_BYTES + 1];
        write_binary_jar(&path, &[("Person.class", &oversized)]);
        assert!(read_class_file(&path, "Person.class", "origin").is_err());
        fs::write(root.join("Person.class"), &oversized).unwrap();
        assert!(read_class_file(&root, "Person.class", "origin").is_err());
        fs::write(root.join("Person.class"), b"not a classfile").unwrap();
        assert!(read_class_file(&root, "Person.class", "origin").is_err());
        let mut forged = fs::read(&path).unwrap();
        let central = forged
            .windows(4)
            .position(|bytes| bytes == b"PK\x01\x02")
            .unwrap();
        forged[central + 24..central + 28].copy_from_slice(&1u32.to_le_bytes());
        fs::write(&path, forged).unwrap();
        assert!(read_class_file(&path, "Person.class", "origin").is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_ap_disable_keeps_javac_plugin_and_manifest_guards() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_proc_none_services_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for (name, entries, expected) in [
            (
                "processor",
                vec![(
                    "META-INF/services/javax.annotation.processing.Processor",
                    "ap.Provider",
                )],
                true,
            ),
            (
                "plugin",
                vec![(
                    "META-INF/services/com.sun.source.util.Plugin",
                    "plugin.Provider",
                )],
                false,
            ),
            (
                "both",
                vec![
                    (
                        "META-INF/services/javax.annotation.processing.Processor",
                        "ap.Provider",
                    ),
                    (
                        "META-INF/services/com.sun.source.util.Plugin",
                        "plugin.Provider",
                    ),
                ],
                false,
            ),
            (
                "redirect",
                vec![(
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\nClass-Path: hidden.jar\n",
                )],
                false,
            ),
        ] {
            let jar = root.join(format!("{name}.jar"));
            write_jar(&jar, &entries);
            assert_eq!(
                plain_java_classpath_entry(&jar, true),
                expected,
                "jar {name}"
            );
            let directory = root.join(name);
            for (entry, contents) in entries {
                let path = directory.join(entry);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, contents).unwrap();
            }
            assert_eq!(
                plain_java_classpath_entry(&directory, true),
                expected,
                "directory {name}"
            );
            assert!(
                !plain_java_classpath_entry(&jar, false),
                "normal discovery {name}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plain_java_classpath_checks_services_and_manifest_in_jars_and_directories() {
        let cases: &[(&str, &[(&str, &str)], bool)] = &[
            ("empty", &[], true),
            (
                "manifest",
                &[("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\r\n\r\n")],
                true,
            ),
            (
                "processor",
                &[(
                    "META-INF/services/javax.annotation.processing.Processor",
                    "",
                )],
                false,
            ),
            (
                "plugin",
                &[("META-INF/services/com.sun.source.util.Plugin", "")],
                false,
            ),
            (
                "redirect",
                &[(
                    "META-INF/MANIFEST.MF",
                    "Manifest-Version: 1.0\nClass-Path: dependency.jar\n",
                )],
                false,
            ),
            (
                "empty_redirect",
                &[("META-INF/MANIFEST.MF", "cLaSs-PaTh:\r\n")],
                false,
            ),
        ];
        let root =
            std::env::temp_dir().join(format!("ktlsp_plain_classpath_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        for (name, entries, expected) in cases {
            let jar = root.join(format!("{name}.JAR"));
            write_jar(&jar, entries);
            assert_eq!(
                plain_java_classpath_entry(&jar, false),
                *expected,
                "jar {name}"
            );
            let directory = root.join(name);
            fs::create_dir_all(&directory).unwrap();
            for (entry, contents) in *entries {
                let path = directory.join(entry);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, contents).unwrap();
            }
            assert_eq!(
                plain_java_classpath_entry(&directory, false),
                *expected,
                "directory {name}"
            );
        }
        assert!(!plain_java_classpath_entry(
            &root.join("missing.jar"),
            false
        ));
        write_jar(&root.join("unknown.zip"), &[]);
        assert!(!plain_java_classpath_entry(
            &root.join("unknown.zip"),
            false
        ));
        fs::write(root.join("corrupt.jar"), b"not a zip").unwrap();
        assert!(!plain_java_classpath_entry(
            &root.join("corrupt.jar"),
            false
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn plain_java_classpath_rejects_unreadable_and_oversized_manifests() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_plain_manifest_io_{}", std::process::id()));
        fs::create_dir_all(root.join("META-INF")).unwrap();
        let manifest = root.join("META-INF/MANIFEST.MF");
        let jar = root.join("test.jar");
        for body in [b"\xff".as_slice(), vec![b'x'; 64 * 1024 + 1].as_slice()] {
            fs::write(&manifest, body).unwrap();
            write_binary_jar(&jar, &[("META-INF/MANIFEST.MF", body)]);
            assert!(!plain_java_classpath_entry(&root, false));
            assert!(!plain_java_classpath_entry(&jar, false));
        }
        write_jar(&jar, &[("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\n")]);
        let mut corrupt = fs::read(&jar).unwrap();
        let start = corrupt
            .windows(17)
            .position(|bytes| bytes == b"Manifest-Version:")
            .unwrap();
        corrupt[start] ^= 1;
        fs::write(&jar, corrupt).unwrap();
        assert!(!plain_java_classpath_entry(&jar, false));
        write_jar(
            &jar,
            &[("meta-inf/manifest.mf", "Class-Path: hidden.jar\n")],
        );
        assert!(!plain_java_classpath_entry(&jar, false));
        fs::remove_file(&manifest).unwrap();
        fs::create_dir(&manifest).unwrap();
        assert!(!plain_java_classpath_entry(&root, false));
        fs::remove_dir_all(root).unwrap();
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
    #[test]
    fn source_encoding_all_transports_share_normalized_utf8() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_encoding_transports_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("sources.jar");
        let java = b"// Bj\xf6rn \x80\xa3\npackage api; public class Useful { public void m\xf6ve() {} public void target() {} }\n";
        let kotlin = "\u{feff}// \u{65e5}\u{672c}\npackage api\nclass KotlinUseful\n";
        write_binary_jar(
            &archive,
            &[
                ("api/Useful.java", java),
                ("api/KotlinUseful.kt", kotlin.as_bytes()),
            ],
        );
        let expected = java.iter().copied().map(char::from).collect::<String>();
        let encoding = JavaSourceEncoding::Iso88591;
        let visited_root = root.join("visited");
        let mut visited = std::collections::BTreeMap::new();
        assert_eq!(
            visit_sources_with_encoding(&archive, &visited_root, encoding, |p, t| {
                visited.insert(
                    p.strip_prefix(&visited_root).unwrap().to_path_buf(),
                    t.to_owned(),
                );
            })
            .unwrap(),
            2
        );
        assert_eq!(visited[Path::new("api/Useful.java")], expected);
        assert_eq!(visited[Path::new("api/KotlinUseful.kt")], kotlin);
        let bytes: std::sync::Arc<[u8]> = fs::read(&archive).unwrap().into();
        let mut sharded = std::collections::BTreeMap::new();
        for shard in 0..2 {
            visit_sources_shard_with_encoding(
                bytes.clone(),
                &archive,
                &visited_root,
                shard,
                2,
                encoding,
                |p, t| {
                    sharded.insert(
                        p.strip_prefix(&visited_root).unwrap().to_path_buf(),
                        t.to_owned(),
                    );
                },
            )
            .unwrap();
        }
        assert_eq!(sharded, visited);
        let full = root.join("full");
        assert_eq!(
            extract_sources_with_encoding(&archive, &full, encoding)
                .unwrap()
                .len(),
            2
        );
        let single = root.join("single");
        for (relative, text) in visited {
            let target = single.join(&relative);
            assert!(
                extract_source_file_with_encoding(&archive, &single, &target, encoding).unwrap()
            );
            assert_eq!(fs::read(&target).unwrap(), text.as_bytes());
            assert_eq!(fs::read(full.join(relative)).unwrap(), text.as_bytes());
        }
        assert!(visit_sources(&archive, &root.join("strict"), |_, _| {}).is_err());
        assert!(extract_sources(&archive, &root.join("strict-full")).is_err());
        assert!(!root.join("strict-full/api/Useful.java").exists());
        for encoding in [JavaSourceEncoding::Utf8, JavaSourceEncoding::Iso88591] {
            write_binary_jar(&archive, &[("Bad.kt", b"// \xf6\nclass Bad")]);
            assert!(
                visit_sources_with_encoding(&archive, &visited_root, encoding, |_, _| {}).is_err()
            );
            assert!(extract_source_file_with_encoding(
                &archive,
                &single,
                &single.join("Bad.kt"),
                encoding
            )
            .is_err());
            assert!(!single.join("Bad.kt").exists());
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_encoding_crc_is_checked_before_decode_or_materialization() {
        let root = std::env::temp_dir().join(format!("ktlsp_encoding_crc_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("sources.jar");
        for encoding in [JavaSourceEncoding::Utf8, JavaSourceEncoding::Iso88591] {
            let payload = b"// invalid UTF8 \xff\nclass Corrupt {}\n";
            write_binary_jar(&archive, &[("Corrupt.java", payload)]);
            let mut bytes = fs::read(&archive).unwrap();
            let start = bytes
                .windows(payload.len())
                .position(|part| part == payload)
                .unwrap();
            bytes[start + payload.len() - 1] ^= 1;
            fs::write(&archive, &bytes).unwrap();
            let dest = root.join(format!("{encoding:?}"));
            let target = dest.join("Corrupt.java");
            let mut callbacks = 0;
            let errors = [
                visit_sources_with_encoding(&archive, &dest, encoding, |_, _| callbacks += 1)
                    .unwrap_err(),
                visit_sources_shard_with_encoding(
                    bytes.clone().into(),
                    &archive,
                    &dest,
                    0,
                    1,
                    encoding,
                    |_, _| callbacks += 1,
                )
                .unwrap_err(),
                extract_sources_with_encoding(&archive, &dest, encoding).unwrap_err(),
                extract_source_file_with_encoding(&archive, &dest, &target, encoding).unwrap_err(),
            ];
            for error in errors {
                assert!(
                    format!("{error:#}")
                        .to_ascii_lowercase()
                        .contains("checksum"),
                    "CRC must win over decode: {error:#}"
                );
            }
            assert_eq!(callbacks, 0);
            assert!(!target.exists());
        }
        fs::remove_dir_all(root).unwrap();
    }
    fn outer97_binary_fixture(ownership: &str) -> (crate::workspace::Workspace, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-binary-outer-{}-{}-{ownership}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let classes = [
            (
                "Item",
                include_bytes!("../../ktcore/tests/fixtures/classfile/outerfixture/Item.class")
                    .as_slice(),
            ),
            (
                "SpecialItem",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/outerfixture/SpecialItem.class"
                )
                .as_slice(),
            ),
            (
                "Other",
                include_bytes!("../../ktcore/tests/fixtures/classfile/outerfixture/Other.class")
                    .as_slice(),
            ),
            (
                "Outer",
                include_bytes!("../../ktcore/tests/fixtures/classfile/outerfixture/Outer.class")
                    .as_slice(),
            ),
            (
                "Outer$Inner",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/outerfixture/Outer$Inner.class"
                )
                .as_slice(),
            ),
            (
                "Outer$Shadow",
                include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/outerfixture/Outer$Shadow.class"
                )
                .as_slice(),
            ),
            (
                "Factory",
                include_bytes!("../../ktcore/tests/fixtures/classfile/outerfixture/Factory.class")
                    .as_slice(),
            ),
        ];
        let names: Vec<_> = classes
            .iter()
            .filter(|(name, _)| ownership != "missing" || *name != "Outer")
            .map(|(name, bytes)| (format!("outerfixture/{name}.class"), *bytes))
            .collect();
        let mut entries: Vec<_> = names
            .iter()
            .map(|(name, bytes)| (name.as_str(), *bytes))
            .collect();
        if ownership == "ambiguous" {
            // Equal-length alternate path becomes a duplicate enclosing-class entry after ZIP
            // creation, including both local/central names. No archive-order owner choice.
            entries.push((
                "outerfixture/Ovter.class",
                classes.iter().find(|(n, _)| *n == "Outer").unwrap().1,
            ));
        }
        let jar = root.join("api.jar");
        write_binary_jar(&jar, &entries);
        if ownership == "ambiguous" {
            let bytes = fs::read(&jar).unwrap();
            let from = b"outerfixture/Ovter.class";
            let to = b"outerfixture/Outer.class";
            let mut changed = bytes.clone();
            for i in 0..bytes.len().saturating_sub(from.len()) {
                if &bytes[i..i + from.len()] == from {
                    changed[i..i + from.len()].copy_from_slice(to);
                }
            }
            fs::write(&jar, changed).unwrap();
        }
        let mut workspace = crate::workspace::Workspace::new();
        fs::create_dir_all(root.join("src")).unwrap();
        workspace.set_project_model(std::sync::Arc::new(crate::classpath::ProjectSnapshot::new(
            crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCESET\tmain\nSOURCE\tmain\t{}\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\touterfixture\tapi\t1\nEND\t:app\n",
                root.display(), root.join("src").display(), jar.display(), jar.display()
            )),
        )));
        let coord = crate::coords::Coordinate::parse("outerfixture:api:1").unwrap();
        let mut kotlin = crate::parser::KotlinParser::new();
        let mut java = crate::java::JavaParser::new();
        for batch in crate::deps::resolve_module_binary_stubs(
            &coord,
            &[(jar.clone(), crate::classpath::file_origin_id(&jar))],
            &root.join("stubs"),
            &mut kotlin,
            &mut java,
        )
        .unwrap()
        {
            assert!(workspace.index_dependency_batch(batch));
        }
        (workspace, root)
    }

    fn outer97_assert_chain(
        workspace: &mut crate::workspace::Workspace,
        key: &str,
        source: &str,
        chain: &str,
        useful: &str,
        wrong: &str,
    ) {
        let at = source.find(chain).unwrap() + chain.len();
        let query = workspace
            .completion_query(key, at)
            .expect("binary generic chain query");
        assert!(
            query
                .candidates
                .iter()
                .any(|candidate| candidate.label == useful),
            "{key}: {chain} missing {useful}: {:?}",
            query.candidates
        );
        assert!(
            query
                .candidates
                .iter()
                .all(|candidate| candidate.label != wrong),
            "{key}: {chain} leaked {wrong}"
        );
        let defs = workspace.goto_definition(key, at);
        assert_eq!(defs.len(), 1, "{key}: exact {useful} navigation");
        let target = workspace.doc_text(&defs[0].file).unwrap();
        assert_eq!(&target[defs[0].start_byte..defs[0].end_byte], useful);
        assert!(defs[0].file.ends_with(if useful == "special" {
            "SpecialItem.java"
        } else {
            "Other.java"
        }));
    }

    #[test]
    fn binary_outer_instantiation_reaches_shared_queries() {
        let (mut workspace, root) = outer97_binary_fixture("complete");
        for (name, source) in [
            ("Use.java", "package outerfixture; class Use { void use(Factory factory, Other other) { factory.make().outer().special(); factory.make().inner().pong(); factory.value.outer().special(); factory.make().shadow(other).pong(); factory.shadow().own().pong(); } }"),
            ("Use.kt", "package outerfixture\nfun use(factory: Factory, other: Other) { factory.make().outer().special(); factory.make().inner().pong(); factory.value.outer().special(); factory.make().shadow(other).pong(); factory.shadow().own().pong() }")
        ] {
            let key=crate::paths::file_key(&root.join("src").join(name)); workspace.open(&key,source.into()); workspace.scan(&root.join("src"));
            for (chain,useful,wrong) in [
                ("factory.make().outer().","special","pong"),
                ("factory.make().inner().","pong","ping"),
                ("factory.value.outer().","special","pong"),
                ("factory.make().shadow(other).","pong","ping"),
                ("factory.shadow().own().","pong","ping")
            ] { outer97_assert_chain(&mut workspace,&key,source,chain,useful,wrong); }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_outer_requires_unique_enclosing_owner_proof() {
        for ownership in ["missing", "ambiguous"] {
            let (mut workspace, root) = outer97_binary_fixture(ownership);
            for (name,source) in [
                ("Use.java","package outerfixture; class Use { void use(Factory factory) { factory.make().outer().special(); } }"),
                ("Use.kt","package outerfixture\nfun use(factory: Factory) { factory.make().outer().special() }")
            ] {
                let key=crate::paths::file_key(&root.join("src").join(name)); workspace.open(&key,source.into()); workspace.scan(&root.join("src"));
                let at=source.find("factory.make().outer().").unwrap()+"factory.make().outer().".len();
                assert!(workspace.completion_query(&key,at).is_none_or(|q|q.candidates.iter().all(|c|c.label!="special")), "{ownership}: unproved owner arguments must not be inferred");
                assert!(workspace.goto_definition(&key,at).is_empty());
            }
            fs::remove_dir_all(root).unwrap();
        }
    }
}
