//! Dependency-source indexing: version catalog -> coordinates -> source jars -> indexed symbols.
//!
//! The heavy work (locate/download/extract/parse) is lock-free and returns `(file_key, symbols)`
//! batches; the caller inserts them into the shared index under brief locks so goto-definition can
//! interleave while indexing proceeds.
//!
//! Parsing a dependency's sources is the dominant startup cost (~10s for kotlin-stdlib). Source
//! jars are parsed directly from ZIP entries; a source entry is only materialized on disk when an
//! editor needs to open its goto target. We persist parsed symbols to a `symcache` keyed by a
//! cheap jar fingerprint (path + mtime + size); a cache hit
//! deserializes the symbols and skips parsing entirely, turning that ~10s into a one-time-per-jar
//! cost.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::LazyLock;
use std::sync::Mutex;
use std::sync::Once;
use std::time::{Duration, Instant};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::{DirEntry, WalkDir};

use crate::artifacts::{self, Repos};
use crate::catalog;
use crate::coords::{compare_versions, Coordinate};
use crate::jar;
use crate::java::JavaParser;
use crate::language::{self, SourceLanguage};
use crate::parser::KotlinParser;
use crate::project_model::{FileOrigin, SourceRepresentation};
use crate::source_encoding::{JavaSourceEncoding, SourceTextPolicy};
use crate::symbol::IndexedSymbol;

/// One file's worth of indexed symbols, keyed by its stable on-disk path (the goto target is
/// materialized lazily from its source jar when necessary).
#[derive(Serialize, Deserialize)]
pub struct FileSymbols {
    pub file: String,
    pub symbols: Vec<IndexedSymbol>,
    pub jvm: Option<ktcore::jvm::SourceFileFacts>,
    /// Runtime project ownership is reapplied after loading the shared symbol cache.
    #[serde(skip)]
    pub origin: Option<FileOrigin>,
    /// Exact selected archive authority, reapplied after shared symbol-cache loads.
    #[serde(skip)]
    pub library_source: Option<std::sync::Arc<LibrarySource>>,
}

/// A resolved source location for a dependency coordinate. The `identity` is stable for the actual
/// source artifact, not just the requested coordinate, so `foo` and its `foo-jvm` fallback can be
/// recognized as the same library before parsing.
#[derive(Clone, Debug)]
pub struct LibrarySource {
    dest: PathBuf,
    jar: Option<PathBuf>,
    fingerprint: Option<String>,
    origin_id: String,
    policy: Result<SourceTextPolicy, String>,
}

impl LibrarySource {
    fn jdk_archive(src_zip: &Path, dest: &Path, fingerprint: &str) -> Self {
        Self {
            dest: dest.into(),
            jar: Some(src_zip.into()),
            fingerprint: Some(fingerprint.into()),
            origin_id: crate::classpath::jdk_origin_id(src_zip),
            policy: Ok(SourceTextPolicy::default()),
        }
    }
    pub(crate) fn is_current(&self) -> bool {
        let (Some(jar), Some(expected), Ok(policy)) = (&self.jar, &self.fingerprint, &self.policy)
        else {
            return false;
        };
        jar_fingerprint(jar).as_ref() == Some(expected) && policy.validate().is_ok()
    }
    fn stamp_batches(&self, mut batches: Vec<FileSymbols>) -> Vec<FileSymbols> {
        let source = std::sync::Arc::new(self.clone());
        for batch in &mut batches {
            batch.library_source = Some(source.clone());
        }
        batches
    }

    pub fn identity(&self) -> String {
        let original = if let Some(fingerprint) = &self.fingerprint {
            format!("jar:{fingerprint}")
        } else if let Some(jar) = &self.jar {
            format!("jar:{}", jar.to_string_lossy())
        } else {
            format!("extracted:{}", self.dest.to_string_lossy())
        };
        match &self.policy {
            Ok(policy) => format!("source:{}", policy.cache_key(&original)),
            Err(reason) => format!(
                "invalid-source:{:x}",
                Sha256::digest(format!("{original}:{reason}"))
            ),
        }
    }
}

/// Read coordinates from every version catalog under `root` (`gradle/libs.versions.toml`, a
/// top-level `libs.versions.toml`, and nested Gradle builds' catalogs), then ensure kotlin-stdlib is
/// among them (the Kotlin Gradle plugin adds it implicitly, so most projects never list it).
/// Returns empty if no catalog is present, it can't be parsed, AND the root doesn't look like a
/// Gradle project.
pub fn coordinates_for_root(root: &Path) -> Vec<Coordinate> {
    let mut coords = Vec::new();
    for path in catalog_paths(root) {
        if let Ok(src) = fs::read_to_string(&path) {
            match catalog::parse_catalog(&src) {
                Ok(c) => {
                    coords.extend(c);
                }
                Err(e) => tracing::warn!("failed to parse {}: {e}", path.display()),
            }
        }
    }
    coords.sort();
    coords.dedup();
    inject_stdlib(&mut coords, is_gradle_project(root));
    coords
}

/// Binary jars for catalog entries whose version can be inferred from already-populated local
/// Gradle/Maven caches. This covers BOM-managed aliases without executing Gradle.
pub fn cached_catalog_binary_jars(root: &Path, repos: &Repos) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();
    for coord in cached_catalog_coordinates(root, repos) {
        if let Some(jar) = artifacts::binary_jar(repos, &coord) {
            out.insert(jar);
        }
    }
    out.into_iter().collect()
}

/// Coordinates for catalog library aliases whose versions are explicit, BOM-inferred, or already
/// present in local Gradle/Maven caches.
pub fn cached_catalog_coordinates(root: &Path, repos: &Repos) -> Vec<Coordinate> {
    let mut out = BTreeSet::new();
    for path in catalog_paths(root) {
        let Ok(src) = fs::read_to_string(&path) else {
            continue;
        };
        if let Ok(coords) = catalog::parse_catalog(&src) {
            out.extend(coords);
        }
        let Ok(modules) = catalog::parse_catalog_modules(&src) else {
            continue;
        };
        for (group, artifact) in modules {
            let Some(coord) = artifacts::best_cached_coordinate(repos, &group, &artifact) else {
                continue;
            };
            out.insert(coord);
        }
    }
    out.into_iter().collect()
}

/// Literal Maven coordinates declared directly in Gradle build files.
///
/// This avoids running Gradle for large or toolchain-sensitive repos while still covering common
/// dependencies that are not in a version catalog, including BOM-managed calls such as:
/// `api(platform("software.amazon.awssdk:bom:2.31.17")); api("software.amazon.awssdk:s3")`.
pub fn coordinates_from_build_files(root: &Path, repos: &Repos) -> Vec<Coordinate> {
    let mut out = BTreeSet::new();
    for entry in WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !is_catalog_excluded(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !matches!(name, "build.gradle" | "build.gradle.kts") {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        out.extend(parse_build_file_coordinates(&text, repos));
    }
    out.into_iter().collect()
}

fn parse_build_file_coordinates(text: &str, repos: &Repos) -> Vec<Coordinate> {
    let mut managed_versions = BTreeMap::<String, BTreeSet<String>>::new();
    for line in text.lines() {
        if !line.contains("platform(") && !line.contains("enforcedPlatform(") {
            continue;
        }
        for literal in quoted_values(line) {
            let Some((group, artifact, Some(version))) = split_maven_literal(literal) else {
                continue;
            };
            if is_bom_like_artifact(&artifact) {
                managed_versions.entry(group).or_default().insert(version);
            }
        }
    }
    let managed_versions: BTreeMap<String, String> = managed_versions
        .into_iter()
        .filter_map(|(group, versions)| {
            (versions.len() == 1).then(|| {
                let version = versions.into_iter().next().unwrap();
                (group, version)
            })
        })
        .collect();

    let mut out = BTreeSet::new();
    for line in text.lines() {
        for literal in quoted_values(line) {
            let Some((group, artifact, version)) = split_maven_literal(literal) else {
                continue;
            };
            let version = version
                .or_else(|| managed_versions.get(&group).cloned())
                .or_else(|| {
                    artifacts::best_cached_coordinate(repos, &group, &artifact)
                        .map(|coord| coord.version)
                });
            let Some(version) = version else {
                continue;
            };
            if let Some(coord) = Coordinate::parse(&format!("{group}:{artifact}:{version}")) {
                out.insert(coord);
            }
        }
    }
    out.into_iter().collect()
}

fn quoted_values(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut chars = text.char_indices();
    while let Some((start, ch)) = chars.next() {
        if ch != '"' && ch != '\'' {
            continue;
        }
        let quote = ch;
        let value_start = start + ch.len_utf8();
        let mut value_end = None;
        for (idx, c) in chars.by_ref() {
            if c == quote {
                value_end = Some(idx);
                break;
            }
        }
        if let Some(value_end) = value_end {
            out.push(&text[value_start..value_end]);
        } else {
            break;
        }
    }
    out
}

fn split_maven_literal(value: &str) -> Option<(String, String, Option<String>)> {
    let parts: Vec<_> = value.split(':').collect();
    match parts.as_slice() {
        [group, artifact] if safe_maven_component(group) && safe_maven_component(artifact) => {
            Some(((*group).to_string(), (*artifact).to_string(), None))
        }
        [group, artifact, version]
            if safe_maven_component(group)
                && safe_maven_component(artifact)
                && safe_maven_component(version) =>
        {
            Some((
                (*group).to_string(),
                (*artifact).to_string(),
                Some((*version).to_string()),
            ))
        }
        _ => None,
    }
}

fn safe_maven_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

fn is_bom_like_artifact(artifact: &str) -> bool {
    artifact.ends_with("-bom")
        || artifact.ends_with("-dependencies")
        || artifact.ends_with("-platform")
        || artifact == "bom"
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModuleKey {
    group: String,
    artifact: String,
}

impl From<&Coordinate> for ModuleKey {
    fn from(coord: &Coordinate) -> Self {
        ModuleKey {
            group: coord.group.clone(),
            artifact: coord.artifact.clone(),
        }
    }
}

/// Decision for a coordinate considered by [`CoordinateSelector`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CoordinateDecision {
    /// This coordinate is currently the selected version for its `group:artifact`.
    Selected,
    /// This coordinate superseded a lower selected version; callers should remove any symbols
    /// already indexed for `previous` before indexing the new coordinate.
    Replaces(Coordinate),
    /// A newer or equal coordinate for this `group:artifact` is already selected.
    ShadowedBy(Coordinate),
}

/// Gradle-style fixed-version conflict collapse for dependency source indexing.
///
/// ktlsp's dependency index is advisory and intentionally avoids executing Gradle during startup.
/// This selector applies the one piece that matters for duplicate source definitions: for a Maven
/// module (`group:artifact`), keep only one selected version, preferring the newer fixed version in
/// the same way Gradle's default conflict resolution does for ordinary releases.
#[derive(Default)]
pub struct CoordinateSelector {
    selected: BTreeMap<ModuleKey, Coordinate>,
}

impl CoordinateSelector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn consider(&mut self, coord: Coordinate) -> CoordinateDecision {
        let key = ModuleKey::from(&coord);
        let Some(current) = self.selected.get(&key).cloned() else {
            self.selected.insert(key, coord);
            return CoordinateDecision::Selected;
        };

        match compare_versions(&coord.version, &current.version) {
            Ordering::Greater => {
                self.selected.insert(key, coord);
                CoordinateDecision::Replaces(current)
            }
            Ordering::Equal | Ordering::Less => CoordinateDecision::ShadowedBy(current),
        }
    }
}

fn catalog_paths(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let root_catalog = root.join("gradle/libs.versions.toml");
    if root_catalog.is_file() {
        out.push(root_catalog);
    }
    let top_level_catalog = root.join("libs.versions.toml");
    if top_level_catalog.is_file() {
        out.push(top_level_catalog);
    }
    let nested = git_catalog_paths(root).unwrap_or_else(|| {
        WalkDir::new(root)
            .into_iter()
            .filter_entry(|entry| !is_catalog_excluded(entry))
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .collect()
    });
    for path in nested {
        if path.file_name().and_then(|n| n.to_str()) != Some("libs.versions.toml") {
            continue;
        }
        let Some(parent) = path.parent() else {
            continue;
        };
        if parent.file_name().and_then(|n| n.to_str()) != Some("gradle") {
            continue;
        }
        if !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    }
    // React Native commonly supplies Android dependencies through its own Gradle included build
    // under `node_modules/react-native`.  It is intentionally ignored by the broad workspace
    // walk above, but its version catalog is part of the active Android build's dependency graph.
    // Probe only direct packages (and direct scoped packages) rather than walking node_modules.
    for path in node_modules_catalog_paths(root) {
        if !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn node_modules_catalog_paths(root: &Path) -> Vec<PathBuf> {
    let node_modules = root.join("node_modules");
    let Ok(entries) = fs::read_dir(node_modules) else {
        return Vec::new();
    };
    let mut packages = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('@') {
            if let Ok(scoped) = fs::read_dir(path) {
                packages.extend(scoped.flatten().map(|entry| entry.path()));
            }
        } else {
            packages.push(path);
        }
    }
    let mut catalogs = packages
        .into_iter()
        .map(|package| package.join("gradle/libs.versions.toml"))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    catalogs.sort();
    catalogs.dedup();
    catalogs
}

fn git_catalog_paths(root: &Path) -> Option<Vec<PathBuf>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            ":(glob)**/libs.versions.toml",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut paths = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|rel| !rel.is_empty())
        .map(|rel| root.join(String::from_utf8_lossy(rel).as_ref()))
        .filter(|path| !path_has_catalog_excluded_dir(root, path))
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    Some(paths)
}

fn path_has_catalog_excluded_dir(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "build" | "out" | "target" | "node_modules" | ".gradle"
            )
    })
}

fn is_catalog_excluded(entry: &DirEntry) -> bool {
    let name = entry.file_name().to_string_lossy();
    if entry.file_type().is_dir() {
        return name.starts_with('.')
            || matches!(
                name.as_ref(),
                "build" | "out" | "target" | "node_modules" | ".gradle"
            );
    }
    false
}

/// Pinned standalone default and fallback when a project's Kotlin version is unavailable.
const DEFAULT_KOTLIN_VERSION: &str = "2.1.20";

/// Add a `org.jetbrains.kotlin:kotlin-stdlib` coordinate if absent and this is a Gradle project,
/// versioned from an existing `org.jetbrains.kotlin` coordinate when possible (e.g. kotlin-reflect),
/// else the pinned fallback. The gating avoids indexing ~10s of stdlib for an unrelated directory of
/// loose `.kt` files. Pure (no filesystem) so it is unit-testable.
pub(crate) fn inject_stdlib(coords: &mut Vec<Coordinate>, kotlin_project: bool) {
    const GROUP: &str = "org.jetbrains.kotlin";
    const ARTIFACT: &str = "kotlin-stdlib";
    if !kotlin_project
        || coords
            .iter()
            .any(|c| c.group == GROUP && c.artifact == ARTIFACT)
    {
        return;
    }
    let version = coords
        .iter()
        .find(|c| c.group == GROUP)
        .map(|c| c.version.clone())
        .unwrap_or_else(|| DEFAULT_KOTLIN_VERSION.to_string());
    if let Some(c) = Coordinate::parse(&format!("{GROUP}:{ARTIFACT}:{version}")) {
        coords.push(c);
    }
}

/// Whether `root` itself looks like a Gradle/Kotlin project (worth auto-indexing kotlin-stdlib
/// for, and the gate for spawning a gradle compile).
pub fn is_gradle_project(root: &Path) -> bool {
    gradle_root(root).is_some()
}

/// Find the directory to run Gradle from. If `root` itself is a Gradle project, use it; otherwise
/// search a bounded set of nested directories. Monorepos often keep their Android build below an
/// application directory (for example `apps/mobile/android`) while the editor opens the Git root.
/// Wrapper/settings roots outrank standalone module build files.
pub fn gradle_root(root: &Path) -> Option<PathBuf> {
    if root_has_gradle_files(root) {
        return Some(root.to_path_buf());
    }
    let mut candidates = WalkDir::new(root)
        .min_depth(1)
        .max_depth(6)
        .into_iter()
        .filter_entry(|entry| !is_catalog_excluded(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_dir() && root_has_gradle_files(entry.path()))
        .map(DirEntry::into_path)
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        gradle_root_score(b)
            .cmp(&gradle_root_score(a))
            .then_with(|| a.components().count().cmp(&b.components().count()))
            .then_with(|| a.cmp(b))
    });
    candidates.into_iter().next()
}

fn gradle_root_score(path: &Path) -> i32 {
    let mut score = 0;
    if path
        .join(if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        })
        .exists()
    {
        score += 10;
    }
    if path.join("settings.gradle.kts").exists() || path.join("settings.gradle").exists() {
        score += 5;
    }
    if path.join("build.gradle.kts").exists() || path.join("build.gradle").exists() {
        score += 1;
    }
    score
}

fn root_has_gradle_files(root: &Path) -> bool {
    [
        "settings.gradle.kts",
        "settings.gradle",
        "build.gradle.kts",
        "build.gradle",
        "gradle/libs.versions.toml",
        "libs.versions.toml",
    ]
    .iter()
    .any(|f| root.join(f).exists())
}

pub const CACHE_DIR_ENV: &str = "KTLSP_CACHE_DIR";
pub const JDK_SRC_ENV: &str = "KTLSP_JDK_SRC";
pub const ANDROID_SOURCES_ENV: &str = "KTLSP_ANDROID_SOURCES";
const ANDROID_REPOSITORY_ENV: &str = "KTLSP_ANDROID_REPOSITORY";
const ANDROID_DOWNLOAD_ENV: &str = "KTLSP_ANDROID_SOURCES_DOWNLOAD";
const ANDROID_REPOSITORY_BASE: &str = "https://dl.google.com/android/repository";
static ANDROID_SOURCES_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
static ANDROID_DOWNLOAD_FAILURES: LazyLock<Mutex<BTreeMap<u32, Instant>>> =
    LazyLock::new(|| Mutex::new(BTreeMap::new()));

/// ktlsp's cache root (`KTLSP_CACHE_DIR`, `~/.cache/ktlsp`, or under the temp dir if home is unknown).
pub fn cache_home() -> PathBuf {
    cache_home_from(
        std::env::var_os(CACHE_DIR_ENV),
        crate::paths::user_home().map(PathBuf::into_os_string),
    )
}

fn cache_home_from(cache_dir: Option<OsString>, home: Option<OsString>) -> PathBuf {
    if let Some(path) = cache_dir.filter(|p| !p.is_empty()) {
        return PathBuf::from(path);
    }
    home.map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(".cache/ktlsp")
}

/// Where extracted library sources live (the goto targets).
pub fn extract_root() -> PathBuf {
    cache_home().join("extracted")
}

/// Locate the current JDK's `src.zip`, if available. `KTLSP_JDK_SRC` is an explicit override for
/// scripted tests or unusual installations; otherwise try `JAVA_HOME`, macOS' `java_home`, then
/// common JDK install directories.
pub fn jdk_src_zip() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(JDK_SRC_ENV).and_then(existing_src_zip) {
        return Some(path);
    }
    if let Some(path) = std::env::var_os("JAVA_HOME")
        .map(PathBuf::from)
        .and_then(src_zip_for_home)
    {
        return Some(path);
    }
    if let Some(path) = macos_java_home_src_zip() {
        return Some(path);
    }
    common_jdk_src_zip()
}

fn existing_src_zip(path: OsString) -> Option<PathBuf> {
    let path = PathBuf::from(path);
    path.is_file().then_some(path)
}

pub(crate) fn src_zip_for_home(home: PathBuf) -> Option<PathBuf> {
    let candidates = [home.join("lib/src.zip"), home.join("src.zip")];
    candidates.into_iter().find(|p| p.is_file())
}

pub(crate) fn jdk_home_for_source_archive(source: &Path) -> Option<&Path> {
    let parent = source.parent()?;
    if parent.file_name().is_some_and(|name| name == "lib") {
        parent.parent()
    } else {
        Some(parent)
    }
}

fn macos_java_home_src_zip() -> Option<PathBuf> {
    let tool = Path::new("/usr/libexec/java_home");
    if !tool.is_file() {
        return None;
    }
    let output = Command::new(tool).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let home = String::from_utf8(output.stdout).ok()?;
    src_zip_for_home(PathBuf::from(home.trim()))
}

fn common_jdk_src_zip() -> Option<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Library/Java/JavaVirtualMachines"),
        PathBuf::from("/usr/lib/jvm"),
    ];
    if let Some(home) = crate::paths::user_home() {
        roots.push(home.join("Library/Java/JavaVirtualMachines"));
    }
    for root in roots {
        if let Some(path) = find_src_zip_under(&root) {
            return Some(path);
        }
    }
    None
}

fn find_src_zip_under(root: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for entry in entries {
        for candidate in [
            entry.join("Contents/Home/lib/src.zip"),
            entry.join("lib/src.zip"),
            entry.join("src.zip"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Where serialized per-jar symbol tables live.
fn symcache_dir() -> PathBuf {
    cache_home().join("symcache")
}

/// Bumped whenever serialized symbols or their extracted facts change. bincode is positional /
/// non-self-describing, and an old cache can also omit newly indexed semantic facts (such as Java
/// inheritance); folding this tag into the fingerprint forces a one-time re-parse.
const SYMCACHE_VERSION: &[u8] = b"v33-instantiated-generic-owners";
const SOURCE_JAR_MARKER: &str = ".ktlsp-source-jar";
static LEGACY_SYMCACHE_PRUNED: Once = Once::new();
static SYMCACHE_WRITE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A cheap, stat-only fingerprint of a resolved jar (path + full mtime + size).
/// shortcut: replacement bytes with identical size and preserved mtime are not detected;
/// use a content identity if mutable artifact stores require that guarantee.
fn jar_fingerprint(path: &Path) -> Option<String> {
    let meta = fs::metadata(path).ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(SYMCACHE_VERSION);
    hasher.update(b"|");
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(b"|");
    hasher.update(mtime.to_le_bytes());
    hasher.update(b"|");
    hasher.update(meta.len().to_le_bytes());
    Some(format!("{:x}", hasher.finalize()))
}

fn symcache_load(fingerprint: &str, source_dir: &Path) -> Option<Vec<FileSymbols>> {
    symcache_load_file(
        &symcache_dir().join(format!("{fingerprint}.zst")),
        source_dir,
    )
}

fn symcache_load_file(cache_file: &Path, source_dir: &Path) -> Option<Vec<FileSymbols>> {
    let decode_start = crate::trace::enabled().then(std::time::Instant::now);
    // A missing cache file is a normal miss (not an error).
    let bytes = fs::read(cache_file).ok()?;
    let decoded = zstd::stream::decode_all(&bytes[..]).ok()?;
    let mut cached: Vec<FileSymbols> = match bincode::deserialize(&decoded) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(
                "symcache {} is corrupt ({e}); re-parsing",
                cache_file.display()
            );
            return None;
        }
    };
    if let Some(start) = decode_start {
        crate::trace::span(
            "deps.symcache_decode",
            "deps",
            start,
            serde_json::json!({
                "cache": cache_file.file_name().map(|name| name.to_string_lossy()),
                "compressedBytes": bytes.len(),
                "decodedBytes": decoded.len(),
                "files": cached.len(),
            }),
        );
    }
    let restore_start = crate::trace::enabled().then(std::time::Instant::now);
    // Cached facts must have the same identity as the editor's file URI, including caches written
    // with native separators before path keys were normalized.
    for batch in &mut cached {
        batch.file = crate::paths::file_key(Path::new(&batch.file));
    }
    // A lazy source directory can recreate indexed entries from its recorded source identity. Legacy
    // fully-extracted directories still require each cached file to exist.
    if source_dir.join(SOURCE_JAR_MARKER).is_file()
        || cached.iter().all(|fs| Path::new(&fs.file).exists())
    {
        for batch in &mut cached {
            ktcore::symbol::reshare_file_type_contexts(&mut batch.symbols, batch.jvm.as_mut());
        }
        if let Some(start) = restore_start {
            crate::trace::span(
                "deps.symcache_restore",
                "deps",
                start,
                serde_json::json!({
                    "cache": cache_file.file_name().map(|name| name.to_string_lossy()),
                    "files": cached.len(),
                }),
            );
        }
        Some(cached)
    } else {
        tracing::warn!(
            "symcache {} references missing files; re-parsing",
            cache_file.display()
        );
        None
    }
}

#[derive(Serialize, Deserialize, PartialEq)]
struct SourceJarMarker {
    jar: PathBuf,
    fingerprint: String,
    policy: SourceTextPolicy,
}

/// Discard materialized text when its source identity changes, including legacy path-only markers.
fn prepare_lazy_source_dir(jar: &Path, dest: &Path, fingerprint: &str) -> bool {
    prepare_lazy_source_dir_with_policy(jar, dest, fingerprint, &SourceTextPolicy::default())
}

fn discard_lazy_source_dir(dest: &Path) -> bool {
    !dest.exists() || fs::remove_dir_all(dest).is_ok()
}

fn prepare_lazy_source_dir_with_policy(
    jar: &Path,
    dest: &Path,
    fingerprint: &str,
    policy: &SourceTextPolicy,
) -> bool {
    if jar_fingerprint(jar).as_deref() != Some(fingerprint) || policy.validate().is_err() {
        return false;
    }
    let marker = dest.join(SOURCE_JAR_MARKER);
    let expected = SourceJarMarker {
        jar: jar.to_path_buf(),
        fingerprint: fingerprint.to_string(),
        policy: policy.clone(),
    };
    if fs::read(&marker)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SourceJarMarker>(&bytes).ok())
        .is_some_and(|stored| stored == expected)
    {
        return true;
    }
    if !discard_lazy_source_dir(dest) {
        return false;
    }
    if fs::create_dir_all(dest).is_err() {
        return false;
    }
    let Ok(bytes) = serde_json::to_vec(&expected) else {
        return false;
    };
    let tmp = dest.join(format!("{SOURCE_JAR_MARKER}.{}.tmp", std::process::id()));
    fs::write(&tmp, bytes).is_ok() && fs::rename(tmp, marker).is_ok()
}

/// Materialize a lazily indexed source path, if it belongs to a directory owned by a source-jar
/// marker. This is intentionally path-based so `Workspace` can remain unaware of dependency
/// coordinates and cache serialization details.
pub fn materialize_source_file(path: &Path) -> bool {
    for source_dir in path.ancestors().skip(1) {
        let marker = source_dir.join(SOURCE_JAR_MARKER);
        let Ok(bytes) = fs::read(&marker) else {
            continue;
        };
        let Ok(marker) = serde_json::from_slice::<SourceJarMarker>(&bytes) else {
            return false;
        };
        if jar_fingerprint(&marker.jar).as_deref() != Some(marker.fingerprint.as_str())
            || marker.policy.validate().is_err()
        {
            let _ = fs::remove_file(path);
            return false;
        }
        let extracted = jar::extract_source_file_with_encoding(
            &marker.jar,
            source_dir,
            path,
            marker.policy.java,
        )
        .inspect_err(|error| tracing::warn!("materialize {} failed: {error}", path.display()))
        .unwrap_or(false);
        if jar_fingerprint(&marker.jar).as_deref() != Some(marker.fingerprint.as_str())
            || marker.policy.validate().is_err()
        {
            let _ = fs::remove_file(path);
            return false;
        }
        return extracted;
    }
    false
}

fn symcache_store(fingerprint: &str, data: &[FileSymbols]) {
    let dir = symcache_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    LEGACY_SYMCACHE_PRUNED.call_once(|| {
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) == Some("bin") {
                    let _ = fs::remove_file(path);
                }
            }
        }
    });
    let path = dir.join(format!("{fingerprint}.zst"));
    let sequence = SYMCACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
    let tmp = path.with_extension(format!("zst.{}.{sequence}.tmp", std::process::id()));
    if symcache_write_file(&tmp, data).is_ok() {
        let _ = fs::rename(tmp, path);
    }
}

fn symcache_write_file(path: &Path, data: &[FileSymbols]) -> anyhow::Result<()> {
    // Keep the existing bincode payload while avoiding full raw and compressed cache buffers.
    let file = fs::File::create(path)?;
    let encoder = zstd::stream::write::Encoder::new(file, 3)?;
    // Batch primitive serde writes before entering the compressor.
    let mut writer = io::BufWriter::new(encoder);
    bincode::serialize_into(&mut writer, data)?;
    let encoder = writer.into_inner().map_err(|error| error.into_error())?;
    // Finish the frame and close its file before the caller publishes it (required on Windows).
    drop(encoder.finish()?);
    Ok(())
}

/// Parse every extracted `.kt`/`.java` source under `dest` into per-file symbol batches.
fn parse_dir(
    dest: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    let mut out = Vec::new();
    for path in jar::collect_sources(dest)? {
        let text =
            fs::read_to_string(&path).with_context(|| format!("read source {}", path.display()))?;
        if let Some(file) = parse_source_file(&path, &text, kotlin, java) {
            out.push(file);
        }
    }
    Ok(out)
}

fn parse_source_jar(
    jar_path: &Path,
    dest: &Path,
    java_encoding: JavaSourceEncoding,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    let mut out = Vec::new();
    jar::visit_sources_with_encoding(jar_path, dest, java_encoding, |path, text| {
        if let Some(file) = parse_source_file(path, text, kotlin, java) {
            out.push(file);
        }
    })?;
    Ok(out)
}

fn parse_source_file(
    path: &Path,
    text: &str,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Option<FileSymbols> {
    let language = SourceLanguage::for_project_path(path)?;
    let tree = match language {
        SourceLanguage::Kotlin => kotlin.parse(text),
        SourceLanguage::Java => java.parse(text),
    };
    let facts = language::symbol_facts(language, &tree, text);
    (!facts.symbols.is_empty()).then(|| FileSymbols {
        jvm: facts.jvm,
        file: crate::paths::file_key(path),
        symbols: facts.symbols,
        library_source: None,
        origin: None,
    })
}

/// Resolve one coordinate to its source artifact/extraction location without parsing it. Returns
/// nothing if the coordinate has no sources jar and no prior extraction.
pub fn coordinate_source(
    coord: &Coordinate,
    repos: &Repos,
    extract_root: &Path,
) -> Option<LibrarySource> {
    coordinate_source_checked(coord, repos, extract_root).unwrap_or_else(|error| {
        tracing::warn!("resolve {} failed: {error}", coord.label());
        legacy_coordinate_source(coord, extract_root)
    })
}

/// Preserve source lookup failure separately from absence before selecting a binary fallback.
pub(crate) fn coordinate_source_checked(
    coord: &Coordinate,
    repos: &Repos,
    extract_root: &Path,
) -> anyhow::Result<Option<LibrarySource>> {
    if Coordinate::parse(&coord.label()).as_ref() != Some(coord) {
        return Ok(None);
    }
    let selected = match artifacts::selected_sources_jar(repos, coord) {
        Ok(selected) => selected,
        Err(error) => {
            if let Some(source) = legacy_coordinate_source(coord, extract_root) {
                tracing::warn!("resolve {} failed: {error}", coord.label());
                return Ok(Some(source));
            }
            return Err(error);
        }
    };
    let Some((selected, jar)) = selected else {
        return Ok(legacy_coordinate_source(coord, extract_root));
    };
    let policy = SourceTextPolicy::resolve(&selected, |c| artifacts::cached_pom_files(repos, c))
        .map_err(|error| {
            format!(
                "source encoding metadata for {}: {error:#}",
                selected.label()
            )
        });
    Ok(Some(selected_coordinate_source(
        coord,
        jar,
        extract_root,
        policy,
    )))
}

fn selected_coordinate_source(
    coord: &Coordinate,
    jar: PathBuf,
    extract_root: &Path,
    policy: Result<SourceTextPolicy, String>,
) -> LibrarySource {
    let fingerprint = jar_fingerprint(&jar);
    LibrarySource {
        dest: extract_root
            .join(&coord.group)
            .join(&coord.artifact)
            .join(&coord.version),
        jar: Some(jar),
        fingerprint,
        origin_id: crate::classpath::maven_origin_id(coord),
        policy,
    }
}

pub(crate) fn coordinate_source_for_model(
    coord: &Coordinate,
    repos: &Repos,
    extract_root: &Path,
    model: Option<&crate::classpath::ProjectSnapshot>,
) -> anyhow::Result<Option<LibrarySource>> {
    if Coordinate::parse(&coord.label()).as_ref() != Some(coord) {
        return Ok(None);
    }
    let Some(model) = model else {
        return coordinate_source_checked(coord, repos, extract_root);
    };
    let selections = model.coordinate_sources(coord)?.ok_or_else(|| {
        anyhow::anyhow!(
            "Gradle source selection is unavailable for {}",
            coord.label()
        )
    })?;
    let [(jar, expected)] = selections.as_slice() else {
        return Ok(None);
    };
    anyhow::ensure!(
        jar.is_file() && crate::classpath::file_origin_id(jar) == *expected,
        "selected Gradle source artifact changed since project discovery"
    );
    let mut metadata_origins = BTreeMap::new();
    let policy = SourceTextPolicy::resolve_certified(coord, |current| {
        let poms = model.coordinate_source_poms(current)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Gradle POM selection is unavailable for {}",
                current.label()
            )
        })?;
        let mut paths = Vec::with_capacity(poms.len());
        for (path, expected) in poms {
            anyhow::ensure!(
                path.is_file() && crate::classpath::file_origin_id(&path) == expected,
                "selected Gradle POM changed since project discovery"
            );
            paths.push(path.clone());
            metadata_origins.insert(path, expected);
        }
        Ok(paths)
    })
    .and_then(|policy| {
        for (path, expected) in &metadata_origins {
            anyhow::ensure!(
                crate::classpath::file_origin_id(path) == *expected,
                "selected Gradle POM changed during source resolution"
            );
        }
        Ok(policy)
    })
    .map_err(|error| format!("source encoding metadata for {}: {error:#}", coord.label()));
    let source = selected_coordinate_source(coord, jar.clone(), extract_root, policy);
    anyhow::ensure!(
        crate::classpath::file_origin_id(jar) == *expected,
        "selected Gradle source artifact changed during source resolution"
    );
    Ok(Some(source))
}

fn legacy_coordinate_source(coord: &Coordinate, extract_root: &Path) -> Option<LibrarySource> {
    // A lazy directory may contain only a marker or a materialized subset, not the library.
    let dest = extract_root
        .join(&coord.group)
        .join(&coord.artifact)
        .join(&coord.version);
    (dest.is_dir() && !dest.join(SOURCE_JAR_MARKER).exists()).then_some(LibrarySource {
        dest,
        jar: None,
        fingerprint: None,
        origin_id: crate::classpath::maven_origin_id(coord),
        policy: Ok(SourceTextPolicy::default()),
    })
}

/// Index an already-resolved library source: ensure it's extracted, then load parsed symbols from
/// the symcache (skipping parse) or parse + cache them.
pub fn resolve_library_source(
    source: &LibrarySource,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    let mut batches = resolve_library_source_unowned(source, None, kotlin, java)?;
    tag_batches(
        &mut batches,
        &source.origin_id,
        SourceRepresentation::Source,
    );
    Ok(batches)
}

fn tag_batches(batches: &mut [FileSymbols], origin_id: &str, representation: SourceRepresentation) {
    for batch in batches {
        batch.origin = Some(FileOrigin {
            origin_id: origin_id.to_string(),
            representation,
            project_scope: None,
        });
    }
}

fn resolve_library_source_unowned(
    source: &LibrarySource,
    cache_fingerprint: Option<&str>,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    let policy = match &source.policy {
        Ok(policy) => policy,
        Err(reason) => {
            anyhow::ensure!(
                discard_lazy_source_dir(&source.dest),
                "discard stale source text failed"
            );
            anyhow::bail!("{reason}");
        }
    };
    if let Err(error) = policy.validate() {
        anyhow::ensure!(
            discard_lazy_source_dir(&source.dest),
            "discard stale source text failed"
        );
        return Err(error);
    }
    let Some(jar) = &source.jar else {
        return parse_dir(&source.dest, kotlin, java).map(|batches| source.stamp_batches(batches));
    };

    let archive_fingerprint = source
        .fingerprint
        .as_deref()
        .context("source archive identity unavailable")?;
    if !prepare_lazy_source_dir_with_policy(jar, &source.dest, archive_fingerprint, policy) {
        anyhow::bail!(
            "prepare lazy source directory {} failed",
            source.dest.display()
        );
    }

    // Local companion sources retain the binary-origin cache partition and extraction path.
    let fingerprint = policy.cache_key(cache_fingerprint.unwrap_or(archive_fingerprint));
    // Symbol cache: a hit skips the (dominant) parse cost entirely.
    if let Some(cached) = symcache_load(&fingerprint, &source.dest) {
        anyhow::ensure!(
            jar_fingerprint(jar).as_deref() == Some(archive_fingerprint),
            "source archive {} changed while loading cached symbols",
            jar.display()
        );
        policy.validate()?;
        return Ok(source.stamp_batches(cached));
    }
    let symbols = parse_source_jar(jar, &source.dest, policy.java, kotlin, java)?;
    anyhow::ensure!(
        jar_fingerprint(jar).as_deref() == Some(archive_fingerprint),
        "source archive {} changed while parsing symbols",
        jar.display()
    );
    policy.validate()?;
    symcache_store(&fingerprint, &symbols);
    Ok(source.stamp_batches(symbols))
}

/// Resolve one coordinate to indexed symbols: locate/download its sources jar, ensure it's
/// extracted, then load parsed symbols from the symcache (skipping parse) or parse + cache them.
/// Lock-free; returns nothing if the coordinate has no sources jar and no prior extraction.
pub fn resolve_coordinate(
    coord: &Coordinate,
    repos: &Repos,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    coordinate_source(coord, repos, extract_root)
        .map(|source| {
            resolve_library_source(&source, kotlin, java).unwrap_or_else(|error| {
                tracing::warn!("index {} failed: {error:#}", coord.label());
                Vec::new()
            })
        })
        .unwrap_or_default()
}

/// Resolve a local binary jar using adjacent sources, or shallow class stubs if absent.
///
/// Local `files("libs/foo.jar")` dependencies have no Maven coordinate and often no source jar.
/// Stubs let imports, type references and positive Java method facts resolve without sources.
pub fn resolve_local_jar_stubs(
    jar_path: &Path,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    resolve_local_jar_stubs_for_origin(
        jar_path,
        &crate::classpath::file_origin_id(jar_path),
        extract_root,
        kotlin,
        java,
    )
}

pub fn resolve_local_jar_stubs_for_origin(
    jar_path: &Path,
    origin_id: &str,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    if !jar_path.is_file() || crate::classpath::file_origin_id(jar_path) != origin_id {
        anyhow::bail!(
            "local artifact {} changed since project discovery",
            jar_path.display()
        );
    }
    let mut hasher = Sha256::new();
    hasher.update(SYMCACHE_VERSION);
    // Binary stubs without hierarchy and shadow facts must miss.
    hasher.update(b"|local-jar-java-members-v7|");
    hasher.update(origin_id.as_bytes());
    let fingerprint = format!("{:x}", hasher.finalize());
    let (mut batches, representation) = if let Some(sources) =
        crate::classpath::adjacent_sources_jar(jar_path).filter(|sources| sources.is_file())
    {
        let source = LibrarySource {
            dest: extract_root.join("local-jars").join(&fingerprint),
            fingerprint: jar_fingerprint(&sources),
            jar: Some(sources),
            origin_id: origin_id.to_string(),
            policy: Ok(SourceTextPolicy::default()),
        };
        (
            resolve_library_source_unowned(&source, Some(&fingerprint), kotlin, java)?,
            SourceRepresentation::Source,
        )
    } else {
        (
            resolve_local_jar_stubs_unowned(jar_path, extract_root, &fingerprint, kotlin, java)?,
            SourceRepresentation::Stub,
        )
    };
    if crate::classpath::file_origin_id(jar_path) != origin_id {
        anyhow::bail!(
            "local artifact {} changed while indexing",
            jar_path.display()
        );
    }
    tag_batches(&mut batches, origin_id, representation);
    Ok(batches)
}

/// Positive member facts from one exact Gradle-selected module binary; this is not source closure.
pub(crate) fn resolve_module_binary_stubs(
    coord: &Coordinate,
    selected_binaries: &[(PathBuf, String)],
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    if Coordinate::parse(&coord.label()).as_ref() != Some(coord) {
        return Ok(Vec::new());
    }
    // The existing model supplies no variant precedence for multiple paths of one coordinate.
    let [(jar_path, expected_origin)] = selected_binaries else {
        return Ok(Vec::new());
    };
    if jar_path
        .extension()
        .and_then(|extension| extension.to_str())
        != Some("jar")
    {
        return Ok(Vec::new());
    }
    anyhow::ensure!(
        jar_path.is_file() && crate::classpath::file_origin_id(jar_path) == *expected_origin,
        "selected module artifact changed since project discovery: {}",
        jar_path.display()
    );
    let origin_id = crate::classpath::maven_origin_id(coord);
    let mut hasher = Sha256::new();
    hasher.update(SYMCACHE_VERSION);
    hasher.update(b"|module-jar-java-members-v7|");
    hasher.update(origin_id.as_bytes());
    hasher.update(b"|");
    hasher.update(expected_origin.as_bytes());
    let fingerprint = format!("{:x}", hasher.finalize());
    let mut batches =
        resolve_local_jar_stubs_unowned(jar_path, extract_root, &fingerprint, kotlin, java)?;
    anyhow::ensure!(
        crate::classpath::file_origin_id(jar_path) == *expected_origin,
        "selected module artifact changed while indexing: {}",
        jar_path.display()
    );
    // Name-only fallbacks are classes; a decoded marker interface has useful exact type facts.
    batches.retain(|batch| {
        batch.symbols.iter().any(|symbol| {
            matches!(
                symbol.kind,
                ktcore::symbol::SymbolKind::Function
                    | ktcore::symbol::SymbolKind::Property
                    | ktcore::symbol::SymbolKind::Interface
            ) || symbol.kind.is_type_like() && symbol.unavailable_members.is_some()
        })
    });
    tag_batches(&mut batches, &origin_id, SourceRepresentation::Stub);
    Ok(batches)
}

fn resolve_local_jar_stubs_unowned(
    jar_path: &Path,
    extract_root: &Path,
    fingerprint: &str,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    if !jar_path.is_file() {
        anyhow::bail!("local artifact {} is missing", jar_path.display());
    }
    let dest = extract_root.join("local-jars").join(&fingerprint);

    if let Some(cached) = symcache_load(&fingerprint, &dest) {
        return Ok(cached);
    }

    // A previous interrupted extraction can leave a directory without a complete cache.
    let mut extracted: std::collections::BTreeMap<_, _> =
        jar::extract_class_stub_facts(jar_path, &dest)?
            .into_iter()
            .collect();

    let mut symbols = parse_dir(&dest, kotlin, java)?;
    for batch in &mut symbols {
        if let Some(owners) = extracted.remove(Path::new(&batch.file)).flatten() {
            for owner in batch
                .symbols
                .iter_mut()
                .filter(|symbol| symbol.kind.is_type_like())
            {
                let name = match owner.container.as_deref() {
                    Some(parent) => format!("{parent}.{}", owner.name),
                    None => owner.name.clone(),
                };
                if let Some(unavailable) = owners.get(&name) {
                    owner.unavailable_members = Some(Box::new(unavailable.clone()));
                }
            }
        }
    }

    symcache_store(&fingerprint, &symbols);
    Ok(symbols)
}

/// Resolve the local classpath jar that declares `import_fqn`, using sources when available.
pub fn resolve_import_fqn_from_local_jars(
    import_fqn: &str,
    jars: Vec<PathBuf>,
    model: Option<&crate::classpath::ProjectSnapshot>,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let Some((package, name)) = import_fqn.rsplit_once('.') else {
        return Vec::new();
    };
    for jar_path in jars {
        let origin_id = model
            .and_then(|model| model.artifact_origin(&jar_path))
            .map(str::to_string)
            .unwrap_or_else(|| crate::classpath::file_origin_id(&jar_path));
        let batches = match resolve_local_jar_stubs_for_origin(
            &jar_path,
            &origin_id,
            extract_root,
            kotlin,
            java,
        ) {
            Ok(batches) => batches,
            Err(error) => {
                tracing::warn!("index {} failed: {error:#}", jar_path.display());
                continue;
            }
        };
        let matches = batches
            .into_iter()
            .filter(|batch| {
                batch.symbols.iter().any(|sym| {
                    sym.name == name && sym.package == package && sym.kind.is_type_like()
                })
            })
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

/// Resolve just the source file that declares `import_fqn`, walking source-artifact metadata from
/// the given roots. This is a request-path fallback for explicit import goto before the full
/// background dependency index has warmed up.
pub fn resolve_import_fqn_from_coordinates(
    import_fqn: &str,
    roots: Vec<Coordinate>,
    repos: &Repos,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let Some((package, name)) = import_fqn.rsplit_once('.') else {
        return Vec::new();
    };
    let mut queue = prioritized_coordinates(roots, package);
    let mut seen = BTreeSet::new();
    const MAX_IMPORT_COORDINATES: usize = 256;

    while let Some(coord) = queue.pop_front() {
        if seen.len() >= MAX_IMPORT_COORDINATES {
            break;
        }
        if !seen.insert(coord.clone()) {
            continue;
        }

        let batches = resolve_coordinate(&coord, repos, extract_root, kotlin, java);
        let mut matches = Vec::new();
        for batch in batches {
            if batch
                .symbols
                .iter()
                .any(|sym| sym.name == name && sym.package == package && sym.kind.is_type_like())
            {
                matches.push(batch);
            }
        }
        if !matches.is_empty() {
            return matches;
        }

        for dep in artifacts::dependency_coordinates(repos, &coord) {
            if !seen.contains(&dep) && !queue.iter().any(|queued| queued == &dep) {
                queue.push_back(dep);
            }
        }
    }

    Vec::new()
}

/// Resolve an explicit import from coordinates plausibly owning its package, without walking the
/// transitive dependency graph.  This is the request-path counterpart to
/// [`resolve_import_fqn_from_coordinates`]: a go-to-definition request must stay responsive
/// while the background index performs the exhaustive traversal.
pub fn resolve_explicit_import_fqn_from_coordinates(
    import_fqn: &str,
    roots: Vec<Coordinate>,
    repos: &Repos,
    model: Option<&crate::classpath::ProjectSnapshot>,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let Some((package, name)) = import_fqn.rsplit_once('.') else {
        return Vec::new();
    };
    let mut candidates = roots
        .into_iter()
        .filter(|coord| coordinate_related_to_package(&coord.group, package))
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        coordinate_package_score(&b, package)
            .cmp(&coordinate_package_score(&a, package))
            .then(a.cmp(b))
    });
    candidates.dedup();

    for coord in candidates.into_iter().take(32) {
        let batches = match coordinate_source_for_model(&coord, repos, extract_root, model) {
            Ok(Some(source)) => resolve_library_source(&source, kotlin, java),
            Ok(None) => {
                let binaries = model
                    .and_then(|model| model.coordinate_binaries(&coord))
                    .unwrap_or_default();
                resolve_module_binary_stubs(&coord, &binaries, extract_root, kotlin, java)
            }
            Err(_) if model.is_some() => {
                let binaries = model
                    .and_then(|model| model.coordinate_binaries(&coord))
                    .unwrap_or_default();
                resolve_module_binary_stubs(&coord, &binaries, extract_root, kotlin, java)
            }
            Err(error) => Err(error),
        }
        .unwrap_or_else(|error| {
            tracing::warn!("index {} failed: {error:#}", coord.label());
            Vec::new()
        });
        let matches = batches
            .into_iter()
            .filter(|batch| {
                batch.symbols.iter().any(|sym| {
                    sym.name == name && sym.package == package && sym.kind.is_type_like()
                })
            })
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

pub fn coordinate_related_to_package(group: &str, package: &str) -> bool {
    if package == group || package.starts_with(&format!("{group}.")) {
        return true;
    }
    group.split('.').zip(package.split('.')).take(2).count() == 2
        && group
            .split('.')
            .zip(package.split('.'))
            .take(2)
            .all(|(left, right)| left == right)
}

fn prioritized_coordinates(mut coords: Vec<Coordinate>, package: &str) -> VecDeque<Coordinate> {
    coords.sort_by(|a, b| {
        coordinate_package_score(b, package)
            .cmp(&coordinate_package_score(a, package))
            .then(a.cmp(b))
    });
    coords.into()
}

fn coordinate_package_score(coord: &Coordinate, package: &str) -> usize {
    if package == coord.group || package.starts_with(&format!("{}.", coord.group)) {
        coord.group.len()
    } else {
        0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum AndroidSourceLocation {
    Directory(PathBuf),
    Archive(PathBuf),
}

/// Android API level selected by the workspace's Gradle configuration.
///
/// Version catalogs are the reliable path for modern Android builds; literal `compileSdk` values
/// in Gradle files cover smaller and older projects.
pub fn android_compile_sdk(root: &Path) -> Option<u32> {
    let mut levels = BTreeSet::new();
    for path in catalog_paths(root) {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for key in ["compileSdk", "compile-sdk", "compile_sdk"] {
            if let Ok(Some(value)) = catalog::parse_version(&text, key) {
                if let Ok(level) = value.parse::<u32>() {
                    levels.insert(level);
                }
            }
        }
    }
    if let Some(level) = levels.iter().next_back().copied() {
        return Some(level);
    }

    for entry in WalkDir::new(root)
        .max_depth(8)
        .into_iter()
        .filter_entry(|entry| !is_catalog_excluded(entry))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        if !matches!(
            entry.path().file_name().and_then(|name| name.to_str()),
            Some("build.gradle" | "build.gradle.kts")
        ) {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines().filter(|line| line.contains("compileSdk")) {
            if let Some(level) = compile_sdk_literal(line) {
                levels.insert(level);
            }
        }
    }
    levels.into_iter().next_back()
}

fn compile_sdk_literal(line: &str) -> Option<u32> {
    let start = line.find("compileSdk")? + "compileSdk".len();
    let tail = line[start..]
        .strip_prefix("Version")
        .unwrap_or(&line[start..]);
    let tail = tail
        .trim_start_matches(|ch: char| ch.is_ascii_whitespace() || matches!(ch, '=' | '('))
        .trim_start_matches(['\'', '"']);
    let digits = tail
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

/// Resolve the Android source files capable of declaring the requested imported FQNs. Only the
/// matching files are extracted and parsed; the roughly 50 MB platform source archive is never
/// expanded wholesale.
pub fn resolve_android_imports(
    root: &Path,
    fqns: &[String],
    extract_root: &Path,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let suffixes = fqns
        .iter()
        .filter(|fqn| fqn.starts_with("android."))
        .flat_map(|fqn| android_source_suffixes(fqn))
        .collect::<BTreeSet<_>>();
    if suffixes.is_empty() {
        return Vec::new();
    }
    let Some(location) = android_source_location(root) else {
        return Vec::new();
    };

    resolve_android_imports_from_location(location, &suffixes, extract_root, java)
}

fn resolve_android_imports_from_location(
    location: AndroidSourceLocation,
    suffixes: &BTreeSet<String>,
    extract_root: &Path,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let mut paths = match location {
        AndroidSourceLocation::Directory(directory) => suffixes
            .iter()
            .filter_map(|suffix| {
                [directory.join(suffix), directory.join("src").join(suffix)]
                    .into_iter()
                    .find(|path| path.is_file())
            })
            .collect::<Vec<_>>(),
        AndroidSourceLocation::Archive(archive) => {
            let _guard = ANDROID_SOURCES_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(fingerprint) = jar_fingerprint(&archive) else {
                return Vec::new();
            };
            let dest = extract_root.join("android").join(&fingerprint);
            if !prepare_lazy_source_dir(&archive, &dest, &fingerprint) {
                return Vec::new();
            }
            let mut paths = Vec::new();
            let mut missing = BTreeSet::new();
            for suffix in suffixes {
                if let Some(path) = find_extracted_source(&dest, suffix) {
                    paths.push(path);
                } else {
                    missing.insert(suffix.clone());
                }
            }
            if !missing.is_empty() {
                paths.extend(extract_sources_by_suffix(&archive, &dest, &missing));
            }
            paths
        }
    };
    paths.sort();
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| parse_java_source_file(&path, java))
        .collect()
}

fn android_source_suffixes(fqn: &str) -> Vec<String> {
    let parts = fqn.split('.').collect::<Vec<_>>();
    (3..=parts.len())
        .rev()
        .map(|len| format!("{}.java", parts[..len].join("/")))
        .collect()
}

fn android_source_location(root: &Path) -> Option<AndroidSourceLocation> {
    let api = android_compile_sdk(root);
    if let Some(path) = std::env::var_os(ANDROID_SOURCES_ENV).map(PathBuf::from) {
        if let Some(location) = android_source_location_from_path(&path, api) {
            return Some(location);
        }
    }
    for sdk in android_sdk_roots(root) {
        if let Some(location) = android_source_location_from_path(&sdk, api) {
            return Some(location);
        }
    }
    if android_download_enabled() {
        api.and_then(download_android_sources)
            .map(AndroidSourceLocation::Archive)
    } else {
        None
    }
}

fn android_source_location_from_path(
    path: &Path,
    api: Option<u32>,
) -> Option<AndroidSourceLocation> {
    if path.is_file() {
        return Some(AndroidSourceLocation::Archive(path.to_path_buf()));
    }
    if !path.is_dir() {
        return None;
    }
    if path.join("android").is_dir() || path.join("src/android").is_dir() {
        return Some(AndroidSourceLocation::Directory(path.to_path_buf()));
    }
    let sources = path.join("sources");
    if let Some(api) = api {
        let exact = sources.join(format!("android-{api}"));
        if exact.is_dir() {
            return Some(AndroidSourceLocation::Directory(exact));
        }
        return None;
    }
    let mut installed = fs::read_dir(&sources)
        .ok()?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let name = entry.file_name();
            let level = name
                .to_string_lossy()
                .strip_prefix("android-")?
                .parse::<u32>()
                .ok()?;
            Some((level, entry.path()))
        })
        .collect::<Vec<_>>();
    installed.sort_by_key(|(level, _)| *level);
    installed
        .pop()
        .map(|(_, path)| AndroidSourceLocation::Directory(path))
}

fn android_sdk_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = BTreeSet::new();
    for name in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(path) = std::env::var_os(name).filter(|path| !path.is_empty()) {
            roots.insert(PathBuf::from(path));
        }
    }
    let mut property_files = vec![root.join("local.properties")];
    if let Some(gradle) = gradle_root(root) {
        property_files.push(gradle.join("local.properties"));
    }
    for path in property_files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(value) = property_value(&text, "sdk.dir") {
            roots.insert(PathBuf::from(value));
        }
    }
    if let Some(home) = crate::paths::user_home() {
        roots.insert(home.join("Library/Android/sdk"));
        roots.insert(home.join("Android/Sdk"));
    }
    roots.into_iter().filter(|path| path.is_dir()).collect()
}

fn property_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (name, value) = line.split_once('=')?;
        (name.trim() == key).then(|| value.trim().replace("\\\\", "\\"))
    })
}

fn android_download_enabled() -> bool {
    !std::env::var(ANDROID_DOWNLOAD_ENV)
        .ok()
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "0" | "false" | "no"))
}

fn download_android_sources(api: u32) -> Option<PathBuf> {
    let _guard = ANDROID_SOURCES_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if ANDROID_DOWNLOAD_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&api)
        .is_some_and(|failed| failed.elapsed() < Duration::from_secs(60))
    {
        return None;
    }
    let result = download_android_sources_locked(api);
    let mut failures = ANDROID_DOWNLOAD_FAILURES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if result.is_some() {
        failures.remove(&api);
    } else {
        failures.insert(api, Instant::now());
    }
    result
}

fn download_android_sources_locked(api: u32) -> Option<PathBuf> {
    let base = std::env::var(ANDROID_REPOSITORY_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| ANDROID_REPOSITORY_BASE.to_string());
    let directory = cache_home().join("android").join(format!("android-{api}"));
    if let Some(existing) = fs::read_dir(&directory)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .find(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("zip") && valid_zip(path)
        })
    {
        return Some(existing);
    }

    let repository_url = format!("{}/repository2-1.xml", base.trim_end_matches('/'));
    let xml = http_get_text(&repository_url, Duration::from_secs(15))?;
    let archive_name = android_source_archive_name(&xml, api)?;
    let safe_name = Path::new(&archive_name).file_name()?.to_string_lossy();
    let destination = directory.join(safe_name.as_ref());
    if destination.is_file() && valid_zip(&destination) {
        return Some(destination);
    }
    let url = format!(
        "{}/{}",
        base.trim_end_matches('/'),
        archive_name.trim_start_matches('/')
    );
    if let Err(error) = http_download_with_timeout(&url, &destination, Duration::from_secs(120)) {
        tracing::debug!("failed to download Android {api} sources: {error}");
        return None;
    }
    valid_zip(&destination).then_some(destination)
}

fn android_source_archive_name(xml: &str, api: u32) -> Option<String> {
    let package = format!(r#"<remotePackage path="sources;android-{api}">"#);
    let start = xml.find(&package)?;
    let block = &xml[start..];
    let end = block.find("</remotePackage>")?;
    let block = &block[..end];
    let url_start = block.find("<url>")? + "<url>".len();
    let url_end = block[url_start..].find("</url>")? + url_start;
    Some(block[url_start..url_end].trim().to_string())
}

fn http_get_text(url: &str, timeout: Duration) -> Option<String> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build(),
    );
    let mut response = agent.get(url).call().ok()?;
    let mut text = String::new();
    response
        .body_mut()
        .as_reader()
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

fn http_download_with_timeout(
    url: &str,
    destination: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let agent = ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .build(),
    );
    let mut response = agent.get(url).call()?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = destination.with_extension(format!("zip.{}.part", std::process::id()));
    let mut output = fs::File::create(&temporary)?;
    io::copy(&mut response.body_mut().as_reader(), &mut output)?;
    drop(output);
    fs::rename(temporary, destination)?;
    Ok(())
}

fn valid_zip(path: &Path) -> bool {
    fs::File::open(path)
        .ok()
        .and_then(|file| zip::ZipArchive::new(file).ok())
        .is_some()
}

/// Resolve the local JDK `src.zip` into indexed Java symbols. JDK sources are not Maven
/// dependencies, but imported JDK types (`java.sql.Connection`, `java.time.Instant`, …) need the
/// same durable index path as dependency source jars.
///
/// Pre-work for JDK source indexing, split out so the LSP server can shard parsing across its
/// dependency worker pool: fingerprint, lazy-dir marker, symcache probe, and (on a miss) the zip
/// bytes read once and shared across shard workers.
pub enum JdkIndexPlan {
    /// Symcache hit: no parsing needed.
    Cached(Vec<FileSymbols>),
    /// Symcache miss: parse via [`parse_jdk_shard`], then persist via [`store_jdk_symcache`].
    Parse(JdkParsePlan),
}

#[derive(Clone)]
pub struct JdkParsePlan {
    fingerprint: String,
    dest: PathBuf,
    src_zip: PathBuf,
    bytes: std::sync::Arc<[u8]>,
}

pub fn plan_jdk_index(src_zip: &Path, extract_root: &Path) -> Option<JdkIndexPlan> {
    if !src_zip.is_file() {
        return None;
    }
    let fingerprint = jar_fingerprint(src_zip)?;
    let dest = extract_root.join("jdk").join(&fingerprint);

    // The lazy-dir marker write is not race-safe; it must happen once, before shards spawn.
    if !prepare_lazy_source_dir(src_zip, &dest, &fingerprint) {
        tracing::warn!(
            "prepare lazy JDK source directory {} failed",
            dest.display()
        );
        return None;
    }

    if let Some(cached) = symcache_load(&fingerprint, &dest) {
        let mut cached = cached;
        tag_batches(
            &mut cached,
            &crate::classpath::jdk_origin_id(src_zip),
            SourceRepresentation::Source,
        );
        return Some(JdkIndexPlan::Cached(
            LibrarySource::jdk_archive(src_zip, &dest, &fingerprint).stamp_batches(cached),
        ));
    }

    let bytes = fs::read(src_zip).ok()?;
    Some(JdkIndexPlan::Parse(JdkParsePlan {
        fingerprint,
        dest,
        src_zip: src_zip.to_path_buf(),
        bytes: std::sync::Arc::from(bytes),
    }))
}

/// Parse the `index % shards == shard` slice of the JDK source zip. Interleaving evens out entry
/// sizes across shards better than contiguous ranges.
pub fn parse_jdk_shard(
    plan: &JdkParsePlan,
    shard: usize,
    shards: usize,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> anyhow::Result<Vec<FileSymbols>> {
    let mut out = Vec::new();
    jar::visit_sources_shard(
        plan.bytes.clone(),
        &plan.src_zip,
        &plan.dest,
        shard,
        shards,
        |path, text| {
            if let Some(file) = parse_source_file(path, text, kotlin, java) {
                out.push(file);
            }
        },
    )?;
    tag_batches(
        &mut out,
        &crate::classpath::jdk_origin_id(&plan.src_zip),
        SourceRepresentation::Source,
    );
    Ok(LibrarySource::jdk_archive(&plan.src_zip, &plan.dest, &plan.fingerprint).stamp_batches(out))
}

/// Persist the merged shard results. Call only when every shard succeeded, so a partial parse
/// never poisons the cache.
pub fn store_jdk_symcache(plan: &JdkParsePlan, batches: &[FileSymbols]) {
    symcache_store(&plan.fingerprint, batches);
}

/// Single-threaded JDK source resolution (CLI one-shots, tests): plan, parse, store in one call.
pub fn resolve_jdk_sources(
    src_zip: &Path,
    extract_root: &Path,
    kotlin: &mut KotlinParser,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    match plan_jdk_index(src_zip, extract_root) {
        Some(JdkIndexPlan::Cached(batches)) => batches,
        Some(JdkIndexPlan::Parse(plan)) => match parse_jdk_shard(&plan, 0, 1, kotlin, java) {
            Ok(batches) => {
                store_jdk_symcache(&plan, &batches);
                batches
            }
            Err(error) => {
                tracing::warn!("parse {} failed: {error}", src_zip.display());
                Vec::new()
            }
        },
        None => Vec::new(),
    }
}

/// Resolve a small set of explicitly imported JDK classes before the full JDK index is ready.
///
/// This keeps cold editor requests responsive for common Java shapes such as
/// `import java.util.concurrent.ExecutorService; ex.awaitTermination(...)`: the target file is
/// extracted from `src.zip`, parsed, and inserted into the durable index while the broad background
/// dependency index continues.
pub fn resolve_jdk_imports(
    fqns: &[String],
    extract_root: &Path,
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    let Some(src_zip) = jdk_src_zip() else {
        return Vec::new();
    };
    resolve_jdk_imports_from_zip(&src_zip, extract_root, fqns, java)
}

pub(crate) fn resolve_jdk_imports_from_zip(
    src_zip: &Path,
    extract_root: &Path,
    fqns: &[String],
    java: &mut JavaParser,
) -> Vec<FileSymbols> {
    if !src_zip.is_file() {
        return Vec::new();
    }
    let Some(fingerprint) = jar_fingerprint(src_zip) else {
        return Vec::new();
    };
    let dest = extract_root.join("jdk").join(&fingerprint);
    if !prepare_lazy_source_dir(src_zip, &dest, &fingerprint) {
        return Vec::new();
    }
    let suffixes = fqns
        .iter()
        .filter(|fqn| fqn.starts_with("java.") || fqn.starts_with("javax."))
        .map(|fqn| format!("{}.java", fqn.replace('.', "/")))
        .collect::<BTreeSet<_>>();
    if suffixes.is_empty() {
        return Vec::new();
    }

    let start = Instant::now();
    let Ok(file) = fs::File::open(src_zip) else {
        return Vec::new();
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return Vec::new();
    };
    let mut by_basename = BTreeMap::<&str, Vec<&String>>::new();
    for suffix in &suffixes {
        by_basename
            .entry(suffix.rsplit('/').next().unwrap_or(suffix.as_str()))
            .or_default()
            .push(suffix);
    }
    // Scan central-directory names once; local/unknown type names must not repeat directory walks.
    let mut matches = Vec::new();
    for name in archive.file_names() {
        let normalized = name.replace('\\', "/");
        let basename = normalized.rsplit('/').next().unwrap_or(&normalized);
        if let Some(candidates) = by_basename.get(basename) {
            for suffix in candidates {
                if normalized.ends_with(suffix.as_str()) {
                    matches.push((name.to_string(), (*suffix).clone()));
                }
            }
        }
    }
    let mut paths = Vec::new();
    let mut remaining = suffixes.clone();
    let mut extracted = 0;
    for (name, suffix) in matches {
        if !remaining.contains(&suffix) {
            continue;
        }
        let Ok(mut entry) = archive.by_name(&name) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let path = dest.join(enclosed);
        if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_file()) {
            remaining.remove(&suffix);
            paths.push(path);
            continue;
        }
        let read =
            jar::read_source_text(&mut entry, 8192, Path::new(&name), JavaSourceEncoding::Utf8);
        match read.and_then(|text| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            jar::write_source_text(&path, &text)
        }) {
            Ok(()) => {
                remaining.remove(&suffix);
                paths.push(path);
                extracted += 1;
            }
            Err(error) => tracing::warn!("extract source seed {name} failed: {error:#}"),
        }
    }
    crate::trace::span(
        "deps.source_seed_extract",
        "deps",
        start,
        serde_json::json!({
            "requested": suffixes.len(), "extracted": extracted, "missing": remaining.len(),
        }),
    );
    paths.sort();
    paths.dedup();
    let mut batches: Vec<_> = paths
        .into_iter()
        .filter_map(|path| parse_java_source_file(&path, java))
        .collect();
    tag_batches(
        &mut batches,
        &crate::classpath::jdk_origin_id(src_zip),
        SourceRepresentation::Source,
    );
    LibrarySource::jdk_archive(src_zip, &dest, &fingerprint).stamp_batches(batches)
}

fn find_extracted_source(dest: &Path, suffix: &str) -> Option<PathBuf> {
    if !dest.is_dir() {
        return None;
    }
    let normalized_suffix = suffix.replace('\\', "/");
    let mut matches = WalkDir::new(dest)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.to_string_lossy()
                .replace('\\', "/")
                .ends_with(&normalized_suffix)
        })
        .collect::<Vec<_>>();
    matches.sort();
    matches.into_iter().next()
}

fn extract_sources_by_suffix(
    src_zip: &Path,
    dest: &Path,
    suffixes: &BTreeSet<String>,
) -> Vec<PathBuf> {
    let start = std::time::Instant::now();
    let mut out = Vec::new();
    let Ok(file) = fs::File::open(src_zip) else {
        return out;
    };
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return out;
    };
    let mut remaining = suffixes.clone();
    for i in 0..archive.len() {
        if remaining.is_empty() {
            break;
        }
        let Ok(mut entry) = archive.by_index(i) else {
            continue;
        };
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().replace('\\', "/");
        let Some(suffix) = remaining
            .iter()
            .find(|suffix| name.ends_with(*suffix))
            .cloned()
        else {
            continue;
        };
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let path = dest.join(enclosed);
        let read =
            jar::read_source_text(&mut entry, 8192, Path::new(&name), JavaSourceEncoding::Utf8);
        match read.and_then(|text| {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            jar::write_source_text(&path, &text)
        }) {
            Ok(()) => {
                remaining.remove(&suffix);
                out.push(path);
            }
            Err(error) => tracing::warn!("extract source seed {name} failed: {error:#}"),
        }
    }
    crate::trace::span(
        "deps.source_seed_extract",
        "deps",
        start,
        serde_json::json!({
            "requested": suffixes.len(),
            "extracted": out.len(),
            "missing": remaining.len(),
        }),
    );
    out
}

fn parse_java_source_file(path: &Path, java: &mut JavaParser) -> Option<FileSymbols> {
    let start = std::time::Instant::now();
    let text = fs::read_to_string(path).ok()?;
    let tree = java.parse(&text);
    let facts = language::symbol_facts(SourceLanguage::Java, &tree, &text);
    crate::trace::span(
        "deps.source_seed_parse",
        "deps",
        start,
        serde_json::json!({
            "file": path.file_name().and_then(|name| name.to_str()),
            "path": path.to_string_lossy(),
            "symbols": facts.symbols.len(),
        }),
    );
    (!facts.symbols.is_empty()).then(|| FileSymbols {
        jvm: facts.jvm,
        file: crate::paths::file_key(path),
        symbols: facts.symbols,
        library_source: None,
        origin: None,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn pending_gradle_source_query_keeps_exact_binary_import_facts_available() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp-pending-source-import-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("api-1.jar");
        let mut zip = zip::ZipWriter::new(fs::File::create(&binary).unwrap());
        zip.start_file("fieldfixture/FieldOnly.class", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(include_bytes!(
            "../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class"
        ))
        .unwrap();
        zip.finish().unwrap();
        let coord = Coordinate::parse("fieldfixture:api:1").unwrap();
        let repos = Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        for state in ["", "SOURCEQUERY\tmain\tfailed\n"] {
            let model = crate::classpath::ProjectSnapshot::new(crate::classpath::parse_dump(
                &format!(
                    "PROJECT\t:app\t{}\nSOURCESET\tmain\nSCP\tmain\t{}\nARTIFACT\tmain\t{}\tMODULE\tfieldfixture\tapi\t1\n{state}END\t:app\n",
                    root.display(),
                    binary.display(),
                    binary.display()
                ),
            ));
            assert_eq!(model.coordinate_binaries(&coord).unwrap().len(), 1);
            let batches = resolve_explicit_import_fqn_from_coordinates(
                "fieldfixture.FieldOnly",
                vec![coord.clone()],
                &repos,
                Some(&model),
                &root.join("extract"),
                &mut KotlinParser::new(),
                &mut JavaParser::new(),
            );
            assert!(batches
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| symbol.name == "count"));
            assert!(batches
                .iter()
                .all(|batch| batch.origin.as_ref().unwrap().representation
                    == SourceRepresentation::Stub));
            assert!(
                !matches!(model.coordinate_sources(&coord), Ok(Some(_))),
                "binary facts do not invent source availability"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn gradle_selected_pom_ignores_foreign_cached_encoding() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_selected_pom_encoding_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let coord = Coordinate::parse("example:encoding:1").unwrap();
        let repos = Repos {
            gradle_cache: root.join("gradle/caches"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let selected = root.join("selected/encoding-1-sources.jar");
        let selected_pom = root.join("selected/encoding-1.pom");
        let binary = root.join("encoding-1.jar");
        write_sources_jar(
            &selected,
            &[(
                "example/Policy.java",
                "package example; public class Policy { public int café() { return 1; } }",
            )],
        );
        write_sources_jar(&binary, &[]);
        let pom = |encoding: &str| {
            format!(
                "<project><groupId>example</groupId><artifactId>encoding</artifactId><version>1</version><properties><project.build.sourceEncoding>{encoding}</project.build.sourceEncoding></properties></project>"
            )
        };
        fs::write(&selected_pom, pom("UTF-8")).unwrap();
        let foreign = repos.m2.join("example/encoding/1/encoding-1.pom");
        fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        fs::write(&foreign, pom("ISO-8859-1")).unwrap();
        let snapshot = |parent_record: &str| {
            crate::classpath::ProjectSnapshot::new(crate::classpath::parse_dump(&format!(
                "PROJECT\t:app\t{}\nSOURCESET\tmain\nARTIFACT\tmain\t{}\tMODULE\texample\tencoding\t1\nSOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t{}\tMODULE\texample\tencoding\t1\nPOMQUERY\tmain\tcomplete\nPOMARTIFACT\tmain\t{}\tMODULE\texample\tencoding\t1\n{parent_record}END\t:app\n",
                root.display(),
                binary.display(),
                selected.display(),
                selected_pom.display()
            )))
        };
        let model = snapshot("");
        let source =
            coordinate_source_for_model(&coord, &repos, &root.join("extract"), Some(&model))
                .unwrap()
                .unwrap();
        let batches =
            resolve_library_source(&source, &mut KotlinParser::new(), &mut JavaParser::new())
                .unwrap();
        let names = batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .map(|symbol| symbol.name.as_str())
            .collect::<Vec<_>>();
        assert!(
            names.contains(&"café"),
            "selected UTF-8 member missing: {names:?}"
        );
        // A foreign cache edit cannot change either decoded names or the selected cache identity.
        fs::write(&foreign, "malformed foreign metadata").unwrap();
        let warm = coordinate_source_for_model(&coord, &repos, &root.join("extract"), Some(&model))
            .unwrap()
            .unwrap();
        assert_eq!(source.identity(), warm.identity());
        let warm_batches =
            resolve_library_source(&warm, &mut KotlinParser::new(), &mut JavaParser::new())
                .unwrap();
        assert!(warm_batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "café"));
        // A selected metadata edit retires both an issued source and the old project snapshot.
        fs::write(&selected_pom, pom("ISO-8859-1")).unwrap();
        assert!(
            resolve_library_source(&source, &mut KotlinParser::new(), &mut JavaParser::new())
                .is_err()
        );
        let stale =
            coordinate_source_for_model(&coord, &repos, &root.join("extract"), Some(&model))
                .unwrap()
                .unwrap();
        assert!(
            resolve_library_source(&stale, &mut KotlinParser::new(), &mut JavaParser::new())
                .is_err()
        );
        // A parent contributes encoding only when Gradle also certifies that exact artifact.
        fs::write(&selected_pom, "<project><groupId>example</groupId><artifactId>encoding</artifactId><version>1</version><parent><groupId>example</groupId><artifactId>parent</artifactId><version>1</version></parent></project>").unwrap();
        let parent = root.join("selected/parent-1.pom");
        fs::write(&parent, "<project><groupId>example</groupId><artifactId>parent</artifactId><version>1</version><properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties></project>").unwrap();
        let mut archive = zip::ZipWriter::new(fs::File::create(&selected).unwrap());
        archive
            .start_file(
                "example/Policy.java",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive
            .write_all(
                b"package example; public class Policy { public int caf\xe9() { return 1; } }",
            )
            .unwrap();
        archive.finish().unwrap();
        let uncertified =
            coordinate_source_for_model(&coord, &repos, &root.join("extract"), Some(&snapshot("")))
                .unwrap()
                .unwrap();
        assert!(resolve_library_source(
            &uncertified,
            &mut KotlinParser::new(),
            &mut JavaParser::new()
        )
        .is_err());
        let parent_record = format!(
            "POMARTIFACT\tmain\t{}\tMODULE\texample\tparent\t1\n",
            parent.display()
        );
        let certified = snapshot(&parent_record);
        let inherited =
            coordinate_source_for_model(&coord, &repos, &root.join("extract"), Some(&certified))
                .unwrap()
                .unwrap();
        let inherited_batches =
            resolve_library_source(&inherited, &mut KotlinParser::new(), &mut JavaParser::new())
                .unwrap();
        assert!(inherited_batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "café"));
        fs::write(&parent, "changed selected parent metadata").unwrap();
        assert!(resolve_library_source(
            &inherited,
            &mut KotlinParser::new(),
            &mut JavaParser::new()
        )
        .is_err());
        let refreshed = snapshot(&parent_record);
        let origin = crate::classpath::maven_origin_id(&coord);
        assert_ne!(
            certified.artifact_revisions()[&origin],
            refreshed.artifact_revisions()[&origin],
            "parent edits must invalidate child source facts"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn gradle_selected_sources_ignore_competing_cache_and_preserve_failure_or_absence() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_selected_gradle_sources_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let coord = Coordinate::parse("example:api:1").unwrap();
        let repos = Repos {
            gradle_cache: root.join("gradle/caches"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let wrong = repos
            .gradle_cache
            .join("modules-2/files-2.1/example/api/1/wrong/api-1-sources.jar");
        let selected = root.join("settings-repository/api-1-sources.jar");
        let selected_pom = root.join("settings-repository/api-1.pom");
        let binary = root.join("api-1.jar");
        write_sources_jar(
            &wrong,
            &[(
                "example/api/Policy.java",
                "package example.api; public class Policy { public int wrongRepository() { return 1; } }",
            )],
        );
        write_sources_jar(
            &selected,
            &[(
                "example/api/Policy.java",
                "package example.api; public class Policy { public int chosenRepository() { return 2; } }",
            )],
        );
        fs::write(&selected_pom, "<project><groupId>example</groupId><artifactId>api</artifactId><version>1</version></project>").unwrap();
        write_sources_jar(&binary, &[]);
        let prefix = format!(
            "PROJECT\t:app\t{}\nSOURCESET\tmain\nARTIFACT\tmain\t{}\tMODULE\texample\tapi\t1\n",
            root.display(),
            binary.display()
        );
        let snapshot = |records: &str| {
            crate::classpath::ProjectSnapshot::new(crate::classpath::parse_dump(&format!(
                "{prefix}{records}END\t:app\n"
            )))
        };
        let records = format!(
            "SOURCEQUERY\tmain\tcomplete\nSOURCEARTIFACT\tmain\t{}\tMODULE\texample\tapi\t1\nPOMQUERY\tmain\tcomplete\nPOMARTIFACT\tmain\t{}\tMODULE\texample\tapi\t1\n",
            selected.display(),
            selected_pom.display()
        );
        let model = snapshot(&records);
        let extract = root.join("extract");
        assert_eq!(
            coordinate_source_checked(&coord, &repos, &extract)
                .unwrap()
                .unwrap()
                .jar,
            Some(wrong)
        );
        let source = coordinate_source_for_model(&coord, &repos, &extract, Some(&model))
            .unwrap()
            .unwrap();
        assert_eq!(source.jar, Some(selected.clone()));
        let warm = coordinate_source_for_model(&coord, &repos, &extract, Some(&model))
            .unwrap()
            .unwrap();
        assert_eq!(source.identity(), warm.identity());
        let batches =
            resolve_library_source(&source, &mut KotlinParser::new(), &mut JavaParser::new())
                .unwrap();
        let names: Vec<_> = batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .map(|symbol| symbol.name.as_str())
            .collect();
        assert!(names.contains(&"chosenRepository"));
        assert!(!names.contains(&"wrongRepository"));
        let absent = snapshot("SOURCEQUERY\tmain\tcomplete\n");
        assert!(
            coordinate_source_for_model(&coord, &repos, &extract, Some(&absent))
                .unwrap()
                .is_none()
        );
        for unavailable in ["", "SOURCEQUERY\tmain\tfailed\n"] {
            assert!(coordinate_source_for_model(
                &coord,
                &repos,
                &extract,
                Some(&snapshot(unavailable))
            )
            .is_err());
        }
        write_sources_jar(
            &selected,
            &[(
                "example/api/Policy.java",
                "package example.api; public class Policy { public long replacementRepository() { return 3; } }",
            )],
        );
        assert!(coordinate_source_for_model(&coord, &repos, &extract, Some(&model)).is_err());
        let changed = snapshot(&records);
        let replacement = coordinate_source_for_model(&coord, &repos, &extract, Some(&changed))
            .unwrap()
            .unwrap();
        assert_ne!(source.identity(), replacement.identity());
        let batches = resolve_library_source(
            &replacement,
            &mut KotlinParser::new(),
            &mut JavaParser::new(),
        )
        .unwrap();
        let names: Vec<_> = batches
            .iter()
            .flat_map(|batch| &batch.symbols)
            .map(|symbol| symbol.name.as_str())
            .collect();
        assert!(names.contains(&"replacementRepository"));
        assert!(!names.contains(&"chosenRepository"));
        fs::remove_dir_all(root).unwrap();
    }

    use super::*;
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    fn coord(s: &str) -> Coordinate {
        Coordinate::parse(s).unwrap()
    }

    fn write_sources_jar(path: &Path, entries: &[(&str, &str)]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        for (name, body) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn snapshot_same_second_same_length_update_misses_cache_and_invalidates_materialization() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_snapshot_nanos_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api-sources.jar");
        let first = "package api; public class Only { public void first() {} }";
        let second = "package api; public class Only { public void other() {} }";
        let mut source = LibrarySource {
            dest: tmp.join("extracted"),
            jar: Some(jar.clone()),
            fingerprint: None,
            origin_id: "maven:fixture:snapshot:1-SNAPSHOT".into(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let mut previous_fingerprint = None;
        let mut previous_length = None;
        for (nanos, text, member, old_member) in [
            (100_000_000, first, "first", "other"),
            (200_000_000, second, "other", "first"),
        ] {
            write_sources_jar(&jar, &[("api/Only.java", text)]);
            let modified = std::time::UNIX_EPOCH + Duration::new(1_700_000_000, nanos);
            fs::File::options()
                .write(true)
                .open(&jar)
                .unwrap()
                .set_times(fs::FileTimes::new().set_modified(modified))
                .unwrap();
            let metadata = fs::metadata(&jar).unwrap();
            assert_eq!(
                metadata.modified().unwrap(),
                modified,
                "filesystem must preserve the controlled timestamp"
            );
            if let Some(length) = previous_length {
                assert_eq!(
                    metadata.len(),
                    length,
                    "the archive update must keep its byte length"
                );
            }
            previous_length = Some(metadata.len());
            source.fingerprint = jar_fingerprint(&jar);
            assert!(source.fingerprint.is_some());
            assert_ne!(
                source.fingerprint, previous_fingerprint,
                "distinct subsecond mtimes must have distinct cache identities"
            );
            previous_fingerprint = source.fingerprint.clone();
            for attempt in 0..2 {
                let batches = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
                assert_eq!(batches.len(), 1);
                let path = Path::new(&batches[0].file);
                if attempt == 0 {
                    assert!(
                        !path.exists(),
                        "a changed marker must evict the previous materialized text"
                    );
                }
                assert!(batches[0]
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == member));
                assert!(!batches[0]
                    .symbols
                    .iter()
                    .any(|symbol| symbol.name == old_member));
                assert!(materialize_source_file(path));
                assert_eq!(fs::read_to_string(path).unwrap(), text);
            }
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn snapshot_legacy_path_marker_is_migrated_before_a_warm_cache_hit() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_snapshot_legacy_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api-sources.jar");
        write_sources_jar(
            &jar,
            &[("api/Only.java", "package api; public class Only {}")],
        );
        let source = LibrarySource {
            dest: tmp.join("extracted"),
            jar: Some(jar.clone()),
            fingerprint: jar_fingerprint(&jar),
            origin_id: "maven:fixture:snapshot:1-SNAPSHOT".into(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let cold = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(cold.len(), 1);
        fs::write(
            source.dest.join(SOURCE_JAR_MARKER),
            jar.to_string_lossy().as_bytes(),
        )
        .unwrap();
        let stale = source.dest.join("api/Removed.java");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "package api; public class Removed {}").unwrap();
        let warm = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(warm.len(), 1);
        assert!(
            !stale.exists(),
            "a legacy path-only marker cannot prove that materialized files match cached facts"
        );
        assert!(materialize_source_file(Path::new(&warm[0].file)));
        assert_eq!(
            fs::read_to_string(&warm[0].file).unwrap(),
            "package api; public class Only {}"
        );
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn snapshot_missing_or_replaced_archive_rejects_a_captured_warm_cache_identity() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_snapshot_missing_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api-sources.jar");
        write_sources_jar(
            &jar,
            &[("api/Only.java", "package api; public class Only {}")],
        );
        let mut source = LibrarySource {
            dest: tmp.join("extracted"),
            jar: Some(jar.clone()),
            fingerprint: jar_fingerprint(&jar),
            origin_id: "maven:fixture:snapshot:1-SNAPSHOT".into(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let cold = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(cold.len(), 1);
        fs::remove_file(&jar).unwrap();
        assert!(
            resolve_library_source(&source, &mut kotlin, &mut java).is_err(),
            "a warm symbol cache cannot substitute for a missing selected archive"
        );
        assert!(!materialize_source_file(Path::new(&cold[0].file)));
        write_sources_jar(
            &jar,
            &[(
                "api/Only.java",
                "package api; public class Only { public void restored() {} }",
            )],
        );
        assert!(
            resolve_library_source(&source, &mut kotlin, &mut java).is_err(),
            "a captured identity cannot admit a replacement archive"
        );
        source.fingerprint = jar_fingerprint(&jar);
        let recovered = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert!(recovered[0]
            .symbols
            .iter()
            .any(|symbol| symbol.name == "restored"));
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn coordinate_selector_keeps_newest_module_version() {
        let mut selector = CoordinateSelector::new();
        let older = coord("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2");
        let newest = coord("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.11.0");
        let lower = coord("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0");
        let other = coord("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.10.2");

        assert_eq!(
            selector.consider(older.clone()),
            CoordinateDecision::Selected
        );
        assert_eq!(
            selector.consider(newest.clone()),
            CoordinateDecision::Replaces(older)
        );
        assert_eq!(
            selector.consider(lower),
            CoordinateDecision::ShadowedBy(newest)
        );
        assert_eq!(selector.consider(other), CoordinateDecision::Selected);
    }

    #[test]
    fn cache_home_prefers_env_root() {
        assert_eq!(
            cache_home_from(
                Some(OsString::from("/tmp/ktlsp-run/cache")),
                Some(OsString::from("/home/me")),
            ),
            PathBuf::from("/tmp/ktlsp-run/cache")
        );
    }

    #[test]
    fn cache_home_ignores_empty_env_root() {
        assert_eq!(
            cache_home_from(Some(OsString::from("")), Some(OsString::from("/home/me"))),
            PathBuf::from("/home/me/.cache/ktlsp")
        );
    }

    #[test]
    fn injects_stdlib_when_absent_in_a_gradle_project() {
        let mut coords = vec![coord("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.9.0")];
        inject_stdlib(&mut coords, true);
        assert!(coords.iter().any(|c| c.artifact == "kotlin-stdlib"
            && c.group == "org.jetbrains.kotlin"
            && c.version == DEFAULT_KOTLIN_VERSION));
    }

    #[test]
    fn derives_stdlib_version_from_a_kotlin_coordinate() {
        let mut coords = vec![coord("org.jetbrains.kotlin:kotlin-reflect:2.0.21")];
        inject_stdlib(&mut coords, true);
        let stdlib = coords
            .iter()
            .find(|c| c.artifact == "kotlin-stdlib")
            .unwrap();
        assert_eq!(
            stdlib.version, "2.0.21",
            "version derived from the kotlin-reflect coordinate"
        );
    }

    #[test]
    fn does_not_duplicate_existing_stdlib() {
        let mut coords = vec![coord("org.jetbrains.kotlin:kotlin-stdlib:2.1.20")];
        inject_stdlib(&mut coords, true);
        assert_eq!(
            coords
                .iter()
                .filter(|c| c.artifact == "kotlin-stdlib")
                .count(),
            1
        );
    }

    fn assert_jdk_seed_type_reference_goto(
        case: &str,
        source: &str,
        anchor: &str,
        expected: crate::symbol::Def,
    ) {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_jdk_seed_goto_{case}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src_zip = tmp.join("src.zip");
        write_sources_jar(
            &src_zip,
            &[(
                "java.base/java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> { java.util.Iterator<T> iterator(); }",
            )],
        );
        let mut ws = crate::workspace::Workspace::new();
        let file = "seed/Use.java";
        let custom = "package custom; public interface Iterable<T> {}";
        ws.open("custom/Iterable.java", custom.to_string());
        ws.open(file, source.to_string());
        let offset = source.find(anchor).unwrap();
        let before = ws.goto_definition(file, offset);
        let mut java = JavaParser::new();
        let tree = java.parse(source);
        let fqns = crate::java::referenced_java_lang_fqns(&tree, source);
        assert!(fqns.iter().any(|fqn| fqn == "java.lang.Iterable"));
        let batches =
            resolve_jdk_imports_from_zip(&src_zip, &tmp.join("extract"), &fqns, &mut java);
        assert_eq!(batches.len(), 1, "only the real JDK source is seeded");
        assert!(batches[0]
            .origin
            .as_ref()
            .unwrap()
            .origin_id
            .starts_with("jdk:"));
        for batch in batches {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.bump_index_revision();
        let after = ws.goto_definition(file, offset);
        eprintln!("{case}: before seed {before:?}; after seed {after:?}");
        let type_defs = ws.type_definition(file, offset);
        eprintln!("{case}: seeded type definitions {type_defs:?}");
        // A parameter type-definition positive is separate; no foreign nominal target is valid.
        assert!(
            type_defs.iter().all(|definition| definition == &expected),
            "{case}: type definition captured a shadowed default type: {type_defs:?}"
        );
        assert_eq!(
            after,
            vec![expected],
            "{case}: seeded default type must not capture a shadowed name"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn jdk_seed_goto_preserves_same_file_type_identity() {
        let source = "package seed; class Iterable<T> {} class Use { Iterable<String> value; }";
        let start = source.find("class Iterable").unwrap() + "class ".len();
        assert_jdk_seed_type_reference_goto(
            "same_file",
            source,
            "Iterable<String>",
            crate::symbol::Def {
                file: "seed/Use.java".into(),
                start_byte: start,
                end_byte: start + "Iterable".len(),
            },
        );
    }

    #[test]
    fn jdk_seed_goto_preserves_explicit_import_identity() {
        let source = "package seed; import custom.Iterable; class Use { Iterable<String> value; }";
        let custom = "package custom; public interface Iterable<T> {}";
        let start = custom.find("interface Iterable").unwrap() + "interface ".len();
        assert_jdk_seed_type_reference_goto(
            "explicit_import",
            source,
            "Iterable<String>",
            crate::symbol::Def {
                file: "custom/Iterable.java".into(),
                start_byte: start,
                end_byte: start + "Iterable".len(),
            },
        );
    }

    #[test]
    fn jdk_seed_goto_preserves_lexical_type_parameter_identity() {
        let source = "package seed; class Use<Iterable> { Iterable value; }";
        let start = source.find("<Iterable>").unwrap() + 1;
        assert_jdk_seed_type_reference_goto(
            "type_parameter",
            source,
            "Iterable value",
            crate::symbol::Def {
                file: "seed/Use.java".into(),
                start_byte: start,
                end_byte: start + "Iterable".len(),
            },
        );
    }

    #[test]
    fn jdk_seed_goto_preserves_nearest_member_type_identity() {
        for (case, source, anchor, declaration, prefix) in [
            (
                "member_nested",
                "package seed; class Outer { class Iterable<T> {} void run(Iterable<String> value) {} } class Iterable<T> {}",
                "Iterable<String>",
                "class Iterable",
                "class ",
            ),
            (
                "nearest_member",
                "package seed; class Outer { class Iterable<T> {} class Inner { class Iterable<T> { int nearest; } void run(Iterable<String> value) {} } } class Iterable<T> {}",
                "Iterable<String>",
                "class Iterable<T> { int nearest; }",
                "class ",
            ),
            (
                "outer_interface_member",
                "package seed; interface Outer { interface Iterable<T> {} class Inner { void run(Iterable<String> value) {} } } class Iterable<T> {}",
                "Iterable<String>",
                "interface Iterable",
                "interface ",
            ),
            (
                "enum_member",
                "package seed; enum Outer { VALUE; class Iterable<T> {} void run(Iterable<String> value) {} } class Iterable<T> {}",
                "Iterable<String>",
                "class Iterable",
                "class ",
            ),
            (
                "nearer_method_parameter",
                "package seed; class Outer { class Iterable<T> {} <Iterable> void run(Iterable value) {} } class Iterable<T> {}",
                "Iterable value",
                "<Iterable>",
                "<",
            ),
        ] {
            let start = source.find(declaration).unwrap() + prefix.len();
            let expected = crate::symbol::Def {
                file: "seed/Use.java".into(),
                start_byte: start,
                end_byte: start + "Iterable".len(),
            };
            assert_jdk_seed_type_reference_goto(case, source, anchor, expected.clone());
            if case != "nearer_method_parameter" {
                let mut ws = crate::workspace::Workspace::new();
                ws.open("seed/Use.java", source.to_string());
                assert_eq!(
                    ws.type_definition("seed/Use.java", source.find(anchor).unwrap()),
                    vec![expected],
                    "{case}: lexical member type definition"
                );
            }
        }
        // Duplicate current member declarations are ambiguous, never a first-name or JDK fallback.
        let source = "package seed; class Outer { class Iterable {} class Iterable {} void run(Iterable value) {} } class Iterable {}";
        let mut ws = crate::workspace::Workspace::new();
        ws.open(
            "java/lang/Iterable.java",
            "package java.lang; public interface Iterable<T> {}".to_string(),
        );
        ws.open("seed/Use.java", source.to_string());
        let offset = source.find("Iterable value").unwrap();
        let definitions = ws.goto_definition("seed/Use.java", offset);
        let type_definitions = ws.type_definition("seed/Use.java", offset);
        eprintln!("duplicate members: goto {definitions:?}; type definitions {type_definitions:?}");
        assert!(definitions.is_empty(), "{definitions:?}");
        assert!(type_definitions.is_empty(), "{type_definitions:?}");
    }

    #[test]
    fn jdk_seed_goto_resolves_current_source_inherited_type_identity() {
        for (case, source, declaration, prefix, parameter) in [
            (
                "inherited_outer",
                "package seed;\nclass Base { class Iterable {} }\nclass Outer {\n    class Iterable {}\n    class Inner extends Base {\n        Iterable selected;\n        void run(Iterable value) {}\n    }\n}\n",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "inherited_package",
                "package seed;\nclass Base { class Iterable {} }\nclass Use extends Base {\n    Iterable selected;\n    void run(Iterable value) {}\n}\nclass Iterable {}\n",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "inherited_import",
                "package seed;\nimport custom.Iterable;\nclass Base { class Iterable {} }\nclass Use extends Base {\n    Iterable selected;\n    void run(Iterable value) {}\n}\n",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "inherited_anonymous",
                "package seed;\nclass Base { class Iterable {} }\nclass Outer {\n    class Iterable {}\n    Object field = new Base() {\n        Iterable selected;\n        void run(Iterable value) {}\n    };\n}\n",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "own_member_before_inheritance",
                "package seed; class Base { class Iterable {} } class Outer { class Iterable {} class Inner extends Base { class Iterable { int own; } void run(Iterable value) {} } }",
                "class Iterable { int own; }",
                "class ",
                false,
            ),
            (
                "own_parameter_before_inheritance",
                "package seed; class Base { class Iterable {} } class Outer { class Iterable {} class Inner<Iterable> extends Base { void run(Iterable value) {} } }",
                "<Iterable>",
                "<",
                true,
            ),
            (
                "method_parameter_before_inheritance",
                "package seed; class Base { class Iterable {} } class Outer { class Iterable {} class Inner extends Base { <Iterable> void run(Iterable value) {} } }",
                "<Iterable>",
                "<",
                true,
            ),
            (
                "inherited_before_outer_parameter",
                "package seed; class Base { class Iterable {} } class Outer<Iterable> { class Inner extends Base { void run(Iterable value) {} } }",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "known_super_without_named_type",
                "package seed; class Base {} class Outer { class Iterable {} class Inner extends Base { void run(Iterable value) {} } }",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "inherited_with_incomplete_body",
                "package seed; class Base { class Iterable {} } class Outer { class Iterable {} class Inner extends Base { void run(Iterable value) { value. } } }",
                "class Iterable",
                "class ",
                false,
            ),
            (
                "qualified_parent_over_member_homonym",
                "package seed; class Base { class Iterable {} } class Outer { class Base { class Iterable {} } class Inner extends seed.Base { void run(Iterable value) {} } }",
                "class Iterable",
                "class ",
                false,
            ),
        ] {
            let start = source.find(declaration).unwrap() + prefix.len();
            let expected = crate::symbol::Def {
                file: "seed/Use.java".into(),
                start_byte: start,
                end_byte: start + "Iterable".len(),
            };
            assert_jdk_seed_type_reference_goto(case, source, "Iterable value", expected.clone());
            let mut ws = crate::workspace::Workspace::new();
            ws.open("seed/Use.java", source.to_string());
            let definitions =
                ws.type_definition("seed/Use.java", source.find("Iterable value").unwrap());
            eprintln!("{case}: current type definitions {definitions:?}");
            if parameter {
                assert!(definitions.is_empty(), "{case}: {definitions:?}");
            } else {
                assert_eq!(
                    definitions,
                    vec![expected],
                    "{case}: exact inherited/member type definition"
                );
            }
        }
        // A known superclass without this name must preserve an ordinary imported type.
        let custom = "package custom; public interface Iterable<T> {}";
        let start = custom.find("interface Iterable").unwrap() + "interface ".len();
        let expected = crate::symbol::Def {
            file: "custom/Iterable.java".into(),
            start_byte: start,
            end_byte: start + "Iterable".len(),
        };
        for (case, source) in [
            (
                "known_super_import",
                "package seed; import custom.Iterable; class Base {} class Use extends Base { Iterable value; }",
            ),
            (
                "known_super_import_incomplete_body",
                "package seed; import custom.Iterable; class Base {} class Use extends Base { void run(Iterable value) { value. } }",
            ),
        ] {
            assert_jdk_seed_type_reference_goto(case, source, "Iterable value", expected.clone());
            let mut ws = crate::workspace::Workspace::new();
            ws.open("custom/Iterable.java", custom.to_string());
            ws.open("seed/Use.java", source.to_string());
            assert_eq!(
                ws.type_definition("seed/Use.java", source.find("Iterable value").unwrap()),
                vec![expected.clone()]
            );
        }
        // These are Unknown boundaries, not assertions that valid inherited navigation is complete.
        for (case, source) in [
            (
                "missing_ancestor",
                "package seed; class Outer { class Iterable {} class Inner extends Missing { void run(Iterable value) {} } } class Iterable {}",
            ),
            (
                "ambiguous_ancestor",
                "package seed; class Base { class Iterable {} } class Base {} class Outer { class Iterable {} class Inner extends Base { void run(Iterable value) {} } } class Iterable {}",
            ),
            (
                "ambiguous_inherited_types",
                "package seed; interface First { class Iterable {} } interface Second { class Iterable {} } class Outer { class Iterable {} class Inner implements First, Second { void run(Iterable value) {} } }",
            ),
            (
                "private_inherited_candidate",
                "package seed; class Base { private class Iterable {} } class Outer { class Iterable {} class Inner extends Base { void run(Iterable value) {} } }",
            ),
            (
                "malformed_ancestor_header",
                "package seed; class Base extends { } class Outer { class Iterable {} class Inner extends Base { void run(Iterable value) {} } } class Iterable {}",
            ),
            (
                "unindexed_anonymous_local_base",
                "package seed; class Base { class Iterable {} } class Outer { class Iterable {} void create() { class Base {} Object value = new Base() { void run(Iterable value) {} }; } }",
            ),
            (
                "ambiguous_parent_container",
                "package seed; class Base { class Iterable {} } class Outer { class Base { class Iterable {} } class Inner extends Base { void run(Iterable value) {} } }",
            ),
        ] {
            let mut ws = crate::workspace::Workspace::new();
            ws.open(
                "java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> {}".to_string(),
            );
            ws.open("seed/Use.java", source.to_string());
            let offset = source.find("Iterable value").unwrap();
            let definitions = ws.goto_definition("seed/Use.java", offset);
            let type_definitions = ws.type_definition("seed/Use.java", offset);
            eprintln!("{case}: goto {definitions:?}; type definitions {type_definitions:?}");
            assert!(definitions.is_empty(), "{case}: {definitions:?}");
            assert!(type_definitions.is_empty(), "{case}: {type_definitions:?}");
        }
    }

    #[test]
    fn jdk_seed_goto_keeps_lexical_parent_identity_before_import_expansion() {
        let base = "package custom; public class Base {}";
        let container = "package custom; public class Container { public static class Base {} }";
        let imported = "package custom; public interface Iterable<T> {}";
        for (case, source, declaration) in [
            (
                "import_shadowed_parent",
                "package seed;\nimport custom.Base;\nclass Outer {\n    class Base { class Iterable {} }\n    class Iterable {}\n    class Inner extends Base {\n        Iterable selected;\n        void run(Iterable value) {}\n    }\n}\n",
                None,
            ),
            (
                "import_shadowed_anonymous_parent",
                "package seed;\nimport custom.Base;\nclass Outer {\n    class Base { class Iterable {} }\n    class Iterable {}\n    Object field = new Base() {\n        Iterable selected;\n        void run(Iterable value) {}\n    };\n}\n",
                None,
            ),
            (
                "import_shadowed_parent_qualifier",
                "package seed; import custom.Container; class Outer { class Container { class Base { class Iterable {} } } class Iterable {} class Inner extends Container.Base { Inner(Container outer) { outer.super(); } void run(Iterable value) {} } }",
                None,
            ),
            (
                "unshadowed_import_parent",
                "package seed;\nimport custom.Base;\nclass Outer {\n    class Iterable {}\n    class Inner extends Base {\n        Iterable selected;\n        void run(Iterable value) {}\n    }\n}\n",
                Some("class Iterable"),
            ),
            (
                "qualified_import_parent",
                "package seed; import custom.Base; class Outer { class Base { class Iterable {} } class Iterable { int selected; } class Inner extends custom.Base { void run(Iterable value) {} } }",
                Some("class Iterable { int selected; }"),
            ),
            (
                "same_lexical_import_parent",
                "package seed; import seed.Outer.Base; class Outer { static class Base { class Iterable {} } class Iterable {} class Inner extends Base { void run(Iterable value) {} } }",
                Some("class Iterable"),
            ),
        ] {
            let mut ws = crate::workspace::Workspace::new();
            ws.open("custom/Base.java", base.to_string());
            ws.open("custom/Container.java", container.to_string());
            ws.open("custom/Iterable.java", imported.to_string());
            ws.open(
                "java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> {}".to_string(),
            );
            ws.open("seed/Use.java", source.to_string());
            let offset = source.find("Iterable value").unwrap();
            let definitions = ws.goto_definition("seed/Use.java", offset);
            let type_definitions = ws.type_definition("seed/Use.java", offset);
            eprintln!("{case}: goto {definitions:?}; type definitions {type_definitions:?}");
            if let Some(declaration) = declaration {
                let start = source.find(declaration).unwrap() + "class ".len();
                let expected = vec![crate::symbol::Def {
                    file: "seed/Use.java".into(),
                    start_byte: start,
                    end_byte: start + "Iterable".len(),
                }];
                assert_eq!(definitions, expected, "{case}: exact useful goto");
                assert_eq!(
                    type_definitions, expected,
                    "{case}: exact useful type definition"
                );
            } else {
                // Valid source needs a lexical-parent resolver; never select the competing outer type.
                assert!(definitions.is_empty(), "{case}: {definitions:?}");
                assert!(type_definitions.is_empty(), "{case}: {type_definitions:?}");
            }
        }
        let source = "package seed; class Use extends Outer.Middle { Use(Outer outer) { outer.super(); } Iterable selected; void run(Iterable value) {} } class Iterable {}";
        for (case, helpers, source, shadowed) in [
            (
                "foreign_import_shadowed_parent",
                "package seed;\nimport custom.Base;\nclass Outer {\n    class Base { class Iterable {} }\n    class Middle extends Base {}\n}\n",
                source,
                true,
            ),
            (
                "foreign_unshadowed_import_parent",
                "package seed;\nimport custom.Base;\nclass Outer {\n    class Middle extends Base {}\n}\n",
                source,
                false,
            ),
            (
                "foreign_import_shadowed_qualifier",
                "package seed; import custom.Container; class Outer { class Container { class Base { class Iterable {} } } class Middle extends Container.Base { Middle(Container outer) { outer.super(); } } }",
                "package seed; class Use extends Outer.Middle { Use(Outer outer, Outer.Container container) { outer.super(container); } Iterable selected; void run(Iterable value) {} } class Iterable {}",
                true,
            ),
        ] {
            let mut ws = crate::workspace::Workspace::new();
            ws.open("custom/Base.java", base.to_string());
            ws.open("custom/Container.java", container.to_string());
            ws.open("seed/Helpers.java", helpers.to_string());
            ws.open(
                "java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> {}".to_string(),
            );
            ws.open("seed/Use.java", source.to_string());
            let offset = source.find("Iterable value").unwrap();
            let definitions = ws.goto_definition("seed/Use.java", offset);
            let type_definitions = ws.type_definition("seed/Use.java", offset);
            eprintln!("{case}: goto {definitions:?}; type definitions {type_definitions:?}");
            if shadowed {
                assert!(definitions.is_empty(), "{case}: {definitions:?}");
                assert!(type_definitions.is_empty(), "{case}: {type_definitions:?}");
            } else {
                let start = source.find("class Iterable").unwrap() + "class ".len();
                let expected = vec![crate::symbol::Def {
                    file: "seed/Use.java".into(),
                    start_byte: start,
                    end_byte: start + "Iterable".len(),
                }];
                assert_eq!(definitions, expected, "{case}: preserve useful fallback");
                assert_eq!(
                    type_definitions, expected,
                    "{case}: preserve useful fallback"
                );
            }
        }
    }

    #[test]
    fn jdk_seed_goto_withholds_unproven_foreign_kotlin_nested_parent() {
        let nested_use = "package seed; class Use extends Outer.Middle { Iterable selected; void run(Iterable value) {} } class Iterable {}";
        let top_level_use = "package seed; class Use extends Middle { Iterable selected; void run(Iterable value) {} } class Iterable {}";
        for (case, helpers_key, helpers, source, unavailable) in [
            (
                "foreign_kotlin_alias_parent",
                "seed/Helpers.kt",
                "package seed\nimport custom.Base as Alias\nclass Outer {\n    open class Alias { class Iterable }\n    open class Middle : Alias()\n}\n",
                nested_use,
                true,
            ),
            (
                "foreign_kotlin_nested_without_parent",
                "seed/Helpers.kt",
                "package seed\nclass Outer { open class Middle }\n",
                nested_use,
                false,
            ),
            (
                "foreign_kotlin_top_level_parent",
                "seed/Helpers.kt",
                "package seed\nopen class Middle : custom.Base()\n",
                top_level_use,
                false,
            ),
            (
                "foreign_java_nested_parent",
                "seed/Helpers.java",
                "package seed; class Outer { static class Middle extends custom.Base {} }",
                nested_use,
                false,
            ),
        ] {
            let mut ws = crate::workspace::Workspace::new();
            ws.open(
                "custom/Base.java",
                "package custom; public class Base {}".to_string(),
            );
            ws.open(helpers_key, helpers.to_string());
            ws.open(
                "java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> {}".to_string(),
            );
            ws.open("seed/Use.java", source.to_string());
            let offset = source.find("Iterable value").unwrap();
            let definitions = ws.goto_definition("seed/Use.java", offset);
            let type_definitions = ws.type_definition("seed/Use.java", offset);
            eprintln!("{case}: goto {definitions:?}; type definitions {type_definitions:?}");
            if unavailable {
                // Expected lexical target is Outer.Alias.Iterable; independent compilation is unmeasured.
                assert!(definitions.is_empty(), "{case}: {definitions:?}");
                assert!(type_definitions.is_empty(), "{case}: {type_definitions:?}");
            } else {
                let start = source.find("class Iterable").unwrap() + "class ".len();
                let expected = vec![crate::symbol::Def {
                    file: "seed/Use.java".into(),
                    start_byte: start,
                    end_byte: start + "Iterable".len(),
                }];
                assert_eq!(definitions, expected, "{case}: preserve useful goto");
                assert_eq!(
                    type_definitions, expected,
                    "{case}: preserve useful type definition"
                );
            }
        }
    }

    #[test]
    fn resolves_explicit_jdk_import_from_modular_src_zip() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_jdk_seed_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src_zip = tmp.join("src.zip");
        write_sources_jar(
            &src_zip,
            &[(
                "java.base/java/util/concurrent/ExecutorService.java",
                r#"
package java.util.concurrent;

public interface ExecutorService {
    boolean awaitTermination(long timeout, TimeUnit unit);
    void shutdownNow();
}
"#,
            )],
        );
        let mut java = JavaParser::new();
        let batches = resolve_jdk_imports_from_zip(
            &src_zip,
            &tmp.join("extract"),
            &["java.util.concurrent.ExecutorService".to_string()],
            &mut java,
        );

        assert_eq!(batches.len(), 1);
        assert!(Path::new(&batches[0].file).is_file());
        assert!(batches[0]
            .symbols
            .iter()
            .any(|sym| sym.name == "ExecutorService"));
        assert!(batches[0]
            .symbols
            .iter()
            .any(|sym| sym.name == "awaitTermination"));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn jdk_seed_default_type_supports_foreach_and_leaves_unknown_names_absent() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_jdk_default_seed_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src_zip = tmp.join("src.zip");
        write_sources_jar(
            &src_zip,
            &[(
                "java.base/java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> { java.util.Iterator<T> iterator(); }",
            )],
        );
        let src = r#"package seed;
class Item { public void useful() {} }
public class DefaultUse {
    void run(Iterable<Item> values) {
        for (var item : values) { item.useful(); }
    }
}
"#;
        let mut java = JavaParser::new();
        let tree = java.parse(src);
        let fqns = crate::java::referenced_java_lang_fqns(&tree, src);
        assert_eq!(fqns, ["java.lang.Item", "java.lang.Iterable"]);
        let mut ws = crate::workspace::Workspace::new();
        ws.open("seed/DefaultUse.java", src.to_string());
        let offset = src.find("item.useful").unwrap() + "item.".len();
        assert!(ws.complete("seed/DefaultUse.java", offset, true).is_none());
        let batches =
            resolve_jdk_imports_from_zip(&src_zip, &tmp.join("extract"), &fqns, &mut java);
        assert_eq!(
            batches.len(),
            1,
            "only the actual archive entry can create facts"
        );
        assert!(batches[0]
            .origin
            .as_ref()
            .unwrap()
            .origin_id
            .starts_with("jdk:"));
        for batch in batches {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.bump_index_revision();
        let got = ws
            .complete("seed/DefaultUse.java", offset, true)
            .expect("seeded Iterable receiver");
        assert!(got.items.iter().any(|item| item.label == "useful"));
        let items = ws.index.lookup_type("Item");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].sym.package, "seed");
        let missing = r#"package seedunknown;
public class UnknownUse {
    void run(DefinitelyNotInJavaLang value) { value.toString(); }
}
"#;
        let tree = java.parse(missing);
        let fqns = crate::java::referenced_java_lang_fqns(&tree, missing);
        assert_eq!(fqns, ["java.lang.DefinitelyNotInJavaLang"]);
        assert!(
            resolve_jdk_imports_from_zip(&src_zip, &tmp.join("extract"), &fqns, &mut java)
                .is_empty()
        );
        assert!(ws.index.lookup_type("DefinitelyNotInJavaLang").is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn jdk_seed_preserves_same_file_and_type_parameter_shadowing() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_jdk_seed_shadow_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src_zip = tmp.join("src.zip");
        write_sources_jar(
            &src_zip,
            &[(
                "java.base/java/lang/Iterable.java",
                "package java.lang; public interface Iterable<T> { java.util.Iterator<T> iterator(); }",
            )],
        );
        let src = r#"package seedshadow;
class Item {}
class Iterable<T> { public void ownOnly() {} }
public class ShadowUse {
    void run(Iterable<Item> values) { values.ownOnly(); }
}
"#;
        let mut java = JavaParser::new();
        let tree = java.parse(src);
        let fqns = crate::java::referenced_java_lang_fqns(&tree, src);
        let mut ws = crate::workspace::Workspace::new();
        ws.open("seedshadow/ShadowUse.java", src.to_string());
        for batch in resolve_jdk_imports_from_zip(&src_zip, &tmp.join("extract"), &fqns, &mut java)
        {
            assert!(ws.index_dependency_batch(batch));
        }
        ws.bump_index_revision();
        assert_eq!(ws.index.lookup_type("Iterable").len(), 2);
        let offset = src.find("values.ownOnly").unwrap() + "values.".len();
        let got = ws
            .complete("seedshadow/ShadowUse.java", offset, true)
            .expect("local Iterable receiver");
        assert!(got.items.iter().any(|item| item.label == "ownOnly"));
        assert!(!got.items.iter().any(|item| item.label == "iterator"));
        let parameter = r#"package seedparameter;
public class ParameterUse<Iterable> {
    void run(Iterable value) { value.toString(); }
}
"#;
        ws.open("seedparameter/ParameterUse.java", parameter.to_string());
        let offset = parameter.find("value.toString").unwrap() + "value.".len();
        assert!(!ws
            .complete("seedparameter/ParameterUse.java", offset, true)
            .is_some_and(|got| { got.items.iter().any(|item| item.label == "iterator") }));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn jdk_seed_flat_archive_selects_exact_members_and_reuses_their_cache() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_jdk_flat_seed_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let src_zip = tmp.join("src.zip");
        write_sources_jar(
            &src_zip,
            &[
                (
                    "other/Iterable.java",
                    "package other; public interface Iterable {}",
                ),
                (
                    "java/lang/Iterable.java",
                    "package java.lang; public interface Iterable<T> {}",
                ),
                (
                    "javax/tools/Iterable.java",
                    "package javax.tools; public interface Iterable {}",
                ),
                (
                    "../java/lang/Missing.java",
                    "package java.lang; public class Missing {}",
                ),
            ],
        );
        let extract = tmp.join("extract");
        let fqns = [
            "java.lang.Iterable".into(),
            "javax.tools.Iterable".into(),
            "java.lang.Missing".into(),
        ];
        let mut java = JavaParser::new();
        let first = resolve_jdk_imports_from_zip(&src_zip, &extract, &fqns, &mut java);
        assert_eq!(first.len(), 2);
        let packages = first
            .iter()
            .flat_map(|batch| &batch.symbols)
            .filter(|symbol| symbol.name == "Iterable")
            .map(|symbol| symbol.package.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(
            packages,
            BTreeSet::from(["java.lang".into(), "javax.tools".into()])
        );
        let dest = extract.join("jdk").join(jar_fingerprint(&src_zip).unwrap());
        let selected = dest.join("java/lang/Iterable.java");
        assert!(first.iter().any(|batch| Path::new(&batch.file) == selected));
        let modified = std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        fs::File::options()
            .write(true)
            .open(&selected)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let before = fs::metadata(&selected).unwrap().modified().unwrap();
        // Stale siblings are not members of this archive, even if their suffix sorts first.
        let stale = dest.join("000/java/lang");
        fs::create_dir_all(&stale).unwrap();
        fs::write(
            stale.join("Iterable.java"),
            "package stale; public class Iterable {}",
        )
        .unwrap();
        fs::write(
            stale.join("Missing.java"),
            "package stale; public class Missing {}",
        )
        .unwrap();
        let second = resolve_jdk_imports_from_zip(&src_zip, &extract, &fqns, &mut java);
        assert_eq!(
            second.iter().map(|batch| &batch.file).collect::<Vec<_>>(),
            first.iter().map(|batch| &batch.file).collect::<Vec<_>>()
        );
        assert_eq!(
            fs::metadata(&selected).unwrap().modified().unwrap(),
            before,
            "warm requests must reuse the actual selected source without rewriting it"
        );
        assert_eq!(
            fs::read_to_string(&selected).unwrap(),
            "package java.lang; public interface Iterable<T> {}"
        );
        assert!(
            !dest
                .parent()
                .unwrap()
                .join("java/lang/Missing.java")
                .exists(),
            "an unsafe matching entry must not escape its extraction root"
        );
        assert!(!dest.join("other/Iterable.java").exists());
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn jdk_seed_never_publishes_invalid_utf8_or_bad_crc() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_jdk_invalid_seed_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let payloads: &[(&str, &[u8])] = &[
            (
                "utf8",
                b"// invalid \xff\npackage java.lang; public class Bad {}\n",
            ),
            ("crc", b"package java.lang; public class Bad {}\n"),
        ];
        let mut java = JavaParser::new();
        for &(case, payload) in payloads {
            let archive = tmp.join(format!("{case}.zip"));
            let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
            writer
                .start_file(
                    "java.base/java/lang/Bad.java",
                    SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
                )
                .unwrap();
            writer.write_all(payload).unwrap();
            writer.finish().unwrap();
            if case == "crc" {
                let mut bytes = fs::read(&archive).unwrap();
                let offset = bytes
                    .windows(payload.len())
                    .position(|part| part == payload)
                    .unwrap();
                bytes[offset + payload.len() - 1] ^= 1;
                fs::write(&archive, bytes).unwrap();
            }
            let extract = tmp.join(case);
            assert!(
                resolve_jdk_imports_from_zip(
                    &archive,
                    &extract,
                    &["java.lang.Bad".into()],
                    &mut java
                )
                .is_empty(),
                "{case}"
            );
            let dest = extract.join("jdk").join(jar_fingerprint(&archive).unwrap());
            assert!(
                !dest.join("java.base/java/lang/Bad.java").exists(),
                "{case}"
            );
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    fn assert_dependency_buffer_lifecycle(batch: FileSymbols, source: &Path, original: &str) {
        use crate::index::Tier;
        use crate::workspace::Workspace;

        let indexed_key = batch.file.clone();
        let source_key = crate::paths::file_key(source);
        let main_key = crate::paths::file_key(&source.with_file_name("Main.kt"));
        let main =
            "package app\nimport lib.Helper\nfun use(helper: Helper) {\n    helper.clear\n}\n";
        let offset = main.find("helper.clear").unwrap() + "helper.clear".len();
        let labels = |ws: &mut Workspace| {
            ws.completion_query(&main_key, offset)
                .expect("known dependency receiver should produce a completion query")
                .candidates
                .into_iter()
                .map(|candidate| candidate.label)
                .collect::<BTreeSet<_>>()
        };

        let mut ws = Workspace::new();
        ws.assume_index_complete_for_tests();
        ws.index
            .replace_file(&batch.file, batch.symbols, Tier::Durable);
        ws.bump_index_revision();
        ws.open(main_key.clone(), main.to_string());
        assert_eq!(
            labels(&mut ws),
            BTreeSet::from(["clearOriginal".to_string()])
        );

        let renamed = original.replace("clearOriginal", "clearChanged");
        ws.open(source_key.clone(), renamed.clone());
        assert_eq!(
            labels(&mut ws),
            BTreeSet::from(["clearChanged".to_string()]),
            "opened source must replace closed facts: indexed={indexed_key:?}, editor={source_key:?}"
        );
        assert_eq!(ws.index.lookup_type("Helper").len(), 1);
        let call = main.replace("helper.clear", "helper.clearChanged()");
        ws.change(&main_key, call.clone());
        let defs = ws.goto_definition(&main_key, call.find("clearChanged").unwrap());
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].file, source_key);
        assert_eq!(
            &renamed[defs[0].start_byte..defs[0].end_byte],
            "clearChanged"
        );
        ws.change(&main_key, main.to_string());

        ws.change(
            &source_key,
            "package lib;\npublic class Helper { }\n".to_string(),
        );
        assert!(
            labels(&mut ws).is_empty(),
            "deleted members must leave no stale completion"
        );
        ws.close(&source_key);
        assert_eq!(
            labels(&mut ws),
            BTreeSet::from(["clearOriginal".to_string()])
        );
        assert_eq!(fs::read_to_string(source).unwrap(), original);
        ws.open(source_key.clone(), renamed);
        assert_eq!(
            labels(&mut ws),
            BTreeSet::from(["clearChanged".to_string()])
        );
        ws.close(&source_key);
        assert_eq!(
            labels(&mut ws),
            BTreeSet::from(["clearOriginal".to_string()])
        );
    }

    #[test]
    fn dependency_source_edits_replace_closed_facts_for_native_editor_paths() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_dependency_overlay_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let source = tmp.join("Helper.java");
        let original =
            "package lib;\npublic class Helper {\n    public void clearOriginal() {}\n}\n";
        fs::write(&source, original).unwrap();
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let broad = parse_source_file(&source, original, &mut kotlin, &mut java).unwrap();
        let seeded = parse_java_source_file(&source, &mut java).unwrap();
        for batch in [broad, seeded] {
            assert_dependency_buffer_lifecycle(batch, &source, original);
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn streamed_symcache_preserves_legacy_payload_and_roundtrips_source_facts() {
        let sequence = SYMCACHE_WRITE_SEQUENCE.fetch_add(1, AtomicOrdering::Relaxed);
        let tmp = std::env::temp_dir().join(format!(
            "ktlsp_streamed_symcache_{}_{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let source = tmp.join("Library.kt");
        let text =
            "package lib\nclass Payload\nsuspend fun first(value: Payload): Payload = TODO()\n";
        fs::write(&source, text).unwrap();
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let mut batch = parse_source_file(&source, text, &mut kotlin, &mut java).unwrap();
        batch.origin = Some(FileOrigin {
            origin_id: "maven:lib:sample:1".into(),
            representation: SourceRepresentation::Source,
            project_scope: None,
        });
        let data = vec![batch];
        assert_eq!(data[0].jvm.as_ref().unwrap().callables.len(), 1);
        let legacy_wire = bincode::serialize(&data).unwrap();
        let pending = tmp.join("pending.zst");
        symcache_write_file(&pending, &data).unwrap();
        let cache = tmp.join("published.zst");
        fs::rename(&pending, &cache).unwrap();
        let decoded = zstd::stream::decode_all(&fs::read(&cache).unwrap()[..]).unwrap();
        assert_eq!(
            decoded, legacy_wire,
            "streaming must preserve the existing bincode payload"
        );
        let loaded = symcache_load_file(&cache, &tmp).unwrap();
        assert_eq!(loaded[0].symbols, data[0].symbols);
        assert_eq!(loaded[0].jvm, data[0].jvm);
        assert!(
            loaded[0].origin.is_none(),
            "runtime origins must remain unpersisted"
        );
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn warm_symcache_restores_type_context_sharing_without_changing_values() {
        use std::sync::Arc;

        fn contexts(symbols: &[IndexedSymbol]) -> [&Arc<Vec<String>>; 3] {
            let first = symbols
                .iter()
                .find(|symbol| symbol.name == "first")
                .unwrap();
            let second = symbols
                .iter()
                .find(|symbol| symbol.name == "second")
                .unwrap();
            [
                &first.return_type.as_ref().unwrap().package_candidates,
                &first.params[0].package_candidates,
                &second.return_type.as_ref().unwrap().package_candidates,
            ]
        }

        let tmp =
            std::env::temp_dir().join(format!("ktlsp_cached_type_contexts_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for (name, text) in [
            (
                "Helper.java",
                "package lib; import elsewhere.*; import api.Imported; public class Helper { public Payload first(Payload value) { return null; } public Payload second(Payload value) { return null; } public Imported third(Imported value) { return null; } }",
            ),
            (
                "Library.kt",
                "package lib\nimport elsewhere.*\nimport api.Imported\nclass Payload\nsuspend fun first(value: Payload): Payload = TODO()\nsuspend fun second(value: Payload): Payload = TODO()\nsuspend fun third(value: Imported): Imported = TODO()\n",
            ),
        ] {
            let source = tmp.join(name);
            fs::write(&source, text).unwrap();
            let cold = parse_source_file(&source, text, &mut kotlin, &mut java).unwrap();
            let pointers = contexts(&cold.symbols);
            assert!(
                pointers[1..]
                    .iter()
                    .all(|other| Arc::ptr_eq(pointers[0], other))
            );

            let wire = bincode::serialize(&vec![cold]).unwrap();
            let raw: Vec<FileSymbols> = bincode::deserialize(&wire).unwrap();
            let pointers = contexts(&raw[0].symbols);
            assert!(!Arc::ptr_eq(pointers[0], pointers[1]));
            let cache = source.with_extension("zst");
            fs::write(&cache, zstd::stream::encode_all(&wire[..], 1).unwrap()).unwrap();
            let loaded = symcache_load_file(&cache, &tmp).unwrap();
            assert_eq!(bincode::serialize(&loaded).unwrap(), wire);
            let pointers = contexts(&loaded[0].symbols);
            assert!(
                pointers[1..]
                    .iter()
                    .all(|other| Arc::ptr_eq(pointers[0], other)),
                "accepted {name} cache must retain cold declaration-context sharing"
            );
            let imported = loaded[0]
                .symbols
                .iter()
                .find(|symbol| symbol.name == "third")
                .unwrap();
            let imported_context = &imported.return_type.as_ref().unwrap().package_candidates;
            assert_eq!(imported_context.as_ref(), &["api"]);
            assert!(!Arc::ptr_eq(pointers[0], imported_context));
            if let Some(jvm) = &loaded[0].jvm {
                assert_eq!(jvm.callables.len(), 3);
                assert!(Arc::ptr_eq(
                    pointers[0],
                    &jvm.callables[0].params[0].package_candidates
                ));
                let result = &jvm.callables[0].return_type;
                assert_eq!(result.name, "Object");
                assert_eq!(result.qualification.as_deref(), Some("java.lang"));
                assert!(Arc::ptr_eq(pointers[0], &result.package_candidates));
                let continuation = jvm.callables[0].params.last().unwrap();
                assert_eq!(
                    continuation.qualification.as_deref(),
                    Some("kotlin.coroutines")
                );
                assert!(Arc::ptr_eq(
                    pointers[0],
                    &continuation.args[0].package_candidates
                ));
            }
            fs::remove_file(source).unwrap();
            assert!(symcache_load_file(&cache, &tmp).is_none());
            assert_eq!(bincode::serialize(&raw).unwrap(), wire);
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn warm_symcache_preserves_dependency_buffer_identity() {
        let tmp = std::env::temp_dir().join(format!(
            "ktlsp_cached_dependency_overlay_{}",
            std::process::id()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let source = tmp.join("Helper.java");
        let original =
            "package lib;\npublic class Helper {\n    public void clearOriginal() {}\n}\n";
        fs::write(&source, original).unwrap();
        let mut java = JavaParser::new();
        let mut legacy_batch = parse_java_source_file(&source, &mut java).unwrap();
        legacy_batch.file = source.to_string_lossy().into_owned();
        let serialized = bincode::serialize(&vec![legacy_batch]).unwrap();
        let cache_file = tmp.join("legacy.zst");
        fs::write(
            &cache_file,
            zstd::stream::encode_all(&serialized[..], 1).unwrap(),
        )
        .unwrap();
        let mut batches = symcache_load_file(&cache_file, &tmp).unwrap();
        assert_eq!(batches.len(), 1);
        assert_dependency_buffer_lifecycle(batches.remove(0), &source, original);
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn library_origin_is_attached_after_cold_parse_and_warm_cache_load() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_library_origin_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api-sources.jar");
        write_sources_jar(
            &jar,
            &[(
                "api/Reader.java",
                "package api; public class Reader { public void useful() {} }",
            )],
        );
        let mut source = LibrarySource {
            dest: tmp.join("extract"),
            jar: Some(jar.clone()),
            fingerprint: jar_fingerprint(&jar),
            origin_id: "maven:vendor:api:1".to_string(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let cold = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(cold.len(), 1);
        assert!(cold[0].symbols.iter().any(|symbol| symbol.name == "useful"));
        assert_eq!(
            cold[0].origin.as_ref().unwrap().origin_id,
            "maven:vendor:api:1"
        );
        assert_eq!(
            cold[0].origin.as_ref().unwrap().representation,
            SourceRepresentation::Source
        );
        let serialized = bincode::serialize(&cold).unwrap();
        let unowned: Vec<FileSymbols> = bincode::deserialize(&serialized).unwrap();
        assert!(
            unowned[0].origin.is_none(),
            "runtime ownership must not be persisted in symbol caches"
        );
        source.origin_id = "maven:vendor:alias:1".to_string();
        let warm = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(warm.len(), 1);
        assert_eq!(warm[0].origin.as_ref().unwrap().origin_id, source.origin_id);
        assert_eq!(warm[0].file, cold[0].file);
        assert_eq!(warm[0].symbols.len(), cold[0].symbols.len());
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn local_stub_cache_miss_repairs_incomplete_extractions() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("ktlsp_partial_stubs_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let binary = root.join("api.jar");
        let mut archive = zip::ZipWriter::new(fs::File::create(&binary).unwrap());
        for name in ["Useful", "Other"] {
            archive
                .start_file(
                    format!("api/{name}.class"),
                    zip::write::SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(b"class-name-only fixture").unwrap();
        }
        archive.finish().unwrap();
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let initial =
            resolve_local_jar_stubs(&binary, &root.join("extracted"), &mut kotlin, &mut java)
                .unwrap();
        assert_eq!(initial.len(), 2);
        fs::remove_file(&initial[1].file).unwrap();
        let repaired =
            resolve_local_jar_stubs(&binary, &root.join("extracted"), &mut kotlin, &mut java)
                .unwrap();
        assert_eq!(
            repaired.len(),
            2,
            "a cache miss cannot accept a partial extraction directory"
        );
        assert!(repaired
            .iter()
            .all(|batch| Path::new(&batch.file).is_file()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn field_only_binary_class_reaches_local_and_module_cold_and_warm_publication() {
        let bytes =
            include_bytes!("../../ktcore/tests/fixtures/classfile/fieldfixture/FieldOnly.class");
        let facts = ktcore::classfile::decode("field-only-fixture", bytes).unwrap();
        assert_eq!(facts.identity.internal_name, "fieldfixture/FieldOnly");
        assert_eq!(facts.access_flags, 0x0031);
        assert_eq!(facts.source_file.as_deref(), Some("FieldOnly.java"));
        assert_eq!(facts.fields.len(), 2);
        for (name, descriptor, flags) in [
            ("LABEL", "Ljava/lang/String;", 0x0019),
            ("count", "I", 0x0001),
        ] {
            let field = facts
                .fields
                .iter()
                .find(|field| field.name.as_ref() == name)
                .unwrap();
            assert_eq!(field.descriptor.as_ref(), descriptor);
            assert_eq!(field.access_flags, flags);
            assert!(field.signature.is_none());
        }
        assert_eq!(facts.methods.len(), 1);
        assert_eq!(facts.methods[0].name.as_ref(), "<init>");
        assert_eq!(facts.methods[0].descriptor.as_ref(), "()V");

        let root = std::env::temp_dir().join(format!(
            "ktlsp_field_only_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let jar = root.join("fields.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            archive
                .start_file("fieldfixture/FieldOnly.class", SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes).unwrap();
            archive.finish().unwrap();
        }
        assert!(!root.join("fields-sources.jar").exists());
        let origin = crate::classpath::file_origin_id(&jar);
        let coord = Coordinate::parse("fixture:field-only:1").unwrap();
        let module_origin = crate::classpath::maven_origin_id(&coord);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for (is_module, owner, tag) in [
            (
                false,
                origin.as_str(),
                b"|local-jar-java-members-v7|".as_slice(),
            ),
            (
                true,
                module_origin.as_str(),
                b"|module-jar-java-members-v7|".as_slice(),
            ),
        ] {
            let mut hasher = Sha256::new();
            hasher.update(SYMCACHE_VERSION);
            hasher.update(tag);
            hasher.update(owner.as_bytes());
            if is_module {
                hasher.update(b"|");
                hasher.update(origin.as_bytes());
            }
            let fingerprint = format!("{:x}", hasher.finalize());
            let extract = root.join(if is_module { "module" } else { "local" });
            let dest = extract.join("local-jars").join(&fingerprint);
            assert!(
                symcache_load(&fingerprint, &dest).is_none(),
                "cold cache must be absent"
            );
            let mut previous = None;
            for warm in [false, true] {
                if warm {
                    let cached =
                        symcache_load(&fingerprint, &dest).expect("warm cache must be usable");
                    assert_eq!(cached.len(), 1);
                    assert!(cached[0].symbols.iter().any(|symbol| {
                        symbol.name == "LABEL"
                            && symbol.kind == ktcore::symbol::SymbolKind::Property
                    }));
                }
                let batches = if is_module {
                    resolve_module_binary_stubs(
                        &coord,
                        &[(jar.clone(), origin.clone())],
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                    .unwrap()
                } else {
                    resolve_local_jar_stubs_for_origin(
                        &jar,
                        &origin,
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                    .unwrap()
                };
                assert_eq!(
                    batches.len(),
                    1,
                    "field-only class must survive publication"
                );
                let batch = &batches[0];
                let provenance = batch.origin.as_ref().unwrap();
                assert_eq!(provenance.origin_id, owner);
                assert_eq!(provenance.representation, SourceRepresentation::Stub);
                assert!(!batch
                    .symbols
                    .iter()
                    .any(|symbol| symbol.kind == ktcore::symbol::SymbolKind::Function));
                for (name, ty, dispatch) in [
                    ("LABEL", "String", ktcore::extensions::Dispatch::Static),
                    ("count", "int", ktcore::extensions::Dispatch::Instance),
                ] {
                    let field = batch
                        .symbols
                        .iter()
                        .find(|symbol| symbol.name == name)
                        .unwrap();
                    assert_eq!(field.kind, ktcore::symbol::SymbolKind::Property);
                    assert_eq!(field.value_type.as_ref().unwrap().name, ty);
                    assert_eq!(field.source_member.dispatch(), Some(dispatch));
                }
                let encoded = bincode::serialize(&batches).unwrap();
                if let Some(previous) = &previous {
                    assert_eq!(&encoded, previous);
                }
                previous = Some(encoded);
                let mut workspace = crate::workspace::Workspace::new();
                for batch in batches {
                    assert!(workspace.index_dependency_batch(batch));
                }
                workspace.bump_index_revision();
                for (key, source, needle, expected) in [
                    (
                        "Fields.java",
                        "import fieldfixture.FieldOnly; class Fields { void f() { FieldOnly.LA; } }",
                        "FieldOnly.LA",
                        "LABEL",
                    ),
                    (
                        "Fields.kt",
                        "import fieldfixture.FieldOnly\nfun f() { FieldOnly.LA }\n",
                        "FieldOnly.LA",
                        "LABEL",
                    ),
                    (
                        "Fields.java",
                        "import fieldfixture.FieldOnly; class Fields { void f(FieldOnly value) { value.co; } }",
                        "value.co",
                        "count",
                    ),
                    (
                        "Fields.kt",
                        "import fieldfixture.FieldOnly\nfun f(value: FieldOnly) { value.co }\n",
                        "value.co",
                        "count",
                    ),
                ] {
                    workspace.open(key, source.to_string());
                    let query = workspace
                        .completion_query(key, source.find(needle).unwrap() + needle.len())
                        .unwrap();
                    assert!(
                        query.candidates.iter().any(|item| item.label == expected),
                        "module={is_module} warm={warm} {key} {needle}: {} {:?}",
                        query.status_label(),
                        query.reason_labels()
                    );
                    assert_eq!(workspace.doc_text(key).as_deref(), Some(source));
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_java_shadow_facts_and_marker_reach_local_module_cold_warm_queries() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_shadow_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let jar = root.join("shadows.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            for (name, bytes) in [
                (
                    "Base",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/Base.class"
                    )
                    .as_slice(),
                ),
                (
                    "Mid",
                    include_bytes!("../../ktcore/tests/fixtures/classfile/shadowfixture/Mid.class")
                        .as_slice(),
                ),
                (
                    "GenericMid",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/GenericMid.class"
                    )
                    .as_slice(),
                ),
                (
                    "NestedBoundMid",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/NestedBoundMid.class"
                    )
                    .as_slice(),
                ),
                (
                    "PublicMid",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/PublicMid.class"
                    )
                    .as_slice(),
                ),
                (
                    "Marker",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/shadowfixture/Marker.class"
                    )
                    .as_slice(),
                ),
            ] {
                archive
                    .start_file(
                        format!("shadowfixture/{name}.class"),
                        SimpleFileOptions::default(),
                    )
                    .unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
        }
        let coord = Coordinate::parse("fixture:shadow-api:1").unwrap();
        let origin = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for is_module in [false, true] {
            let extract = root.join(if is_module { "module" } else { "local" });
            for warm in [false, true] {
                let batches = if is_module {
                    resolve_module_binary_stubs(
                        &coord,
                        &[(jar.clone(), origin.clone())],
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                } else {
                    resolve_local_jar_stubs_for_origin(
                        &jar,
                        &origin,
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                }
                .unwrap();
                assert!(
                    batches
                        .iter()
                        .flat_map(|b| &b.symbols)
                        .any(|s| s.name == "Marker"
                            && s.kind == ktcore::symbol::SymbolKind::Interface),
                    "marker module={is_module} warm={warm}"
                );
                let mut workspace = crate::workspace::Workspace::new();
                for batch in batches {
                    assert!(workspace.index_dependency_batch(batch));
                }
                workspace.open("KnownChild.java", "package shadowfixture; public class KnownChild extends GenericMid { public Object f(int value) { return null; } }".to_string());
                for (file, source, selector, expected_owner) in [
                    (
                        "Use.kt",
                        "import shadowfixture.Mid\nfun use(value: Mid) = value.f()\n",
                        "value.f",
                        Some("Mid.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.Mid; class Use { Object use(Mid value) { return value.f(); } }",
                        "value.f",
                        Some("Mid.java"),
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.GenericMid\nfun use(value: GenericMid) = value.f()\n",
                        "value.f",
                        Some("GenericMid.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.GenericMid; class Use { Object use(GenericMid value) { return value.f(); } }",
                        "value.f",
                        Some("GenericMid.java"),
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.NestedBoundMid\nfun use(value: NestedBoundMid) = value.f()\n",
                        "value.f",
                        None,
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.NestedBoundMid; class Use { Object use(NestedBoundMid value) { return value.f(); } }",
                        "value.f",
                        None,
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.KnownChild\nfun use(value: KnownChild) = value.f()\n",
                        "value.f",
                        Some("GenericMid.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.KnownChild; class Use { Object use(KnownChild value) { return value.f(); } }",
                        "value.f",
                        Some("GenericMid.java"),
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.KnownChild\nfun use(value: KnownChild) = value.f(1)\n",
                        "value.f",
                        Some("KnownChild.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.KnownChild; class Use { Object use(KnownChild value) { return value.f(1); } }",
                        "value.f",
                        Some("KnownChild.java"),
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.GenericMid\nfun use(value: GenericMid) = value.value\n",
                        "value.value",
                        None,
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.GenericMid; class Use { Object use(GenericMid value) { return value.value; } }",
                        "value.value",
                        None,
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.GenericMid\nfun use(value: GenericMid) = value.value()\n",
                        "value.value",
                        Some("Base.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.GenericMid; class Use { void use(GenericMid value) { value.value(); } }",
                        "value.value",
                        Some("Base.java"),
                    ),
                    (
                        "Use.kt",
                        "import shadowfixture.PublicMid\nfun use(value: PublicMid) = value.value\n",
                        "value.value",
                        Some("PublicMid.java"),
                    ),
                    (
                        "Use.java",
                        "import shadowfixture.PublicMid; class Use { Object use(PublicMid value) { return value.value; } }",
                        "value.value",
                        Some("PublicMid.java"),
                    ),
                ] {
                    workspace.open(file, source.to_string());
                    let at = source.find(selector).unwrap() + selector.rfind('.').unwrap() + 1;
                    let targets = workspace.goto_definition(file, at);
                    if let Some(owner) = expected_owner {
                        assert_eq!(
                            targets.len(),
                            1,
                            "{file} {selector} module={is_module} warm={warm}: {targets:?}"
                        );
                        assert!(
                            targets[0].file.ends_with(owner),
                            "{file} {selector}: {targets:?}"
                        );
                        if owner == "KnownChild.java" {
                            let query = workspace.completion_query(file, at + 1).unwrap();
                            assert!(query.candidates.iter().any(|item| item.label == "f"));
                        }
                    } else {
                        assert!(
                            targets.is_empty(),
                            "must not borrow an ancestor: {file} {selector} module={is_module} warm={warm}: {targets:?}"
                        );
                        if selector == "value.f" && !source.contains("KnownChild") {
                            let query = workspace.completion_query(file, at + 1).unwrap();
                            assert!(
                                query.candidates.iter().all(|item| item.label != "f"),
                                "unavailable override: {file} module={is_module} warm={warm}"
                            );
                        }
                    }
                }
                let source = "import shadowfixture.Mid\nclass Leaf : Mid() { override fun f(): String = \"\"; fun test() { super.f() } }\n";
                workspace.open("Leaf.kt", source.to_string());
                let at = source.find("super.f").unwrap() + "super.f".len();
                let query = workspace.completion_query("Leaf.kt", at).unwrap();
                assert!(query.candidates.iter().all(|item| item.label != "f"));
                assert!(workspace.goto_definition("Leaf.kt", at - 1).is_empty());
                workspace.open(
                    "Extensions.kt",
                    "package exts\nfun shadowfixture.NestedBoundMid.f(): String = \"safe\"\n"
                        .to_string(),
                );
                let source = "import shadowfixture.NestedBoundMid\nimport exts.f as safeAlias\nfun use(value: NestedBoundMid) { value. }\n";
                workspace.open("Alias.kt", source.to_string());
                let at = source.find("value.").unwrap() + "value.".len();
                let query = workspace.completion_query("Alias.kt", at).unwrap();
                assert!(
                    query
                        .candidates
                        .iter()
                        .any(|item| item.label == "safeAlias"),
                    "aliased extension module={is_module} warm={warm}"
                );
                assert!(query.candidates.iter().all(|item| item.label != "f"));
                let call = source.replace("value. }", "value.safeAlias() }");
                workspace.open("Alias.kt", call.clone());
                let targets = workspace.goto_definition(
                    "Alias.kt",
                    call.find("value.safeAlias").unwrap() + "value.".len(),
                );
                assert_eq!(targets.len(), 1, "{targets:?}");
                assert_eq!(targets[0].file, "Extensions.kt");
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_java_actual_ponder_interface_reaches_cold_warm_super_and_extension_queries() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_ponder_interface_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let jar = root.join("ponder.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            archive
                .start_file(
                    "net/createmod/ponder/api/VirtualBlockEntity.class",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            archive
                .write_all(include_bytes!(
                    "../../ktcore/tests/fixtures/classfile/ponder1087/VirtualBlockEntity.class"
                ))
                .unwrap();
            archive.finish().unwrap();
        }
        let coord = Coordinate::parse("fixture:ponder:1").unwrap();
        let binary_origin = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let legacy_dir = root.join("legacy");
        fs::create_dir(&legacy_dir).unwrap();
        fs::write(
            legacy_dir.join("VirtualBlockEntity.java"),
            "package net.createmod.ponder.api; public class VirtualBlockEntity {}",
        )
        .unwrap();
        let legacy = parse_dir(&legacy_dir, &mut kotlin, &mut java).unwrap();
        for is_module in [true, false] {
            let extract = root.join(if is_module { "module" } else { "local" });
            let owner = if is_module {
                crate::classpath::maven_origin_id(&coord)
            } else {
                binary_origin.clone()
            };
            let fingerprint = |version| {
                let mut hash = Sha256::new();
                hash.update(SYMCACHE_VERSION);
                hash.update(format!(
                    "|{}-jar-java-members-v{version}|",
                    if is_module { "module" } else { "local" }
                ));
                hash.update(owner.as_bytes());
                if is_module {
                    hash.update(b"|");
                    hash.update(binary_origin.as_bytes());
                }
                format!("{:x}", hash.finalize())
            };
            let old_key = fingerprint(3);
            symcache_store(&old_key, &legacy);
            assert!(
                symcache_load(&old_key, &legacy_dir).is_some(),
                "legacy placeholder cache must exist"
            );
            let current_key = fingerprint(7);
            let dest = extract.join("local-jars").join(&current_key);
            let cache = symcache_dir().join(format!("{current_key}.zst"));
            for warm in [false, true] {
                assert_eq!(
                    symcache_load(&current_key, &dest).is_some(),
                    warm,
                    "cold/warm cache not exercised"
                );
                let before = warm.then(|| {
                    (
                        fs::metadata(&cache).unwrap().modified().unwrap(),
                        fs::read(&cache).unwrap(),
                    )
                });
                let batches = if is_module {
                    resolve_module_binary_stubs(
                        &coord,
                        &[(jar.clone(), binary_origin.clone())],
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                } else {
                    resolve_local_jar_stubs_for_origin(
                        &jar,
                        &binary_origin,
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                }
                .unwrap();
                if let Some(before) = before {
                    assert_eq!(
                        before,
                        (
                            fs::metadata(&cache).unwrap().modified().unwrap(),
                            fs::read(&cache).unwrap()
                        ),
                        "accepted Ponder cache must be consumed without rewriting it"
                    );
                }
                assert!(
                    batches
                        .iter()
                        .flat_map(|batch| &batch.symbols)
                        .any(|symbol| symbol.name == "VirtualBlockEntity"
                            && symbol.kind == ktcore::symbol::SymbolKind::Interface),
                    "interface missing: module={is_module} warm={warm}"
                );
                assert!(batches.iter().all(|batch| {
                    batch
                        .origin
                        .as_ref()
                        .is_some_and(|origin| origin.representation == SourceRepresentation::Stub)
                }));
                let mut workspace = crate::workspace::Workspace::new();
                for batch in batches {
                    assert!(workspace.index_dependency_batch(batch));
                }
                for (file, text) in [
                    (
                        "Base.java",
                        "package probe; public class Base implements net.createmod.ponder.api.VirtualBlockEntity { public void markVirtual() {} public boolean isVirtual() { return false; } protected float getBreakSpeed() { return 1f; } }",
                    ),
                    ("Key.java", "package probe; public class Key<T> {}"),
                    (
                        "Lookup.java",
                        "package probe; public class Lookup<T> { public T getOrThrow(Key<T> key) { return null; } }",
                    ),
                    (
                        "Use.kt",
                        "package probe\ninterface Marker\nclass MarkerImpl : Marker\nfun <T> Base.holderLookup(key: Key<T>): Lookup<T> = TODO()\nclass Drill : Base(), Marker by MarkerImpl() {\n override fun getBreakSpeed(): Float = super.getBreakSpeed()\n fun work(key: Key<String>) = holderLookup(key).getOrThrow(key)\n}\n",
                    ),
                ] {
                    workspace.open(file, text.to_string());
                }
                let source = workspace.doc_text("Use.kt").unwrap();
                for (expression, member, target_file) in [
                    ("super.getBreakSpeed", "getBreakSpeed", "Base.java"),
                    ("holderLookup(key).getOrThrow", "getOrThrow", "Lookup.java"),
                ] {
                    let start = source.find(expression).unwrap();
                    let query = workspace
                        .completion_query("Use.kt", start + expression.len())
                        .unwrap();
                    assert!(
                        query.candidates.iter().any(|item| item.label == member),
                        "{member} module={is_module} warm={warm}: {} {:?}",
                        query.status_label(),
                        query.reason_labels()
                    );
                    let targets = workspace
                        .goto_definition("Use.kt", start + expression.rfind(member).unwrap());
                    assert_eq!(
                        targets.len(),
                        1,
                        "{member} module={is_module} warm={warm}: {targets:?}"
                    );
                    assert_eq!(targets[0].file, target_file);
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_java_protected_members_use_caller_access_context() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_protected_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let jar = root.join("protected.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            for (name, bytes) in [
                (
                    "ProtectedApi",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/protectedfixture/ProtectedApi.class"
                    )
                    .as_slice(),
                ),
                (
                    "Position",
                    include_bytes!(
                        "../../ktcore/tests/fixtures/classfile/protectedfixture/Position.class"
                    )
                    .as_slice(),
                ),
            ] {
                archive
                    .start_file(
                        format!("protectedfixture/{name}.class"),
                        SimpleFileOptions::default(),
                    )
                    .unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
        }
        let coord = Coordinate::parse("fixture:protected-api:1").unwrap();
        let origin = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for module in [false, true] {
            let extract = root.join(if module { "module" } else { "local" });
            let mut hash = Sha256::new();
            hash.update(SYMCACHE_VERSION);
            hash.update(if module {
                b"|module-jar-java-members-v7|".as_slice()
            } else {
                b"|local-jar-java-members-v7|".as_slice()
            });
            if module {
                hash.update(crate::classpath::maven_origin_id(&coord).as_bytes());
                hash.update(b"|");
            }
            hash.update(origin.as_bytes());
            let fingerprint = format!("{:x}", hash.finalize());
            let cache = symcache_dir().join(format!("{fingerprint}.zst"));
            let destination = extract.join("local-jars").join(&fingerprint);
            for warm in [false, true] {
                let before = if warm {
                    assert!(
                        symcache_load(&fingerprint, &destination).is_some(),
                        "cache must be accepted before the repeated consumer load"
                    );
                    let metadata = fs::metadata(&cache).unwrap();
                    Some((
                        metadata.modified().unwrap(),
                        metadata.created().ok(),
                        fs::read(&cache).unwrap(),
                    ))
                } else {
                    assert!(symcache_load(&fingerprint, &destination).is_none());
                    None
                };
                let batches = if module {
                    resolve_module_binary_stubs(
                        &coord,
                        &[(jar.clone(), origin.clone())],
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                } else {
                    resolve_local_jar_stubs_for_origin(
                        &jar,
                        &origin,
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                }
                .unwrap();
                if let Some(before) = before {
                    let metadata = fs::metadata(&cache).unwrap();
                    assert_eq!(
                        before,
                        (
                            metadata.modified().unwrap(),
                            metadata.created().ok(),
                            fs::read(&cache).unwrap()
                        ),
                        "warm consumer must return the accepted cache without rewriting it"
                    );
                }
                let mut ws = crate::workspace::Workspace::new();
                for batch in batches {
                    assert!(ws.index_dependency_batch(batch));
                }
                for (file, source, selector, member, allowed) in [
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { float speed(){return super.getBreakSpeed();} }",
                        "super.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun speed() = super.getBreakSpeed() }",
                        "super.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { Position pass(Position p){return super.transform(p);} }",
                        "super.transform",
                        "transform",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun pass(p: Position) = super.transform(p) }",
                        "super.transform",
                        "transform",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { Position pass(ProtectedApi base, Position p){return super.transform(base.transform(p));} }",
                        "base.transform",
                        "transform",
                        false,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun pass(base: ProtectedApi, p: Position) = super.transform(base.transform(p)) }",
                        "base.transform",
                        "transform",
                        false,
                    ),
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { float speed(){return this.getBreakSpeed();} }",
                        "this.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun speed() = this.getBreakSpeed() }",
                        "this.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { float speed(JavaCaller p){return p.getBreakSpeed();} }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun speed(p: KotlinCaller) = p.getBreakSpeed() }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package other; import protectedfixture.*; class Caller { float speed(ProtectedApi p){return p.getBreakSpeed();} }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        false,
                    ),
                    (
                        "Caller.kt",
                        "package other\nimport protectedfixture.*\nfun speed(p: ProtectedApi) = p.getBreakSpeed()",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        false,
                    ),
                    (
                        "Caller.java",
                        "package protectedfixture; class Caller { float speed(ProtectedApi p){return p.getBreakSpeed();} }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package protectedfixture\nfun speed(p: ProtectedApi) = p.getBreakSpeed()",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package access; import protectedfixture.*; class Caller extends ProtectedApi { float speed(ProtectedApi p){return p.getBreakSpeed();} }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        false,
                    ),
                    (
                        "Caller.kt",
                        "package access\nimport protectedfixture.*\nclass Caller : ProtectedApi() { fun speed(p: ProtectedApi) = p.getBreakSpeed() }",
                        "p.getBreakSpeed",
                        "getBreakSpeed",
                        false,
                    ),
                    (
                        "Caller.java",
                        "package other; import protectedfixture.*; class Caller { int value(ProtectedApi p){return p.known();} }",
                        "p.known",
                        "known",
                        true,
                    ),
                    (
                        "Caller.kt",
                        "package other\nimport protectedfixture.*\nfun value(p: ProtectedApi) = p.known()",
                        "p.known",
                        "known",
                        true,
                    ),
                    (
                        "Caller.java",
                        "package other; import protectedfixture.*; class Caller { int value(ProtectedApi p){return p.hiddenControl();} }",
                        "p.hiddenControl",
                        "hiddenControl",
                        false,
                    ),
                    (
                        "Caller.kt",
                        "package other\nimport protectedfixture.*\nfun value(p: ProtectedApi) = p.hiddenControl()",
                        "p.hiddenControl",
                        "hiddenControl",
                        false,
                    ),
                ] {
                    for authored in [false, true] {
                        let source = source.replace(
                            "class Caller",
                            if file.ends_with(".java") {
                                "class JavaCaller"
                            } else {
                                "class KotlinCaller"
                            },
                        );
                        let source = if authored {
                            ws.open("SourceApi.java", include_str!("../../ktcore/tests/fixtures/classfile/sources/protectedfixture/ProtectedApi.java").replace("ProtectedApi", "SourceApi"));
                            source.replace("ProtectedApi", "SourceApi")
                        } else {
                            source
                        };
                        ws.open(file, source.clone());
                        let at = source.find(selector).unwrap() + selector.rfind('.').unwrap() + 1;
                        let targets = ws.goto_definition(file, at);
                        let query = ws.completion_query(file, at + member.len()).unwrap();
                        assert_eq!(
                            query.candidates.iter().any(|item| item.label == member),
                            allowed,
                            "completion {file} {selector} module={module} warm={warm} authored={authored}: {:?}",
                            query.reason_labels()
                        );
                        assert_eq!(
                            !targets.is_empty(),
                            allowed,
                            "navigation {file} {selector} module={module} warm={warm} authored={authored}: {targets:?}"
                        );
                        if !allowed && matches!(member, "getBreakSpeed" | "transform") {
                            let tree = if file.ends_with(".java") {
                                java.parse(&source)
                            } else {
                                kotlin.parse(&source)
                            };
                            let mut ctx =
                                ktcore::infer::FileCtx::from_tree_in_file(&tree, &source, file);
                            if file.ends_with(".java") {
                                ctx.package = crate::java::package_of(&tree, &source);
                            }
                            let receiver = ktcore::types::Type::class(
                                if authored {
                                    "SourceApi"
                                } else {
                                    "ProtectedApi"
                                },
                                Some("protectedfixture".into()),
                            );
                            let status = ktcore::resolve::member_name_status_for_receiver(
                                &ws.index.scoped_for_file(file),
                                file,
                                &receiver,
                                member,
                                &ktcore::resolve::CompletenessFacts::default(),
                                &ctx,
                                at,
                            );
                            assert!(
                                !status.is_found(),
                                "denied protected member cannot be Found: {file} {selector} authored={authored}"
                            );
                        }
                        if allowed {
                            assert_eq!(targets.len(), 1);
                            assert!(
                                targets[0].file.ends_with(if authored {
                                    "SourceApi.java"
                                } else {
                                    "protectedfixture/ProtectedApi.java"
                                }),
                                "{targets:?}"
                            );
                        }
                    }
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn kotlin_java_primitive_call_results_use_kotlin_scalars() {
        fn expression_type(
            ws: &crate::workspace::Workspace,
            source: &str,
            expression: &str,
        ) -> ktcore::types::Type {
            let tree = KotlinParser::new().parse(source);
            let start = source.rfind(expression).unwrap();
            let node = tree
                .root_node()
                .named_descendant_for_byte_range(start, start + expression.len())
                .unwrap();
            let ctx = ktcore::infer::FileCtx::from_tree_in_file(&tree, source, "Use.kt");
            ktcore::infer::infer(&ws.index.scoped_for_file("Use.kt"), node, source, &ctx)
        }
        let mut ws = crate::workspace::Workspace::new();
        for (raw, kotlin, value) in [
            ("boolean", "Boolean", "false"),
            ("byte", "Byte", "0"),
            ("char", "Char", "'a'"),
            ("short", "Short", "0"),
            ("int", "Int", "0"),
            ("long", "Long", "0L"),
            ("float", "Float", "0f"),
            ("double", "Double", "0d"),
        ] {
            ws.open("Api.java", format!("package app; public class Api {{ public {raw} number; public {raw} value() {{ return {value}; }} }}"));
            for (source, expression, nullable) in [
                (
                    "import app.Api\nfun use(api: Api) = api.value()",
                    "api.value()",
                    false,
                ),
                (
                    "import app.Api\nclass Caller : Api() { fun use() = super.value() }",
                    "super.value()",
                    false,
                ),
                (
                    "import app.Api\nclass Caller : Api() { fun use() = value() }",
                    "value()",
                    false,
                ),
                (
                    "import app.Api\nfun use(api: Api) = api.number",
                    "api.number",
                    false,
                ),
                (
                    "import app.Api\nfun use(api: Api?) = api?.value()",
                    "api?.value()",
                    true,
                ),
            ] {
                ws.open("Use.kt", source.into());
                assert_eq!(
                    expression_type(&ws, source, expression),
                    ktcore::types::Type::class(kotlin, Some("kotlin".into()))
                        .with_nullability(nullable),
                    "{raw} {expression}"
                );
            }
            let java_source = "import app.Api; class Use { void run(Api api) { api.value(); } }";
            ws.open("Use.java", java_source.into());
            let help = ws
                .signature_help(
                    "Use.java",
                    java_source.find("api.value(").unwrap() + "api.value(".len(),
                )
                .unwrap();
            assert!(
                help.signatures
                    .iter()
                    .any(|signature| signature.label == format!("value(): {raw}")),
                "{raw}: {help:?}"
            );
        }
        for name in ["float", "int"] {
            ws.open("Nominal.kt", format!("class {name}"));
            let source = format!("fun use(value: {name}) = value");
            ws.open("Use.kt", source.clone());
            assert_eq!(
                expression_type(&ws, &source, "value"),
                ktcore::types::Type::class(name, Some("".into()))
            );
            ws.open(
                "Api.java",
                format!(
                    "package app; public class Api {{ public {name} value() {{ return {}; }} }}",
                    if name == "float" { "0f" } else { "0" }
                ),
            );
            let call = "import app.Api\nfun use(api: Api) = api.value()";
            ws.open("Use.kt", call.into());
            assert_eq!(
                expression_type(&ws, call, "api.value()"),
                ktcore::types::Type::class(
                    if name == "float" { "Float" } else { "Int" },
                    Some("kotlin".into())
                )
            );
            ws.change("Nominal.kt", String::new());
            ws.open("Use.kt", source.clone());
            assert_eq!(
                expression_type(&ws, &source, "value"),
                ktcore::types::Type::Unknown
            );
        }
    }

    #[test]
    fn kotlin_protected_members_do_not_gain_java_package_access() {
        let mut ws = crate::workspace::Workspace::new();
        ws.open(
            "Api.kt",
            "package access\nopen class Api { protected fun speed(): Int = 1 }".into(),
        );
        for (source, selector, allowed) in [
            (
                "package access\nfun use(p: Api) = p.speed()",
                "p.speed",
                false,
            ),
            (
                "package access\nclass Caller : Api() { fun use() = this.speed() }",
                "this.speed",
                true,
            ),
            (
                "package other\nimport access.Api\nclass Caller : Api() { fun use(p: Api) = p.speed() }",
                "p.speed",
                false,
            ),
        ] {
            ws.open("Use.kt", source.into());
            let at = source.find(selector).unwrap() + selector.rfind('.').unwrap() + 1;
            assert_eq!(
                ws.completion_query("Use.kt", at + 5)
                    .unwrap()
                    .candidates
                    .iter()
                    .any(|item| item.label == "speed"),
                allowed,
                "{source}"
            );
            assert_eq!(
                !ws.goto_definition("Use.kt", at).is_empty(),
                allowed,
                "{source}"
            );
        }
    }

    #[test]
    fn binary_java_actual_create_hierarchy_survives_local_module_and_v2_cache() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_actual_create_hierarchy_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        let jar = root.join("create-api.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            for (name, bytes) in [
                ("com/simibubi/create/content/kinetics/base/BlockBreakingKineticBlockEntity.class", include_bytes!("../../ktcore/tests/fixtures/classfile/create6010/BlockBreakingKineticBlockEntity.class").as_slice()),
                ("com/simibubi/create/content/kinetics/base/KineticBlockEntity.class", include_bytes!("../../ktcore/tests/fixtures/classfile/create6010/KineticBlockEntity.class").as_slice()),
            ] {
                archive.start_file(name, SimpleFileOptions::default()).unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
        }
        let coord = Coordinate::parse("fixture:create-api:1").unwrap();
        let binary_origin = crate::classpath::file_origin_id(&jar);
        let module_origin = crate::classpath::maven_origin_id(&coord);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let legacy_dir = root.join("legacy");
        jar::extract_class_stubs(&jar, &legacy_dir).unwrap();
        let mut legacy = parse_dir(&legacy_dir, &mut kotlin, &mut java).unwrap();
        for batch in &mut legacy {
            for symbol in &mut batch.symbols {
                symbol.supertypes.clear();
                symbol.supertype_refs.clear();
            }
        }
        for (is_module, owner, tag) in [
            (
                false,
                binary_origin.as_str(),
                b"|local-jar-java-members-v2|".as_slice(),
            ),
            (
                true,
                module_origin.as_str(),
                b"|module-jar-java-members-v2|".as_slice(),
            ),
        ] {
            let mut hash = Sha256::new();
            hash.update(SYMCACHE_VERSION);
            hash.update(tag);
            hash.update(owner.as_bytes());
            if is_module {
                hash.update(b"|");
                hash.update(binary_origin.as_bytes());
            }
            let old_key = format!("{:x}", hash.finalize());
            symcache_store(&old_key, &legacy);
            assert!(symcache_load(&old_key, &legacy_dir)
                .unwrap()
                .iter()
                .flat_map(|batch| &batch.symbols)
                .all(|symbol| symbol.supertype_refs.is_empty()));
            let mut hash = Sha256::new();
            hash.update(SYMCACHE_VERSION);
            hash.update(if is_module {
                b"|module-jar-java-members-v7|".as_slice()
            } else {
                b"|local-jar-java-members-v7|".as_slice()
            });
            hash.update(owner.as_bytes());
            if is_module {
                hash.update(b"|");
                hash.update(binary_origin.as_bytes());
            }
            let fingerprint = format!("{:x}", hash.finalize());
            let extract = root.join(if is_module { "module" } else { "local" });
            let dest = extract.join("local-jars").join(&fingerprint);
            assert!(
                symcache_load(&fingerprint, &dest).is_none(),
                "v6 cold cache must be absent"
            );
            for warm in [false, true] {
                if warm {
                    let cached =
                        symcache_load(&fingerprint, &dest).expect("v6 warm cache must be usable");
                    let child = cached
                        .iter()
                        .flat_map(|batch| &batch.symbols)
                        .find(|symbol| symbol.name == "BlockBreakingKineticBlockEntity")
                        .unwrap();
                    assert!(child
                        .supertype_refs
                        .iter()
                        .any(|parent| parent.name == "KineticBlockEntity"
                            && parent.qualification.as_deref()
                                == Some("com.simibubi.create.content.kinetics.base")));
                }
                let batches = if is_module {
                    resolve_module_binary_stubs(
                        &coord,
                        &[(jar.clone(), binary_origin.clone())],
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                } else {
                    resolve_local_jar_stubs_for_origin(
                        &jar,
                        &binary_origin,
                        &extract,
                        &mut kotlin,
                        &mut java,
                    )
                }
                .unwrap();
                let child = batches
                    .iter()
                    .flat_map(|batch| &batch.symbols)
                    .find(|symbol| symbol.name == "BlockBreakingKineticBlockEntity")
                    .unwrap();
                assert!(child
                    .supertype_refs
                    .iter()
                    .any(|parent| parent.name == "KineticBlockEntity"
                        && parent.qualification.as_deref()
                            == Some("com.simibubi.create.content.kinetics.base")));
                assert!(batches.iter().all(|batch| {
                    batch.origin.as_ref().is_some_and(|origin| {
                        origin.origin_id == owner
                            && origin.representation == SourceRepresentation::Stub
                    })
                }));
                let mut workspace = crate::workspace::Workspace::new();
                for batch in batches {
                    assert!(workspace.index_dependency_batch(batch));
                }
                workspace.bump_index_revision();
                for (file, text, selector, member) in [
                    (
                        "Use.java",
                        "import com.simibubi.create.content.kinetics.base.BlockBreakingKineticBlockEntity; class Use { void f(BlockBreakingKineticBlockEntity value) { value.getSpe; } }",
                        "value.getSpe",
                        "getSpeed",
                    ),
                    (
                        "Use.kt",
                        "import com.simibubi.create.content.kinetics.base.BlockBreakingKineticBlockEntity\nfun f(value: BlockBreakingKineticBlockEntity) { value.getSpe }\n",
                        "value.getSpe",
                        "getSpeed",
                    ),
                    (
                        "Use.java",
                        "import com.simibubi.create.content.kinetics.base.BlockBreakingKineticBlockEntity; abstract class Use extends BlockBreakingKineticBlockEntity { float f() { return super.getBreakSpeed(); } }",
                        "super.getBreakSpeed",
                        "getBreakSpeed",
                    ),
                    (
                        "Use.kt",
                        "import com.simibubi.create.content.kinetics.base.BlockBreakingKineticBlockEntity\nabstract class Use : BlockBreakingKineticBlockEntity() { fun f() = super.getBreakSpeed() }\n",
                        "super.getBreakSpeed",
                        "getBreakSpeed",
                    ),
                ] {
                    workspace.open(file, text.to_string());
                    let query = workspace
                        .completion_query(file, text.find(selector).unwrap() + selector.len())
                        .unwrap();
                    assert!(
                        query.candidates.iter().any(|item| item.label == member),
                        "{file} {member} module={is_module}: {} {:?}",
                        query.status_label(),
                        query.reason_labels()
                    );
                    if file.ends_with(".kt") && member == "getBreakSpeed" {
                        let tree = kotlin.parse(text);
                        let start = text.find(selector).unwrap();
                        let call = tree
                            .root_node()
                            .named_descendant_for_byte_range(start, start + selector.len() + 2)
                            .unwrap();
                        assert_eq!(call.kind(), "call_expression");
                        let ctx = ktcore::infer::FileCtx::from_tree_in_file(&tree, text, file);
                        assert_eq!(
                            ktcore::infer::infer(
                                &workspace.index.scoped_for_file(file),
                                call,
                                text,
                                &ctx
                            ),
                            ktcore::types::Type::class("Float", Some("kotlin".into())),
                            "actual Create protected ()F call must infer Kotlin Float: module={is_module} warm={warm}"
                        );
                    }
                }
            }
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn binary_java_jar_members_reach_cold_and_warm_java_kotlin_completion() {
        let factory =
            include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Factory.class")
                .as_slice();
        let item = include_bytes!("../../ktcore/tests/fixtures/classfile/arrayfixture/Item.class")
            .as_slice();
        let person =
            include_bytes!("../../ktcore/tests/fixtures/classfile/pluginfixture/Person.class")
                .as_slice();
        let constants = include_bytes!(
            "../../ktcore/tests/fixtures/classfile/gradle8142/JavaCompilerArgumentsBuilder.class"
        )
        .as_slice();
        let root = std::env::temp_dir().join(format!(
            "ktlsp_binary_member_completion_{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).unwrap();
        let jar = root.join("api.jar");
        {
            let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
            for (name, bytes) in [
                ("arrayfixture/Factory.class", factory),
                ("arrayfixture/Item.class", item),
                ("pluginfixture/Person.class", person),
                (
                    "org/gradle/api/internal/tasks/compile/JavaCompilerArgumentsBuilder.class",
                    constants,
                ),
            ] {
                archive
                    .start_file(name, SimpleFileOptions::default())
                    .unwrap();
                archive.write_all(bytes).unwrap();
            }
            archive.finish().unwrap();
        }
        assert!(!root.join("api-sources.jar").exists());
        let origin = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        // Seed readable v28/methods-v1 caches with the old field-free normalized facts.
        // Existing source files make these valid cache hits unless the binary key changes.
        let legacy_dir = root.join("legacy-stubs");
        jar::extract_class_stubs(&jar, &legacy_dir).unwrap();
        let mut legacy = parse_dir(&legacy_dir, &mut kotlin, &mut java).unwrap();
        for batch in &mut legacy {
            batch
                .symbols
                .retain(|symbol| symbol.kind != ktcore::symbol::SymbolKind::Property);
        }
        let coord = Coordinate::parse("fixture:constants:1").unwrap();
        let module_origin = crate::classpath::maven_origin_id(&coord);
        for (tag, key_origin, selected_binary) in [
            (
                b"|local-jar-java-methods-v1|".as_slice(),
                origin.as_str(),
                None,
            ),
            (
                b"|module-jar-java-methods-v1|".as_slice(),
                module_origin.as_str(),
                Some(origin.as_str()),
            ),
        ] {
            let mut old = Sha256::new();
            old.update(b"v29-binary-shadow-annotation-facts");
            old.update(tag);
            old.update(key_origin.as_bytes());
            if let Some(binary) = selected_binary {
                old.update(b"|");
                old.update(binary.as_bytes());
            }
            let fingerprint = format!("{:x}", old.finalize());
            symcache_store(&fingerprint, &legacy);
            let cached = symcache_load(&fingerprint, &legacy_dir).unwrap();
            assert_eq!(cached.len(), 4);
            assert!(!cached
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| { symbol.name == "EMPTY_SOURCE_PATH_REF_DIR" }));
        }
        let mut previous = None;
        for _ in 0..2 {
            let batches = resolve_local_jar_stubs_for_origin(
                &jar,
                &origin,
                &root.join("extract"),
                &mut kotlin,
                &mut java,
            )
            .unwrap();
            assert_eq!(batches.len(), 4);
            let coord = Coordinate::parse("fixture:constants:1").unwrap();
            let module = resolve_module_binary_stubs(
                &coord,
                &[(jar.clone(), origin.clone())],
                &root.join("module-extract"),
                &mut kotlin,
                &mut java,
            )
            .unwrap();
            assert!(module.iter().all(|batch| {
                let owner = batch.origin.as_ref().unwrap();
                owner.origin_id == crate::classpath::maven_origin_id(&coord)
                    && owner.representation == SourceRepresentation::Stub
            }));
            assert!(module
                .iter()
                .flat_map(|batch| &batch.symbols)
                .any(|symbol| {
                    symbol.name == "EMPTY_SOURCE_PATH_REF_DIR"
                        && symbol.kind == ktcore::symbol::SymbolKind::Property
                }));
            assert!(batches.iter().all(|batch| {
                let owner = batch.origin.as_ref().unwrap();
                owner.origin_id == origin && owner.representation == SourceRepresentation::Stub
            }));
            let encoded = bincode::serialize(&batches).unwrap();
            if let Some(previous) = &previous {
                assert_eq!(&encoded, previous);
            }
            previous = Some(encoded);
            let mut workspace = crate::workspace::Workspace::new();
            for batch in batches {
                assert!(workspace.index_dependency_batch(batch));
            }
            workspace.bump_index_revision();
            for (key, source, needle, expected, absent) in [
                (
                    "Use.java",
                    "import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder; class Use { void f() { JavaCompilerArgumentsBuilder.EMPTY_SOURCE; } }",
                    "JavaCompilerArgumentsBuilder.EMPTY_SOURCE",
                    "EMPTY_SOURCE_PATH_REF_DIR",
                    "withheld",
                ),
                (
                    "Use.kt",
                    "import org.gradle.api.internal.tasks.compile.JavaCompilerArgumentsBuilder\nfun f() { JavaCompilerArgumentsBuilder.EMPTY_SOURCE }\n",
                    "JavaCompilerArgumentsBuilder.EMPTY_SOURCE",
                    "EMPTY_SOURCE_PATH_REF_DIR",
                    "withheld",
                ),
                (
                    "Use.java",
                    "import pluginfixture.Person; class Use { void f(Person person) { person.ge; } }",
                    "person.ge",
                    "getName",
                    "Person",
                ),
                (
                    "Use.kt",
                    "import pluginfixture.Person\nfun f(person: Person) { person.ge }\n",
                    "person.ge",
                    "getName",
                    "Person",
                ),
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.ro; } }",
                    "factory.ro",
                    "rows",
                    "generic",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f(factory: Factory) { factory.ro }\n",
                    "factory.ro",
                    "rows",
                    "generic",
                ),
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.co; } }",
                    "factory.co",
                    "counts",
                    "hidden",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f(factory: Factory) { factory.co }\n",
                    "factory.co",
                    "counts",
                    "hidden",
                ),
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.an; } }",
                    "factory.an",
                    "annotated",
                    "generic",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f(factory: Factory) { factory.an }\n",
                    "factory.an",
                    "annotated",
                    "generic",
                ),
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f(Factory factory) { factory.withArg; } }",
                    "factory.withArg",
                    "withArgument",
                    "hidden",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f(factory: Factory) { factory.withArg }\n",
                    "factory.withArg",
                    "withArgument",
                    "hidden",
                ),
                (
                    "Use.java",
                    "import arrayfixture.Factory; class Use { void f() { Factory.st; } }",
                    "Factory.st",
                    "staticRows",
                    "Factory",
                ),
                (
                    "Use.kt",
                    "import arrayfixture.Factory\nfun f() { Factory.st }\n",
                    "Factory.st",
                    "staticRows",
                    "Factory",
                ),
            ] {
                workspace.open(key, source.to_string());
                let offset = source.find(needle).unwrap() + needle.len();
                let query = workspace.completion_query(key, offset).unwrap();
                let names: Vec<_> = query
                    .candidates
                    .iter()
                    .map(|item| item.label.as_str())
                    .collect();
                assert!(
                    names.contains(&expected),
                    "{key} {needle}: {} {:?} {names:?}",
                    query.status_label(),
                    query.reason_labels()
                );
                assert!(!names.contains(&absent), "{key} {needle}: {names:?}");
                assert_eq!(workspace.doc_text(key).as_deref(), Some(source));
            }
        }
        write_sources_jar(&jar, &[("api/Changed.class", "")]);
        assert!(resolve_local_jar_stubs_for_origin(
            &jar,
            &origin,
            &root.join("extract"),
            &mut kotlin,
            &mut java
        )
        .is_err());
        let next =
            resolve_local_jar_stubs(&jar, &root.join("extract"), &mut kotlin, &mut java).unwrap();
        assert!(!next
            .iter()
            .flat_map(|batch| &batch.symbols)
            .any(|symbol| symbol.name == "rows"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn local_stub_origin_rejects_binary_changes_after_discovery() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_local_stub_origin_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("local.jar");
        // The existing shallow fallback reads class names; it does not claim bytecode members.
        write_sources_jar(&jar, &[("api/Only.class", "")]);
        let captured = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for _ in 0..2 {
            let batches = resolve_local_jar_stubs_for_origin(
                &jar,
                &captured,
                &tmp.join("extract"),
                &mut kotlin,
                &mut java,
            )
            .unwrap();
            assert_eq!(batches.len(), 1);
            assert!(batches[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "Only"));
            assert_eq!(batches[0].origin.as_ref().unwrap().origin_id, captured);
            assert_eq!(
                batches[0].origin.as_ref().unwrap().representation,
                SourceRepresentation::Stub
            );
        }
        write_sources_jar(&jar, &[("api/ChangedAfterSnapshot.class", "")]);
        assert_ne!(crate::classpath::file_origin_id(&jar), captured);
        assert!(resolve_local_jar_stubs_for_origin(
            &jar,
            &captured,
            &tmp.join("extract"),
            &mut kotlin,
            &mut java
        )
        .is_err());
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn extracted_source_errors_do_not_publish_partial_libraries() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_extracted_source_errors_{}",
            std::process::id()
        ));
        let source = LibrarySource {
            dest: root.join("sources"),
            jar: None,
            fingerprint: None,
            origin_id: "test-extracted-library".into(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        assert!(resolve_library_source(&source, &mut kotlin, &mut java).is_err());
        fs::create_dir_all(&source.dest).unwrap();
        fs::write(
            source.dest.join("Good.java"),
            "package api; public class Good {}",
        )
        .unwrap();
        fs::write(source.dest.join("Bad.java"), b"\xff").unwrap();
        assert!(resolve_library_source(&source, &mut kotlin, &mut java).is_err());
        fs::write(
            source.dest.join("Bad.java"),
            "package api; public class Bad {}",
        )
        .unwrap();
        let batches = resolve_library_source(&source, &mut kotlin, &mut java).unwrap();
        assert_eq!(batches.len(), 2);
        assert!(batches
            .iter()
            .all(|batch| batch.origin.as_ref().unwrap().origin_id == source.origin_id));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unreadable_source_entries_do_not_publish_or_cache_partial_libraries() {
        use std::io::Write;
        let tmp = std::env::temp_dir().join(format!(
            "ktlsp_partial_source_archive_{}",
            std::process::id()
        ));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api-sources.jar");
        let mut archive = zip::ZipWriter::new(fs::File::create(&jar).unwrap());
        for (name, body) in [
            (
                "api/Good.java",
                b"package api; public class Good {}".as_slice(),
            ),
            ("api/Bad.java", b"\xff".as_slice()),
        ] {
            archive
                .start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(body).unwrap();
        }
        archive.finish().unwrap();
        let mut source = LibrarySource {
            dest: tmp.join("extract/library"),
            jar: Some(jar.clone()),
            fingerprint: jar_fingerprint(&jar),
            origin_id: "test-library".into(),
            policy: Ok(SourceTextPolicy::default()),
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for _ in 0..2 {
            let error = resolve_library_source(&source, &mut kotlin, &mut java)
                .err()
                .expect("an unreadable entry cannot leave a partial library or cache hit");
            let reason = format!("{error:#}");
            assert!(
                reason.contains("Bad.java") && reason.contains("UTF-8"),
                "the actual decode failure must be reached: {reason}"
            );
            assert!(!symcache_dir()
                .join(format!(
                    "{}.zst",
                    source
                        .policy
                        .as_ref()
                        .unwrap()
                        .cache_key(source.fingerprint.as_ref().unwrap())
                ))
                .exists());
            assert!(
                resolve_jdk_sources(&jar, &tmp.join("extract/jdk"), &mut kotlin, &mut java)
                    .is_empty(),
                "the single-threaded JDK path must also reject partial source archives"
            );
        }
        write_sources_jar(
            &jar,
            &[
                ("api/Good.java", "package api; public class Good {}"),
                ("api/Bad.java", "package api; public class Bad {}"),
            ],
        );
        source.fingerprint = jar_fingerprint(&jar);
        for _ in 0..2 {
            assert_eq!(
                resolve_library_source(&source, &mut kotlin, &mut java)
                    .unwrap()
                    .len(),
                2,
                "a failed parse must not cache an empty result that hides repaired sources"
            );
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn local_jar_reads_adjacent_sources_with_members_and_generic_inheritance() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_local_jar_sources_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api.jar");
        write_sources_jar(&jar, &[("api/Lookup.class", ""), ("api/Getter.class", "")]);
        write_sources_jar(
            &tmp.join("api-sources.jar"),
            &[
                (
                    "api/Getter.java",
                    "package api; public interface Getter<T> { T getOrThrow(); }",
                ),
                (
                    "api/Lookup.java",
                    "package api; public interface Lookup<T> extends Getter<T> {}",
                ),
            ],
        );
        let captured = crate::classpath::file_origin_id(&jar);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        for _ in 0..2 {
            let batches = resolve_local_jar_stubs_for_origin(
                &jar,
                &captured,
                &tmp.join("extract"),
                &mut kotlin,
                &mut java,
            )
            .unwrap();
            let symbols: Vec<_> = batches.iter().flat_map(|batch| &batch.symbols).collect();
            let lookup = symbols
                .iter()
                .find(|symbol| symbol.name == "Lookup")
                .unwrap();
            assert_eq!(lookup.type_params, ["T"]);
            assert_eq!(lookup.supertypes, ["Getter"]);
            assert_eq!(lookup.supertype_refs.len(), 1);
            assert_eq!(lookup.supertype_refs[0].name, "Getter");
            assert_eq!(lookup.supertype_refs[0].args.len(), 1);
            assert_eq!(lookup.supertype_refs[0].args[0].name, "T");
            assert!(symbols.iter().any(|symbol| symbol.name == "getOrThrow"));
            assert!(batches.iter().all(|batch| {
                let origin = batch.origin.as_ref().unwrap();
                origin.origin_id == captured
                    && origin.representation == SourceRepresentation::Source
            }));
            let source = batches
                .iter()
                .find(|batch| batch.file.ends_with("/Getter.java"))
                .unwrap();
            assert!(materialize_source_file(Path::new(&source.file)));
            let text = fs::read_to_string(&source.file).unwrap();
            let method = source
                .symbols
                .iter()
                .find(|symbol| symbol.name == "getOrThrow")
                .unwrap();
            assert_eq!(&text[method.start_byte..method.end_byte], "getOrThrow");
        }
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn local_jar_source_only_updates_and_removal_invalidate_origin_and_cache() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_local_source_refresh_{}", std::process::id()));
        fs::create_dir_all(&tmp).unwrap();
        let jar = tmp.join("api.jar");
        let sources = tmp.join("api-sources.jar");
        write_sources_jar(&jar, &[("api/Only.class", "")]);
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let without_sources = crate::classpath::file_origin_id(&jar);
        let mut previous = without_sources.clone();
        for (nanos, member) in [(1_000_000, "first"), (2_000_000, "other")] {
            write_sources_jar(
                &sources,
                &[(
                    "api/Only.java",
                    &format!("package api; public class Only {{ public void {member}() {{}} }}"),
                )],
            );
            fs::File::options()
                .write(true)
                .open(&sources)
                .unwrap()
                .set_modified(std::time::UNIX_EPOCH + Duration::new(1_700_000_000, nanos))
                .unwrap();
            let current = crate::classpath::file_origin_id(&jar);
            assert_ne!(
                current, previous,
                "source-only edits must invalidate the artifact"
            );
            assert!(resolve_local_jar_stubs_for_origin(
                &jar,
                &previous,
                &tmp.join("extract"),
                &mut kotlin,
                &mut java,
            )
            .is_err());
            for _ in 0..2 {
                let batches = resolve_local_jar_stubs_for_origin(
                    &jar,
                    &current,
                    &tmp.join("extract"),
                    &mut kotlin,
                    &mut java,
                )
                .unwrap();
                let names: Vec<_> = batches
                    .iter()
                    .flat_map(|batch| &batch.symbols)
                    .map(|symbol| symbol.name.as_str())
                    .collect();
                assert!(
                    names.contains(&member),
                    "cold and warm results must contain {member}"
                );
                assert!(!names.contains(&if member == "first" { "other" } else { "first" }));
            }
            previous = current;
        }
        fs::remove_file(sources).unwrap();
        assert_eq!(crate::classpath::file_origin_id(&jar), without_sources);
        assert!(resolve_local_jar_stubs_for_origin(
            &jar,
            &previous,
            &tmp.join("extract"),
            &mut kotlin,
            &mut java,
        )
        .is_err());
        let fallback =
            resolve_local_jar_stubs(&jar, &tmp.join("extract"), &mut kotlin, &mut java).unwrap();
        assert!(!fallback.is_empty());
        assert!(fallback.iter().all(|batch| {
            batch.origin.as_ref().unwrap().representation == SourceRepresentation::Stub
                && batch.symbols.iter().all(|symbol| symbol.name == "Only")
        }));
        write_sources_jar(
            &tmp.join("api-sources.jar"),
            &[(
                "api/Only.java",
                "package api; public class Only { public void first() {} }",
            )],
        );
        fs::remove_file(&jar).unwrap();
        assert!(
            resolve_local_jar_stubs(&jar, &tmp.join("extract"), &mut kotlin, &mut java).is_err(),
            "an orphaned source jar must not add a missing binary dependency"
        );
        fs::remove_dir_all(tmp).unwrap();
    }

    #[test]
    fn skips_injection_for_non_gradle_dir() {
        let mut coords = Vec::new();
        inject_stdlib(&mut coords, false);
        assert!(
            coords.is_empty(),
            "loose-file dirs must not auto-index stdlib"
        );
    }

    #[test]
    fn build_file_coordinates_infer_platform_managed_literals() {
        let tmp = std::env::temp_dir().join(format!("ktlsp_build_coords_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        let repos = Repos {
            gradle_cache: tmp.join("gradle-cache"),
            m2: tmp.join("m2"),
            central_base: "https://example.invalid".to_string(),
            download_dir: tmp.join("downloads"),
            allow_download: false,
        };
        let text = r#"
dependencies {
    api(platform("software.amazon.awssdk:bom:2.31.17"))
    api("software.amazon.awssdk:s3")
    implementation("org.apache.rocketmq:rocketmq-tools:5.3.0")
    implementation(project(":shared-common"))
}
"#;
        let labels: Vec<_> = parse_build_file_coordinates(text, &repos)
            .into_iter()
            .map(|coord| coord.label())
            .collect();
        assert!(labels.contains(&"software.amazon.awssdk:bom:2.31.17".to_string()));
        assert!(labels.contains(&"software.amazon.awssdk:s3:2.31.17".to_string()));
        assert!(labels.contains(&"org.apache.rocketmq:rocketmq-tools:5.3.0".to_string()));
        assert!(!labels.iter().any(|label| label.contains("shared-common")));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn coordinates_from_build_files_walks_gradle_files() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_build_file_walk_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("module")).unwrap();
        fs::write(
            root.join("module/build.gradle.kts"),
            r#"
dependencies {
    api(platform("software.amazon.awssdk:bom:2.31.17"))
    api("software.amazon.awssdk:s3")
}
"#,
        )
        .unwrap();
        let repos = Repos {
            gradle_cache: root.join("gradle-cache"),
            m2: root.join("m2"),
            central_base: "https://example.invalid".to_string(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };

        let labels: Vec<_> = coordinates_from_build_files(&root, &repos)
            .into_iter()
            .map(|coord| coord.label())
            .collect();
        assert!(labels.contains(&"software.amazon.awssdk:s3:2.31.17".to_string()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn coordinates_include_nested_gradle_catalogs() {
        let root = std::env::temp_dir().join(format!("ktlsp_catalogs_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("gradle")).unwrap();
        fs::create_dir_all(root.join("Android/gradle")).unwrap();
        fs::create_dir_all(root.join("build/gradle")).unwrap();
        fs::create_dir_all(root.join(".worktrees/feature/gradle")).unwrap();
        fs::write(root.join("settings.gradle.kts"), "").unwrap();
        fs::write(
            root.join("gradle/libs.versions.toml"),
            r#"
[versions]
root = "1.0"
[libraries]
root-lib = { module = "com.example:root-lib", version.ref = "root" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("Android/gradle/libs.versions.toml"),
            r#"
[versions]
android = "2.0"
[libraries]
android-lib = { module = "com.example:android-lib", version.ref = "android" }
"#,
        )
        .unwrap();
        fs::write(
            root.join("build/gradle/libs.versions.toml"),
            r#"
[libraries]
generated = "com.example:generated:9.9"
"#,
        )
        .unwrap();
        fs::write(
            root.join(".worktrees/feature/gradle/libs.versions.toml"),
            r#"
[libraries]
worktree = "com.example:worktree:9.9"
"#,
        )
        .unwrap();

        let labels: Vec<String> = coordinates_for_root(&root)
            .into_iter()
            .map(|c| c.label())
            .collect();
        assert!(labels.contains(&"com.example:root-lib:1.0".to_string()));
        assert!(labels.contains(&"com.example:android-lib:2.0".to_string()));
        assert!(!labels.iter().any(|l| l.contains("generated")));
        assert!(!labels.iter().any(|l| l.contains("worktree")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn coordinate_source_identity_matches_jvm_variant_fallback() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_source_identity_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let root_coord = coord("io.ktor:ktor-client-apache5:3.5.0");
        let jvm_coord = coord("io.ktor:ktor-client-apache5-jvm:3.5.0");
        let gradle_cache = tmp.join("gradle/caches");

        write_sources_jar(
            &gradle_cache
                .join("modules-2/files-2.1")
                .join(&root_coord.group)
                .join(&root_coord.artifact)
                .join(&root_coord.version)
                .join("feedface")
                .join(root_coord.sources_jar_name()),
            &[("META-INF/MANIFEST.MF", "Manifest-Version: 1.0\n")],
        );
        let jvm_jar = gradle_cache
            .join("modules-2/files-2.1")
            .join(&jvm_coord.group)
            .join(&jvm_coord.artifact)
            .join(&jvm_coord.version)
            .join("deadbeef")
            .join(jvm_coord.sources_jar_name());
        write_sources_jar(
            &jvm_jar,
            &[(
                "io/ktor/client/engine/apache5/Apache5.kt",
                "package io.ktor.client.engine.apache5\nobject Apache5\n",
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

        let root_source = coordinate_source(&root_coord, &repos, &extract_root).unwrap();
        let jvm_source = coordinate_source(&jvm_coord, &repos, &extract_root).unwrap();

        assert_eq!(root_source.identity(), jvm_source.identity());

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn import_fqn_resolution_walks_starter_transitive_sources() {
        let tmp =
            std::env::temp_dir().join(format!("ktlsp_import_fqn_sources_{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let gradle_cache = tmp.join("gradle/caches");
        let starter =
            coord("com.alibaba.cloud:spring-cloud-starter-alibaba-nacos-config:2025.0.0.0");
        let implementation = coord("com.alibaba.cloud:spring-alibaba-nacos-config:2025.0.0.0");

        write_sources_jar(
            &gradle_cache
                .join("modules-2/files-2.1")
                .join(&starter.group)
                .join(&starter.artifact)
                .join(&starter.version)
                .join("feedface")
                .join(starter.sources_jar_name()),
            &[
                (
                    "com/alibaba/cloud/nacos/NacosConfigSpringCloudAutoConfiguration.java",
                    "package com.alibaba.cloud.nacos;\npublic class NacosConfigSpringCloudAutoConfiguration {}\n",
                ),
                (
                    "META-INF/maven/com.alibaba.cloud/spring-cloud-starter-alibaba-nacos-config/pom.xml",
                    r#"
<project>
  <dependencies>
    <dependency>
      <groupId>com.alibaba.cloud</groupId>
      <artifactId>spring-alibaba-nacos-config</artifactId>
      <version>2025.0.0.0</version>
    </dependency>
  </dependencies>
</project>
"#,
                ),
            ],
        );
        write_sources_jar(
            &gradle_cache
                .join("modules-2/files-2.1")
                .join(&implementation.group)
                .join(&implementation.artifact)
                .join(&implementation.version)
                .join("deadbeef")
                .join(implementation.sources_jar_name()),
            &[(
                "com/alibaba/cloud/nacos/NacosConfigManager.java",
                "package com.alibaba.cloud.nacos;\npublic class NacosConfigManager {}\n",
            )],
        );
        let repos = Repos {
            gradle_cache,
            m2: tmp.join("m2"),
            central_base: "http://127.0.0.1:0/unused".to_string(),
            download_dir: tmp.join("dl"),
            allow_download: false,
        };
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let batches = resolve_import_fqn_from_coordinates(
            "com.alibaba.cloud.nacos.NacosConfigManager",
            vec![starter],
            &repos,
            &tmp.join("extracted"),
            &mut kotlin,
            &mut java,
        );

        assert_eq!(batches.len(), 1);
        assert!(batches[0].file.ends_with("NacosConfigManager.java"));
        assert!(batches[0].symbols.iter().any(
            |sym| sym.name == "NacosConfigManager" && sym.package == "com.alibaba.cloud.nacos"
        ));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn android_compile_sdk_reads_nested_version_catalog() {
        let root = std::env::temp_dir().join(format!("ktlsp_android_api_{}", std::process::id()));
        let catalog = root.join("apps/mobile/android/gradle/libs.versions.toml");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        fs::write(&catalog, "[versions]\ncompileSdk = \"36\"\n").unwrap();
        assert_eq!(android_compile_sdk(&root), Some(36));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn compile_sdk_literal_rejects_version_catalog_references() {
        assert_eq!(compile_sdk_literal("compileSdk = 36"), Some(36));
        assert_eq!(compile_sdk_literal("compileSdk 35"), Some(35));
        assert_eq!(compile_sdk_literal("compileSdkVersion(34)"), Some(34));
        assert_eq!(compile_sdk_literal("compileSdk = \"33\""), Some(33));
        assert_eq!(
            compile_sdk_literal("compileSdk = libs.versions.androidApi2025"),
            None
        );
    }

    #[test]
    fn android_sdk_does_not_substitute_a_different_installed_api() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_android_sdk_version_{}", std::process::id()));
        let installed = root.join("sources/android-35/android");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&installed).unwrap();
        assert_eq!(android_source_location_from_path(&root, Some(36)), None);
        assert_eq!(
            android_source_location_from_path(&root, None),
            Some(AndroidSourceLocation::Directory(
                root.join("sources/android-35")
            ))
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn android_repository_metadata_selects_exact_api_archive() {
        let xml = r#"
<sdk:sdk-repository>
  <remotePackage path="sources;android-36"><archives><archive><complete>
    <url>source-36_r01.zip</url>
  </complete></archive></archives></remotePackage>
  <remotePackage path="sources;android-35"><archives><archive><complete>
    <url>source-35_r01.zip</url>
  </complete></archive></archives></remotePackage>
</sdk:sdk-repository>
"#;
        assert_eq!(
            android_source_archive_name(xml, 36).as_deref(),
            Some("source-36_r01.zip")
        );
        assert_eq!(android_source_archive_name(xml, 34), None);
    }

    #[test]
    fn android_import_resolution_extracts_only_matching_source_files() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_android_sources_{}", std::process::id()));
        let archive = root.join("source-36_r01.zip");
        let extract = root.join("extracted");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_sources_jar(
            &archive,
            &[
                (
                    "src/android/graphics/Bitmap.java",
                    "package android.graphics; public final class Bitmap { public int getWidth() { return 0; } }",
                ),
                (
                    "src/android/os/Handler.java",
                    "package android.os; public class Handler {}",
                ),
            ],
        );
        let suffixes = android_source_suffixes("android.graphics.Bitmap")
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut java = JavaParser::new();
        let batches = resolve_android_imports_from_location(
            AndroidSourceLocation::Archive(archive),
            &suffixes,
            &extract,
            &mut java,
        );
        assert_eq!(batches.len(), 1);
        assert!(Path::new(&batches[0].file).ends_with("android/graphics/Bitmap.java"));
        assert!(batches[0]
            .symbols
            .iter()
            .any(|symbol| symbol.package == "android.graphics" && symbol.name == "Bitmap"));
        assert!(!WalkDir::new(&extract)
            .into_iter()
            .filter_map(Result::ok)
            .any(|entry| entry.path().ends_with("android/os/Handler.java")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_android_import_resolution_shares_lazy_extraction_safely() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_android_sources_concurrent_{}",
            std::process::id()
        ));
        let archive = root.join("source-36_r01.zip");
        let extract = root.join("extracted");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        write_sources_jar(
            &archive,
            &[(
                "src/android/graphics/Bitmap.java",
                "package android.graphics; public final class Bitmap {}",
            )],
        );
        let suffixes = android_source_suffixes("android.graphics.Bitmap")
            .into_iter()
            .collect::<BTreeSet<_>>();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let handles = (0..4)
            .map(|_| {
                let archive = archive.clone();
                let extract = extract.clone();
                let suffixes = suffixes.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    let mut java = JavaParser::new();
                    barrier.wait();
                    resolve_android_imports_from_location(
                        AndroidSourceLocation::Archive(archive),
                        &suffixes,
                        &extract,
                        &mut java,
                    )
                })
            })
            .collect::<Vec<_>>();
        for handle in handles {
            let batches = handle.join().unwrap();
            assert_eq!(batches.len(), 1);
            assert!(Path::new(&batches[0].file).ends_with("android/graphics/Bitmap.java"));
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_root_finds_root_level_project() {
        let root = std::env::temp_dir().join(format!("ktlsp_gradle_root_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("build.gradle.kts"), "").unwrap();
        assert_eq!(gradle_root(&root), Some(root.clone()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn catalog_paths_include_direct_node_module_gradle_catalogs() {
        let root = std::env::temp_dir().join(format!("ktlsp_node_catalog_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let catalog = root.join("node_modules/react-native/gradle/libs.versions.toml");
        fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        fs::write(&catalog, "[versions]\nannotation = \"1.0\"\n").unwrap();

        assert!(catalog_paths(&root).contains(&catalog));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_root_finds_nested_module_when_root_is_empty() {
        let root = std::env::temp_dir().join(format!("ktlsp_gradle_nested_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("build.gradle"), "").unwrap();
        fs::write(app.join("gradlew"), "").unwrap();
        fs::write(app.join("gradlew.bat"), "").unwrap();
        assert_eq!(gradle_root(&root), Some(app));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_root_finds_android_build_nested_in_monorepo() {
        let root =
            std::env::temp_dir().join(format!("ktlsp_gradle_monorepo_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let android = root.join("apps/mobile/android");
        let standalone = root.join("modules/example/android");
        fs::create_dir_all(&android).unwrap();
        fs::create_dir_all(&standalone).unwrap();
        fs::write(android.join("settings.gradle"), "").unwrap();
        fs::write(android.join("gradlew"), "").unwrap();
        fs::write(android.join("gradlew.bat"), "").unwrap();
        fs::write(standalone.join("build.gradle"), "").unwrap();
        assert_eq!(gradle_root(&root), Some(android));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_root_prefers_gradlew_over_build_file() {
        let root = std::env::temp_dir().join(format!("ktlsp_gradle_multi_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let a = root.join("a");
        let b = root.join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("build.gradle"), "").unwrap();
        fs::write(b.join("build.gradle"), "").unwrap();
        fs::write(b.join("gradlew"), "").unwrap();
        fs::write(b.join("gradlew.bat"), "").unwrap();
        assert_eq!(gradle_root(&root), Some(b));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn gradle_root_returns_none_when_no_gradle_project() {
        let root = std::env::temp_dir().join(format!("ktlsp_gradle_none_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        assert_eq!(gradle_root(&root), None);
        let _ = fs::remove_dir_all(&root);
    }
    #[test]
    fn source_encoding_policy_only_changes_invalidate_lazy_text_and_warm_symbols() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_encoding_policy_refresh_{}",
            std::process::id()
        ));
        let repos = Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let coordinate = coord("fixture:encoding:1");
        let base = repos.m2.join("fixture/encoding/1");
        fs::create_dir_all(&base).unwrap();
        let archive = base.join("encoding-1-sources.jar");
        let pom = base.join("encoding-1.pom");
        let raw = "// Bj\u{f6}rn\npackage api; public class Useful { public void target() {} }\n";
        write_sources_jar(&archive, &[("api/Useful.java", raw)]);
        let archive_identity = jar_fingerprint(&archive).unwrap();
        let metadata = |encoding| {
            format!(
                "<project><groupId>fixture</groupId><artifactId>encoding</artifactId><version>1</version><properties><project.build.sourceEncoding>{encoding}</project.build.sourceEncoding></properties></project>"
            )
        };
        fs::write(&pom, metadata("UTF-8")).unwrap();
        let mut kotlin = KotlinParser::new();
        let mut java = JavaParser::new();
        let extract = root.join("extracted");
        let first = coordinate_source(&coordinate, &repos, &extract).unwrap();
        let first_key = first
            .policy
            .as_ref()
            .unwrap()
            .cache_key(first.fingerprint.as_ref().unwrap());
        let mut file = None;
        for _ in 0..2 {
            let batches = resolve_library_source(&first, &mut kotlin, &mut java).unwrap();
            assert_eq!(batches.len(), 1);
            file = Some(PathBuf::from(&batches[0].file));
            assert!(batches[0]
                .symbols
                .iter()
                .any(|symbol| symbol.name == "target"));
        }
        let file = file.unwrap();
        assert!(symcache_dir().join(format!("{first_key}.zst")).exists());
        assert!(materialize_source_file(&file));
        assert_eq!(fs::read_to_string(&file).unwrap(), raw);
        fs::write(&pom, metadata("ISO-8859-1")).unwrap();
        assert_eq!(
            jar_fingerprint(&archive).unwrap(),
            archive_identity,
            "only metadata changed"
        );
        assert!(
            !materialize_source_file(&file),
            "captured metadata cannot silently change decoding"
        );
        assert!(
            !file.exists(),
            "stale text must not survive metadata rejection"
        );
        assert!(resolve_library_source(&first, &mut kotlin, &mut java).is_err());
        let second = coordinate_source(&coordinate, &repos, &extract).unwrap();
        assert_ne!(first.identity(), second.identity());
        let expected = raw
            .as_bytes()
            .iter()
            .copied()
            .map(char::from)
            .collect::<String>();
        for _ in 0..2 {
            let batches = resolve_library_source(&second, &mut kotlin, &mut java).unwrap();
            let member = batches[0]
                .symbols
                .iter()
                .find(|symbol| symbol.name == "target")
                .unwrap();
            assert_eq!(&expected[member.start_byte..member.end_byte], "target");
            assert!(materialize_source_file(&file));
            assert_eq!(fs::read_to_string(&file).unwrap(), expected);
        }
        // Pre-policy markers must not retain text decoded under an unknown policy.
        fs::write(
            second.dest.join(SOURCE_JAR_MARKER),
            serde_json::to_vec(
                &serde_json::json!({"jar": archive, "fingerprint": archive_identity}),
            )
            .unwrap(),
        )
        .unwrap();
        fs::write(&file, "stale legacy file").unwrap();
        resolve_library_source(&second, &mut kotlin, &mut java).unwrap();
        assert!(!file.exists());
        assert!(materialize_source_file(&file));
        assert_eq!(fs::read_to_string(&file).unwrap(), expected);
        fs::write(&pom, metadata("Windows-1252")).unwrap();
        let unsupported = coordinate_source(&coordinate, &repos, &extract).unwrap();
        let reason = resolve_library_source(&unsupported, &mut kotlin, &mut java)
            .err()
            .unwrap()
            .to_string();
        assert!(reason.contains("source encoding metadata"), "{reason}");
        assert!(!file.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_encoding_uses_selected_jvm_gav_not_requested_metadata() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_encoding_selected_jvm_{}",
            std::process::id()
        ));
        let repos = Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };
        let requested = coord("fixture:encoding:1");
        for (artifact, encoding) in [("encoding", "UTF8"), ("encoding-jvm", "ISO-8859-1")] {
            let base = repos.m2.join(format!("fixture/{artifact}/1"));
            fs::create_dir_all(&base).unwrap();
            fs::write(base.join(format!("{artifact}-1.pom")), format!("<project><groupId>fixture</groupId><artifactId>{artifact}</artifactId><version>1</version><properties><project.build.sourceEncoding>{encoding}</project.build.sourceEncoding></properties></project>")).unwrap();
        }
        let archive = repos
            .m2
            .join("fixture/encoding-jvm/1/encoding-jvm-1-sources.jar");
        let payload = b"// Bj\xf6rn\npackage api; public class Useful {}\n";
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        writer
            .start_file("api/Useful.java", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(payload).unwrap();
        writer.finish().unwrap();
        let source = coordinate_source(&requested, &repos, &root.join("extracted")).unwrap();
        assert_eq!(source.jar.as_ref(), Some(&archive));
        assert_eq!(
            source.policy.as_ref().unwrap().java,
            JavaSourceEncoding::Iso88591
        );
        let batches =
            resolve_library_source(&source, &mut KotlinParser::new(), &mut JavaParser::new())
                .unwrap();
        assert_eq!(batches.len(), 1);
        assert!(materialize_source_file(Path::new(&batches[0].file)));
        assert_eq!(
            fs::read_to_string(&batches[0].file).unwrap(),
            payload.iter().copied().map(char::from).collect::<String>()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_encoding_suffix_seed_never_publishes_invalid_utf8_or_bad_crc() {
        let root = std::env::temp_dir().join(format!("ktlsp_encoding_seed_{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let archive = root.join("src.zip");
        let dest = root.join("seed");
        let payload = b"// invalid \xff\nclass Bad {}\n";
        let mut writer = zip::ZipWriter::new(fs::File::create(&archive).unwrap());
        writer
            .start_file(
                "java.base/api/Bad.java",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        writer.write_all(payload).unwrap();
        writer.finish().unwrap();
        let suffixes = BTreeSet::from(["api/Bad.java".into()]);
        assert!(extract_sources_by_suffix(&archive, &dest, &suffixes).is_empty());
        assert!(!dest.join("java.base/api/Bad.java").exists());
        let mut bytes = fs::read(&archive).unwrap();
        let position = bytes
            .windows(payload.len())
            .position(|part| part == payload)
            .unwrap();
        bytes[position + payload.len() - 1] ^= 1;
        fs::write(&archive, bytes).unwrap();
        assert!(extract_sources_by_suffix(&archive, &dest, &suffixes).is_empty());
        assert!(!dest.join("java.base/api/Bad.java").exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn coordinate_source_rejects_unsafe_fields_before_legacy_fallback() {
        let root = std::env::temp_dir().join(format!(
            "ktlsp_coordinate_boundary_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let extract = root.join("extracted");
        fs::create_dir_all(&extract).unwrap();
        let outside = root.join("outside").join("fixture").join("1");
        fs::create_dir_all(&outside).unwrap();
        let sentinel = outside.join("sentinel.txt");
        fs::write(&sentinel, "must remain intact").unwrap();
        let repos = Repos {
            gradle_cache: root.join("gradle"),
            m2: root.join("m2"),
            central_base: "http://127.0.0.1:0/unused".into(),
            download_dir: root.join("downloads"),
            allow_download: false,
        };

        let valid = coord("fixture:library:1+build");
        let valid_dest = extract
            .join(&valid.group)
            .join(&valid.artifact)
            .join(&valid.version);
        fs::create_dir_all(&valid_dest).unwrap();
        assert!(coordinate_source(&valid, &repos, &extract).is_some());

        let unsafe_coordinate = Coordinate {
            group: "../outside".into(),
            artifact: "fixture".into(),
            version: "1".into(),
        };
        assert!(Coordinate::parse(&unsafe_coordinate.label()).is_none());
        assert!(coordinate_source(&unsafe_coordinate, &repos, &extract).is_none());
        let model = crate::classpath::ProjectSnapshot::new(Vec::new());
        assert!(
            coordinate_source_for_model(&unsafe_coordinate, &repos, &extract, Some(&model))
                .unwrap()
                .is_none()
        );
        assert_eq!(fs::read_to_string(&sentinel).unwrap(), "must remain intact");
        // This probe never passes the returned location to resolve_library_source or removes it.
        fs::remove_file(&sentinel).unwrap();
    }
}
