use super::*;
use crate::extension_ipc::{Evaluation, Work, Worker};
use serde_json::Value;

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
    fn late_old_failure_cannot_disable_or_clear_replacement_success() {
        let (index, mut host) = host();
        let old = host.take_ipc_work(&index).remove(0);
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
        let fresh = host.take_ipc_work(&index).remove(0);
        host.ipc_batch(&index,fresh.evaluation,"analysis/finish",json!({"evaluation":fresh.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}})).unwrap();
        host.finish_ipc_work(fresh.evaluation, Ok(()));
        let _next = host.take_ipc_work(&index).remove(0);
        let instance = host.ipc_providers["test.ipc"].worker.id;
        assert!(host.ipc_providers["test.ipc"].running);
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
        let work = host.take_ipc_work(&index).remove(0);
        assert!(host.ipc_batch(&index,work.evaluation,"analysis/readFacts",json!({"evaluation":work.evaluation.to_string(),"queries":[{"op":"declaredMembers","target":"foreign:0"}]})).is_err());
        assert!(host.ipc_batch(&index,work.evaluation,"analysis/finish",json!({"evaluation":work.evaluation.to_string(),"contributions":{"sites":[],"diagnostics":[]}})).is_err());
        host.finish_ipc_work(work.evaluation, Err("invalid batch".into()));
        assert!(host
            .store
            .contributions("test.ipc", "/ipc/Owner.kt")
            .is_none());
        assert!(!host.ipc_providers["test.ipc"].enabled);
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
        let work = host.take_ipc_work(&index).remove(0);
        let result=host.ipc_batch(&index,work.evaluation,"analysis/readFacts",json!({"evaluation":work.evaluation.to_string(),"queries":[{"op":"resolveType","name":"ipc.Unopened"}]})).unwrap();
        assert_eq!(result["facts"][0]["state"], "unknown");
        assert!(host.wanted_ipc_sources().contains(file));
        assert!(host
            .store
            .requested_source_files_for("/ipc/Owner.kt")
            .is_empty());
        assert!(host.source_wait(&index, "/ipc/Owner.kt").is_none());
        host.replace_sources(&index, vec![(file.into(), Arc::from(text), tree)]);
        assert!(!host.ipc_current(work.evaluation));
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

    pub(crate) fn take_ipc_work(&mut self, index: &dyn InferenceIndex) -> Vec<Work> {
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
                evaluation.failure = Some(error.clone());
            }
            if result.is_ok() && method == "analysis/finish" {
                self.store
                    .commit(evaluation.pending.take().unwrap())
                    .map_err(|error| format!("{error:?}"))?;
                self.changed.insert(owner.clone());
            }
            result
        })();
        self.ipc_evaluations.insert(id, evaluation);
        result
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
