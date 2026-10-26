use super::*;
use crate::classpath::ProjectSnapshot;
use crate::extension_ipc::{Evaluation, Work, Worker};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) struct Slot {
    command: Vec<String>,
    pub enabled: bool,
    configuration: Arc<str>,
    worker: Arc<Worker>,
    pub queued: BTreeSet<String>,
    running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use ktcore::index::Tier;
    use ktcore::parser::KotlinParser;
    use serde_json::json;

    fn host() -> (Index, AnalysisExtensions) {
        let mut index = Index::new();
        let mut host = AnalysisExtensions::new(Vec::new());
        host.register_ipc(&index, "test.ipc", vec!["fixture-child".into()])
            .unwrap();
        let mut sources = Vec::new();
        for (file, text) in [
            (
                "/ipc/Owner.kt",
                "package ipc\nclass Owner { fun local(): Int = 1 }",
            ),
            (
                "/ipc/Target.kt",
                "package ipc\nclass Target { fun value(): Int = 2 }",
            ),
        ] {
            let tree = KotlinParser::new().parse(text);
            index.replace_file(
                file,
                ktcore::indexer::extract_symbols(&tree, text, "ipc"),
                Tier::Volatile,
            );
            sources.push((file.into(), Arc::from(text), tree));
        }
        host.replace_sources(&index, sources);
        (index, host)
    }

    #[test]
    fn ipc_evaluation_shares_the_accepted_model_and_uses_its_read_set_lifetime() {
        let (index, mut host) = host();
        let model = Arc::new(ProjectSnapshot::new(crate::classpath::parse_dump(
            "PROJECT\t:selected\t/project\nSOURCE\tcustom\t/ipc\nSCP\tcustom\t/library.jar\nEND\t:selected\n",
        )));
        let work = host.take_ipc_work(&index, Some(&model)).remove(0);
        assert!(Arc::ptr_eq(
            host.ipc_evaluations[&work.evaluation]
                .project
                .as_ref()
                .unwrap(),
            &model
        ));
        assert_eq!(Arc::strong_count(&model), 2);
        assert!(work.params.get("project").is_none());
        assert!(work.params.get("projectContext").is_none());
        host.ipc_batch(
            &index,
            work.evaluation,
            "analysis/readFacts",
            json!({"evaluation":work.evaluation.to_string(),"queries":[]}),
        )
        .unwrap();
        host.model_changed(&index);
        assert!(!host.ipc_current(work.evaluation));
        assert!(
            host.ipc_batch(
                &index,
                work.evaluation,
                "analysis/readFacts",
                json!({"evaluation":work.evaluation.to_string(),"queries":[]}),
            )
            .is_err()
        );
        assert!(host.ipc_batch(
            &index,
            work.evaluation,
            "analysis/finish",
            json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}}),
        ).is_err());
        assert!(host.ipc_publication(work.evaluation).is_none());
    }

    #[test]
    fn completion_pending_tracks_queued_and_running_owners_until_commit() {
        let (index, mut host) = host();
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
        assert!(host.ipc_completion_pending("/ipc/Target.kt"));
        assert!(!host.ipc_completion_pending("/ipc/Missing.kt"));
        let work = host.take_ipc_work(&index, None).remove(0);
        assert_eq!(work.params["owner"], "/ipc/Owner.kt");
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
        host.ipc_batch(
            &index,
            work.evaluation,
            "analysis/finish",
            json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}}),
        )
        .unwrap();
        assert!(!host.ipc_completion_pending("/ipc/Owner.kt"));
        assert!(host.ipc_completion_pending("/ipc/Target.kt"));
        host.finish_ipc_work(work.evaluation, Ok(()));
        let other = host.take_ipc_work(&index, None).remove(0);
        assert_eq!(other.params["owner"], "/ipc/Target.kt");
        assert!(host.ipc_completion_pending("/ipc/Target.kt"));
        assert!(!host.ipc_completion_pending("/ipc/Owner.kt"));
        host.configure(&index, "test.ipc", false, Arc::from("{}"));
        assert!(!host.ipc_completion_pending("/ipc/Owner.kt"));
        assert!(!host.ipc_completion_pending("/ipc/Target.kt"));
    }

    #[test]
    fn stale_evaluation_is_not_pending_until_fresh_work_is_queued() {
        let (index, mut host) = host();
        let work = host.take_ipc_work(&index, None).remove(0);
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
        host.store.invalidate_model();
        assert!(!host.ipc_current(work.evaluation));
        assert!(!host.ipc_completion_pending("/ipc/Owner.kt"));
        host.requeue_stale_ipc();
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
    }

    #[test]
    fn publication_requires_current_commit_and_index_publication() {
        let (mut index, mut host) = host();
        let file = "/ipc/Owner.kt";
        let text = "package ipc\nclass Owner { val text = \"雪☃\" }";
        let tree = KotlinParser::new().parse(text);
        index.replace_file(
            file,
            ktcore::indexer::extract_symbols(&tree, text, "ipc"),
            Tier::Volatile,
        );
        host.replace_sources(&index, vec![(file.into(), Arc::from(text), tree)]);
        let work = host.take_ipc_work(&index, None).remove(0);
        assert_eq!(work.params["source"], text);
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_none());
        host.ipc_batch(
            &index,
            work.evaluation,
            "analysis/finish",
            json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}}),
        )
        .unwrap();
        assert!(host.ipc_publication(work.evaluation).is_none());
        host.publish(&mut index);
        let publication = host.ipc_publication(work.evaluation).unwrap();
        assert_eq!(publication.as_object().unwrap().len(), 6);
        assert_eq!(publication["provider"], "test.ipc");
        assert_eq!(publication["owner"], file);
        assert_eq!(publication["instance"], work.worker.id);
        assert_eq!(publication["evaluation"], work.evaluation.to_string());
        assert_eq!(publication["publication_revision"], host.published_revision);
        assert_eq!(
            publication["source_sha256"],
            "484c4648a722d52bcb9403cfa2bc0737067163f43c2f49ef4ec9b0a04bf15d0e"
        );

        host.store
            .configure_provider("test.other", true, Arc::from("{}"));
        let read = host
            .store
            .begin(&index, "test.other", "/ipc/Target.kt")
            .unwrap();
        let pending = read.finish();
        host.store.commit(pending).unwrap();
        host.publish(&mut index);
        let later = host.ipc_publication(work.evaluation).unwrap();
        assert!(
            later["publication_revision"].as_u64().unwrap()
                > publication["publication_revision"].as_u64().unwrap()
        );
        assert_eq!(later["source_sha256"], publication["source_sha256"]);

        let changed = "package ipc\nclass Owner { fun changed() = 1 }";
        let tree = KotlinParser::new().parse(changed);
        index.replace_file(
            file,
            ktcore::indexer::extract_symbols(&tree, changed, "ipc"),
            Tier::Volatile,
        );
        host.replace_sources(&index, vec![(file.into(), Arc::from(changed), tree)]);
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_none());
    }

    #[test]
    fn replacement_retracts_a_previously_published_evaluation() {
        let (mut index, mut host) = host();
        let work = host.take_ipc_work(&index, None).remove(0);
        host.ipc_batch(
            &index,
            work.evaluation,
            "analysis/finish",
            json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}}),
        )
        .unwrap();
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_some());
        host.register_ipc(&index, "test.ipc", vec!["replacement-child".into()])
            .unwrap();
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_none());
        host.finish_ipc_work(work.evaluation, Err("late old failure".into()));
        assert!(host.ipc_publication(work.evaluation).is_none());
    }

    #[test]
    fn late_old_failure_cannot_disable_or_clear_replacement_success() {
        let (mut index, mut host) = host();
        let old = host.take_ipc_work(&index, None).remove(0);
        host.register_ipc(&index, "test.ipc", vec!["replacement-child".into()])
            .unwrap();
        assert!(host
            .ipc_batch(
                &index,
                old.evaluation,
                "analysis/readFacts",
                json!({"evaluation":old.evaluation.to_string(),"queries":[]})
            )
            .is_err());
        assert!(host.ipc_publication(old.evaluation).is_none());
        let fresh = host.take_ipc_work(&index, None).remove(0);
        host.ipc_batch(&index,fresh.evaluation,"analysis/finish",json!({"evaluation":fresh.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}})).unwrap();
        host.publish(&mut index);
        assert!(host.ipc_publication(fresh.evaluation).is_some());
        host.finish_ipc_work(fresh.evaluation, Ok(()));
        let _next = host.take_ipc_work(&index, None).remove(0);
        let instance = host.ipc_providers["test.ipc"].worker.id;
        assert!(host.ipc_providers["test.ipc"].running);
        assert!(host.ipc_publication(old.evaluation).is_none());
        assert!(host.ipc_publication(fresh.evaluation).is_none());
        host.finish_ipc_work(
            old.evaluation,
            Err("late child crash after consumed pending".into()),
        );
        assert_eq!(host.ipc_providers["test.ipc"].worker.id, instance);
        assert!(host.ipc_providers["test.ipc"].enabled);
        assert!(host.ipc_providers["test.ipc"].running);
        assert!(host
            .store
            .contributions("test.ipc", "/ipc/Owner.kt")
            .is_some());
    }

    #[test]
    fn foreign_token_poison_prevents_partial_publication() {
        let (index, mut host) = host();
        let work = host.take_ipc_work(&index, None).remove(0);
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
        assert!(host.ipc_batch(&index,work.evaluation,"analysis/readFacts",json!({"evaluation":work.evaluation.to_string(),"queries":[{"op":"declaredMembers","target":"foreign:0"}]})).is_err());
        assert!(!host.ipc_completion_pending("/ipc/Owner.kt"));
        assert!(host
            .ipc_batch(
                &index,
                work.evaluation,
                "analysis/finish",
                json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}})
            )
            .is_err());
        assert!(host.ipc_publication(work.evaluation).is_none());
        host.finish_ipc_work(work.evaluation, Err("invalid batch".into()));
        assert!(host.ipc_publication(work.evaluation).is_none());
        assert!(host
            .store
            .contributions("test.ipc", "/ipc/Owner.kt")
            .is_none());
        assert!(!host.ipc_providers["test.ipc"].enabled);
        assert!(!host.ipc_completion_pending("/ipc/Target.kt"));
    }

    #[test]
    fn staged_source_frontier_does_not_enter_completion_wait() {
        let (mut index, mut host) = host();
        let file = "/ipc/Unopened.kt";
        let text = "package ipc\nclass Unopened";
        let tree = KotlinParser::new().parse(text);
        index.replace_file(
            file,
            ktcore::indexer::extract_symbols(&tree, text, "ipc"),
            Tier::Volatile,
        );
        let work = host.take_ipc_work(&index, None).remove(0);
        let result=host.ipc_batch(&index,work.evaluation,"analysis/readFacts",json!({"evaluation":work.evaluation.to_string(),"queries":[{"op":"resolveType","name":"ipc.Unopened"}]})).unwrap();
        assert_eq!(result["facts"][0]["state"], "unknown");
        assert!(host.wanted_ipc_sources().contains(file));
        assert!(host
            .store
            .requested_source_files_for("/ipc/Owner.kt")
            .is_empty());
        assert!(host.source_wait(&index, "/ipc/Owner.kt").is_none());
        assert!(host.ipc_completion_pending("/ipc/Owner.kt"));
        host.replace_sources(&index, vec![(file.into(), Arc::from(text), tree)]);
        assert!(!host.ipc_current(work.evaluation));
        assert!(host
            .ipc_batch(
                &index,
                work.evaluation,
                "analysis/finish",
                json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}})
            )
            .is_err());
        host.publish(&mut index);
        assert!(host.ipc_publication(work.evaluation).is_none());
        assert!(host.ipc_providers["test.ipc"]
            .queued
            .contains("/ipc/Owner.kt"));
    }
}

impl AnalysisExtensions {
    pub(crate) fn register_ipc(
        &mut self,
        index: &dyn InferenceIndex,
        id: &str,
        command: Vec<String>,
    ) -> Result<bool, String> {
        if id.is_empty()
            || self.providers.contains_key(id)
            || command.first().is_none_or(|item| item.is_empty())
        {
            return Err("invalid or duplicate extension registration".into());
        }
        if self
            .ipc_providers
            .get(id)
            .is_some_and(|slot| slot.command == command)
        {
            return Ok(false);
        }
        if let Some(old) = self.ipc_providers.remove(id) {
            old.worker.stop();
        }
        self.changed.extend(
            self.store
                .contribution_sets()
                .filter(|(provider, _, _)| *provider == id)
                .map(|(_, owner, _)| owner.to_string()),
        );
        self.store.configure_provider(id, false, Arc::from("{}"));
        self.ipc_providers.insert(
            id.into(),
            Slot {
                worker: Worker::new(command.clone()),
                command,
                enabled: true,
                configuration: Arc::from("{}"),
                queued: BTreeSet::new(),
                running: false,
            },
        );
        self.retain_all_sources = true;
        self.store.configure_provider(id, true, Arc::from("{}"));
        self.normalize_all(index);
        self.evaluate(index);
        Ok(true)
    }

    pub(super) fn configure_ipc(&mut self, id: &str, enabled: bool, config: &Arc<str>) {
        if let Some(slot) = self.ipc_providers.get_mut(id) {
            if slot.enabled != enabled || &slot.configuration != config {
                slot.worker.stop();
                slot.worker = Worker::new(slot.command.clone());
                slot.running = false;
                slot.enabled = enabled;
                slot.configuration = config.clone();
                if !enabled {
                    slot.queued.clear();
                }
            }
        }
    }

    pub(super) fn requeue_stale_ipc(&mut self) {
        for evaluation in self.ipc_evaluations.values() {
            if evaluation
                .pending
                .as_ref()
                .is_some_and(|pending| self.store.validate_pending(pending).is_err())
            {
                if let Some(slot) = self.ipc_providers.get_mut(&evaluation.provider) {
                    if slot.enabled {
                        slot.queued.insert(evaluation.owner.clone());
                    }
                }
            }
        }
    }

    pub(crate) fn take_ipc_work(
        &mut self,
        index: &dyn InferenceIndex,
        project: Option<&Arc<ProjectSnapshot>>,
    ) -> Vec<Work> {
        let dead: Vec<_> = self
            .ipc_providers
            .iter()
            .filter(|(_, slot)| slot.enabled && slot.worker.failed())
            .map(|(id, _)| id.clone())
            .collect();
        for id in dead {
            let slot = self.ipc_providers.get_mut(&id).unwrap();
            slot.enabled = false;
            slot.queued.clear();
            slot.worker.stop();
            self.changed.extend(
                self.store
                    .contribution_sets()
                    .filter(|(provider, _, _)| *provider == id)
                    .map(|(_, owner, _)| owner.to_string()),
            );
            self.store
                .configure_provider(&id, false, slot.configuration.clone());
            crate::trace::span(
                "extension.ipc.exited",
                "extension",
                std::time::Instant::now(),
                serde_json::json!({"provider":id,"instance":slot.worker.id}),
            );
        }
        if self.suspended {
            return Vec::new();
        }
        let mut work = Vec::new();
        let ids: Vec<_> = self.ipc_providers.keys().cloned().collect();
        for provider in ids {
            let slot = self.ipc_providers.get_mut(&provider).unwrap();
            if !slot.enabled || slot.running {
                continue;
            }
            let Some(owner) = slot.queued.pop_first() else {
                continue;
            };
            let worker = slot.worker.clone();
            let mut evaluation = Evaluation::new(provider.clone(), owner.clone(), worker.id);
            let Ok(mut read) = self.store.begin(index, &provider, &owner) else {
                continue;
            };
            // begin's model read guards this snapshot through resume and commit.
            evaluation.project = project.cloned();
            let params = evaluation.source(&mut read);
            evaluation.pending = Some(read.finish());
            match params {
                Ok(params) => {
                    slot.running = true;
                    work.push(Work {
                        evaluation: evaluation.id,
                        provider,
                        worker,
                        params,
                    });
                    self.ipc_evaluations.insert(evaluation.id, evaluation);
                }
                Err(error) => {
                    let _ = self.store.fail(evaluation.pending.take().unwrap());
                    tracing::warn!(provider, owner, error, "extension snapshot rejected");
                }
            }
        }
        work
    }

    pub(crate) fn ipc_current(&self, id: u64) -> bool {
        self.ipc_evaluations
            .get(&id)
            .and_then(|evaluation| evaluation.pending.as_ref())
            .is_some_and(|pending| self.store.validate_pending(pending).is_ok())
    }

    pub(crate) fn ipc_completion_pending(&self, owner: &str) -> bool {
        self.inputs.contains_key(owner)
            && self.ipc_providers.iter().any(|(provider, slot)| {
                slot.enabled
                    && (slot.queued.contains(owner)
                        || (slot.running
                            && self.ipc_evaluations.values().any(|evaluation| {
                                evaluation.provider == *provider
                                    && evaluation.owner == owner
                                    && evaluation.instance == slot.worker.id
                                    && evaluation.failure.is_none()
                                    && evaluation.pending.as_ref().is_some_and(|pending| {
                                        self.store.validate_pending(pending).is_ok()
                                    })
                            })))
            })
    }

    pub(crate) fn ipc_batch(
        &mut self,
        index: &dyn InferenceIndex,
        id: u64,
        method: &str,
        params: Value,
    ) -> Result<Value, String> {
        let mut evaluation = self.ipc_evaluations.remove(&id).ok_or("Stale")?;
        if let Some(error) = evaluation.failure.clone() {
            self.ipc_evaluations.insert(id, evaluation);
            return Err(error);
        }
        evaluation.project_operations = self
            .ipc_providers
            .get(&evaluation.provider)
            .filter(|slot| slot.worker.id == evaluation.instance)
            .map_or(0, |slot| slot.worker.project_operations());
        let owner = evaluation.owner.clone();
        let Some(pending) = evaluation.pending.take() else {
            self.ipc_evaluations.insert(id, evaluation);
            return Err("Stale".into());
        };
        let result = (|| {
            let mut read = self
                .store
                .resume(index, &owner, pending)
                .map_err(|error| format!("{error:?}"))?;
            let result = evaluation.batch(&mut read, method, params);
            evaluation.pending = Some(read.finish());
            if let Err(error) = &result {
                if method != "analysis/readFacts"
                    || error != crate::extension_ipc::READ_FACTS_RESPONSE_BUDGET_EXCEEDED
                {
                    evaluation.failure = Some(error.clone());
                }
            }
            if result.is_ok() && method == "analysis/finish" {
                self.store
                    .commit(evaluation.pending.take().unwrap())
                    .map_err(|error| format!("{error:?}"))?;
                evaluation.committed_revision = Some(self.store.publication_revision());
                self.changed.insert(owner.clone());
            }
            result
        })();
        self.ipc_evaluations.insert(id, evaluation);
        result
    }

    pub(crate) fn ipc_publication(&self, id: u64) -> Option<Value> {
        let evaluation = self.ipc_evaluations.get(&id)?;
        let committed = evaluation.committed_revision?;
        let slot = self.ipc_providers.get(&evaluation.provider)?;
        if evaluation.pending.is_some()
            || evaluation.failure.is_some()
            || !slot.enabled
            || !slot.running
            || slot.worker.id != evaluation.instance
            || self.published_revision != self.store.publication_revision()
            || committed > self.published_revision
            || self
                .store
                .contributions(&evaluation.provider, &evaluation.owner)
                .is_none()
        {
            return None;
        }
        let source = self.inputs.get(&evaluation.owner)?;
        let source_sha256 = format!("{:x}", Sha256::digest(source.text.as_bytes()));
        Some(serde_json::json!({
            "provider": evaluation.provider, "owner": evaluation.owner,
            "instance": evaluation.instance, "evaluation": id.to_string(),
            "publication_revision": self.published_revision, "source_sha256": source_sha256,
        }))
    }

    pub(crate) fn finish_ipc_work(&mut self, id: u64, result: Result<(), String>) {
        let Some(mut evaluation) = self.ipc_evaluations.remove(&id) else {
            return;
        };
        let result = if evaluation
            .pending
            .as_ref()
            .is_some_and(|pending| self.store.validate_pending(pending).is_err())
        {
            Err("Stale".into())
        } else {
            result
        };
        if let Some(pending) = evaluation.pending.take() {
            let _ = self.store.fail(pending);
        }
        let Some(slot) = self.ipc_providers.get_mut(&evaluation.provider) else {
            return;
        };
        if slot.worker.id != evaluation.instance {
            return;
        }
        slot.running = false;
        if let Err(error) = result {
            tracing::warn!(
                provider = evaluation.provider,
                owner = evaluation.owner,
                error,
                "extension evaluation rejected"
            );
            crate::trace::span(
                "extension.ipc.rejected",
                "extension",
                std::time::Instant::now(),
                serde_json::json!({"provider":evaluation.provider,"owner":evaluation.owner,"evaluation":id,"reason":error}),
            );
            if error == "Stale" {
                if slot.enabled {
                    slot.queued.insert(evaluation.owner);
                }
            } else {
                self.changed.extend(
                    self.store
                        .contribution_sets()
                        .filter(|(provider, _, _)| *provider == evaluation.provider)
                        .map(|(_, owner, _)| owner.to_string()),
                );
                self.store.configure_provider(
                    &evaluation.provider,
                    false,
                    slot.configuration.clone(),
                );
                // A failed child stays disabled until an explicit enable/configuration change.
                slot.enabled = false;
                slot.worker.stop();
                slot.queued.clear();
            }
        }
    }

    pub(super) fn wanted_ipc_sources(&self) -> BTreeSet<String> {
        self.ipc_evaluations
            .values()
            .filter_map(|evaluation| evaluation.pending.as_ref())
            .filter(|pending| self.store.validate_pending(pending).is_ok())
            .flat_map(|pending| pending.requested_source_files().iter().cloned())
            .collect()
    }

    pub(crate) fn stop_ipc(&mut self) -> Vec<Arc<Worker>> {
        let mut workers = Vec::new();
        let ids: Vec<_> = self.ipc_providers.keys().cloned().collect();
        for id in ids {
            let slot = self.ipc_providers.get_mut(&id).unwrap();
            slot.enabled = false;
            slot.queued.clear();
            slot.worker.stop();
            workers.push(slot.worker.clone());
            self.changed.extend(
                self.store
                    .contribution_sets()
                    .filter(|(provider, _, _)| *provider == id)
                    .map(|(_, owner, _)| owner.to_string()),
            );
            self.store
                .configure_provider(&id, false, slot.configuration.clone());
        }
        workers
    }
}
