//! Factory configuration — one file per factory. It *is* that factory's OPC-UA
//! Machinery `Machines/` folder, expressed as TOML:
//!
//! ```toml
//! [factory]
//! id   = "si-racha"
//! name = "Prin — Si Racha"
//!
//! [opcua]
//! host = "0.0.0.0"
//! port = 4840
//!
//! [[machine]]
//! id     = "howick-1"
//! driver = "howick-frama"
//! [machine.identification]          # standard OPC-UA Machinery nameplate
//! manufacturer = "Howick"
//! model        = "FRAMA"
//! [machine.howick]                  # driver-specific, parsed by the driver
//! usb_mount   = "/mnt/usb_share"
//! coil_sensor = true
//! ```
//!
//! The gateway parses `[factory]`, `[opcua]`, and each `[[machine]]`'s `id` /
//! `driver` / `[machine.identification]`. The remaining per-machine table(s)
//! (e.g. `[machine.howick]`) are captured verbatim and handed to the driver.

use factory_machine_model::Identification;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct FactoryConfig {
    pub factory: FactoryInfo,
    #[serde(default)]
    pub opcua: OpcUaSettings,
    #[serde(default)]
    pub http: HttpSettings,
    #[serde(default, rename = "machine")]
    pub machines: Vec<MachineSpec>,
}

/// Operator HTTP dashboard + job-submit endpoint.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct HttpSettings {
    pub host: String,
    pub port: u16,
}

impl Default for HttpSettings {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_owned(),
            port: 4841,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FactoryInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpcUaSettings {
    pub host: String,
    pub port: u16,
    /// Namespace URI for this factory's machine nodes.
    pub namespace_uri: String,
}

impl Default for OpcUaSettings {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_owned(),
            port: 4840,
            namespace_uri: "http://joeblew999.github.io/factory-floor/".to_owned(),
        }
    }
}

/// One machine in the factory.
#[derive(Debug, Clone, Deserialize)]
pub struct MachineSpec {
    /// `Machines/<id>` browse key.
    pub id: String,
    /// Which driver kind handles it (matches a registered driver).
    pub driver: String,
    /// Standard OPC-UA Machinery nameplate.
    #[serde(default)]
    pub identification: Identification,
    /// `true` = this machine is run by a remote **edge agent** (a separate
    /// process physically wired to it) that connects to the gateway over OPC-UA.
    /// The gateway publishes jobs for it and waits for the agent to report back,
    /// rather than running the driver in-process. This is the real factory-floor
    /// topology for distributed machines. `false` = run the driver in-process
    /// (fine for a single co-located machine).
    #[serde(default)]
    pub edge: bool,
    /// Any remaining `[machine.<key>]` tables — driver-specific config, opaque here.
    #[serde(flatten)]
    pub extra: toml::Table,
}

impl MachineSpec {
    /// The driver-specific config table under the given key (e.g. `"howick"`),
    /// deserialized into the driver's typed config.
    pub fn driver_config<T: serde::de::DeserializeOwned>(&self, key: &str) -> anyhow::Result<T> {
        match self.extra.get(key) {
            Some(v) => Ok(v.clone().try_into()?),
            None => Ok(toml::Value::Table(toml::Table::new()).try_into()?),
        }
    }
}

impl FactoryConfig {
    pub fn from_toml(s: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(s)?)
    }

    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        Self::from_toml(&std::fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
        [factory]
        id = "si-racha"
        name = "Prin — Si Racha"

        [[machine]]
        id = "howick-1"
        driver = "howick-frama"
        [machine.identification]
        manufacturer = "Howick"
        model = "FRAMA"
        [machine.howick]
        usb_mount = "/mnt/usb_share"
        coil_sensor = true
    "#;

    #[test]
    fn parses_factory_and_machines() {
        let c = FactoryConfig::from_toml(SAMPLE).unwrap();
        assert_eq!(c.factory.id, "si-racha");
        assert_eq!(c.machines.len(), 1);
        let m = &c.machines[0];
        assert_eq!(m.id, "howick-1");
        assert_eq!(m.driver, "howick-frama");
        assert_eq!(m.identification.manufacturer, "Howick");
        // driver-specific table is captured and routable
        assert!(m.extra.contains_key("howick"));
    }
}
