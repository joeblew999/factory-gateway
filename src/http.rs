//! Operator HTTP face — a small dashboard + a job-submit endpoint, ported from
//! the original Howick build. It's a convenience over the standards: a `POST` of
//! a cut-list becomes an ISA-95 `StoreAndStart` on the machine's JobOrderReceiver
//! (same path an OPC-UA MES client would take). Runs alongside the OPC-UA server.
//!
//! Routes:
//!   - `GET  /`                 → live dashboard (polls `/status`)
//!   - `GET  /status`           → JSON: factory + machine states
//!   - `POST /jobs/<machine_id>` → submit a cut-list (CSV body) as a job
//!   - `GET  /health`           → `{"ok":true}`

use std::sync::Arc;
use std::time::SystemTime;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use factory_machine_model::JobOrder;

use crate::config::HttpSettings;
use crate::gateway::Gateway;

pub async fn run(settings: HttpSettings, gw: Arc<Mutex<Gateway>>) -> anyhow::Result<()> {
    let listener = TcpListener::bind((settings.host.as_str(), settings.port)).await?;
    tracing::info!("HTTP dashboard on http://{}:{}/", settings.host, settings.port);
    loop {
        let (stream, _) = listener.accept().await?;
        let gw = gw.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, gw).await {
                tracing::debug!("http: {e}");
            }
        });
    }
}

async fn handle(mut stream: tokio::net::TcpStream, gw: Arc<Mutex<Gateway>>) -> anyhow::Result<()> {
    let mut buf = vec![0u8; 512 * 1024];
    let n = stream.read(&mut buf).await?;
    buf.truncate(n);
    let raw = String::from_utf8_lossy(&buf);
    let head_end = raw.find("\r\n\r\n").unwrap_or(n);
    let body = &raw[(head_end + 4).min(raw.len())..];
    let mut parts = raw.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    let (code, ctype, payload) = match (method, path) {
        ("GET", "/") => ("200", "text/html; charset=utf-8", DASHBOARD.to_string()),
        ("GET", "/health") => ("200", "application/json", r#"{"ok":true}"#.to_string()),
        ("GET", "/status") => ("200", "application/json", status_json(&gw).await),
        ("POST", p) if p.starts_with("/jobs/") => {
            let machine = p.trim_start_matches("/jobs/").to_string();
            let csv = body.trim().to_string();
            if csv.is_empty() {
                ("400", "application/json", r#"{"error":"empty body"}"#.to_string())
            } else {
                let job_id = format!("{machine}-{}", unix_secs());
                let mut g = gw.lock().await;
                match g.submit(
                    &machine,
                    JobOrder::with_payload(job_id.clone(), "CutListCsv", csv.into_bytes()),
                ) {
                    Ok(()) => (
                        "200",
                        "application/json",
                        format!(r#"{{"ok":true,"job_order_id":"{job_id}"}}"#),
                    ),
                    Err(e) => ("404", "application/json", format!(r#"{{"error":"{e}"}}"#)),
                }
            }
        }
        _ => ("404", "application/json", r#"{"error":"not found"}"#.to_string()),
    };

    let reason = match code {
        "200" => "OK",
        "400" => "Bad Request",
        "404" => "Not Found",
        _ => "Error",
    };
    let resp = format!(
        "HTTP/1.1 {code} {reason}\r\nContent-Type: {ctype}\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream.write_all(resp.as_bytes()).await?;
    Ok(())
}

async fn status_json(gw: &Arc<Mutex<Gateway>>) -> String {
    let g = gw.lock().await;
    let machines: Vec<String> = g
        .machines
        .values()
        .map(|m| {
            let telem: Vec<String> = m
                .telemetry
                .iter()
                .map(|(k, v)| format!(r#""{k}":{}"#, v.as_f64().unwrap_or(0.0)))
                .collect();
            format!(
                r#"{{"id":"{}","kind":"{}","manufacturer":"{}","model":"{}","edge":{},"state":"{}","queue_depth":{},"telemetry":{{{}}}}}"#,
                m.spec.id,
                m.descriptor.kind,
                m.descriptor.identification.manufacturer,
                m.descriptor.identification.model,
                m.edge,
                m.state.as_str(),
                m.jobs.queue_depth(),
                telem.join(",")
            )
        })
        .collect();
    format!(
        r#"{{"factory":{{"id":"{}","name":"{}"}},"machines":[{}]}}"#,
        g.factory.id,
        g.factory.name,
        machines.join(",")
    )
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

const DASHBOARD: &str = r#"<!doctype html><html><head><meta charset=utf-8>
<title>factory-gateway</title><style>
body{font:14px system-ui;margin:2rem;background:#0f1115;color:#e6e6e6}
h1{font-size:1.1rem}table{border-collapse:collapse;width:100%;margin:1rem 0}
th,td{text-align:left;padding:.4rem .6rem;border-bottom:1px solid #283}
.Executing{color:#6f6}.NotExecuting{color:#9ad}.NotAvailable,.OutOfService{color:#f87}
small{color:#789}</style></head><body>
<h1>factory-gateway <small id=f></small></h1>
<table id=t><thead><tr><th>Machine<th>Make/Model<th>Mode<th>State<th>Queue<th>Telemetry</tr></thead><tbody></tbody></table>
<small>OPC UA for Machinery + ISA-95 Job Control · auto-refreshes</small>
<script>
async function tick(){
 let s=await (await fetch('/status')).json();
 f.textContent=s.factory.name;
 t.tBodies[0].innerHTML=s.machines.map(m=>`<tr><td>${m.id}<td>${m.manufacturer} ${m.model}
 <td>${m.edge?'edge':'in-process'}<td class="${m.state}">${m.state}<td>${m.queue_depth}
 <td>${Object.entries(m.telemetry).map(([k,v])=>k+'='+v).join(' ')||'—'}</tr>`).join('');
}
setInterval(tick,1000);tick();
</script></body></html>"#;
