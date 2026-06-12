//! # factory-gateway
//!
//! The generic OPC-UA gateway for a factory. It reads one factory's config,
//! instantiates the driver for each machine via the [`registry`], and runs an
//! ISA-95 job-control loop over them ([`gateway::Gateway`]). The OPC-UA server
//! ([`opcua`]) projects the running machines into a standard address space:
//! **OPC UA for Machinery** (`Machines/` + `Identification` + `MachineryItemState`)
//! and **OPC UA for ISA-95 Job Control** (`JobOrderReceiver`).
//!
//! The gateway depends only on [`factory_machine_model`] for the contract and on
//! whichever `factory-<machine>-driver` crates it was built to support.

pub mod config;
pub mod gateway;
pub mod jobs;
pub mod opcua;
pub mod registry;

#[cfg(test)]
mod roundtrip;

pub use config::FactoryConfig;
pub use gateway::Gateway;
pub use registry::DriverRegistry;
