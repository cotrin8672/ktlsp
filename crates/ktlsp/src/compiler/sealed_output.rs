//! Retained output ownership shared by ordinary compiler invocations.
use super::*;
use serde_json::Value;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

pub(in crate::compile) struct SealedOutput {
    roots: Vec<PathBuf>,
    facts: Vec<Value>,
    invalid: Arc<AtomicBool>,
    _watch: Arc<crate::compiled_watch::NativeInputWatch>,
}

impl SealedOutput {
    /// Cheap retained-watch epoch check for stored receipts under workspace locks.
    /// Full byte/inventory verification remains on the background compiler path.
    pub(in crate::compile) fn epoch_live(&self) -> bool {
        !self.invalid.load(Ordering::Acquire)
    }

    pub(in crate::compile) fn seal(roots: Vec<PathBuf>) -> anyhow::Result<Self> {
        let invalid = Arc::new(AtomicBool::new(false));
        let plan = crate::compiled_watch::WatchPlan::read_inputs(&roots)?;
        let (wake, _receiver) = tokio::sync::mpsc::channel(1);
        let watch = crate::compiled_watch::NativeInputWatch::new(invalid.clone(), plan, wake)?;
        // Coverage precedes the first inventory; absence and membership are reads too.
        let facts = roots
            .iter()
            .map(|p| inventory(p))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let lease = Self {
            roots,
            facts,
            invalid,
            _watch: watch,
        };
        lease.check_live()?;
        Ok(lease)
    }

    pub(in crate::compile) fn check_live(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "processor-sealed-output-changed"
        );
        for (path, expected) in self.roots.iter().zip(&self.facts) {
            anyhow::ensure!(
                inventory(path)? == *expected,
                "processor-sealed-output-inventory-changed"
            );
        }
        anyhow::ensure!(
            !self.invalid.load(Ordering::Acquire),
            "processor-sealed-output-changed"
        );
        Ok(())
    }
}

