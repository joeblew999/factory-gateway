//! ISA-95 JobOrderReceiver — the standard job-control surface (OPC 10031-4).
//!
//! Holds job orders for one machine and tracks each through its [`JobState`]
//! lifecycle. The method names mirror the spec's JobOrderReceiver methods so the
//! gateway's OPC-UA address space can expose them 1:1.

use factory_machine_model::{JobOrder, JobState};

/// A stored job order plus its current state.
#[derive(Debug, Clone)]
pub struct JobEntry {
    pub order: JobOrder,
    pub state: JobState,
}

/// Per-machine job order receiver.
#[derive(Debug, Default)]
pub struct JobOrderReceiver {
    entries: Vec<JobEntry>,
}

impl JobOrderReceiver {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Store` — accept a job order into the queue (state `Stored`).
    pub fn store(&mut self, order: JobOrder) {
        self.entries.push(JobEntry {
            order,
            state: JobState::Stored,
        });
    }

    /// `StoreAndStart` — store, then immediately mark it eligible to run.
    pub fn store_and_start(&mut self, order: JobOrder) {
        self.entries.push(JobEntry {
            order,
            state: JobState::Queued,
        });
    }

    /// `Start` — move a stored order to `Queued` (eligible to run).
    pub fn start(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Queued)
    }

    /// `Pause` — interrupt a running/queued order.
    pub fn pause(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Interrupted)
    }

    /// `Resume` — re-queue a paused order.
    pub fn resume(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Queued)
    }

    /// `Stop` / `Cancel` / `Abort` — terminate an order.
    pub fn abort(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Aborted)
    }

    /// `Clear` — remove terminal orders from the receiver.
    pub fn clear_finished(&mut self) {
        self.entries.retain(|e| !e.state.is_terminal());
    }

    /// Next order eligible to execute (first `Queued`).
    pub fn next_runnable(&self) -> Option<JobOrder> {
        self.entries
            .iter()
            .find(|e| e.state == JobState::Queued)
            .map(|e| e.order.clone())
    }

    pub fn mark_running(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Running)
    }

    pub fn mark_ended(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Ended)
    }

    pub fn entries(&self) -> &[JobEntry] {
        &self.entries
    }

    pub fn queue_depth(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| matches!(e.state, JobState::Stored | JobState::Queued))
            .count()
    }

    fn set(&mut self, job_order_id: &str, state: JobState) -> anyhow::Result<()> {
        let e = self
            .entries
            .iter_mut()
            .find(|e| e.order.job_order_id == job_order_id)
            .ok_or_else(|| anyhow::anyhow!("unknown job order '{job_order_id}'"))?;
        e.state = state;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_store_start_run_end() {
        let mut r = JobOrderReceiver::new();
        r.store(JobOrder::with_payload("J1", "CutListCsv", b"x".to_vec()));
        assert_eq!(r.queue_depth(), 1);
        assert!(r.next_runnable().is_none(), "Stored is not yet runnable");

        r.start("J1").unwrap();
        assert_eq!(r.next_runnable().unwrap().job_order_id, "J1");

        r.mark_running("J1").unwrap();
        r.mark_ended("J1").unwrap();
        assert_eq!(r.queue_depth(), 0);
        r.clear_finished();
        assert!(r.entries().is_empty());
    }
}
