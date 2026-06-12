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

    /// One scheduler pass: for each machine, run its next eligible job, then
    /// refresh its state + telemetry from the driver.
    pub async fn tick(&mut self) -> anyhow::Result<()> {
        for m in self.machines.values_mut() {
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
