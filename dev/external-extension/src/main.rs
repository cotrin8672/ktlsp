//! Test-only external consumer: production framework policy belongs in its own project.
use std::sync::Arc;

use ktcore::diagnostics::{Diagnostic, Severity};
use ktcore::extensions::source::{AnnotationValue, SourceAnnotation};
use ktcore::extensions::*;
use ktcore::knowledge::Knowledge;
use tower_lsp_server::{LspService, Server};

const PROVIDER: &str = "fixture.external-shadow";
struct ShadowProvider;

fn annotation(read: &mut ReadSession<'_>, annotation: &SourceAnnotation, name: &str) -> bool {
    matches!(read.resolve_type_ref(&annotation.ty), Knowledge::Found(ty) if ty.qualified_name() == name)
}

impl AnalysisProvider for ShadowProvider {
    fn id(&self) -> &str {
        PROVIDER
    }

    fn analyze(&self, read: &mut ReadSession<'_>) -> Result<(), ContractError> {
        let configuration: serde_json::Value =
            serde_json::from_str(read.configuration()).unwrap_or(serde_json::Value::Null);
        let package = configuration
            .get("mixinPackage")
            .and_then(|value| value.as_str())
            .unwrap_or("fixture");
        let declarations = read.declarations().to_vec();
        for owner in &declarations {
            let Some(mixin) = owner
                .annotations
                .iter()
                .find(|item| annotation(read, item, "org.spongepowered.asm.mixin.Mixin"))
            else {
                continue;
            };
            let spelling = &read.source()[owner.definition.start_byte..owner.definition.end_byte];
            let owner_type = read.resolve_type(&format!("{package}.{spelling}"));
            if !matches!(owner_type, Knowledge::Found(ref ty) if ty.definition() == &owner.definition)
            {
                continue;
            }
            let Some(value) = mixin.arguments.iter().find(|argument| {
                argument.name.is_none() || argument.name.as_deref() == Some("value")
            }) else {
                continue;
            };
            // shortcut: one class-literal target only; widen this external fixture for multiple targets.
            let target = match &value.value {
                Knowledge::Found(AnnotationValue::Class(ty)) => read.resolve_type_ref(ty),
                _ => continue,
            };
            for declaration in declarations
                .iter()
                .filter(|item| item.owner.as_ref() == Some(&owner.definition))
            {
                let Some(shadow) = declaration
                    .annotations
                    .iter()
                    .find(|item| annotation(read, item, "org.spongepowered.asm.mixin.Shadow"))
                else {
                    continue;
                };
                let Some(member) = &declaration.member else {
                    continue;
                };
                let mut prefix = "shadow$";
                if let Some(argument) = shadow
                    .arguments
                    .iter()
                    .find(|argument| argument.name.as_deref() == Some("prefix"))
                {
                    let Knowledge::Found(AnnotationValue::String { value, .. }) = &argument.value
                    else {
                        continue;
                    };
                    prefix = value;
                }
                let range = declaration.definition.start_byte..declaration.definition.end_byte;
                let authored_name = read.source()[range.clone()].to_string();
                let (name, link, editable) = match authored_name
                    .strip_prefix(prefix)
                    .filter(|_| !prefix.is_empty())
                {
                    Some(name) => (
                        name.to_string(),
                        NameLink::Prefixed(prefix.into()),
                        range.start + prefix.len()..range.end,
                    ),
                    None => (authored_name, NameLink::Direct, range.clone()),
                };
                let authored = read.declaration(range.clone())?;
                let kind = match &member.signature {
                    Knowledge::Found(signature) => signature.kind(),
                    _ => continue,
                };
                let requirement = read.require(MemberRequirement {
                    target: target.clone(),
                    authored: Some(authored),
                    names: MemberNames::Exact(vec![name]),
                    kind,
                    signature: member.signature.clone(),
                    dispatch: member.dispatch.clone(),
                    name_link: link,
                })?;
                read.bind_site(requirement, range.clone(), editable, RenameSubject::Target)?;
                if !read.resolution(requirement)?.binding.is_found() {
                    // Unknown is useful feedback, never proof that a generated target is absent.
                    read.diagnostic(Diagnostic {
                        start_byte: range.start,
                        end_byte: range.end,
                        severity: Severity::Hint,
                        code: None,
                        message: "External Shadow: target binding is incomplete".into(),
                    })?;
                }
            }
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("ktlsp=info")),
        )
        .init();
    let (service, socket) = LspService::new(|client| {
        ktlsp::lsp::Backend::with_analysis_providers(client, vec![Arc::new(ShadowProvider)])
    });
    Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
        .serve(service)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_source_annotations_bind_and_target_edits_retract() {
        let mut workspace =
            ktlsp::workspace::Workspace::with_analysis_providers(vec![Arc::new(ShadowProvider)]);
        for (file, source) in [
            (
                "Mixin.java",
                include_str!("../project/org/spongepowered/asm/mixin/Mixin.java"),
            ),
            (
                "Shadow.java",
                include_str!("../project/org/spongepowered/asm/mixin/Shadow.java"),
            ),
            (
                "Target.java",
                include_str!("../project/fixture/Target.java"),
            ),
            (
                "CaseMixin.java",
                include_str!("../project/fixture/CaseMixin.java"),
            ),
        ] {
            workspace.open(file, source.into());
        }
        let source = include_str!("../project/fixture/CaseMixin.java");
        let offset = source.find("shadow$twice").unwrap() + "shadow$".len();
        let targets = workspace.goto_definition("CaseMixin.java", offset);
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file, "Target.java");
        workspace.change(
            "Target.java",
            include_str!("../project/fixture/Target.java").replace("twice", "changed"),
        );
        assert!(workspace
            .goto_definition("CaseMixin.java", offset)
            .iter()
            .all(|target| target.file != "Target.java"));
    }
}
