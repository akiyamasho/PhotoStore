# Architecture overview

PhotoStore separates the document engine from presentation and transport. The desktop UI, CLI, JSON control channel, and MCP server reach editing behavior through the command registry rather than maintaining separate implementations.

```text
desktop UI       CLI       JSON control       MCP
    |             |              |              |
    +-------------+--------------+--------------+
                          |
                   command registry
                          |
                    engine Session
                          |
       document model + operations + compositors
                          |
             PSD / raster / .pcraft I/O
```

The enforced dependency direction runs from format-independent foundation crates through the document and operation layers to composition, I/O, the engine, and finally UI/automation. `cargo xtask layers` checks this rule.

The complete design narrative and dependency diagram remain in [`docs/architecture.md`](https://github.com/akiyamasho/PhotoStore/blob/main/docs/architecture.md). That document contains historical design material as well as implemented architecture; confirm behavior in current source when security or compatibility depends on it.

## Core properties

- `photostore-doc` holds the pure-data document model.
- `photostore-engine::Session` owns open documents, active state, history, and command dispatch.
- `photostore-compose` is the CPU reference compositor; `photostore-gpu` accelerates supported interactive paths.
- `photostore-io` maps between documents and PSD or flat formats.
- `photostore-format` reads and writes the native `.pcraft` bundle.
- `photostore-ui-egui` is a thin shell over engine commands.
- `photostore-automation` exposes the headless engine and the live-GUI bridge.

See [Crates and dependency layers](crates.md) for the complete workspace map.
