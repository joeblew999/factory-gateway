//! OPC-UA server — projects the [`Gateway`] into a **standard** address space and
//! exposes the ISA-95 job-control surface as real OPC-UA methods.
//!
//! Per machine, under `Objects/Machines/<id>/` (OPC UA for Machinery, OPC 40001-1):
//!   - `Identification/{Manufacturer,Model,SerialNumber}` — the nameplate
//!   - `MachineryItemState` — the state machine (live)
//!   - `Telemetry/*` — driver-declared variables (live)
//!   - `JobOrderReceiver/` — ISA-95 Job Control (OPC 10031-4) methods:
//!     `StoreAndStart(JobOrderID, CutListCsv)` · `Start(JobOrderID)` ·
//!     `Abort(JobOrderID)` · `Clear()`, plus a live `QueueDepth`.
//!
//! This is the **standard job-input path**: a MES is an OPC-UA *client* that calls
//! `StoreAndStart` to dispatch work. Method callbacks (sync) enqueue a [`Command`];
//! the async scheduler loop drains them into the gateway, then runs a tick.
//!
//! Methods exposed here are the core lifecycle; the remaining receiver methods
//! (`Store`, `RevokeStart`, `Pause`, `Resume`, `Stop`, `Cancel`, `Update`) exist
//! in [`crate::jobs`] and follow the identical pattern to add.

use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use opcua::server::address_space::{MethodBuilder, Variable};
use opcua::server::diagnostics::NamespaceMetadata;
use opcua::server::node_manager::memory::{
    simple_node_manager, InMemoryNodeManager, SimpleNodeManager, SimpleNodeManagerImpl,
};
use opcua::server::ServerBuilder;
use opcua::types::{BuildInfo, DataTypeId, DataValue, DateTime, NodeId, StatusCode, UAString, Variant};
use tokio::sync::Mutex;

use factory_machine_model::{JobOrder, MachineDescriptor, Value};

use crate::config::OpcUaSettings;
use crate::gateway::Gateway;

fn node(ns: u16, name: &str) -> NodeId {
    NodeId::new(ns, name)
}

/// A job-control command enqueued by an OPC-UA method callback, applied to the
/// gateway by the scheduler loop.
enum Command {
    StoreAndStart { machine: String, job_id: String, csv: String },
    Start { machine: String, job_id: String },
    Abort { machine: String, job_id: String },
    Clear { machine: String },
}

/// Which receiver method a node represents.
#[derive(Clone, Copy)]
enum Method {
    StoreAndStart,
    Start,
    Abort,
    Clear,
}

type Commands = Arc<StdMutex<Vec<Command>>>;

/// Build and run the OPC-UA server, projecting `gw` into a Machinery + ISA-95
/// address space.
pub async fn serve(settings: OpcUaSettings, gw: Arc<Mutex<Gateway>>) -> anyhow::Result<()> {
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

    let commands: Commands = Arc::new(StdMutex::new(Vec::new()));
    build_address_space(ns, &node_manager, &descriptors, commands.clone());

    // Scheduler loop: apply queued job-control commands, run the gateway, sync nodes.
    let nm = node_manager.clone();
    let subs = subscriptions.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            let pending: Vec<Command> = commands.lock().unwrap().drain(..).collect();
            let updates = {
                let mut g = gw.lock().await;
                for cmd in pending {
                    apply(&mut g, cmd);
                }
                if let Err(e) = g.tick().await {
                    tracing::warn!("gateway tick: {e}");
                }
                collect_updates(ns, &g)
            };
            let _ = nm.set_values(&subs, updates.iter().map(|(n, r, dv)| (n, r.as_ref(), dv.clone())));
        }
    });

    tracing::info!(
        "OPC-UA server (Machinery + ISA-95) on opc.tcp://{}:{}/",
        settings.host,
        settings.port
    );
    server
        .run()
        .await
        .map_err(|e| anyhow::anyhow!("server run failed: {e:?}"))
}

/// Apply one job-control command to the gateway.
fn apply(g: &mut Gateway, cmd: Command) {
    let result = match cmd {
        Command::StoreAndStart { machine, job_id, csv } => g
            .machines
            .get_mut(&machine)
            .map(|m| {
                m.jobs
                    .store_and_start(JobOrder::with_payload(job_id, "CutListCsv", csv.into_bytes()));
            })
            .ok_or(machine),
        Command::Start { machine, job_id } => g
            .machines
            .get_mut(&machine)
            .map(|m| {
                let _ = m.jobs.start(&job_id);
            })
            .ok_or(machine),
        Command::Abort { machine, job_id } => g
            .machines
            .get_mut(&machine)
            .map(|m| {
                let _ = m.jobs.abort(&job_id);
            })
            .ok_or(machine),
        Command::Clear { machine } => g
            .machines
            .get_mut(&machine)
            .map(|m| m.jobs.clear_finished())
            .ok_or(machine),
    };
    if let Err(machine) = result {
        tracing::warn!(%machine, "job-control command for unknown machine");
    }
}

/// `Objects/Machines/<id>/{Identification,MachineryItemState,Telemetry,JobOrderReceiver}`.
fn build_address_space(
    ns: u16,
    manager: &Arc<InMemoryNodeManager<SimpleNodeManagerImpl>>,
    descriptors: &[MachineDescriptor],
    commands: Commands,
) {
    // (machine_id, method, node_id) collected while building, registered after.
    let mut methods: Vec<(String, Method, NodeId)> = Vec::new();

    {
        let address_space = manager.address_space();
        let mut space = address_space.write();

        let machines = node(ns, "Machines");
        space.add_folder(&machines, "Machines", "Machines", &NodeId::objects_folder_id());

        for d in descriptors {
            let mid = &d.machine_id;
            let m = node(ns, &format!("Machines/{mid}"));
            space.add_folder(&m, mid.as_str(), mid.as_str(), &machines);

            // Identification nameplate (OPC 40001-1)
            let ident = node(ns, &format!("Machines/{mid}/Identification"));
            space.add_folder(&ident, "Identification", "Identification", &m);
            space.add_variables(
                vec![
                    Variable::new(
                        &node(ns, &format!("Machines/{mid}/Identification/Manufacturer")),
                        "Manufacturer",
                        "Manufacturer",
                        UAString::from(d.identification.manufacturer.clone()),
                    ),
                    Variable::new(
                        &node(ns, &format!("Machines/{mid}/Identification/Model")),
                        "Model",
                        "Model",
                        UAString::from(d.identification.model.clone()),
                    ),
                    Variable::new(
                        &node(ns, &format!("Machines/{mid}/Identification/SerialNumber")),
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
                    &node(ns, &format!("Machines/{mid}/MachineryItemState")),
                    "MachineryItemState",
                    "MachineryItemState",
                    UAString::from("NotAvailable"),
                )],
                &m,
            );

            // Telemetry/ (machine-specific)
            let telem = node(ns, &format!("Machines/{mid}/Telemetry"));
            space.add_folder(&telem, "Telemetry", "Telemetry", &m);
            let vars: Vec<Variable> = d
                .telemetry
                .iter()
                .map(|f| {
                    Variable::new(
                        &node(ns, &format!("Machines/{mid}/Telemetry/{}", f.name)),
                        f.name.as_str(),
                        f.name.as_str(),
                        0f64,
                    )
                })
                .collect();
            if !vars.is_empty() {
                space.add_variables(vars, &telem);
            }

            // JobOrderReceiver/ (OPC 10031-4) — QueueDepth + the control methods
            let jor = node(ns, &format!("Machines/{mid}/JobOrderReceiver"));
            space.add_folder(&jor, "JobOrderReceiver", "JobOrderReceiver", &m);
            space.add_variables(
                vec![Variable::new(
                    &node(ns, &format!("Machines/{mid}/JobOrderReceiver/QueueDepth")),
                    "QueueDepth",
                    "QueueDepth",
                    0u32,
                )],
                &jor,
            );

            // StoreAndStart(JobOrderID: String, CutListCsv: String)
            let n = node(ns, &format!("Machines/{mid}/JobOrderReceiver/StoreAndStart"));
            MethodBuilder::new(&n, "StoreAndStart", "StoreAndStart")
                .component_of(jor.clone())
                .input_args(
                    &mut *space,
                    &node(ns, &format!("Machines/{mid}/JobOrderReceiver/StoreAndStart/In")),
                    &[
                        ("JobOrderID", DataTypeId::String).into(),
                        ("CutListCsv", DataTypeId::String).into(),
                    ],
                )
                .insert(&mut *space);
            methods.push((mid.clone(), Method::StoreAndStart, n));

            // Start(JobOrderID: String)
            let n = node(ns, &format!("Machines/{mid}/JobOrderReceiver/Start"));
            MethodBuilder::new(&n, "Start", "Start")
                .component_of(jor.clone())
                .input_args(
                    &mut *space,
                    &node(ns, &format!("Machines/{mid}/JobOrderReceiver/Start/In")),
                    &[("JobOrderID", DataTypeId::String).into()],
                )
                .insert(&mut *space);
            methods.push((mid.clone(), Method::Start, n));

            // Abort(JobOrderID: String)
            let n = node(ns, &format!("Machines/{mid}/JobOrderReceiver/Abort"));
            MethodBuilder::new(&n, "Abort", "Abort")
                .component_of(jor.clone())
                .input_args(
                    &mut *space,
                    &node(ns, &format!("Machines/{mid}/JobOrderReceiver/Abort/In")),
                    &[("JobOrderID", DataTypeId::String).into()],
                )
                .insert(&mut *space);
            methods.push((mid.clone(), Method::Abort, n));

            // Clear()
            let n = node(ns, &format!("Machines/{mid}/JobOrderReceiver/Clear"));
            MethodBuilder::new(&n, "Clear", "Clear")
                .component_of(jor.clone())
                .insert(&mut *space);
            methods.push((mid.clone(), Method::Clear, n));
        }
    } // address-space write guard dropped

    // Register method callbacks — each enqueues a Command for the scheduler loop.
    for (machine, method, method_node) in methods {
        let commands = commands.clone();
        manager.inner().add_method_callback(method_node, move |args| {
            let cmd = match method {
                Method::StoreAndStart => Command::StoreAndStart {
                    machine: machine.clone(),
                    job_id: str_arg(args, 0)?,
                    csv: str_arg(args, 1)?,
                },
                Method::Start => Command::Start {
                    machine: machine.clone(),
                    job_id: str_arg(args, 0)?,
                },
                Method::Abort => Command::Abort {
                    machine: machine.clone(),
                    job_id: str_arg(args, 0)?,
                },
                Method::Clear => Command::Clear {
                    machine: machine.clone(),
                },
            };
            commands.lock().unwrap().push(cmd);
            Ok(Vec::new())
        });
    }

    tracing::info!(machines = descriptors.len(), "OPC-UA address space built (Machinery + ISA-95)");
}

/// Pull a String method argument.
fn str_arg(args: &[Variant], i: usize) -> Result<String, StatusCode> {
    match args.get(i) {
        Some(Variant::String(s)) => Ok(s.value().clone().unwrap_or_default()),
        _ => Err(StatusCode::BadArgumentsMissing),
    }
}

type Update = (NodeId, Option<opcua::types::NumericRange>, DataValue);

/// Snapshot the gateway's live state into node updates.
fn collect_updates(ns: u16, gw: &Gateway) -> Vec<Update> {
    let mut out: Vec<Update> = Vec::new();
    for m in gw.machines.values() {
        let mid = &m.spec.id;
        out.push((
            node(ns, &format!("Machines/{mid}/MachineryItemState")),
            None,
            DataValue::new_now(UAString::from(m.state.as_str())),
        ));
        out.push((
            node(ns, &format!("Machines/{mid}/JobOrderReceiver/QueueDepth")),
            None,
            DataValue::new_now(m.jobs.queue_depth() as u32),
        ));
        for (name, value) in &m.telemetry {
            let dv = match value {
                Value::Double(v) => DataValue::new_now(*v),
                Value::UInt(v) => DataValue::new_now(*v as f64),
                Value::Int(v) => DataValue::new_now(*v as f64),
                Value::Bool(v) => DataValue::new_now(*v),
                Value::String(s) => DataValue::new_now(UAString::from(s.clone())),
            };
            out.push((node(ns, &format!("Machines/{mid}/Telemetry/{name}")), None, dv));
        }
    }
    out
}
