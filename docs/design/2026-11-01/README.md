# GPT Image design references — 2026-11-01

These are generated design references, not running PhotoStore screenshots, implementation evidence, QA, or user acceptance. Their exact prompts and tool metadata are recorded in [prompts.json](prompts.json). The built-in GPT Image tool does not expose its underlying model identifier. Neither image used external assets, private artwork, or character references as inputs.

| Asset | Intended use | Provenance | Limits |
| --- | --- | --- | --- |
| [video-timeline-concept.png](video-timeline-concept.png) | Proposed Video Timeline hierarchy for issue #23 research/specification. | Generated with the built-in GPT Image tool on 2026-11-01; prompt in `prompts.json`; no external asset input. | It is not an application capture or proof that the controls work. It has no explicit `Video Timeline` mode badge, all track locks appear closed although the demo clips are intended editable, and the music clip uses a film-like icon rather than an audio glyph. Its panel widths/proportions are illustrative, not normative. The written requirements in [the plan](../../plans/video-timeline-2026-11-01.md) are authoritative for mode clarity, lane controls, audio identity, and sizing. The image's media samples are generated illustrations, not project/user artwork. |
| [rename-brush-reference.png](rename-brush-reference.png) | Intent reference for layer/group rename clipboard routing and brush decrease reaching 1 px. | Generated with the built-in GPT Image tool on 2026-11-01; prompt in `prompts.json`; no external asset input. | It illustrates behavior only. Its Ctrl shortcuts are not authoritative for PhotoStore on macOS; adapt platform labels to the effective `Cmd` bindings. The pictured context menu does not request a new context menu implementation; existing menu behavior is not a design requirement. |

## Design review

Both generated images were visually inspected on 2026-11-01. They contain no characters or private artwork. Use the timeline image to discuss hierarchy, clip density, synchronized selection, and separation of the inspector from clip lanes. Treat the written spec as the source for details the image misses or approximates. The timeline proposal is not an instruction to build every drawn control before its source semantics and persistence requirements are decided.
