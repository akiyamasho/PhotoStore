# Implementation decisions

## 2026-10-11: Personal PhotoStore fork identity

PhotoStore is maintained as an independent private fork of PhotoCraft for personal use and workflows. The repository keeps original source history, authorship, licenses, notices, and attribution records, with no upstream sync relationship.

## 2026-10-11: Action-driven native recording

The recording milestone captures committed engine actions into bounded native document frames. It excludes undo, UI-only input, cursor/selection movement, and wall-clock sampling; fixes resolution and quality when a new canvas is created; replays and exports at 30 fps; and supports full or 30-second export. A 256 MiB encoded cap pauses recording until purge. Private layer marking affects future frames only, and imported or existing documents cannot reconstruct earlier recording progress. The 30-second selector uses an original deterministic visual-change heuristic and does not claim Procreate algorithm parity.

## 2026-10-11: 0.8.0 release pass scope

The next release pass groups Audio, Smart Filter Masks, and the New Document Fix. Source implementation status is tracked independently from compiler, packaging, release, and reinstall evidence; those gates remain pending until recorded. Further Photoshop backfill is outside the twelve-step release count.

## 2026-10-10: Smart Object contents identity (#2163)

Smart Object instances carry a document-scoped contents ID separately from their layer ID. Normal duplicates preserve it; New Smart Object via Copy creates a new one. Contents edits update the matching instances together, retaining their individual placement and filters. Copies into another document remap contents IDs as a batch, preserving sharing within the copied layers without joining existing destination objects.

Native files persist this ID. Legacy embedded objects without an ID remain independent: identical file bytes do not prove that two objects were intended to share edits. Legacy nonempty linked paths and PSD source identifiers retain their explicit sharing relationship. PSD export keys embedded sources by contents identity as well as bytes, so independent equal-content objects stay independent.
