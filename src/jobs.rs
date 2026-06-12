//! ISA-95 JobOrderReceiver — the standard job-control surface (OPC 10031-4 §6.2).
//!
//! Holds job orders for one machine and tracks each through its [`JobState`]
//! lifecycle. Method names + semantics mirror the spec's JobOrderReceiver methods
//! so the OPC-UA address space (see [`crate::opcua`]) can expose them 1:1:
//!
//! | Method        | Effect                                            |
//! |---------------|---------------------------------------------------|
//! | `Store`       | add order → `NotAllowedToStart`                   |
//! | `StoreAndStart` | add order → `AllowedToStart`                    |
//! | `Start`       | `NotAllowedToStart` → `AllowedToStart`            |
//! | `RevokeStart` | `AllowedToStart` → `NotAllowedToStart`           |
//! | `Pause`       | → `Interrupted`                                   |
//! | `Resume`      | `Interrupted` → `AllowedToStart`                  |
//! | `Stop`        | → `Completed` (controlled finish)                 |
//! | `Cancel` / `Abort` | → `Aborted`                                  |
//! | `Clear`       | remove terminal orders from the list              |

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

    /// `Store` — accept an order, not yet permitted to run.
    pub fn store(&mut self, order: JobOrder) {
        self.push(order, JobState::NotAllowedToStart);
    }

    /// `StoreAndStart` — accept an order and permit it to run when ready.
    pub fn store_and_start(&mut self, order: JobOrder) {
        self.push(order, JobState::AllowedToStart);
    }

    /// `Start` — permit a stored order to run.
    pub fn start(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::AllowedToStart)
    }

    /// `RevokeStart` — withdraw permission to run.
    pub fn revoke_start(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::NotAllowedToStart)
    }

    /// `Pause` — interrupt a running/permitted order.
    pub fn pause(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Interrupted)
    }

    /// `Resume` — re-permit a paused order.
    pub fn resume(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::AllowedToStart)
    }

    /// `Stop` — controlled finish.
    pub fn stop(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Completed)
    }

    /// `Cancel` / `Abort` — terminate before completion.
    pub fn abort(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Aborted)
    }

    /// `Clear` — remove terminal orders from the receiver.
    pub fn clear_finished(&mut self) {
        self.entries.retain(|e| !e.state.is_terminal());
    }

    // ── Execution helpers (driven by the gateway scheduler) ──────────────────

    /// Next order eligible to execute (first `AllowedToStart`).
    pub fn next_runnable(&self) -> Option<JobOrder> {
        self.entries
            .iter()
            .find(|e| e.state == JobState::AllowedToStart)
            .map(|e| e.order.clone())
    }

    pub fn mark_running(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Running)
    }

    pub fn mark_completed(&mut self, job_order_id: &str) -> anyhow::Result<()> {
        self.set(job_order_id, JobState::Completed)
    }

    pub fn entries(&self) -> &[JobEntry] {
        &self.entries
    }

    /// Orders not yet in a terminal state.
    pub fn queue_depth(&self) -> usize {
        self.entries.iter().filter(|e| !e.state.is_terminal()).count()
    }

    fn push(&mut self, order: JobOrder, state: JobState) {
        self.entries.push(JobEntry { order, state });
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

    fn order(id: &str) -> JobOrder {
        JobOrder::with_payload(id, "CutListCsv", b"x".to_vec())
    }

    #[test]
    fn store_is_not_runnable_until_started() {
        let mut r = JobOrderReceiver::new();
        r.store(order("J1"));
        assert!(r.next_runnable().is_none(), "Store → NotAllowedToStart");
        r.start("J1").unwrap();
        assert_eq!(r.next_runnable().unwrap().job_order_id, "J1");
    }

    #[test]
    fn store_and_start_runs_then_completes_and_clears() {
        let mut r = JobOrderReceiver::new();
        r.store_and_start(order("J1"));
        assert_eq!(r.next_runnable().unwrap().job_order_id, "J1");
        r.mark_running("J1").unwrap();
        r.mark_completed("J1").unwrap();
        assert_eq!(r.queue_depth(), 0);
        r.clear_finished();
        assert!(r.entries().is_empty());
    }

    #[test]
    fn revoke_and_abort() {
        let mut r = JobOrderReceiver::new();
        r.store_and_start(order("J1"));
        r.revoke_start("J1").unwrap();
        assert!(r.next_runnable().is_none());
        r.abort("J1").unwrap();
        assert_eq!(r.queue_depth(), 0);
    }
}
