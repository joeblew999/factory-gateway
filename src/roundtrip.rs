//! End-to-end OPC-UA round-trip test (in-crate so it sees the `opcua` client).
//!
//! An OPC-UA *client* dispatches a job via the standard ISA-95 `StoreAndStart`
//! method; we verify it flows all the way through gateway → driver → a real
//! cut-list file written to the machine's USB mount. Proves the pipeline works
//! on the wire, not just that it compiles.

#![cfg(test)]

use std::sync::Arc;
use std::time::Duration;

use opcua::client::{ClientBuilder, IdentityToken};
use opcua::crypto::SecurityPolicy;
use opcua::types::{
    AttributeId, MessageSecurityMode, NodeId, ReadValueId, TimestampsToReturn, UserTokenPolicy,
    VariableId, Variant,
};
use tokio::sync::Mutex;

use crate::{config::FactoryConfig, gateway::Gateway, opcua as gw_opcua, registry::DriverRegistry};

const PORT: u16 = 4855;
const NS_URI: &str = "http://joeblew999.github.io/factory-floor/";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_storeandstart_writes_cutlist_to_the_machine() {
    let usb_mount = std::env::temp_dir().join(format!("factory-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&usb_mount);

    let toml = format!(
        r#"
        [factory]
        id = "test"
        name = "Round-trip Test"
        [opcua]
        host = "127.0.0.1"
        port = {PORT}
        namespace_uri = "{NS_URI}"
        [[machine]]
        id = "howick-1"
        driver = "howick-frama"
        [machine.identification]
        manufacturer = "Howick"
        model = "FRAMA"
        [machine.howick]
        usb_mount = "{mount}"
    "#,
        mount = usb_mount.display()
    );

    // Start the gateway OPC-UA server in the background.
    let config = FactoryConfig::from_toml(&toml).unwrap();
    let registry = DriverRegistry::with_builtin_drivers();
    let gateway = Arc::new(Mutex::new(Gateway::build(&config, &registry).unwrap()));
    tokio::spawn(gw_opcua::serve(config.opcua, gateway));
    tokio::time::sleep(Duration::from_millis(1500)).await; // let it bind

    // Connect an OPC-UA client.
    let mut client = ClientBuilder::new()
        .application_name("factory-roundtrip-test")
        .application_uri("urn:factory-roundtrip-test")
        .trust_server_certs(true)
        .create_sample_keypair(true)
        .session_retry_limit(3)
        .client()
        .unwrap();

    let url = format!("opc.tcp://127.0.0.1:{PORT}/");
    let (session, event_loop) = client
        .connect_to_matching_endpoint(
            (
                url.as_str(),
                SecurityPolicy::None.to_str(),
                MessageSecurityMode::None,
                UserTokenPolicy::anonymous(),
            ),
            IdentityToken::Anonymous,
        )
        .await
        .expect("connect to gateway");
    let _h = event_loop.spawn();
    session.wait_for_connection().await;

    let ns = resolve_ns(&session).await.unwrap_or(2);

    // Call the standard ISA-95 method: StoreAndStart(JobOrderID, CutListCsv).
    let csv = "UNIT,MILLIMETRE\nT1,3945\n";
    let receiver = NodeId::new(ns, "Machines/howick-1/JobOrderReceiver");
    let store_and_start = NodeId::new(ns, "Machines/howick-1/JobOrderReceiver/StoreAndStart");
    session
        .call_one((
            receiver,
            store_and_start,
            Some(vec![
                Variant::String("T1-1".into()),
                Variant::String(csv.into()),
            ]),
        ))
        .await
        .expect("StoreAndStart method call");

    // The scheduler should run the job → driver writes <usb_mount>/T1-1.csv.
    let written = usb_mount.join("T1-1.csv");
    let mut got = None;
    for _ in 0..40 {
        if let Ok(contents) = std::fs::read_to_string(&written) {
            got = Some(contents);
            break;
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }

    let _ = std::fs::remove_dir_all(&usb_mount);
    let contents = got.expect("driver should have written the cut-list to the USB mount");
    assert_eq!(contents, csv, "the CSV the client sent should reach the machine verbatim");
}

/// Resolve our namespace index from the server's namespace array.
async fn resolve_ns(session: &opcua::client::Session) -> Option<u16> {
    let results = session
        .read(
            &[ReadValueId {
                node_id: VariableId::Server_NamespaceArray.into(),
                attribute_id: AttributeId::Value as u32,
                ..Default::default()
            }],
            TimestampsToReturn::Server,
            0.0,
        )
        .await
        .ok()?;
    if let Some(Variant::Array(arr)) = &results.first()?.value {
        arr.values.iter().enumerate().find_map(|(i, v)| match v {
            Variant::String(s) if s.value().as_deref() == Some(NS_URI) => Some(i as u16),
            _ => None,
        })
    } else {
        None
    }
}
