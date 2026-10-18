//! Java missing-method positives from current owned generated API only.
use super::*;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

struct Origin {
    kind: &'static str,
    digest: String,
    /// Binary names come from the existing classfile decoder, not from a path spelling.
    binary_name: Option<String>,
    public_zero_argument_methods: Option<BTreeSet<String>>,
}

pub(super) struct MethodApi<'a> {
    generated: &'a processor::GeneratedSources,
    origins: BTreeMap<PathBuf, Origin>,
    invalid: Arc<AtomicBool>,
    _watch: Arc<crate::compiled_watch::NativeInputWatch>,
}

impl<'a> MethodApi<'a> {
    /// Call only AFTER validated_lease has checked the header component's canonical diagnostic
    /// identity and source ownership. A saved directory or metadata JSON is not this capability.
    pub(super) fn prepare(
        generated: &'a processor::GeneratedSources,
        request: &KotlinDiagnosticRequest,
        overlay: &KotlinEditorOverlay,
        header_receipt: &Value,
    ) -> anyhow::Result<Self> {
        generated.check_live(request)?;
        let lease = overlay.directory.join("lease");
        let mut java_sources = Vec::new();
        for path in &overlay.sources {
            if path.extension().and_then(|s| s.to_str()) != Some("java") {
                continue;
            }
            let Some(original) = overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(path)))
            else {
                continue;
            };
            if generated.source_digest(Path::new(original)).is_some() {
                java_sources.push(path.clone());
            }
        }
        let mut reads = java_sources.clone();
        reads.push(lease.clone());
        let invalid = Arc::new(AtomicBool::new(false));
        let (wake, _receiver) = tokio::sync::mpsc::channel(1);
        let watch = crate::compiled_watch::NativeInputWatch::new(
            invalid.clone(),
            crate::compiled_watch::WatchPlan::read_inputs(&reads)?,
            wake,
        )?;
        let mut origins = BTreeMap::new();
        for path in java_sources {
            let original = overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(&path)))
                .unwrap();
            let digest = sha(&std::fs::read(&path)?);
            anyhow::ensure!(
                generated.source_digest(Path::new(original)) == Some(digest.as_str()),
                "java-generated-source-not-current"
            );
            origins.insert(
                path,
                Origin {
                    kind: "source",
                    digest,
                    binary_name: None,
                    public_zero_argument_methods: None,
                },
            );
        }
        for output in array(header_receipt, "outputs")? {
            let relative = value_string(output, "relativePath")?;
            if !relative.ends_with(".class") || output["apiStatus"] != "SignatureChecked" {
                continue;
            }
            let source_owners = array(output, "sources")?;
            if source_owners.is_empty()
                || !source_owners.iter().all(|source| {
                    source
                        .as_str()
                        .and_then(|s| overlay.paths.get(&kotlin_path_key(s)))
                        .is_some_and(|original| {
                            generated.source_digest(Path::new(original)).is_some()
                        })
                })
            {
                continue;
            }
            // Paths outside this private lease (including .. and absolute paths) are unavailable.
            let relative_path = Path::new(relative);
            if relative_path.is_absolute()
                || relative_path
                    .components()
                    .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                continue;
            }
            let path = lease.join(relative_path);
            if !path.is_file() {
                continue;
            } // rejected API sources never enter validated_lease
            let bytes = std::fs::read(&path)?;
            anyhow::ensure!(
                sha(&bytes) == value_string(output, "sha256")?,
                "java-current-api-class-differs"
            );
            let class = ktcore::classfile::decode("owned-current-generated-api", &bytes)?;
            if class.super_name.as_deref() != Some("java/lang/Object")
                || !class.interfaces.is_empty()
                || class.signature.is_some()
                || class.identity.internal_name.contains('$')
            {
                continue;
            }
            let methods: BTreeSet<_> = class
                .methods
                .iter()
                .filter(|m| {
                    m.access_flags & 0x0001 != 0
                        && m.access_flags & (0x0008 | 0x0040 | 0x1000) == 0
                        && m.signature.is_none()
                        && m.descriptor.starts_with("()")
                        && !m.name.starts_with('<')
                })
                .map(|m| m.name.to_string())
                .collect();
            origins.insert(
                path,
                Origin {
                    kind: "class",
                    digest: sha(&bytes),
                    binary_name: Some(class.identity.internal_name.replace('/', ".")),
                    public_zero_argument_methods: Some(methods),
                },
            );
        }
        let api = Self {
            generated,
            origins,
            invalid,
            _watch: watch,
        };
        api.check_live(request)?;
        Ok(api)
    }

    pub(super) fn check_live(&self, request: &KotlinDiagnosticRequest) -> anyhow::Result<()> {
        self.generated.check_live(request)?;
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "java-generated-api-read-invalidated"
        );
        for (path, origin) in &self.origins {
            anyhow::ensure!(
                sha(&std::fs::read(path)?) == origin.digest,
                "java-generated-api-origin-changed"
            );
        }
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "java-generated-api-read-invalidated"
        );
        Ok(())
    }
}

fn position(row: &Value, key: &str) -> Option<usize> {
    row.get(key)?.as_u64().and_then(|n| usize::try_from(n).ok())
}

fn identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|c| c.is_alphabetic() || matches!(c, '_' | '$'))
        && chars.all(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
}

/// The caller first runs import_diagnostics to validate the entire SAME-task source/runtime,
/// option, source-order, parse coverage, error stream and exit receipt. This adds positives only.
pub(super) fn admitted(
    value: &Value,
    request: &KotlinDiagnosticRequest,
    overlay: &KotlinEditorOverlay,
    expected_sources: &[(PathBuf, String)],
    api: &MethodApi<'_>,
) -> anyhow::Result<Vec<CompileDiagnostic>> {
    api.check_live(request)?;
    let calls = array(value, "methodCalls")?;
    let mut sources = BTreeMap::new();
    for (row, (path, digest)) in array(value, "sourcesBefore")?.iter().zip(expected_sources) {
        anyhow::ensure!(
            same_path(Path::new(value_string(row, "path")?), path)
                && value_string(row, "sha256")? == digest,
            "java-method-selected-source-differs"
        );
        sources.insert(value_string(row, "uri")?, path);
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for diagnostic in array(value, "diagnostics")? {
        if diagnostic["kind"] != "ERROR"
            || diagnostic["code"] != "compiler.err.cant.resolve.location.args"
        {
            continue;
        }
        let Some(uri) = diagnostic.get("sourceUri").and_then(Value::as_str) else {
            continue;
        };
        let Some(path) = sources.get(uri) else {
            continue;
        };
        let text = std::fs::read_to_string(path)?;
        for call in calls {
            if call["sourceUri"] != uri || call["simpleOwnedApi"] != true {
                continue;
            }
            let (Some(receiver), Some(method), Some(kind), Some(origin_path), Some(digest)) = (
                call.get("receiver").and_then(Value::as_str),
                call.get("method").and_then(Value::as_str),
                call.get("originKind").and_then(Value::as_str),
                call.get("originPath").and_then(Value::as_str),
                call.get("originSha256").and_then(Value::as_str),
            ) else {
                continue;
            };
            if !identifier(receiver) || !identifier(method) {
                continue;
            }
            let Some((owned_path, origin)) = api
                .origins
                .iter()
                .find(|(p, _)| same_path(p, Path::new(origin_path)))
            else {
                continue;
            };
            if origin.kind != kind || origin.digest != digest {
                continue;
            }
            if let Some(name) = &origin.binary_name {
                if call["receiverType"] != *name {
                    continue;
                }
            }
            let methods: BTreeSet<_> = array(call, "publicInstanceZeroArgumentMethods")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .filter(|s| identifier(s))
                        .ok_or_else(|| anyhow::anyhow!("java-method-api-inventory-malformed"))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
                .into_iter()
                .map(str::to_owned)
                .collect();
            anyhow::ensure!(
                methods.len() == array(call, "publicInstanceZeroArgumentMethods")?.len(),
                "java-method-api-inventory-duplicate"
            );
            if methods.contains(method)
                || origin
                    .public_zero_argument_methods
                    .as_ref()
                    .is_some_and(|actual| actual != &methods)
            {
                continue;
            }
            // An erroneous generated Java declaration is not absence authority for its receiver.
            if kind == "source"
                && array(value, "sourcesBefore")?.iter().any(|source| {
                    source
                        .get("path")
                        .and_then(Value::as_str)
                        .is_some_and(|p| same_path(Path::new(p), owned_path))
                        && array(value, "diagnostics").is_ok_and(|ds| {
                            ds.iter()
                                .any(|d| d["kind"] == "ERROR" && d["sourceUri"] == source["uri"])
                        })
                })
            {
                continue;
            }
            let (Some(start), Some(end), Some(receiver_end), Some(call_end)) = (
                position(call, "selectStart"),
                position(call, "selectEnd"),
                position(call, "receiverEnd"),
                position(call, "callEnd"),
            ) else {
                continue;
            };
            let (Some(byte_start), Some(byte_call_end), Some(selector)) = (
                utf16_byte_offset(&text, start),
                utf16_byte_offset(&text, call_end),
                receiver_end
                    .checked_add(1)
                    .and_then(|offset| utf16_byte_offset(&text, offset)),
            ) else {
                continue;
            };
            if position(call, "callStart") != Some(start)
                || position(call, "receiverStart") != Some(start)
                || position(diagnostic, "start") != Some(start)
                || position(diagnostic, "end") != Some(end)
                || position(diagnostic, "position") != Some(receiver_end)
                || start.checked_add(receiver.encode_utf16().count()) != Some(receiver_end)
                || receiver_end
                    .checked_add(1)
                    .and_then(|offset| offset.checked_add(method.encode_utf16().count()))
                    != Some(end)
                || end.checked_add(2) != Some(call_end)
                || text.get(byte_start..byte_call_end)
                    != Some(format!("{receiver}.{method}()").as_str())
            {
                continue;
            }
            let Some(original) = overlay
                .paths
                .get(&kotlin_path_key(&crate::paths::file_key(path)))
            else {
                continue;
            };
            if !seen.insert((original.clone(), selector)) {
                continue;
            }
            let (line, col) = crate::text::LineIndex::new(&text).position(&text, selector);
            result.push(CompileDiagnostic {
                path: original.clone(),
                line: line + 1,
                col: col + 1,
                severity: Severity::Error,
                message: value_string(diagnostic, "message")?.to_owned(),
            });
        }
    }
    api.check_live(request)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(
        owner_name: &str,
        receiver: &str,
        current: &str,
        removed: &str,
    ) -> (
        KotlinEditorOverlay,
        KotlinDiagnosticRequest,
        KotlinEditorOverlay,
        processor::GeneratedSources,
        Value,
        Vec<(PathBuf, String)>,
    ) {
        let parent = std::env::temp_dir().join("ktlsp-current-method-api-controls");
        let owner = KotlinEditorOverlay::empty_in(
            &kotlin_test_request(&parent.join("unused")),
            &parent.join("runs"),
        )
        .unwrap();
        let root = owner.directory.join("project");
        let java = root.join("src/Main.java");
        let generated = root.join(format!("src/generated/{owner_name}.java"));
        std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
        let text = format!("// 前置😀\r\nimport generated.{owner_name}; class Main {{ void use({owner_name} {receiver}) {{ /* 同行😀 */ {receiver}.{removed}(); {receiver}.{current}(); }} }}");
        std::fs::write(&java, &text).unwrap();
        std::fs::write(
            &generated,
            format!("package generated; public class {owner_name} {{ public int {current}() {{ return 2; }} }}"),
        )
        .unwrap();
        let mut request = kotlin_test_request(&root);
        request.sources = vec![java, generated.clone()];
        request.task.source_files = request.sources.clone();
        let produced =
            processor::current_generated_sources_for_test(&request, &[generated]).unwrap();
        let overlay =
            KotlinEditorOverlay::create_in(&request, &owner.directory.join("compiler")).unwrap();
        let expected: Vec<_> = overlay
            .sources
            .iter()
            .map(|p| (p.clone(), sha(&std::fs::read(p).unwrap())))
            .collect();
        let main = overlay
            .sources
            .iter()
            .find(|p| p.file_name().unwrap() == "Main.java")
            .unwrap();
        let api = overlay
            .sources
            .iter()
            .find(|p| p.file_name().unwrap() == format!("{owner_name}.java").as_str())
            .unwrap();
        let byte_start = text.find(&format!("{receiver}.{removed}()")).unwrap();
        let start = text[..byte_start].encode_utf16().count();
        let receiver_end = start + receiver.encode_utf16().count();
        let end = receiver_end + 1 + removed.encode_utf16().count();
        let before: Vec<_> = expected
            .iter()
            .map(|(p, digest)| {
                json!({"path":crate::paths::file_key(p), "sha256":digest,
            "uri":if p == main {"file:///current/Main.java"} else {"file:///current/Api.java"}})
            })
            .collect();
        let value = json!({"sourcesBefore":before,
            "diagnostics":[{"kind":"ERROR", "code":"compiler.err.cant.resolve.location.args", "sourceUri":"file:///current/Main.java",
                "start":start,"end":end,"position":receiver_end,"message":format!("cannot find symbol: method {removed}()") }],
            "methodCalls":[{"sourceUri":"file:///current/Main.java", "simpleOwnedApi":true,"receiver":receiver,"method":removed,
                "receiverType":format!("generated.{owner_name}"), "originKind":"source", "originPath":crate::paths::file_key(api),
                "originSha256":sha(&std::fs::read(api).unwrap()), "publicInstanceZeroArgumentMethods":[current],
                "selectStart":start,"selectEnd":end,"receiverStart":start,"receiverEnd":receiver_end,"callStart":start,"callEnd":end+2}]});
        (owner, request, overlay, produced, value, expected)
    }

    #[test]
    fn processor_method_positive_requires_current_owned_receiver_api_and_exact_method_range() {
        let (_owner, request, overlay, produced, value, expected) =
            fixture("Api", "api", "current", "removed");
        let api =
            MethodApi::prepare(&produced, &request, &overlay, &json!({"outputs":[]})).unwrap();
        let diagnostics = admitted(&value, &request, &overlay, &expected, &api).unwrap();
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics[0].path.ends_with("Main.java"));
        assert_eq!(diagnostics[0].line, 2);
        assert_eq!(
            diagnostics[0].col,
            "import generated.Api; class Main { void use(Api api) { /* 同行😀 */ api."
                .encode_utf16()
                .count() as u32
                + 1
        );
    }

    #[test]
    fn processor_method_positive_accepts_current_japanese_identifiers() {
        for names in [
            ("Api", "対象", "current", "removed"),
            ("Api", "api", "current", "削除済"),
            ("生成API", "api", "現在", "removed"),
        ] {
            let (_owner, request, overlay, produced, value, expected) =
                fixture(names.0, names.1, names.2, names.3);
            let api =
                MethodApi::prepare(&produced, &request, &overlay, &json!({"outputs":[]})).unwrap();
            let diagnostics = admitted(&value, &request, &overlay, &expected, &api).unwrap();
            assert_eq!(diagnostics.len(), 1, "{names:?}");
            assert_eq!(diagnostics[0].line, 2);
            let text = std::fs::read_to_string(&request.sources[0]).unwrap();
            let selector =
                text.find(&format!("{}.{}()", names.1, names.3)).unwrap() + names.1.len() + 1;
            assert_eq!(
                diagnostics[0].col,
                crate::text::LineIndex::new(&text)
                    .position(&text, selector)
                    .1
                    + 1
            );
        }
        for invalid in ["", "1対象", "対象.現在", "対象()", "対象 名前"] {
            assert!(!identifier(invalid), "{invalid:?}");
        }
    }

    #[test]
    fn processor_method_wrong_receiver_origin_current_method_and_range_are_unavailable() {
        let (_owner, request, overlay, produced, value, expected) =
            fixture("Api", "api", "current", "removed");
        let api =
            MethodApi::prepare(&produced, &request, &overlay, &json!({"outputs":[]})).unwrap();
        for kind in [
            "receiver",
            "origin",
            "digest",
            "known-method",
            "range",
            "position",
            "receiver-error",
        ] {
            let mut changed = value.clone();
            match kind {
                "receiver" => changed["methodCalls"][0]["receiver"] = json!("wrong"),
                "origin" => {
                    changed["methodCalls"][0]["originPath"] =
                        json!(request.project_dir.join("saved/Api.java"))
                }
                "digest" => changed["methodCalls"][0]["originSha256"] = json!("old-generation"),
                "known-method" => {
                    changed["methodCalls"][0]["publicInstanceZeroArgumentMethods"] =
                        json!(["current", "removed"])
                }
                "range" => changed["diagnostics"][0]["end"] = json!(1),
                "position" => changed["diagnostics"][0]["position"] = json!(1),
                "receiver-error" => changed["diagnostics"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"kind":"ERROR","sourceUri":"file:///current/Api.java"})),
                _ => unreachable!(),
            }
            assert!(
                admitted(&changed, &request, &overlay, &expected, &api)
                    .unwrap()
                    .is_empty(),
                "{kind}"
            );
        }
        for methods in [json!(["current", false]), json!(["current", "current"])] {
            let mut changed = value.clone();
            changed["methodCalls"][0]["publicInstanceZeroArgumentMethods"] = methods;
            assert!(admitted(&changed, &request, &overlay, &expected, &api).is_err());
        }
    }

    #[test]
    fn processor_method_other_current_generation_cannot_reuse_old_api() {
        let (_owner, mut request, overlay, produced, value, expected) =
            fixture("Api", "api", "current", "removed");
        let api =
            MethodApi::prepare(&produced, &request, &overlay, &json!({"outputs":[]})).unwrap();
        request.generation += 1;
        assert!(admitted(&value, &request, &overlay, &expected, &api).is_err());
    }
}
