# factory-gateway

<https://github.com/joeblew999/factory-gateway>

The generic **OPC-UA gateway** for a factory. It reads one factory's config,
instantiates a driver for each machine, runs an ISA-95 job-control loop over them,
and serves the whole factory as a **standard OPC-UA address space** — so any
conformant SCADA / MES / historian interoperates with no bespoke glue. It knows
nothing about any specific machine; that lives in the driver crates.

Part of the `factory-` family:

| Repo | Role |
|------|------|
| [factory-machine-model](https://github.com/joeblew999/factory-machine-model) | the OPC-UA-grounded contract this gateway is built on |
| **factory-gateway** (this) | OPC-UA server + driver registry + ISA-95 job control |
| [factory-howick-driver](https://github.com/joeblew999/factory-howick-driver) | a machine driver it composes |
| [factory-floor](https://github.com/joeblew999/factory-floor) | umbrella workspace + docs/ADRs |

## The OPC-UA structure it serves

Built on two OPC-UA companion specs (see
[factory-machine-model](https://github.com/joeblew999/factory-machine-model) for
the full model):

```text
Objects/
└── Machines/                              ← OPC UA for Machinery (OPC 40001-1)
    └── <machine-id>/        e.g. howick-1
        ├── Identification/   Manufacturer · Model · SerialNumber       ← standard nameplate
        ├── MachineryItemState   NotAvailable | OutOfService | NotExecuting | Executing
        ├── Telemetry/         driver-declared, e.g. PiecesProduced · CoilRemaining
        └── JobOrderReceiver   ← OPC UA for ISA-95 Job Control (OPC 10031-4)
              Store · StoreAndStart · Start · Stop · Cancel · Pause · Resume · Abort · Clear
```

The gateway builds this `Machines/<id>` subtree for every machine from the driver's
`descriptor()`, then syncs live `MachineryItemState` + `Telemetry` values into the
nodes every 500 ms. (The ISA-95 JobOrderReceiver *logic* — store/start/run/end with
the standard `JobState` lifecycle — is implemented in `jobs.rs`/`gateway.rs`;
exposing those methods on the OPC-UA address space is the next transport addition.)

## Run it

```bash
factory-gateway --config examples/factory.toml
# OPC-UA server on opc.tcp://0.0.0.0:4840/  →  browse Objects/Machines/howick-1
```

## How a factory is configured

One TOML file per factory = that factory's `Machines/` folder. Each `[[machine]]`
names its `driver` and carries a standard `[machine.identification]` nameplate plus
a typed driver-specific table. See [`examples/factory.toml`](examples/factory.toml).
A different factory with different hardware is just a different config + driver set.

## Adding a machine type

Depend on its `factory-<machine>-driver` crate and register it in
[`src/registry.rs`](src/registry.rs). No other gateway change.

## Licence

MIT OR Apache-2.0.
