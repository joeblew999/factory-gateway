# factory-gateway

<https://github.com/joeblew999/factory-gateway>

The generic **OPC-UA gateway** for factory-floor automation: one server that
aggregates many machines under `/Machines/<id>`, holds the job queue, serves the
operator dashboard, and exposes everything northbound to SCADA. Knows nothing
about any specific machine — it builds its address space from the descriptors
each driver declares.

Part of the `factory-` family:

| Repo | Role |
|------|------|
| [factory-machine-model](https://github.com/joeblew999/factory-machine-model) | the shared contract this gateway is written against |
| **factory-gateway** (this) | OPC-UA server + job queue + dashboard + SCADA northbound |
| [factory-howick-driver](https://github.com/joeblew999/factory-howick-driver) | first machine driver — implements the contract |
| `factory-<machine>-driver` | one per machine type |

## Data flow

```
plat-trunk (jobs) ──► factory-gateway ◄── SCADA / dashboard (OPC-UA client)
                            ▲
                       OPC-UA │  (each driver presents the standard node-tree)
                       factory-howick-driver · factory-<robot>-driver ...
```

- **DOWN** = jobs: producer → gateway queue → driver pulls → machine.
- **UP** = telemetry/status: machine → driver → gateway nodes → SCADA + historian.

## Depends on

- [`factory-machine-model`](https://github.com/joeblew999/factory-machine-model)
  — builds the `/Machines/<id>` node-tree from each `MachineDescriptor`.

## Status — extraction in progress 🚧

Being carved out of [`opcua-howick`](https://github.com/joeblew999/opcua-howick)
(crate `opcua-server`). Tracking the genericization seam on branch
[`feat/standard-machine-model`](https://github.com/joeblew999/opcua-howick/tree/feat/standard-machine-model);
design in ADR-0006. What moves here: the OPC-UA server, job queue, HTTP
dashboard, file watcher, and the plat-trunk job poller — with the Howick-specific
node names and routes generalized to `/Machines/<id>` and `/api/jobs/<id>`.
