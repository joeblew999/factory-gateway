//! OPC-UA server — projects the [`Gateway`] runtime into a **standard** address
//! space: OPC UA for Machinery (`Machines/<id>/Identification` +
//! `MachineryItemState` + `Telemetry/`). A background task drives the gateway and
//! syncs live values into the nodes.
//!
//! The ISA-95 JobOrderReceiver *methods* (Store/Start/…) are the planned next
//! addition to this transport; the job-control logic itself already lives in
//! [`crate::jobs`] and [`crate::gateway`] and is reachable via the gateway API.
//!
//! Address-space construction mirrors the async-opcua `SimpleNodeManager` pattern.

use std::sync::Arc;
use std::time::Duration;

use opcua::server::address_space::Variable;
use opcua::server::diagnostics::NamespaceMetadata;
use opcua::server::node_manager::memory::{
    simple_node_manager, InMemoryNodeManager, SimpleNodeManager, SimpleNodeManagerImpl,
};
use opcua::server::ServerBuilder;
use opcua::types::{BuildInfo, DataValue, DateTime, NodeId, UAString};
use tokio::sync::Mutex;

use factory_machine_model::{MachineDescriptor, Value};

use crate::config::OpcUaSettings;
use crate::gateway::Gateway;

fn node(ns: u16, name: &str) -> NodeId {
    NodeId::new(ns, name)
}

/// Build and run the OPC-UA server, projecting `gw` into a Machinery address space.
pub async fn serve(settings: OpcUaSettings, gw: Arc<Mutex<Gateway>>) -> anyhow::Result<()> {
    // Snapshot the machine descriptors to build the static node-tree.
    let descriptors: Vec<MachineDescriptor> = {
        let g = gw.lock().await;
        g.machines.values().map(|m| m.descriptor.clone()).collect()
    };

    let app_uri = format!("{}server", settings.namespace_uri);
    let endpoint = format!("opc.tcp://127.0.0.1:{}/", settings.port);
    let (server, handle) = ServerBuilder::new_anonymous("factory-gateway")
        .application_uri(app_uri)
        .product_uri("https://github.com/joeblew999/factory-gateway")
        .host(settings.host.clone())
        .port(settings.port)
        .pki_dir(std::path::PathBuf::from(format!("./pki-{}", settings.port)))
        .build_info(BuildInfo {
            product_uri: "https://github.com/joeblew999/factory-gateway".into(),
            manufacturer_name: "joeblew999".into(),
            product_name: "factory-gateway".into(),
            software_version: env!("CARGO_PKG_VERSION").into(),
            build_number: "1".into(),
            build_date: DateTime::now(),
        })
        .with_node_manager(simple_node_manager(
            NamespaceMetadata {
                namespace_uri: settings.namespace_uri.clone(),
                ..Default::default()
            },
            "factory",
        ))
        .trust_client_certs(true)
        .diagnostics_enabled(false)
        .discovery_urls(vec![endpoint])
        .build()
        .map_err(|e| anyhow::anyhow!("server build failed: {e:?}"))?;

    let node_manager: Arc<InMemoryNodeManager<SimpleNodeManagerImpl>> = handle
        .node_managers()
        .get_of_type::<SimpleNodeManager>()
        .ok_or_else(|| anyhow::anyhow!("no SimpleNodeManager"))?;
    let ns = handle
        .get_namespace_index(&settings.namespace_uri)
        .ok_or_else(|| anyhow::anyhow!("namespace not registered"))?;
    let subscriptions = handle.subscriptions().clone();

    build_address_space(ns, &node_manager, &descriptors);

    // Background loop: drive the gateway, sync live values into the nodes.
    let nm = node_manager.clone();
    let subs = subscriptions.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let updates = {
                let mut g = gw.lock().await;
                if let Err(e) = g.tick().await {
                    tracing::warn!("gateway tick: {e}");
                }
                collect_updates(ns, &g)
            };
            let _ = nm.set_values(
                &subs,
                updates.iter().map(|(n, r, dv)| (n, r.as_ref(), dv.clone())),
            );
        }
    });

    tracing::info!(
        "OPC-UA server (Machinery) on opc.tcp://{}:{}/",
        settings.host,
        settings.port
    );
    server
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("server run failed: {e:?}"))
}

/// `Objects/Machines/<id>/{Identification,MachineryItemState,Telemetry/*}`.
fn build_address_space(
    ns: u16,
    manager: &Arc<InMemoryNodeManager<SimpleNodeManagerImpl>>,
    descriptors: &[MachineDescriptor],
) {
    let address_space = manager.address_space();
    let mut space = address_space.write();

    // OPC UA for Machinery well-known folder: Objects/Machines
    let machines = node(ns, "Machines");
    space.add_folder(&machines, "Machines", "Machines", &NodeId::objects_folder_id());

    for d in descriptors {
        let m = node(ns, &format!("Machines/{}", d.machine_id));
        space.add_folder(&m, d.machine_id.as_str(), d.machine_id.as_str(), &machines);

        // Identification nameplate (OPC 40001-1)
        let ident = node(ns, &format!("Machines/{}/Identification", d.machine_id));
        space.add_folder(&ident, "Identification", "Identification", &m);
        space.add_variables(
            vec![
                Variable::new(
                    &node(ns, &format!("Machines/{}/Identification/Manufacturer", d.machine_id)),
                    "Manufacturer",
                    "Manufacturer",
                    UAString::from(d.identification.manufacturer.clone()),
                ),
                Variable::new(
                    &node(ns, &format!("Machines/{}/Identification/Model", d.machine_id)),
                    "Model",
                    "Model",
                    UAString::from(d.identification.model.clone()),
                ),
                Variable::new(
                    &node(ns, &format!("Machines/{}/Identification/SerialNumber", d.machine_id)),
                    "SerialNumber",
                    "SerialNumber",
                    UAString::from(d.identification.serial_number.clone().unwrap_or_default()),
                ),
            ],
            &ident,
        );

        // MachineryItemState (OPC 40001-1)
        space.add_variables(
            vec![Variable::new(
                &node(ns, &format!("Machines/{}/MachineryItemState", d.machine_id)),
                "MachineryItemState",
                "MachineryItemState",
                UAString::from("NotAvailable"),
            )],
            &m,
        );

        // Telemetry/ (machine-specific)
        let telem = node(ns, &format!("Machines/{}/Telemetry", d.machine_id));
        space.add_folder(&telem, "Telemetry", "Telemetry", &m);
        let vars: Vec<Variable> = d
            .telemetry
            .iter()
            .map(|f| {
                Variable::new(
                    &node(ns, &format!("Machines/{}/Telemetry/{}", d.machine_id, f.name)),
                    f.name.as_str(),
                    f.name.as_str(),
                    0f64,
                )
            })
            .collect();
        if !vars.is_empty() {
            space.add_variables(vars, &telem);
        }
    }

    tracing::info!(machines = descriptors.len(), "OPC-UA Machinery address space built");
}

/// Snapshot the gateway's live state into node updates.
type Update = (NodeId, Option<opcua::types::NumericRange>, DataValue);

fn collect_updates(ns: u16, gw: &Gateway) -> Vec<Update> {
    let mut out: Vec<Update> = Vec::new();
    for m in gw.machines.values() {
        out.push((
            node(ns, &format!("Machines/{}/MachineryItemState", m.spec.id)),
            None,
            DataValue::new_now(UAString::from(m.state.as_str())),
        ));
        for (name, value) in &m.telemetry {
            let dv = match value {
                Value::Double(v) => DataValue::new_now(*v),
                Value::UInt(v) => DataValue::new_now(*v as f64),
                Value::Int(v) => DataValue::new_now(*v as f64),
                Value::Bool(v) => DataValue::new_now(*v),
                Value::String(s) => DataValue::new_now(UAString::from(s.clone())),
            };
            out.push((
                node(ns, &format!("Machines/{}/Telemetry/{}", m.spec.id, name)),
                None,
                dv,
            ));
        }
    }
    out
}
