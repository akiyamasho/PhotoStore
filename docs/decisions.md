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
build/publish, and install evidence are complete. The global disable control is not proof of
per-document enabled state.

## 2026-10-11: 0.12.0 post-install closing limit

The canonical 0.12.0 install passed the targeted default-off New Document flow. An older unsaved
0.11.0 document's quit prompt was canceled, but its process later disappeared during bundle
replacement; no force-kill or discard occurred, and no recovery artifact was found or compared.
This is an unresolved closing/recovery reliability limit, not a proven cause or a claim of recovered
artwork.

## 2026-10-11: 0.13.0 shortcut exchange draft

The native JSON shortcut format is stable version 1; the current exchange draft is a UI working
draft until accepted, not an unstable wire format. Exports may contain empty tombstones, imports are
explicit and do not apply automatically, and limits are 10,000 combined map entries plus alternate
keys, 32 alternate keys per binding, and 4 MiB. Shared tool keys are intentional; non-tool conflicts are
reassigned. Shift switching defaults true while preserving explicit false and mouse choice. Existing
KYS import remains supported; KYS export is not claimed.

## 2026-10-11: 0.14.0 close and timeline persistence scope

The 0.14.0 pass removes the application-quit bypass, resets close intent on Cancel, and targets
selective same-playhead post-edit video-surface and dense-property persistence before history/save.
The 33.04-second compiler gate, 5m32s preview build, native 8,314-byte demo proof, and targeted
captures remain complete. The earlier consolidated development compiler exited 0; its log is
`/private/tmp/photostore-0.14.0-dev-check-20261011.log`, but it predates and does not cover the new
namespace. The goal-specific compiler exited 0; log
`/private/tmp/photostore-0.14.0-goal-rc-check-20261011.log`. Three standard-only namespace tests
passed with zero failures; log `/private/tmp/photostore-build-rs-test-20261011.log`. The local RC
source is `08e285ea80ea160af9661596ceee9449b4122f8f`; its 5m22s development package is installed
at `/Applications/PhotoStore-dev-timeline-safety.app` with binary SHA-256
`5d677b261e2ddf7c281fbcf11fb4fd0458a3d5682db4cd4b219cabb439720168`. Focused runtime evidence is
`/private/tmp/photostore-dev-timeline-safety-repair-20261011/runtime-evidence.json`. Nine of nine
steps are complete: 1–6, 7, 8, and 9; the private branch push is complete at remote SHA
`da874a22738885cef9cee37094f6251a3fd65cfc`. The RC
uses `PhotoStore-dev-timeline-safety`,
bundle ID `ai.photostore.app.dev.timeline-safety`, and default macOS configuration
`~/Library/Application Support/PhotoStore-dev-timeline-safety`; production remains untouched.
No production tag, release, publication, or production reinstall is claimed. The allocation-free
sampler uses O(n) sortedness validation plus lower-bound lookup. The bounded pass is complete; the
overall Photoshop backfill remains active. The next bounded pass is ordinary-layer and Smart opacity
stopwatches on `/private/tmp/PhotoStore-opacity-animation-20261011`, branch `codex/opacity-stopwatch`,
in progress. The behavior remains bounded and evidence-gated;
no complete timeline persistence or Photoshop parity claim is made.

## 2026-10-11: 0.15.0 opacity stopwatch scope

The 0.15.0 pass uses sparse copy-on-write opacity keys with Linear/Hold interpolation, typed-property
auto-key, previous/next markers, UI static-disable confirmation (explicit direct API false deletes
without prompt), editable default-unassigned shortcuts, and native backward-compatible persistence.
It caps native state at 256 owned tracks, 100,000 keys total per document, maximum frame index 999,999
(timeline capacity 1,000,000 frames), and a 64 MiB estimated property-snapshot limit; Smart root
curves preserve parent state through unpack/group. Existing coupled video XY/opacity behavior is
implemented and unchanged. Only independent video-opacity stopwatch UI/migration, PSD temporal export,
audio acceptance, and full Photoshop parity remain future work. Engine/native and timeline-UI source
steps are implemented; the required compiler passed, log
`/private/tmp/photostore-opacity-app-check-20261011.log`. The local candidate package/runtime receipt
is complete, with no screenshot produced because the focus=false capture timed out; visual interaction
acceptance remains unverified. Candidate isolated identity is `PhotoStore-dev-opacity-animation` with
bundle ID `ai.photostore.app.dev.opacity-animation`; production remains untouched.

## 2026-10-10: Smart Object contents identity (#2163)

Smart Object instances carry a document-scoped contents ID separately from their layer ID. Normal duplicates preserve it; New Smart Object via Copy creates a new one. Contents edits update the matching instances together, retaining their individual placement and filters. Copies into another document remap contents IDs as a batch, preserving sharing within the copied layers without joining existing destination objects.

Native files persist this ID. Legacy embedded objects without an ID remain independent: identical file bytes do not prove that two objects were intended to share edits. Legacy nonempty linked paths and PSD source identifiers retain their explicit sharing relationship. PSD export keys embedded sources by contents identity as well as bytes, so independent equal-content objects stay independent.

## 2026-10-11: Native shortcut-file autosave (#4)

`shortcutFile` is an optional stored preference (serialized as `shortcutFile`) with an empty
default. The engine validates the path but performs no filesystem I/O; the native shell writes the
existing version-1 `photostore-shortcuts` JSON only after accepted shortcut changes are saved. The
native target must be absolute, end in `.json`, and use an existing parent directory. Export errors
remain pending for the bounded preferences retry and report that preferences themselves were saved;
the last successful `(path, encoded shortcuts)` snapshot is retained to avoid rewriting it for
unrelated preference changes. Existing preference files remain compatible because the field is
optional, and web shells leave the control unavailable when native persistence is unsupported.

## 2026-10-11: 0.16.0 independent Position animation development

Position animation uses a native optional `Layer.PositionAnimation` property. Missing data remains
backward-compatible; stored state is an applied Position anchor and value rather than a bounding-box
derivation. Keys use whole-pixel signed XY values bounded to ±1,000,000, frame indices through
999,999, and Linear/Hold
interpolation. Position has independent limits of 100,000 total keys and 256 tracks. Raster, Text,
Shape, and Smart layers are authorable; independent video Position and full Smart affine transforms
remain future work. Legacy video XY and independent Opacity behavior remain unchanged.

Movement updates the first owned Position ancestor, adding or updating a key when enabled and
changing static Position when disabled; unowned groups recurse, and a direct child move keys that
child. This prevents duplicate parent-wrapper and child-curve movement. Generic bounding-box
painting does not auto-key. First enable migrates legacy dense Position snapshots to bounded Hold
keys and reconciles
the baseline/current pose; re-enabling an existing track is a no-op. Disable or delete-last-key
freezes the playhead pose and clears owned keys. Smart conversion, Video-to-Frame, Frame-to-Video,
and destructive timeline operations preserve or clear ownership according to
[position-animation.md](position-animation.md), with atomic failure for unrepresentable empty or
clipped positional geometry.

The candidate uses the separate `PhotoStore-dev-position-animation` executable, settings, recovery
process, and bundle ID `ai.photostore.app.dev.position-animation`, with no registered file
associations. Development version remains 0.16.0. The baked source SHA is
`a4c52a3f2ec39e122ab5d145b6dd8b6b398b4316`, read from
`/Applications/PhotoStore-dev-position-animation.app/Contents/Info.plist` as `PhotoStoreBuildCommit`;
`fd414d2ed15c27ff323b19fcf6ae84351b3a4557` is exactly the base planning reference. The pass
has engine (2 steps), UI (2 steps), and delivery (4 steps) milestones. The source is frozen,
implemented, root-reviewed, compiled, packaged, installed, and runtime-tested for this local
delivery. Version is 0.16.0 development at `/Applications/PhotoStore-dev-position-animation.app`,
bundle `ai.photostore.app.dev.position-animation`, baked source
`a4c52a3f2ec39e122ab5d145b6dd8b6b398b4316`, and binary SHA-256
`6e9238d8d04954b4a1b611ebae16f711ceae74c47d719aa2df223cd218112b1d`. The final compiler/package
passed in 216.29 seconds; log `/private/tmp/photostore-position-animation-package-hierarchy-final-20261011.log`.
The runtime receipt recorded 69 requests, 69 responses, and 0 failures in
`/private/tmp/photostore-position-runtime-20261011-evidence/runtime.jsonl`; launch was
hidden/inactive/non-frontmost after helper-confirmed finished launching, and cleanup closed the
listener. Development branch checkpoint: `codex/position-stopwatch`. No screenshot/UI-pixel, Smart visual, external
PSD, or full-affine acceptance is claimed; the hierarchy source fix compiled but was not runtime-
tested for Smart hierarchy behavior.

## 2026-10-11: 0.16.0 independent video-opacity migration

Independent video opacity reuses existing owned tracks and leaves coupled video XY/opacity behavior
unchanged. The first enable preserves the legacy opacity curve and dense snapshots. Sparse legacy
curves with unique keys migrate to Linear interpolation; duplicate or dense legacy curves migrate to
bounded exact integer-frame Hold keys, including finite out-of-unit legacy alpha values, by retaining
raw interpolation before integer clamping. Migration is atomic and bounded at 1,000,000 frames,
100,000 owned keys, and 256 tracks, with non-finite or invalid input rejected. Sparse unique unit
keys retain future keys through Linear migration. After the dense extent, migration restores the
legacy/static value at frame extent when the extent is at most 999,999. The engine/UI implementation
is complete for this source pass. The baked source is `d66ddbcbed3989f66cb568b1f6d8f7f30fc9d464`.
The combined compiler passed in 45.07 seconds with exit 0; log
`/private/tmp/photostore-video-opacity-check-20261011.log` records 7 engine warnings and 1 existing
UI warning with no errors. The development package completed in 6m13s; build log
`/private/tmp/photostore-video-opacity-rc-build-20261011.log`. The installed candidate is
`/Applications/PhotoStore-dev-video-opacity.app` (`ai.photostore.app.dev.video-opacity`) with
executable SHA-256 `597d2e4da6e32cbaf12f0bcfb6db592ac70dc5dfbcdd9670f82250d0bcad89c6`. Installer
verification matched the hash and confirmed signature, ARM64 architecture, no document association,
no goal process, no default configuration, and no other applications touched. These installer results
were reported by root; the installed hash and bundle identity were independently read by root. No
standalone installer log was retained. The focused native runtime receipt passed on the installed
candidate: a fresh hidden launch reported PID 61599, the gate enforced finished launching, hidden,
inactive, and non-frontmost state, and the final flow produced 42 RPC responses, including one
expected rejection (`id18`, `timeline.addKeyframe` with owned opacity) and 41 successful responses,
before a clean quit with the listener closed. Screenshot and release evidence remain pending; the
development branch checkpoint is prepared on `codex/video-opacity`.

## 2026-10-11: 0.16.0 independent video Position development

Video Position persists canonical clip translation as `base_translation` plus the last rendered
`applied_translation`; frame snapshots use `video_position` rather than a source-frame bbox. The
owned Position track is independent from Opacity and uses whole-pixel Linear/Hold keys with bounded
100,000-key, 256-track, frame-999999, and ±1m limits. First enable migrates legacy placement at
integer frames with exact bounded Hold samples, rejects ambiguous dense bbox snapshots and owned
Position ancestors, and preserves raw XY for alpha-only operations. Rendering, seeking, source-frame
painting, save/reopen, and frame conversion use canonical offsets so motion is not baked into footage.
Destructive timeline operations freeze the visible pose into the base translation and preserve alpha.

This is a source-reviewed development checkpoint on `codex/video-position`, base
`2b93f6dd260291f9f49c8a5efda0698d3eb6cef8`, targeting `PhotoStore-dev-video-position` with bundle
ID `ai.photostore.app.dev.video-position`. Compiler, package, install, native-runtime, screenshot,
PSD, Smart visual, full-affine, release, and parity evidence remain pending. Future work includes
animated Position parents, full Smart affine support, and external PSD temporal acceptance.
