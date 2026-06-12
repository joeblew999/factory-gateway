//! The gateway runtime — ties config → drivers → ISA-95 job dispatch → telemetry.
//!
//! One [`Gateway`] per factory. It builds a [`Machine`] for every `[[machine]]`
//! in the config (via the [`DriverRegistry`]), accepts ISA-95 job orders, and on
//! each [`Gateway::tick`] runs the next eligible job on each machine and refreshes
//! that machine's state + telemetry. The OPC-UA server (see `opcua` module)
//! projects this runtime into a standard Machinery address space.

use std::collections::BTreeMap;

use factory_machine_model::{BoxedDriver, JobOrder, MachineDescriptor, MachineryItemState, Telemetry};

use crate::config::{FactoryConfig, FactoryInfo, MachineSpec};
use crate::jobs::JobOrderReceiver;
use crate::registry::DriverRegistry;

/// One machine instance: its config, its driver, its job receiver, and the last
/// observed state + telemetry.
pub struct Machine {
    pub spec: MachineSpec,
    pub descriptor: MachineDescriptor,
    pub driver: BoxedDriver,
    pub jobs: JobOrderReceiver,
    pub state: MachineryItemState,
    pub telemetry: Telemetry,
    /// `true` = run by a remote edge agent over OPC-UA (see [`MachineSpec::edge`]).
    pub edge: bool,
    /// For edge machines: the job currently published for the agent to pick up,
    /// as `(job_order_id, payload_utf8)`. `None` when idle.
    pub published_job: Option<(String, String)>,
}

/// The per-factory gateway runtime.
pub struct Gateway {
    pub factory: FactoryInfo,
    pub machines: BTreeMap<String, Machine>,
}

impl Gateway {
    /// Build all machines for a factory from its config and the driver registry.
    pub fn build(config: &FactoryConfig, registry: &DriverRegistry) -> anyhow::Result<Self> {
        let mut machines = BTreeMap::new();
        for spec in &config.machines {
            let driver = registry.build(spec).map_err(|e| {
                anyhow::anyhow!("machine '{}' ({}): {e}", spec.id, spec.driver)
            })?;
            let descriptor = driver.descriptor();
            machines.insert(
                spec.id.clone(),
                Machine {
                    spec: spec.clone(),
                    descriptor,
                    driver,
                    jobs: JobOrderReceiver::new(),
                    state: MachineryItemState::NotAvailable,
                    telemetry: Telemetry::new(),
                    edge: spec.edge,
                    published_job: None,
                },
            );
        }
        Ok(Self {
            factory: config.factory.clone(),
            machines,
        })
    }

    /// Submit an ISA-95 job order to a machine (StoreAndStart semantics).
    pub fn submit(&mut self, machine_id: &str, order: JobOrder) -> anyhow::Result<()> {
        let m = self
            .machines
            .get_mut(machine_id)
            .ok_or_else(|| anyhow::anyhow!("unknown machine '{machine_id}'"))?;
        m.jobs.store_and_start(order);
        Ok(())
    }

    /// One scheduler pass. For **in-process** machines: run the next eligible job
    /// on the local driver. For **edge** machines: publish the next eligible job
    /// for the remote agent to pick up (the agent reports back via
    /// [`Gateway::report_complete`]); never touch the driver locally.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        for m in self.machines.values_mut() {
            if m.edge {
                // Publish the next job for the edge agent (one in flight at a time).
                if m.published_job.is_none()
                    && let Some(order) = m.jobs.next_runnable()
                {
                    let id = order.job_order_id.clone();
                    let csv = String::from_utf8_lossy(order.payload().unwrap_or(&[])).to_string();
                    m.jobs.mark_running(&id)?;
                    tracing::info!(machine = %m.spec.id, job = %id, "published to edge agent");
                    m.published_job = Some((id, csv));
                }
                m.state = if m.published_job.is_some() {
                    MachineryItemState::Executing
                } else {
                    MachineryItemState::NotExecuting
                };
                continue; // telemetry for edge machines is reported by the agent
            }

            // In-process machine.
            if let Some(order) = m.jobs.next_runnable() {
                let id = order.job_order_id.clone();
                m.jobs.mark_running(&id)?;
                match m.driver.run_job(&order).await {
                    Ok(()) => {
                        m.jobs.mark_completed(&id)?;
                        tracing::info!(machine = %m.spec.id, job = %id, "job completed");
                    }
                    Err(e) => {
                        m.jobs.abort(&id)?;
                        tracing::warn!(machine = %m.spec.id, job = %id, "job aborted: {e}");
                    }
                }
            }
            m.state = m.driver.state().await;
            m.telemetry = m.driver.poll_telemetry().await.unwrap_or_default();
        }
        Ok(())
    }

    /// An edge agent reports a published job finished. Marks it completed and
    /// clears the published slot so the next job can be dispatched.
    pub fn report_complete(&mut self, machine_id: &str, job_order_id: &str) {
        if let Some(m) = self.machines.get_mut(machine_id) {
            let _ = m.jobs.mark_completed(job_order_id);
            if m.published_job.as_ref().map(|(id, _)| id == job_order_id).unwrap_or(false) {
                m.published_job = None;
            }
            tracing::info!(machine = %machine_id, job = %job_order_id, "edge agent reported complete");
        }
    }

    /// An edge agent reports telemetry for its machine.
    pub fn report_telemetry(&mut self, machine_id: &str, telemetry: Telemetry) {
        if let Some(m) = self.machines.get_mut(machine_id) {
            m.telemetry = telemetry;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [factory]
        id = "test"
        name = "Test"
        [[machine]]
        id = "howick-1"
        driver = "howick-frama"
        [machine.identification]
        manufacturer = "Howick"
        model = "FRAMA"
        [machine.howick]
        usb_mount = "/tmp/factory-gateway-test"
    "#;

    #[tokio::test]
    async fn builds_machines_and_runs_a_job() {
        let cfg = FactoryConfig::from_toml(SAMPLE).unwrap();
        let reg = DriverRegistry::with_builtin_drivers();
        let mut gw = Gateway::build(&cfg, &reg).unwrap();
        assert_eq!(gw.machines.len(), 1);

        gw.submit("howick-1", JobOrder::with_payload("J1", "CutListCsv", b"UNIT\n".to_vec()))
            .unwrap();
        gw.tick().await.unwrap();

        let m = &gw.machines["howick-1"];
        assert_eq!(m.state, MachineryItemState::NotExecuting); // job finished this tick
        assert_eq!(m.jobs.queue_depth(), 0);
        assert_eq!(m.descriptor.identification.manufacturer, "Howick");
    }
}
