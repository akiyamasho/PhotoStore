# Implementation decisions

## 2026-10-11: Personal PhotoStore fork identity

PhotoStore is maintained as an independent private fork of PhotoCraft for personal use and workflows. The repository keeps original source history, authorship, licenses, notices, and attribution records, with no upstream sync relationship.

## 2026-10-11: Action-driven native recording

The recording milestone captures committed engine actions into bounded native document frames. It excludes undo, UI-only input, cursor/selection movement, and wall-clock sampling; fixes resolution and quality when a new canvas is created; replays and exports at 30 fps; and supports full or 30-second export. A 256 MiB encoded cap pauses recording until purge. Private layer marking affects future frames only, and imported or existing documents cannot reconstruct earlier recording progress. The 30-second selector uses an original deterministic visual-change heuristic and does not claim Procreate algorithm parity.

## 2026-10-11: 0.8.0 release pass scope

The completed 0.8.0 release grouped Audio, Smart Filter Masks, and the New Document Fix. Source implementation status was tracked independently from compiler, packaging, release, and reinstall evidence; further Photoshop backfill is outside that twelve-step release count.

## 2026-10-11: 0.9.0 mask, channel, and keyframe pass

The 0.9.0 source pass keeps mask transfer atomic and document-safe: copy and move use document IDs, inherited locks, copy-on-write copies, explicit overwrite, and one undo edit. Channel references use strings for mask properties and open-document indexes for channel sources. Keyframe Set/Delete uses typed pending values, document timeline bounds, target-aware guards, atomic persistence, and no-op detection. Compiler, packaging, release, and reinstall evidence are complete for this pass; broad UI acceptance and Photoshop parity remain unverified.

## 2026-10-11: 0.10.0 repeat and onion playback pass

Repeat counts remain persisted as bounded total plays, with `None` reserved for forever. Onion-skin
settings persist in native documents, while preview pixels are endpoint-bounded and capped by ghost
count, working resolution, buffers, and one composite texture; those preview pixels do not enter
exports or action recordings. The compiler, release, and install evidence are complete for this
pass; broad UI acceptance and Photoshop parity remain unverified.

## 2026-10-11: 0.11.0 Smart Contents reliability pass

Close and contents-save flows are fallible and child-first: dirty child commits propagate errors
before parent/tab cleanup, while Save/Don't Save/Cancel decisions retain document IDs and revisions.
Batch discard is bounded and strictly validated. Contents context actions remain explicit, and linked
child Save Contents embeds native contents without claiming external linked-file synchronization. The
0.11.0 compiler, release, and install evidence are complete; broad UI acceptance and Photoshop parity remain unverified.

## 2026-10-11: 0.12.0 recording default-off policy

New Document recording defaults off and resets its checkbox to false on each opening. Explicit API or
preset opt-in remains available, and existing stored enabled documents retain their settings and
frames. The source compiler gate and preview/screenshot evidence passed; commit, tag, final
build/publish, and install remain separate pending gates. The global disable control is not proof of
per-document enabled state.

## 2026-10-10: Smart Object contents identity (#2163)

Smart Object instances carry a document-scoped contents ID separately from their layer ID. Normal duplicates preserve it; New Smart Object via Copy creates a new one. Contents edits update the matching instances together, retaining their individual placement and filters. Copies into another document remap contents IDs as a batch, preserving sharing within the copied layers without joining existing destination objects.

Native files persist this ID. Legacy embedded objects without an ID remain independent: identical file bytes do not prove that two objects were intended to share edits. Legacy nonempty linked paths and PSD source identifiers retain their explicit sharing relationship. PSD export keys embedded sources by contents identity as well as bytes, so independent equal-content objects stay independent.
