//! Static published-POM source text declarations, not an effective Maven/compiler model.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{ensure, Context};
use quick_xml::{
    events::{BytesDecl, BytesStart, Event},
    name::ResolveResult,
    NsReader, Reader,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::coords::Coordinate;

const VERSION: u8 = 2;
const MAX_POM: usize = 1 << 20;
const MAX_CHAIN_BYTES: usize = 4 << 20;
const MAX_PARENTS: usize = 8;
const MAX_EVENTS: usize = 50_000;
const MAX_DEPTH: usize = 128;
const MAX_PROPERTIES: usize = 512;
const MAX_SCALAR: usize = 16 << 10;
const MAVEN_NS: &[u8] = b"http://maven.apache.org/POM/4.0.0";
const SOURCE_ENCODING: &str = "project.build.sourceEncoding";
const USER_ENCODING: &str = "encoding";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) enum JavaSourceEncoding {
    #[default]
    Utf8,
    Iso88591,
}

impl JavaSourceEncoding {
    /// Exact supported decoders; no lossy UTF8 or Windows-1252 interpretation.
    pub(crate) fn decode(self, bytes: Vec<u8>) -> anyhow::Result<String> {
        match self {
            Self::Utf8 => String::from_utf8(bytes).context("invalid UTF-8 source text"),
            Self::Iso88591 => Ok(bytes.into_iter().map(char::from).collect()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct PomReceipt {
    gav: String,
    path: PathBuf,
    sha256: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct SourceTextPolicy {
    version: u8,
    pub(crate) java: JavaSourceEncoding,
    selected_gav: Option<String>,
    poms: Vec<PomReceipt>,
}

impl Default for SourceTextPolicy {
    fn default() -> Self {
        Self {
            version: VERSION,
            java: JavaSourceEncoding::Utf8,
            selected_gav: None,
            poms: Vec::new(),
        }
    }
}

impl SourceTextPolicy {
    /// The lookup is the existing cached exact-coordinate artifact lookup: no downloads/relativePath.
    pub(crate) fn resolve(
        selected: &Coordinate,
        mut cached_poms: impl FnMut(&Coordinate) -> Vec<PathBuf>,
    ) -> anyhow::Result<Self> {
        Self::resolve_with(selected, |coordinate| Ok(cached_poms(coordinate)), true)
    }

    /// Gradle metadata must come from its selected repository, including every declared parent.
    pub(crate) fn resolve_certified(
        selected: &Coordinate,
        selected_poms: impl FnMut(&Coordinate) -> anyhow::Result<Vec<PathBuf>>,
    ) -> anyhow::Result<Self> {
        Self::resolve_with(selected, selected_poms, false)
    }

    fn resolve_with(
        selected: &Coordinate,
        mut poms: impl FnMut(&Coordinate) -> anyhow::Result<Vec<PathBuf>>,
        allow_absent_root: bool,
    ) -> anyhow::Result<Self> {
        safe_coordinate(selected)?;
        let mut result = Self {
            selected_gav: Some(selected.label()),
            ..Self::default()
        };
        let mut current = selected.clone();
        let mut seen = BTreeSet::new();
        let mut chain = Vec::new();
        let mut total = 0;
        loop {
            ensure!(
                chain.len() < MAX_PARENTS,
                "POM parent chain exceeds {MAX_PARENTS}"
            );
            ensure!(seen.insert(current.label()), "POM parent cycle");
            let mut paths = poms(&current)?;
            paths.sort();
            paths.dedup();
            ensure!(paths.len() <= 3, "too many cached POM candidates");
            if paths.is_empty() {
                ensure!(
                    chain.is_empty() && allow_absent_root,
                    "source encoding POM {} unavailable",
                    current.label()
                );
                break; // No published metadata: retain strict UTF8, never guess another encoding.
            }
            let mut chosen = None;
            for path in paths {
                let bytes = read_bounded(&path)?;
                total += bytes.len();
                ensure!(total <= MAX_CHAIN_BYTES, "POM chain byte limit exceeded");
                let pom = parse(&bytes).with_context(|| format!("parse POM {}", path.display()))?;
                ensure!(
                    pom.coordinate()? == current,
                    "cached POM GAV differs from selected {}",
                    current.label()
                );
                ensure!(
                    chosen.as_ref().is_none_or(|prior| prior == &pom),
                    "conflicting cached POM declarations"
                );
                result.poms.push(PomReceipt {
                    gav: current.label(),
                    path,
                    sha256: Sha256::digest(&bytes).into(),
                });
                chosen = Some(pom);
            }
            let pom = chosen.context("no POM candidate")?;
            let parent = pom.parent()?;
            chain.push(pom);
            let Some(parent) = parent else { break };
            current = parent;
        }
        result.java = encoding(&chain)?;
        result.validate()?;
        Ok(result)
    }

    /// Captured content receipts are checked around cache/parse/materialization reads, never per keypress.
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            self.version == VERSION && self.poms.len() <= 3 * MAX_PARENTS,
            "unsupported source text policy"
        );
        ensure!(
            self.java == JavaSourceEncoding::Utf8
                || self.selected_gav.is_some() && !self.poms.is_empty(),
            "declared encoding provenance absent"
        );
        if let Some(gav) = &self.selected_gav {
            safe_coordinate(&Coordinate::parse(gav).context("invalid selected source GAV")?)?;
        }
        let mut total = 0;
        for receipt in &self.poms {
            let bytes = read_bounded(&receipt.path)?;
            total += bytes.len();
            ensure!(total <= MAX_CHAIN_BYTES, "POM chain byte limit exceeded");
            ensure!(
                <[u8; 32]>::from(Sha256::digest(bytes)) == receipt.sha256,
                "source encoding metadata changed: {}",
                receipt.path.display()
            );
        }
        Ok(())
    }

    pub(crate) fn cache_key(&self, partition: &str) -> String {
        let mut hash = Sha256::new();
        hash.update(b"ktlsp-source-text-policy-v2\0");
        hash.update(partition.as_bytes());
        hash.update(b"\0");
        // This fixed struct/ordered receipt list has no map iteration or symbol payload.
        hash.update(serde_json::to_vec(self).expect("source text policy is serializable"));
        format!("{:x}", hash.finalize())
    }
}

fn read_bounded(path: &Path) -> anyhow::Result<Vec<u8>> {
    let mut file = fs::File::open(path).with_context(|| format!("open POM {}", path.display()))?;
    let before = file.metadata()?;
    ensure!(before.len() <= MAX_POM as u64, "POM size limit exceeded");
    let mut bytes = Vec::with_capacity(before.len() as usize);
    file.by_ref()
        .take(MAX_POM as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = fs::metadata(path)?;
    ensure!(
        bytes.len() <= MAX_POM
            && before.len() == after.len()
            && before.modified()? == after.modified()?,
        "POM changed while reading or exceeded limit"
    );
    Ok(bytes)
}

fn safe_coordinate(c: &Coordinate) -> anyhow::Result<()> {
    ensure!(
        Coordinate::parse(&c.label()).as_ref() == Some(c),
        "unsupported POM coordinate"
    );
    Ok(())
}

#[derive(Default, Debug, Eq, PartialEq)]
struct Pom {
    group: Option<String>,
    artifact: Option<String>,
    version: Option<String>,
    parent_group: Option<String>,
    parent_artifact: Option<String>,
    parent_version: Option<String>,
    properties: BTreeMap<String, String>,
    ambiguous_properties: BTreeSet<String>,
    profile_properties: BTreeSet<String>,
    active_encoding: Option<String>,
    managed_encoding: Option<String>,
    unsupported_compiler_encoding: bool,
}

impl Pom {
    fn parent(&self) -> anyhow::Result<Option<Coordinate>> {
        match (
            &self.parent_group,
            &self.parent_artifact,
            &self.parent_version,
        ) {
            (None, None, None) => Ok(None),
            (Some(group), Some(artifact), Some(version)) => {
                let parent = Coordinate {
                    group: group.clone(),
                    artifact: artifact.clone(),
                    version: version.clone(),
                };
                safe_coordinate(&parent)?;
                Ok(Some(parent))
            }
            _ => anyhow::bail!("incomplete parent GAV"),
        }
    }

    fn coordinate(&self) -> anyhow::Result<Coordinate> {
        let c = Coordinate {
            group: self
                .group
                .as_ref()
                .or(self.parent_group.as_ref())
                .context("POM groupId absent")?
                .clone(),
            artifact: self
                .artifact
                .as_ref()
                .context("POM artifactId absent")?
                .clone(),
            version: self
                .version
                .as_ref()
                .or(self.parent_version.as_ref())
                .context("POM version absent")?
                .clone(),
        };
        safe_coordinate(&c)?;
        Ok(c)
    }
}

#[derive(Default)]
struct Plugin {
    group: Option<String>,
    artifact: Option<String>,
    encoding: Option<String>,
    profile: bool,
    managed: bool,
    unsupported: bool,
}
struct Frame {
    name: String,
    text: String,
    attributes: bool,
    argument_override: bool,
    children: bool,
    plugin: Option<Plugin>,
}
fn path(stack: &[Frame], expected: &[&str]) -> bool {
    stack.len() == expected.len() && stack.iter().zip(expected).all(|(a, b)| a.name == *b)
}
fn set(slot: &mut Option<String>, text: String) -> anyhow::Result<()> {
    ensure!(
        slot.replace(text).is_none(),
        "duplicate relevant POM declaration"
    );
    Ok(())
}
fn xml_chars(text: &str) -> bool {
    text.chars()
        .all(|c| matches!(c, '\t' | '\r' | '\n') || c >= ' ' && c != '\u{fffe}' && c != '\u{ffff}')
}

fn xml_declared_encoding(declaration: &BytesDecl<'_>) -> anyhow::Result<JavaSourceEncoding> {
    // BytesDecl::encoding alone returns the first occurrence and permits duplicate/order errors.
    // Validate the complete declaration with quick-xml's checked attribute iterator instead.
    ensure!(declaration.is_ascii(), "non-ASCII XML declaration");
    let start = BytesStart::from_content(std::str::from_utf8(declaration.as_ref())?, 3);
    let mut state = 0;
    let mut encoding = JavaSourceEncoding::Utf8;
    for attribute in start.attributes() {
        let attribute = attribute?;
        match (attribute.key.as_ref(), state) {
            (b"version", 0) => {
                ensure!(
                    attribute.value.as_ref() == b"1.0",
                    "unsupported XML version"
                );
                state = 1;
            }
            (b"encoding", 1) => {
                let label = attribute.value.as_ref();
                encoding = if label.eq_ignore_ascii_case(b"UTF-8")
                    || label.eq_ignore_ascii_case(b"UTF8")
                {
                    JavaSourceEncoding::Utf8
                } else if label.eq_ignore_ascii_case(b"ISO-8859-1")
                    || label.eq_ignore_ascii_case(b"ISO8859-1")
                {
                    JavaSourceEncoding::Iso88591
                } else {
                    anyhow::bail!("unsupported POM XML encoding");
                };
                state = 2;
            }
            (b"standalone", 1 | 2) => {
                ensure!(
                    matches!(attribute.value.as_ref(), b"yes" | b"no"),
                    "invalid XML standalone declaration"
                );
                state = 3;
            }
            _ => anyhow::bail!("unsupported XML declaration"),
        }
    }
    ensure!(state != 0, "XML version absent");
    Ok(encoding)
}

fn pom_text(bytes: &[u8]) -> anyhow::Result<(String, JavaSourceEncoding)> {
    let utf8_bom = bytes.starts_with(b"\xef\xbb\xbf");
    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    ensure!(!bytes.starts_with(b"\xef\xbb\xbf"), "duplicate UTF8 BOM");
    // The declaration is ASCII and can be read before interpreting any body bytes.
    // A declaration elsewhere never selects a decoder; the full parser rejects its position.
    let mut header = Reader::from_reader(bytes);
    let encoding = match header.read_event()? {
        Event::Decl(declaration) => xml_declared_encoding(&declaration)?,
        _ => JavaSourceEncoding::Utf8,
    };
    ensure!(
        !utf8_bom || encoding == JavaSourceEncoding::Utf8,
        "BOM contradicts XML encoding"
    );
    Ok((encoding.decode(bytes.to_vec())?, encoding))
}

fn parse(bytes: &[u8]) -> anyhow::Result<Pom> {
    ensure!(bytes.len() <= MAX_POM, "POM size limit exceeded");
    let (text, metadata_encoding) = pom_text(bytes)?;
    ensure!(xml_chars(&text), "invalid XML character");
    let mut reader = NsReader::from_str(&text);
    let mut stack: Vec<Frame> = Vec::new();
    let mut pom = Pom::default();
    let mut roots = 0;
    let mut declaration_allowed = true;
    let mut eof = false;
    for _ in 0..MAX_EVENTS {
        let (namespace, event) = reader.read_resolved_event()?;
        let empty = matches!(&event, Event::Empty(_));
        match event {
            Event::Start(start) | Event::Empty(start) => {
                ensure!(
                    matches!(&namespace, ResolveResult::Unbound)
                        || matches!(&namespace, ResolveResult::Bound(ns) if ns.as_ref() == MAVEN_NS),
                    "unsupported XML namespace"
                );
                let name = std::str::from_utf8(start.local_name().as_ref())?.to_owned();
                ensure!(
                    name.len() <= MAX_SCALAR && stack.len() < MAX_DEPTH,
                    "POM name/depth limit exceeded"
                );
                let argument_list = name == "compilerArgs"
                    && stack.last().is_some_and(|f| f.name == "configuration")
                    && stack.iter().any(|f| f.plugin.is_some());
                let mut attributes = false;
                let mut argument_override = false;
                for attr in start.attributes() {
                    let attr = attr?;
                    let value = attr.unescape_value()?;
                    ensure!(xml_chars(&value), "invalid escaped XML attribute character");
                    let override_attribute = argument_list
                        && attr.key.as_ref() == b"combine.self"
                        && value == "override";
                    argument_override |= override_attribute;
                    attributes |= attr.key.as_ref() != b"xmlns"
                        && !attr.key.as_ref().starts_with(b"xmlns:")
                        && !(argument_list
                            && attr.key.as_ref() == b"combine.children"
                            && value == "append")
                        && !override_attribute;
                }
                if stack.is_empty() {
                    roots += 1;
                    ensure!(roots == 1 && name == "project", "expected one project root");
                } else {
                    stack.last_mut().unwrap().children = true;
                }
                let plugin = (name == "plugin"
                    && [
                        &["project", "build", "plugins"][..],
                        &["project", "build", "pluginManagement", "plugins"][..],
                        &["project", "profiles", "profile", "build", "plugins"][..],
                        &[
                            "project",
                            "profiles",
                            "profile",
                            "build",
                            "pluginManagement",
                            "plugins",
                        ][..],
                    ]
                    .iter()
                    .any(|p| path(&stack, p)))
                .then(|| Plugin {
                    profile: stack.iter().any(|f| f.name == "profiles"),
                    managed: stack.iter().any(|f| f.name == "pluginManagement"),
                    ..Plugin::default()
                });
                stack.push(Frame {
                    name,
                    text: String::new(),
                    attributes,
                    argument_override,
                    children: false,
                    plugin,
                });
                declaration_allowed = false;
                if empty {
                    let frame = stack.pop().unwrap();
                    finish(frame, &mut stack, &mut pom)?;
                }
            }
            Event::End(end) => {
                let frame = stack.pop().context("unexpected XML end")?;
                ensure!(
                    frame.name.as_bytes() == end.local_name().as_ref(),
                    "mismatched XML end"
                );
                finish(frame, &mut stack, &mut pom)?;
            }
            Event::Text(text) => {
                let value = text.unescape()?;
                ensure!(xml_chars(&value), "invalid escaped XML character");
                if let Some(frame) = stack.last_mut() {
                    ensure!(
                        frame.text.len() + value.len() <= MAX_SCALAR,
                        "POM scalar limit exceeded"
                    );
                    frame.text.push_str(&value);
                } else {
                    ensure!(value.trim().is_empty(), "text outside project root");
                    declaration_allowed = false;
                }
            }
            Event::CData(text) => {
                let value = std::str::from_utf8(text.as_ref())?;
                let frame = stack.last_mut().context("CDATA outside project root")?;
                ensure!(
                    frame.text.len() + value.len() <= MAX_SCALAR,
                    "POM scalar limit exceeded"
                );
                frame.text.push_str(value);
            }
            Event::Decl(decl) => {
                ensure!(
                    declaration_allowed
                        && roots == 0
                        && xml_declared_encoding(&decl)? == metadata_encoding,
                    "unsupported XML declaration"
                );
                declaration_allowed = false;
            }
            Event::Comment(_) => declaration_allowed = false,
            Event::DocType(_) | Event::PI(_) => {
                anyhow::bail!("DTD/processing instructions are unsupported")
            }
            Event::Eof => {
                ensure!(roots == 1 && stack.is_empty(), "incomplete POM XML");
                eof = true;
                break;
            }
        }
    }
    ensure!(eof, "POM event limit exceeded");
    Ok(pom)
}

fn finish(frame: Frame, stack: &mut [Frame], pom: &mut Pom) -> anyhow::Result<()> {
    let text = frame.text.trim().to_owned();
    if path(stack, &["project"]) {
        let slot = match frame.name.as_str() {
            "groupId" => Some(&mut pom.group),
            "artifactId" => Some(&mut pom.artifact),
            "version" => Some(&mut pom.version),
            _ => None,
        };
        if let Some(slot) = slot {
            ensure!(
                !frame.attributes && !frame.children,
                "unsupported GAV shape"
            );
            set(slot, text.clone())?;
        }
    }
    if path(stack, &["project", "parent"]) {
        let slot = match frame.name.as_str() {
            "groupId" => Some(&mut pom.parent_group),
            "artifactId" => Some(&mut pom.parent_artifact),
            "version" => Some(&mut pom.parent_version),
            _ => None,
        };
        if let Some(slot) = slot {
            ensure!(
                !frame.attributes && !frame.children,
                "unsupported parent GAV shape"
            );
            set(slot, text.clone())?;
        }
    }
    if path(stack, &["project", "properties"]) {
        ensure!(
            !frame.attributes && !frame.children && pom.properties.len() < MAX_PROPERTIES,
            "unsupported property shape/limit"
        );
        if pom
            .properties
            .insert(frame.name.clone(), text.clone())
            .is_some_and(|previous| previous != text)
        {
            pom.ambiguous_properties.insert(frame.name.clone());
        }
    }
    if path(stack, &["project", "profiles", "profile", "properties"]) {
        ensure!(
            pom.profile_properties.len() < MAX_PROPERTIES,
            "profile property limit exceeded"
        );
        pom.profile_properties.insert(frame.name.clone());
    }
    if let Some(index) = stack.iter().rposition(|f| f.plugin.is_some()) {
        let names: Vec<String> = stack[index + 1..]
            .iter()
            .map(|f| f.name.clone())
            .chain(std::iter::once(frame.name.clone()))
            .collect();
        let relative: Vec<&str> = names.iter().map(String::as_str).collect();
        let plugin = stack[index].plugin.as_mut().unwrap();
        match relative.as_slice() {
            ["groupId"] => set(&mut plugin.group, text.clone())?,
            ["artifactId"] => set(&mut plugin.artifact, text.clone())?,
            ["inherited"] => plugin.unsupported |= text != "true",
            ["configuration", "encoding"] => {
                plugin.unsupported |= frame.attributes || frame.children;
                set(&mut plugin.encoding, text.clone())?;
            }
            ["configuration"] => plugin.unsupported |= frame.attributes,
            ["configuration", "compilerArgs"] => {
                // shortcut: only empty overrides; model inherited argument merging when an artifact needs it.
                plugin.unsupported |= frame.attributes
                    || !text.is_empty()
                    || frame.argument_override && frame.children;
            }
            ["configuration", "compilerArgs", _] => {
                // This fixed JVM boot-path option stays one CLI value; interpolation may only
                // change its path payload, never introduce a javac source-encoding option.
                let boot_path = !frame.attributes
                    && !frame.children
                    && text.starts_with("-J-Xbootclasspath/p:");
                plugin.unsupported |= frame.attributes
                    || frame.children
                    || !boot_path
                        && (text.starts_with('@')
                            || text.contains("-encoding")
                            || text.contains("${"));
            }
            ["configuration", "compilerArgs", ..] => plugin.unsupported = true,
            _ if relative.contains(&"encoding") => plugin.unsupported = true,
            _ if relative.iter().any(|p| {
                matches!(
                    *p,
                    "compilerArgs" | "compilerArguments" | "compilerArgument"
                )
            }) && (text.starts_with('@')
                || text.contains("-encoding")
                || text.contains("${")
                || relative.contains(&"encoding")) =>
            {
                plugin.unsupported = true
            }
            _ => {}
        }
    }
    if let Some(plugin) = frame.plugin {
        ensure!(
            !(plugin.encoding.is_some() || plugin.unsupported)
                || !plugin
                    .artifact
                    .as_deref()
                    .is_some_and(|value| value.contains("${"))
                    && !plugin
                        .group
                        .as_deref()
                        .is_some_and(|value| value.contains("${")),
            "interpolated compiler identity with encoding configuration is unsupported"
        );
        if plugin.artifact.as_deref() == Some("maven-compiler-plugin")
            && plugin
                .group
                .as_deref()
                .is_none_or(|g| g == "org.apache.maven.plugins")
        {
            pom.unsupported_compiler_encoding |=
                plugin.unsupported || plugin.profile && plugin.encoding.is_some();
            if !plugin.profile {
                if let Some(value) = plugin.encoding {
                    set(
                        if plugin.managed {
                            &mut pom.managed_encoding
                        } else {
                            &mut pom.active_encoding
                        },
                        value,
                    )?;
                }
            }
        }
    }
    Ok(())
}

fn encoding(chain: &[Pom]) -> anyhow::Result<JavaSourceEncoding> {
    let mut properties = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    let mut active = None;
    let mut managed = None;
    for pom in chain.iter().rev() {
        ensure!(
            !pom.unsupported_compiler_encoding,
            "unsupported conditional/merged compiler encoding"
        );
        for (key, value) in &pom.properties {
            properties.insert(key.clone(), value.clone());
            if pom.ambiguous_properties.contains(key) {
                ambiguous.insert(key.clone());
            } else {
                ambiguous.remove(key);
            }
        }
        if pom.active_encoding.is_some() {
            active = pom.active_encoding.clone();
        }
        if pom.managed_encoding.is_some() {
            managed = pom.managed_encoding.clone();
        }
    }
    let mut used = BTreeSet::new();
    // Explicit compiler configuration wins. Otherwise the documented parameter expression is
    // ${encoding}, with default ${project.build.sourceEncoding}; maven.compiler.encoding is
    // an ordinary property unless this selected expression explicitly references it.
    let raw = active.as_ref().or(managed.as_ref()).or_else(|| {
        used.insert(USER_ENCODING.to_owned());
        properties.get(USER_ENCODING).or_else(|| {
            used.insert(SOURCE_ENCODING.to_owned());
            properties.get(SOURCE_ENCODING)
        })
    });
    let selected = if let Some(raw) = raw {
        let text = expand(raw, &properties, &mut used, &mut BTreeSet::new())?;
        match text.to_ascii_uppercase().as_str() {
            "UTF-8" | "UTF8" => JavaSourceEncoding::Utf8,
            "ISO-8859-1" | "ISO8859-1" => JavaSourceEncoding::Iso88591,
            _ => anyhow::bail!("unsupported declared Java source encoding {text:?}"),
        }
    } else {
        JavaSourceEncoding::Utf8
    };
    ensure!(
        ambiguous.is_disjoint(&used),
        "ambiguous property in source encoding closure"
    );
    ensure!(
        chain
            .iter()
            .all(|p| p.profile_properties.is_disjoint(&used)),
        "profile affects source encoding property closure"
    );
    Ok(selected)
}

fn expand(
    raw: &str,
    properties: &BTreeMap<String, String>,
    used: &mut BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
) -> anyhow::Result<String> {
    if !raw.contains("${") {
        return Ok(raw.trim().to_owned());
    }
    let key = raw
        .strip_prefix("${")
        .and_then(|s| s.strip_suffix('}'))
        .context("unsupported compound encoding interpolation")?;
    ensure!(
        !key.contains(['{', '}'])
            && visiting.len() < MAX_PROPERTIES
            && visiting.insert(key.to_owned()),
        "encoding property cycle/limit"
    );
    used.insert(key.to_owned());
    let value = expand(
        properties
            .get(key)
            .context("unresolved encoding property")?,
        properties,
        used,
        visiting,
    )?;
    visiting.remove(key);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir();
            let path = root.join(format!(
                "ktlsp-encoding-pure-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            assert!(path
                .canonicalize()
                .unwrap()
                .starts_with(root.canonicalize().unwrap()));
            Self(path)
        }
        fn write(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, text).unwrap();
            path
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            assert!(self
                .0
                .canonicalize()
                .unwrap()
                .starts_with(std::env::temp_dir().canonicalize().unwrap()));
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn pom(body: &str) -> String {
        format!("<project><groupId>example</groupId><artifactId>library</artifactId><version>1</version>{body}</project>")
    }
    fn declared(body: &str) -> anyhow::Result<JavaSourceEncoding> {
        encoding(&[parse(pom(body).as_bytes())?])
    }
    fn plugin(config: &str) -> String {
        format!("<build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration>{config}</configuration></plugin></plugins></build>")
    }

    #[test]
    fn static_declaration_positive_table() {
        for (name, body, expected) in [
            ("strict default", "".to_owned(), JavaSourceEncoding::Utf8),
            ("numeric entity", "<properties><project.build.sourceEncoding>UTF&#45;8</project.build.sourceEncoding></properties>".to_owned(), JavaSourceEncoding::Utf8),
            ("CDATA", "<properties><project.build.sourceEncoding><![CDATA[ISO-8859-1]]></project.build.sourceEncoding></properties>".to_owned(), JavaSourceEncoding::Iso88591),
            ("direct compiler", plugin("<encoding>ISO8859-1</encoding>"), JavaSourceEncoding::Iso88591),
            ("compiler overrides source default", "<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties>".to_owned() + &plugin("<encoding>ISO-8859-1</encoding>"), JavaSourceEncoding::Iso88591),
            ("unused compiler-named property", "<properties><maven.compiler.encoding>ISO-8859-1</maven.compiler.encoding></properties>".to_owned(), JavaSourceEncoding::Utf8),
            ("parameter expression property", "<properties><encoding>ISO-8859-1</encoding><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties>".to_owned(), JavaSourceEncoding::Iso88591),
            ("explicitly referenced compiler-named property", "<properties><maven.compiler.encoding>ISO-8859-1</maven.compiler.encoding></properties>".to_owned() + &plugin("<encoding>${maven.compiler.encoding}</encoding>"), JavaSourceEncoding::Iso88591),
            ("unused unresolved default", "<properties><project.build.sourceEncoding>${absent}</project.build.sourceEncoding><encoding>${alsoAbsent}</encoding></properties>".to_owned() + &plugin("<encoding>ISO-8859-1</encoding>"), JavaSourceEncoding::Iso88591),
            ("unselected property profile", plugin("<encoding>ISO-8859-1</encoding>") + "<profiles><profile><properties><project.build.sourceEncoding>ASCII</project.build.sourceEncoding><maven.compiler.encoding>ASCII</maven.compiler.encoding><encoding>ASCII</encoding></properties></profile></profiles>", JavaSourceEncoding::Iso88591),
            ("unrelated profile", "<properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties><profiles><profile><properties><debug>true</debug></properties></profile></profiles>".to_owned(), JavaSourceEncoding::Iso88591),
            ("unrelated plugin", "<profiles><profile><build><plugins><plugin><artifactId>maven-javadoc-plugin</artifactId><configuration><encoding>ASCII</encoding></configuration></plugin></plugins></build></profile></profiles>".to_owned(), JavaSourceEncoding::Utf8),
        ] {
            assert_eq!(declared(&body).unwrap(), expected, "{name}");
        }
        for text in [
            format!("\u{feff}<?xml version=\"1.0\" encoding=\"UTF-8\"?>{}", pom("")),
            pom("").replacen("<project>", "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">", 1),
            "<m:project xmlns:m=\"http://maven.apache.org/POM/4.0.0\"><m:groupId>example</m:groupId><m:artifactId>library</m:artifactId><m:version>1</m:version></m:project>".to_owned(),
        ] { assert_eq!(encoding(&[parse(text.as_bytes()).unwrap()]).unwrap(), JavaSourceEncoding::Utf8); }
    }

    #[test]
    fn unsupported_declaration_negative_table() {
        let source = |text: &str| {
            format!("<properties><project.build.sourceEncoding>{text}</project.build.sourceEncoding></properties>")
        };
        for (name, text) in [
            ("wrong namespace", pom("").replacen("<project>", "<project xmlns=\"urn:foreign\">", 1)),
            ("unknown prefix", "<m:project/>".to_owned()),
            ("DTD", format!("<!DOCTYPE project [<!ENTITY enc 'UTF-8'>]>{}", pom(&source("&enc;")))),
            ("custom entity", pom(&source("&custom;"))),
            ("invalid numeric character", pom(&source("&#0;"))),
            ("invalid attribute character", pom("").replacen("<project>", "<project ignored=\"&#0;\">", 1)),
            ("bad metadata encoding", format!("<?xml version=\"1.0\" encoding=\"windows-1252\"?>{}", pom(""))),
            ("duplicate root", pom("") + &pom("")),
            ("trailing text", pom("") + "foreign"),
            ("truncated", "<project><groupId>example</groupId>".to_owned()),
            ("malformed end", "<project></other>".to_owned()),
            ("processing instruction", format!("<?custom encoding='UTF-8'?>{}", pom(""))),
            ("unknown encoding", pom(&source("windows-1252"))),
            ("unresolved property", pom(&source("${absent}"))),
            ("compound property", pom(&source("UTF-${number}"))),
            ("property cycle", pom("<properties><project.build.sourceEncoding>${a}</project.build.sourceEncoding><a>${b}</a><b>${a}</b></properties>")),
            ("duplicate declaration", pom(&(source("UTF8") + &source("ISO-8859-1")))),
            ("relevant profile", pom(&(source("UTF8") + "<profiles><profile><properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties></profile></profiles>"))),
            ("profile introduces parameter override", pom("<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties><profiles><profile><properties><encoding>ISO-8859-1</encoding></properties></profile></profiles>")),
            ("selected expression profile", pom(&("<properties><charset>UTF8</charset></properties>".to_owned() + &plugin("<encoding>${charset}</encoding>") + "<profiles><profile><properties><charset>ISO-8859-1</charset></properties></profile></profiles>"))),
            ("encoding argument", pom(&plugin("<compilerArgs><arg>-encoding</arg><arg>UTF8</arg></compilerArgs>"))),
            ("unknown argument property", pom(&plugin("<compilerArgs><arg>${opts}</arg></compilerArgs>"))),
            ("execution encoding", pom("<build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><executions><execution><configuration><encoding>UTF8</encoding></configuration></execution></executions></plugin></plugins></build>")),
            ("merge attribute", pom("<build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration combine.self=\"override\"><encoding>UTF8</encoding></configuration></plugin></plugins></build>")),
            ("interpolated compiler identity", pom("<build><plugins><plugin><artifactId>${compiler}</artifactId><configuration><encoding>UTF8</encoding></configuration></plugin></plugins></build>")),
            ("disabled plugin inheritance", pom("<build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><inherited>false</inherited><configuration><encoding>UTF8</encoding></configuration></plugin></plugins></build>")),
        ] { assert!(parse(text.as_bytes()).and_then(|p| encoding(&[p])).is_err(), "{name}"); }
    }

    #[test]
    fn certified_metadata_requires_root_and_preserves_parent_encoding() {
        let temp = Temp::new();
        let root = Coordinate::parse("example:child:1").unwrap();
        let child = temp.write("child.pom", "<project><groupId>example</groupId><artifactId>child</artifactId><version>1</version><parent><groupId>example</groupId><artifactId>parent</artifactId><version>1</version></parent></project>");
        let parent = temp.write("parent.pom", "<project><groupId>example</groupId><artifactId>parent</artifactId><version>1</version><properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties></project>");
        assert!(SourceTextPolicy::resolve_certified(&root, |_| Ok(Vec::new())).is_err());
        assert!(SourceTextPolicy::resolve_certified(&root, |coordinate| Ok(
            if coordinate == &root {
                vec![child.clone()]
            } else {
                Vec::new()
            }
        ))
        .is_err());
        let policy = SourceTextPolicy::resolve_certified(&root, |coordinate| {
            Ok(vec![if coordinate == &root {
                child.clone()
            } else {
                parent.clone()
            }])
        })
        .unwrap();
        assert_eq!(policy.java.decode(b"caf\xe9".to_vec()).unwrap(), "café");
        fs::write(parent, "changed selected parent").unwrap();
        assert!(policy.validate().is_err());
    }

    #[test]
    fn parent_properties_child_precedence_and_profiles() {
        let parent = parse(pom("<properties><charset>UTF8</charset><project.build.sourceEncoding>${charset}</project.build.sourceEncoding></properties>").as_bytes()).unwrap();
        let child = parse(pom("<properties><charset>ISO-8859-1</charset></properties>").as_bytes())
            .unwrap();
        assert_eq!(
            encoding(&[child, parent]).unwrap(),
            JavaSourceEncoding::Iso88591
        );
        let parent = parse(pom("<properties><charset>UTF8</charset><project.build.sourceEncoding>${charset}</project.build.sourceEncoding></properties><profiles><profile><properties><charset>ISO-8859-1</charset></properties></profile></profiles>").as_bytes()).unwrap();
        assert!(encoding(&[parent]).is_err());
        let parent = parse(pom(&plugin("<encoding>UTF8</encoding>")).as_bytes()).unwrap();
        let child = parse(pom(&plugin("<encoding>ISO-8859-1</encoding>")).as_bytes()).unwrap();
        assert_eq!(
            encoding(&[child, parent]).unwrap(),
            JavaSourceEncoding::Iso88591
        );
        let managed = "<build><pluginManagement><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration><encoding>UTF8</encoding></configuration></plugin></plugins></pluginManagement></build>";
        let parent = parse(pom(managed).as_bytes()).unwrap();
        let child = parse(pom(&plugin("<encoding>ISO-8859-1</encoding>")).as_bytes()).unwrap();
        assert_eq!(
            encoding(&[child, parent]).unwrap(),
            JavaSourceEncoding::Iso88591
        );
    }

    #[test]
    fn absent_parent_wrong_gav_and_parent_cycles_are_not_default_utf8() {
        let tmp = Temp::new();
        let c = Coordinate::parse("example:library:1").unwrap();
        assert_eq!(
            SourceTextPolicy::resolve(&c, |_| vec![]).unwrap().java,
            JavaSourceEncoding::Utf8
        );
        let file = tmp.write("child.pom", &pom("<parent><groupId>example</groupId><artifactId>parent</artifactId><version>1</version></parent>"));
        assert!(SourceTextPolicy::resolve(&c, |value| if value == &c {
            vec![file.clone()]
        } else {
            vec![]
        })
        .is_err());
        assert!(SourceTextPolicy::resolve(&c, |_| vec![tmp.write(
            "wrong.pom",
            &pom("").replace("<artifactId>library", "<artifactId>wrong")
        )])
        .is_err());
        let cyclic = tmp.write("cyclic.pom", &pom("<parent><groupId>example</groupId><artifactId>library</artifactId><version>1</version></parent>"));
        assert!(SourceTextPolicy::resolve(&c, |_| vec![cyclic.clone()]).is_err());
    }

    #[test]
    fn captured_metadata_receipts_and_cache_partition_are_current() {
        let tmp = Temp::new();
        let file = tmp.write("child.pom", &pom("<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties>"));
        let c = Coordinate::parse("example:library:1").unwrap();
        let first = SourceTextPolicy::resolve(&c, |_| vec![file.clone()]).unwrap();
        first.validate().unwrap();
        assert_ne!(
            first.cache_key("local-origin-A"),
            first.cache_key("local-origin-B")
        );
        let serialized = serde_json::to_vec(&first).unwrap();
        let restored: SourceTextPolicy = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(restored, first);
        let mut old_version = restored.clone();
        old_version.version = 1;
        assert!(old_version.validate().is_err());
        assert_ne!(
            old_version.cache_key("same-jar-stat"),
            first.cache_key("same-jar-stat")
        );
        assert!(SourceTextPolicy {
            java: JavaSourceEncoding::Iso88591,
            ..SourceTextPolicy::default()
        }
        .validate()
        .is_err());
        fs::write(&file, pom("<properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties>")).unwrap();
        assert!(first.validate().is_err());
        let next = SourceTextPolicy::resolve(&c, |_| vec![file.clone()]).unwrap();
        assert_eq!(next.java, JavaSourceEncoding::Iso88591);
        assert_ne!(
            first.cache_key("same-jar-stat"),
            next.cache_key("same-jar-stat")
        );
        fs::remove_file(&file).unwrap();
        assert!(next.validate().is_err());
    }

    #[test]
    fn conflicting_cached_metadata_is_unknown() {
        let tmp = Temp::new();
        let a = tmp.write("a.pom", &pom(""));
        let b = tmp.write("b.pom", &pom("<properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties>"));
        let c = Coordinate::parse("example:library:1").unwrap();
        assert!(SourceTextPolicy::resolve(&c, |_| vec![a.clone(), b.clone()]).is_err());
    }

    #[test]
    fn bounded_xml_and_parent_work() {
        let deep = format!(
            "<project>{}{}</project>",
            "<nested>".repeat(MAX_DEPTH),
            "</nested>".repeat(MAX_DEPTH)
        );
        assert!(parse(deep.as_bytes()).is_err());
        assert!(parse(
            pom(&format!(
                "<description>{}</description>",
                "x".repeat(MAX_SCALAR + 1)
            ))
            .as_bytes()
        )
        .is_err());
        assert!(parse(pom(&"<ignored/>".repeat(MAX_EVENTS)).as_bytes()).is_err());
        assert!(parse(&vec![b' '; MAX_POM + 1]).is_err());
        assert!(parse(b"<project>\xff</project>").is_err());
        // Selecting ISO for metadata must not bypass the original byte/event/depth bounds.
        let header = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>";
        for (body, expected_reason) in [
            (deep, "depth limit"),
            (pom(&"<ignored/>".repeat(MAX_EVENTS)), "event limit"),
            (" ".repeat(MAX_POM), "size limit"),
        ] {
            let error = parse(format!("{header}{body}").as_bytes()).unwrap_err();
            assert!(format!("{error:#}").contains(expected_reason), "{error:#}");
        }
        let properties = (0..=MAX_PROPERTIES)
            .map(|n| format!("<p{n}>x</p{n}>"))
            .collect::<String>();
        assert!(parse(pom(&format!("<properties>{properties}</properties>")).as_bytes()).is_err());
        let tmp = Temp::new();
        let chain = (0..=MAX_PARENTS).map(|n| tmp.write(&format!("{n}.pom"), &format!("<project><groupId>example</groupId><artifactId>p{n}</artifactId><version>1</version><parent><groupId>example</groupId><artifactId>p{}</artifactId><version>1</version></parent></project>", n + 1))).collect::<Vec<_>>();
        let c = Coordinate::parse("example:p0:1").unwrap();
        assert!(SourceTextPolicy::resolve(&c, |c| vec![chain
            [c.artifact[1..].parse::<usize>().unwrap()]
        .clone()])
        .is_err());
    }

    #[test]
    fn published_commons_parent_chain_is_declared_iso_without_allowlist() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/source-encoding");
        let selected = Coordinate::parse("org.apache.commons:commons-compress:1.26.0").unwrap();
        let policy = SourceTextPolicy::resolve(&selected, |c| {
            vec![fixture.join(format!("{}-{}.pom", c.artifact, c.version))]
        })
        .unwrap();
        assert_eq!(policy.java, JavaSourceEncoding::Iso88591);
        assert_eq!(policy.poms.len(), 3);
        policy.validate().unwrap();
    }

    #[test]
    fn xml_metadata_declared_encoding_and_byte_boundaries() {
        let content =
            pom("<properties><sample>© café &#xF6; <![CDATA[é]]> &amp;</sample></properties>");
        let iso = |text: &str| {
            text.chars()
                .map(|c| u8::try_from(u32::from(c)).unwrap())
                .collect::<Vec<_>>()
        };
        let declaration =
            |label: &str| format!("<?xml version=\"1.0\" encoding=\"{label}\"?>{content}");
        for (name, bytes) in [
            ("implicit UTF8", content.as_bytes().to_vec()),
            (
                "declaration without encoding",
                format!("<?xml version=\"1.0\"?>{content}").into_bytes(),
            ),
            ("UTF8 declaration", declaration("UTF-8").into_bytes()),
            ("UTF8 alias", declaration("utf8").into_bytes()),
            (
                "UTF8 BOM",
                [b"\xef\xbb\xbf".as_slice(), content.as_bytes()].concat(),
            ),
            (
                "UTF8 BOM and declaration",
                [b"\xef\xbb\xbf".as_slice(), declaration("UTF-8").as_bytes()].concat(),
            ),
            ("declared ISO", iso(&declaration("ISO-8859-1"))),
            ("ISO alias", iso(&declaration("iso8859-1"))),
            (
                "ISO standalone",
                iso(&format!(
                    "<?xml version=\"1.0\" encoding=\"ISO-8859-1\" standalone=\"yes\"?>{content}"
                )),
            ),
        ] {
            let parsed = parse(&bytes).unwrap_or_else(|error| panic!("{name}: {error:#}"));
            assert_eq!(parsed.properties["sample"], "© café ö é &", "{name}");
            // The POM's own encoding does not itself declare Java source encoding.
            assert_eq!(
                encoding(&[parsed]).unwrap(),
                JavaSourceEncoding::Utf8,
                "{name}"
            );
        }
        for (name, bytes) in [
            ("undeclared ISO bytes", iso(&content)),
            (
                "UTF8 declaration with ISO bytes",
                iso(&declaration("UTF-8")),
            ),
            (
                "unknown declaration",
                declaration("windows-1252").into_bytes(),
            ),
            (
                "unsupported UTF16 declaration",
                declaration("UTF-16").into_bytes(),
            ),
            (
                "UTF16 LE BOM",
                [b"\xff\xfe".as_slice(), content.as_bytes()].concat(),
            ),
            (
                "UTF8 BOM contradicts ISO",
                [b"\xef\xbb\xbf".as_slice(), &iso(&declaration("ISO-8859-1"))].concat(),
            ),
            (
                "double UTF8 BOM",
                [b"\xef\xbb\xbf\xef\xbb\xbf".as_slice(), content.as_bytes()].concat(),
            ),
            (
                "declaration after whitespace",
                format!(
                    " <?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>{}",
                    pom("")
                )
                .into_bytes(),
            ),
            (
                "declaration after comment",
                format!(
                    "<!--before--><?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>{}",
                    pom("")
                )
                .into_bytes(),
            ),
            (
                "duplicate encoding",
                format!(
                    "<?xml version=\"1.0\" encoding=\"UTF8\" encoding=\"ISO-8859-1\"?>{}",
                    pom("")
                )
                .into_bytes(),
            ),
            (
                "duplicate version",
                format!("<?xml version=\"1.0\" version=\"1.0\"?>{}", pom("")).into_bytes(),
            ),
            (
                "unknown declaration attribute",
                format!("<?xml version=\"1.0\" other=\"UTF8\"?>{}", pom("")).into_bytes(),
            ),
            (
                "encoding after standalone",
                format!(
                    "<?xml version=\"1.0\" standalone=\"yes\" encoding=\"UTF8\"?>{}",
                    pom("")
                )
                .into_bytes(),
            ),
            (
                "invalid XML scalar",
                b"<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?><project>\x00</project>".to_vec(),
            ),
        ] {
            assert!(parse(&bytes).is_err(), "{name}");
        }
        // Published Lang shape: parent compiler configuration outranks the child's default.
        let child = parse(pom("<properties><project.build.sourceEncoding>UTF-8</project.build.sourceEncoding></properties>").as_bytes()).unwrap();
        let parent = parse(format!("<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>{}", pom("<properties><commons.encoding>iso-8859-1</commons.encoding><project.build.sourceEncoding>${commons.encoding}</project.build.sourceEncoding></properties><build><pluginManagement><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration><encoding>${commons.encoding}</encoding></configuration></plugin></plugins></pluginManagement><plugins><plugin><artifactId>maven-compiler-plugin</artifactId></plugin></plugins></build>")).as_bytes()).unwrap();
        assert_eq!(
            encoding(&[child, parent]).unwrap(),
            JavaSourceEncoding::Iso88591
        );
    }

    #[test]
    fn selected_encoding_closure_ignores_unrelated_duplicate_properties() {
        for (name, body) in [
            ("unused differing property", "<properties><releaseManager>Gary</releaseManager><releaseManager>${user.name}</releaseManager></properties>".to_owned() + &plugin("<encoding>UTF8</encoding>")),
            ("unused identical property", "<properties><releaseManager>Gary</releaseManager><releaseManager>Gary</releaseManager><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties>".to_owned()),
            ("selected identical property", "<properties><charset>UTF8</charset><charset>UTF8</charset></properties>".to_owned() + &plugin("<encoding>${charset}</encoding>")),
            ("unselected differing encoding default", "<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties>".to_owned() + &plugin("<encoding>UTF8</encoding>")),
        ] {
            assert_eq!(declared(&body).unwrap(), JavaSourceEncoding::Utf8, "{name}");
        }
        for (name, body) in [
            ("selected differing property", "<properties><charset>UTF8</charset><charset>ISO-8859-1</charset></properties>".to_owned() + &plugin("<encoding>${charset}</encoding>")),
            ("selected differing default", "<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties>".to_owned()),
            ("selected transitive differing property", "<properties><project.build.sourceEncoding>${charset}</project.build.sourceEncoding><charset>UTF8</charset><charset>ISO-8859-1</charset></properties>".to_owned()),
        ] {
            assert!(declared(&body).is_err(), "{name}");
        }
    }

    #[test]
    fn child_property_override_replaces_parent_ambiguity_only_for_that_key() {
        let parent = parse(pom("<properties><charset>UTF8</charset><charset>ISO-8859-1</charset><project.build.sourceEncoding>${charset}</project.build.sourceEncoding></properties>").as_bytes()).unwrap();
        let child =
            parse(pom("<properties><charset>UTF8</charset></properties>").as_bytes()).unwrap();
        assert_eq!(
            encoding(&[child, parent]).unwrap(),
            JavaSourceEncoding::Utf8
        );

        let parent = parse(pom("<properties><charset>UTF8</charset><project.build.sourceEncoding>${charset}</project.build.sourceEncoding></properties>").as_bytes()).unwrap();
        let child = parse(
            pom("<properties><charset>UTF8</charset><charset>ISO-8859-1</charset></properties>")
                .as_bytes(),
        )
        .unwrap();
        assert!(encoding(&[child, parent]).is_err());

        let parent = parse(pom("<properties><charset>UTF8</charset><charset>ISO-8859-1</charset><project.build.sourceEncoding>${charset}</project.build.sourceEncoding></properties>").as_bytes()).unwrap();
        let unrelated_child =
            parse(pom("<properties><other>UTF8</other></properties>").as_bytes()).unwrap();
        assert!(encoding(&[unrelated_child, parent]).is_err());
    }

    #[test]
    fn compiler_argument_boundaries_preserve_encoding_closure() {
        let fixed = "-J-Xbootclasspath/p:${settings.localRepository}/compiler/${version}/javac-${version}.jar";
        for configuration in [
            format!("<encoding>UTF8</encoding><compilerArgs><arg>{fixed}</arg></compilerArgs>"),
            "<encoding>UTF8</encoding><compilerArgs><arg>-J-Xbootclasspath/p:${path}/has-encoding-in-name.jar</arg></compilerArgs>".to_owned(),
        ] {
            assert_eq!(declared(&plugin(&configuration)).unwrap(), JavaSourceEncoding::Utf8);
        }
        let profile = format!("<profiles><profile><build><plugins><plugin><artifactId>maven-compiler-plugin</artifactId><configuration><compilerArgs combine.children=\"append\"><arg>{fixed}</arg></compilerArgs></configuration></plugin></plugins></build></profile></profiles>");
        assert_eq!(
            declared(&(plugin("<encoding>UTF8</encoding>") + &profile)).unwrap(),
            JavaSourceEncoding::Utf8
        );
        for (name, configuration) in [
            ("entire option dynamic", "<compilerArgs><arg>${opts}</arg></compilerArgs>"),
            ("JVM option selector dynamic", "<compilerArgs><arg>-J${opts}</arg></compilerArgs>"),
            ("bootpath selector delimiter dynamic", "<compilerArgs><arg>-J-Xbootclasspath${separator}${path}</arg></compilerArgs>"),
            ("encoding option", "<compilerArgs><arg>-encoding</arg><arg>ISO-8859-1</arg></compilerArgs>"),
            ("encoding option dynamic suffix", "<compilerArgs><arg>-encoding${suffix}</arg></compilerArgs>"),
            ("dynamic argument file", "<compilerArgs><arg>@${argsFile}</arg></compilerArgs>"),
            ("literal argument file", "<compilerArgs><arg>@args.txt</arg></compilerArgs>"),
            ("scalar argument list", "<compilerArgs>${opts}</compilerArgs>"),
            ("unmodeled argument child", "<compilerArgs><item>@args.txt</item></compilerArgs>"),
            ("bootpath leaf merge attribute", "<compilerArgs><arg combine.self=\"override\">-J-Xbootclasspath/p:${path}</arg></compilerArgs>"),
            ("bootpath nested value", "<compilerArgs><arg><value>-J-Xbootclasspath/p:${path}</value></arg></compilerArgs>"),
            ("unmodeled argument-list merge", "<compilerArgs combine.self=\"override\"><arg>-J-Xbootclasspath/p:${path}</arg></compilerArgs>"),
            ("different argument transport", "<compilerArgument>-J-Xbootclasspath/p:${path}</compilerArgument>"),
            ("JVM file-encoding setting", "<compilerArgs><arg>-J-Dfile.encoding=${charset}</arg></compilerArgs>"),
        ] {
            assert!(declared(&plugin(&("<encoding>UTF8</encoding>".to_owned() + configuration))).is_err(), "{name}");
        }
    }
    #[test]
    fn compiler_arg_alias_preserves_the_checked_string_boundary() {
        for element in ["arg", "compilerArg", "arbitraryItem"] {
            let argument =
                format!("<compilerArgs><{element}>-Xlint:all,-options</{element}></compilerArgs>");
            assert_eq!(
                declared(&plugin(
                    &("<encoding>UTF8</encoding>".to_owned() + &argument)
                ))
                .unwrap(),
                JavaSourceEncoding::Utf8
            );
            for text in [
                "-encoding",
                "${opts}",
                "@args.txt",
                "-J${opts}",
                "-J-Dfile.encoding=${charset}",
            ] {
                let argument =
                    format!("<compilerArgs><{element}>{text}</{element}></compilerArgs>");
                assert!(
                    declared(&plugin(
                        &("<encoding>UTF8</encoding>".to_owned() + &argument)
                    ))
                    .is_err(),
                    "{element}: {text}"
                );
            }
            for argument in [
                format!("<compilerArgs><{element} implementation=\"java.lang.String\">-Xlint:all</{element}></compilerArgs>"),
                format!("<compilerArgs><{element}><value>-Xlint:all</value></{element}></compilerArgs>"),
            ] {
                assert!(declared(&plugin(&("<encoding>UTF8</encoding>".to_owned() + &argument))).is_err(), "{element}: {argument}");
            }
        }
    }

    #[test]
    fn empty_argument_override_preserves_parent_encoding_uncertainty() {
        for empty in [
            "<compilerArgs combine.self=\"override\"/>",
            "<compilerArgs combine.self=\"override\"></compilerArgs>",
            "<compilerArgs combine.self=\"override\"> \n<!-- no argument -->\t </compilerArgs>",
        ] {
            let parent = parse(pom(&("<properties><project.build.sourceEncoding>UTF8</project.build.sourceEncoding></properties>".to_owned() + &plugin("<compilerArgs><arg>--add-exports=java.base/jdk.internal.javac=ALL-UNNAMED</arg></compilerArgs>"))).as_bytes()).unwrap();
            let child = parse(pom(&plugin(empty)).as_bytes()).unwrap();
            assert_eq!(
                encoding(&[child, parent]).unwrap(),
                JavaSourceEncoding::Utf8
            );

            for parent_body in [
                "<properties><project.build.sourceEncoding>${charset}</project.build.sourceEncoding><charset>UTF8</charset><charset>ISO-8859-1</charset></properties>".to_owned(),
                plugin("<compilerArgs><arg>${opts}</arg></compilerArgs>"),
                plugin("<compilerArgs><arg>-encoding</arg><arg>ISO-8859-1</arg></compilerArgs>"),
            ] {
                let parent = parse(pom(&parent_body).as_bytes()).unwrap();
                let child = parse(pom(&plugin(empty)).as_bytes()).unwrap();
                assert!(encoding(&[child, parent]).is_err(), "{empty}: {parent_body}");
            }
        }
        for unsupported in [
            "<compilerArgs combine.self=\"override\"><arg>-Xlint:all</arg></compilerArgs>",
            "<compilerArgs combine.self=\"override\">-Xlint:all</compilerArgs>",
            "<compilerArgs combine.self=\"unknown\"/>",
            "<compilerArgs combine.self=\"override\" implementation=\"java.util.ArrayList\"/>",
        ] {
            assert!(
                declared(&plugin(
                    &("<encoding>UTF8</encoding>".to_owned() + unsupported)
                ))
                .is_err(),
                "{unsupported}"
            );
        }
    }
}
