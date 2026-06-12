//! Driver registry — maps a config `driver = "..."` string to a constructor that
//! builds the driver from its `MachineSpec`. This is where the gateway binary
//! declares which machine types it supports; a gateway is built with the driver
//! set its factories need.
//!
//! Adding support for a new machine = depend on that `factory-<machine>-driver`
//! crate and `register` it here (or via a cargo feature).

use std::collections::HashMap;

use factory_machine_model::BoxedDriver;
use factory_howick_driver::{HowickConfig, HowickFrama};

use crate::config::MachineSpec;

/// Builds a boxed driver from a machine's config.
pub type Constructor = fn(&MachineSpec) -> anyhow::Result<BoxedDriver>;

/// Registry of supported driver kinds.
pub struct DriverRegistry {
    constructors: HashMap<&'static str, Constructor>,
}

impl DriverRegistry {
    /// A registry with every driver this gateway was built to support.
    pub fn with_builtin_drivers() -> Self {
        let mut r = Self {
            constructors: HashMap::new(),
        };
        r.register(factory_howick_driver::KIND, build_howick);
        r
    }

    pub fn register(&mut self, kind: &'static str, ctor: Constructor) {
        self.constructors.insert(kind, ctor);
    }

    /// Instantiate the driver for a machine spec, or error if its kind is unknown.
    pub fn build(&self, spec: &MachineSpec) -> anyhow::Result<BoxedDriver> {
        let ctor = self
            .constructors
            .get(spec.driver.as_str())
            .ok_or_else(|| anyhow::anyhow!("no driver registered for kind '{}'", spec.driver))?;
        ctor(spec)
    }

    pub fn supported_kinds(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.constructors.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

/// Constructor for the Howick FRAMA driver — reads the `[machine.howick]` table.
fn build_howick(spec: &MachineSpec) -> anyhow::Result<BoxedDriver> {
    let cfg: HowickConfig = spec.driver_config("howick")?;
    Ok(Box::new(HowickFrama::new(
        spec.id.clone(),
        spec.identification.clone(),
        cfg,
    )))
}
