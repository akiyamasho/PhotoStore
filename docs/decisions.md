# Implementation decisions

## 2026-10-18: Provider-independent Generative Fill

Generative Fill belongs to the shared engine command registry. A versioned file contract supplies
the full composite, selection mask, crop, exact placement, and user prompt to a local Codex process
or a trusted custom adapter. Only validated, correctly sized output reaches an undoable top-level
raster layer; the original feathered selection is the final authority for coverage. Existing
layers remain intact. The provider owns inference and authentication. Untrusted automation cannot
launch providers or set executable preferences. See [the contract](generative-fill.md).

## 2026-10-11: Consolidated development watcher boundary

The default local edit loop uses one isolated `PhotoStore-dev` installation with incremental
shared-target builds and safe process restarts. It does not require a pull request or a bundle
reinstall for each edit, and it is not hot module replacement: open or unsaved documents and
recovery state defer a restart until a disposable demo document is saved or closed. Independent
goals use separate `PhotoStore-dev-<goal>` identities. Trusted `main` updates the consolidated
dev installation; production remains eligible only from a published, non-draft, non-prerelease
release whose tag resolves to its exact source SHA.

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

At the source checkpoint on `codex/video-position`, based on
`2b93f6dd260291f9f49c8a5efda0698d3eb6cef8`, targeting `PhotoStore-dev-video-position` with bundle
ID `ai.photostore.app.dev.video-position`, compiler, package, install, and native-runtime evidence
were pending; the delivery receipt below records completion. Screenshot, PSD, Smart visual,
full-affine, release, and parity evidence remain pending. Future work includes animated Position
parents, full Smart affine support, and external PSD temporal acceptance.

## 2026-10-11: video Position delivery receipt

The `PhotoStore-dev-video-position` candidate is installed at
`/Applications/PhotoStore-dev-video-position.app` and was packaged at
`/private/tmp/photostore-video-position-rc-0.16.0-2c3fd1099/PhotoStore-dev-video-position.app`.
Bundle ID is `ai.photostore.app.dev.video-position`, version is 0.16.0 development, baked source
SHA is `2c3fd1099d516587b5893d08f2734832cbcd8751`, and the installed executable SHA-256 is
`514f390dba78eb664a8ebd904db076c820eba1c15e5bd6b727f9c73ab3c7f632`. The final compiler/package
passed exit 0 in 3m00s; installer and package logs are retained at
`/private/tmp/photostore-video-position-install-20261011.log` and
`/private/tmp/photostore-video-position-package-final-20261011.log`.

One corrected native flow passed with 107 requests and responses, 106 successes, one expected legacy
XY ownership rejection, and zero unexpected failures. It covered three in-app source frames,
including an empty frame, painted placement, opacity samples 1.0/0.4/0.5, save/reopen,
Video↔Frame conversion, alpha preservation, recording-off state, and disabled Position freeze at
[10,6] with zero keys. No screenshot, Smart visual, animated-parent, PSD, full-affine, release, or
full-product parity claim is made.

## 2026-10-11: 0.16.0 production release evidence boundary

The production candidate may combine independently documented earlier milestones, but evidence keeps
its capture version and source provenance. Historical 0.13.1 and 0.14.0 images are linked as such;
they do not substitute for 0.16.0 screenshots. Raster Position and video Position/Opacity captures
must be real, legible PhotoStore captures from disposable demo documents before the docs checkpoint
and immutable release tag. The 0.16.0 preview build is not a final production package and cannot
establish publication or production installation. Status labels remain separate for source
implementation, preview compilation, screenshot verification, and publication. Full Photoshop,
Smart Object, PSD, affine, and audio parity are not claimed by this release.

## 2026-10-11: 0.16.0 production build profile

Reserve the final production build for after the reviewed documentation commit and immutable source
tag. Use Cargo opt level 2, LTO disabled, one Cargo job, and offline dependency resolution so the
single required production package uses the existing local dependency cache and limits resource
pressure. The ad hoc macOS package is not notarized. Preview-build and screenshot receipts remain
separate from final source/package identity and publication evidence.

## 2026-10-12: Open issue sweep scope and evidence boundary

This sweep records five open issues from the complete repository inventory. Issues #7, #8, and #11
are bounded source fixes intended for closure only after their landed commit is on `main`: #7 keeps
the dedicated original toolbar icons for Fill, Pen, and Lasso; #8 gives Escape precedence to reset
the active canvas view after ordinary cancel, modal, and text-edit handling; and #11 serializes the
`tools.moveAutoSelect` preference, restoring the last on/off value with a default of `true`. Issue
#9 implements its selected/candidate source marker and transformed brush-region preview feedback,
with the canonical engine mapping, and is intended to close after the landed commit reaches `main`.
The supplied reproduction has Aligned enabled: that mode intentionally keeps a fixed
source/destination offset, so the source region follows the pointer after release. Normal
post-release slot math was equivalent before and after resolver cleanup, so this pass does not
claim a stale-source or post-release shift fix. Issue #10 is closed under the accepted fallback:
the current recording implementation remains off by default, and no capture, measurement,
benchmark, or enablement is added by this sweep.

Implementation, compiler evidence, runtime acceptance, and publication are separate statuses. The
integrated source commit `d4cd1c035d1e43a74ef4b594056e7cc71199b78e` passed the required command
`cargo check --locked --offline -p photostore --features dev-install` with the shared target,
single Cargo job, Rayon one, and `nice -n 10`; the receipt is
`/tmp/photostore-sweep-check-final-20261012.log`. The documentation follow-up does not change
compiled source and requires no rebuild. Manual runtime acceptance was not run. No issue is closed
from documentation alone, and no version bump, release, reinstall, screenshot, or UI-verified claim
is implied; live GitHub state is authoritative for source delivery and issue closure.

## 2026-10-11: 0.16.1 Free Transform pointer scaling

Free Transform corner and edge drags resize freely by default; Shift constrains proportions for
both handle types. The legacy scaling preference remains stored for compatibility but no longer
changes the visible pointer gesture. Lasso selection state continues to receive transform pointer
movement through drag and release.

## 2026-10-12: 0.16.2 bug-fix release preparation

The 0.16.2 source release carries the four delivered fixes from the open-issue sweep: dedicated
Fill, Pen, and Lasso glyphs; Escape reset for an idle canvas view while preserving cancel, text-edit,
and modal priority; Clone Stamp current hover/candidate sampled-region feedback with canonical
first-stroke mapping and the user-confirmed Aligned fixed-offset behavior; and serialized
`tools.moveAutoSelect` state with default `true` across the theme bars and Move tool presets.
Timelapse remains off by default under the accepted fallback: encoded frames can retain up to
256 MiB per document, raw RGBA8 frame sizes are approximately 7.91 MiB at 1920×1080 and 31.64 MiB
at 3840×2160 before other allocations, and no representative RSS measurement was run. No automatic
enablement is claimed.

The release source is version 0.16.2 for the ARM64 app and CLI artifact pair. The prior 0.16.1
screenshots remain historical and are not reused as 0.16.2 evidence. Source implementation,
compiler/build, package verification, installation, runtime acceptance, and publication remain
separate statuses; the final release transaction must produce the app and CLI once from one exact
source SHA before local installation and any authorized publication.

## 2026-10-12: 0.16.2 final delivery receipt

The 0.16.2 source is delivered on `main` and immutable tag `v0.16.2` at
`a7a2dd6c306d2f4ce33bd3a6fe87044cac03d520`. GitHub release
`RE_kwDOVDxG7c4YeXjs` is published at
https://github.com/akiyamasho/PhotoStore/releases/tag/v0.16.2. One combined ARM64 app-plus-CLI
release build passed in 12m12s using the shared target, one Cargo job, Rayon one, and `nice -n 10`.
The DMG SHA-256 is `c5750663625c9d1ad30986e8a98a0d15a0be781a670d919318b2df7e6e0fac8d`; the CLI ZIP
SHA-256 is `e93ae784bed80553f8452f85f419f5fc514cf3fecad70b9d0e04cfbaa09ffeea`; GitHub asset
metadata matched both digests and the checksum manifest.

The app is installed at `/Applications/PhotoStore.app` with executable SHA-256
`c6e2c3351e7685c3b23ff39e3c33cf6678c7d24dcfe01abb153f6efbfc6ad260`; the CLI is installed at
`/Users/computer/tools_bin/photostore-cli` with executable SHA-256
`26004fddb0e3ab810d5498740937f6fd5ab1d1fef5a61cf9b46e257a02e7dd85`. The previous app bundle is
preserved at `/Applications/.PhotoStore.backup.v0.16.2.1791810390.app`. Artifacts are ad hoc signed
and not notarized. The production app was not launched and no manual runtime, screenshot, or GUI
acceptance was run.


## 2026-10-18: Generative Fill boundary matching

Default gradient-domain boundary matching reuses the existing cancellable membrane solver. It
matches generated colors to the original composite on the selected region's inner boundary,
retaining generated texture and original selection coverage without touching preserved pixels.
A `seamless` boolean in provider preferences and command parameters exposes direct provider colors
when desired. Selecting the whole canvas has no preserved outer boundary and retains direct colors.


## 2026-10-18: Prompt-only Generative Fill and central setup

Provider, executable, adapter arguments, timeout, and boundary matching live in Preferences →
Generative Fill. The edit dialog contains only a prompt, plus an Open Settings link when setup
is incomplete or generation fails. A bounded background check verifies executable availability
and Codex sign-in without sending image data. Preferences opens over the prompt so returning
retains the draft; Check setup refreshes readiness after installation/sign-in.

## 2026-10-18: Native app update flow (issue #12)

The native app updater checks stable releases from the fixed `akiyamasho/PhotoStore`
repository through GitHub's API, using the existing authenticated `gh` CLI without
embedding credentials. Engine commands are `app.update.check`, `app.update.status`,
`app.update.prepare`, and `app.update.install`; the Help menu opens an update dialog,
where checks are user initiated. This implementation targets native macOS app installs.

Before replacing the app, the updater verifies the release checksum and requires a
strict signature check. It stages the incoming app, preserves user-set xattrs, modes,
and ACLs from the installed app while retaining the incoming content and signature,
then requests a clean quit before install. A recoverable backup supports rollback;
installation is refused while document tabs or jobs remain open. Workspace-limited
automation cannot invoke check, prepare, or install because those actions access the
network or install outside granted workspace authority; read-only status remains
available. The updater does not migrate macOS TCC/privacy identity grants; signing
changes may require renewed user consent. Source implementation is present; the
metadata helper's standalone synthetic harness passed three focused tests, engine/UI
checks passed, and native app compilation passed. The live replacement path has not
been verified against an installed app; no release, local installation, or runtime
verification is claimed.

## 2026-10-18: 0.18.0 source preparation

Prepare PhotoStore 0.18.0 with the native in-app GitHub updater as the release scope. The updater
uses authenticated `gh` access to stable releases, checks the release digest and strict app
signature, and stages updates for the Apple Silicon macOS app. Preserve user xattrs, POSIX modes,
ownership, flags, and ACLs for matching paths or abort before replacement; retain incoming signed
contents, clean-quit/open-document safeguards, and a recoverable backup. TCC/privacy grants are not
migrated and a signing identity change may require renewed consent.

The workspace and lockfile now declare 0.18.0. The prior updater source passed its engine/UI and
native compiler checks, and the metadata harness passed three focused tests before this version
preparation. The version-bumped source still needs its single release build and final artifact
verification. Local installation and publication remain pending; no tag, source SHA receipt,
artifact hash, or runtime update claim is recorded here.
