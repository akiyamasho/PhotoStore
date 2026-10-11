# PhotoStore goal ledger

Updated 2026-10-11. This is an operational ledger for the independent PhotoStore fork. It records evidence and next actions; it does not convert inherited menu wiring into behavior parity or claim that the product is complete.

## Product and provenance

- Origin source: `git@github.com:storytold/photocraft.git`; original history, licenses, `NOTICE`, attribution records, and provenance are retained.
- Product identity: PhotoStore, with the independent executable/configuration/package identity already established in the repository.
- Established GitHub repository: private `akiyamasho/PhotoStore`, with `origin` as the only remote.
- Milestone references: `photostore-foundation-2026-10-10` at `d83f8323`; `photostore-animation-smart-objects-2026-10-10` at `06e736db`; `photostore-animation-media-2026-10-11` at `4019c6ce28966236e070a3d894a56c2968643820`.
- Icon direction: PhotoStore icon with the requested twist; keep the asset provenance row with the icon work.
- README/AGENTS direction: keep badges, install, run, and contributor guidance lean and operational.
- Branding release completed: tag `photostore-branding-2026-10-11` points to `e3703d4be2ceba37e04b81b2761f7b3acc27235b`; the [PhotoStore 0.6.1 release](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-branding-2026-10-11) contains the Apple Silicon DMG, CLI ZIP, and `SHA256SUMS`.
- Audio/video source commit `405d102f1ddc8f13ed7ae8b6ce8779db0c0f28ec` is pushed on `origin/main`; the earlier `287f7c94c065d37d3bdc177c3e44f28719490e84` is archived WIP. The integrated audio/clip milestone is documented in [audio-timeline.md](audio-timeline.md); it is compiler-verified and included in the packaged, installed 0.8.0 release.
- Photoshop-facing scope: timeline/frame animation and Smart Objects are first product priorities, while the inherited feature surface remains evidence to audit rather than a completeness claim.

## Evidence rules

- `docs/parity.md` currently reports 627/627 menu items live. This measures command dispatch only; it does not establish Photoshop behavior, interaction quality, file fidelity, or user readiness.
- `docs/roadmap.md` records a historical caution that menu and feature-surface counts overstate behavior parity, and identifies AI, missing tools, professional workflow depth, and the plug-in ecosystem as major gaps. Do not carry its historical aggregate estimate forward as a current PhotoStore measurement.
- `docs/scorecard.md` is inherited historical evidence and a useful signal for areas to revalidate; its PhotoStore labels and measurements are not independent PhotoStore evidence. Revalidate each claim before treating it as current PhotoStore status.
- `docs/animated-smart-objects.md` records implemented child-timeline persistence, playback synchronization, bounded caches, nested refresh limits, and transform reset; its stated gaps remain open.
- A milestone is `implemented` only when the behavior and persistence path are present in the source of truth. `unverified` means code exists but acceptance evidence is missing. `pending` means the capability or integration is still absent.

## Integration evidence (2026-10-11)

- All-targets Cargo check passed after the source repairs, including compiler coverage for the documented timeline, Smart Object, KYS, and native media paths.
- The exact external KYS API integration checks passed 3/3; the KYS library checks passed 4/4.
- The 8×8 MP4/MOV/WebM media smoke passed 1/1 for colors, frame order, range selection, cancellation, and alpha handling.
- The new Smart appearance regression passed 1/1, and the post-repair engine check passed.
- The exact Photoshop fixture imported 152 primary rows, 3 alternates, and 51 nonempty tool keys. Unknown scripts, Bridge, Smart Object, filtering/minimize/hide, and UI-only candidates are reported rather than silently substituted.
- These are compiler/function and targeted integration results. They do not establish visual UI QA, Photoshop behavior parity, or full PSD/external-format compatibility.
- Release record: annotated tag `photostore-animation-media-2026-10-11` was pushed at `4019c6ce28966236e070a3d894a56c2968643820`.
- Published prerelease: [PhotoStore 0.6.0 prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-animation-media-2026-10-11) includes the Apple Silicon DMG, CLI ZIP, and `SHA256SUMS`; artifacts are ad hoc signed and not notarized.
- The existing `0.6.0` release record and tag `4019c6ce` remain historical; the completed `0.6.1` branding release supersedes it for the current product identity.
- Branding release integrity: the ARM64 build and package checks passed for bundle ID, version, baked SHA, icon, licenses, strict signatures, and checksums. The artifacts are ad hoc signed and not notarized; no broad visual QA or new feature tests were run.
- Local install record: `/Applications/PhotoStore.app` version 0.6.1, bundle `ai.photostore.app`, installed with the matching source SHA; strict signature validation passed, startup was confirmed, and the app remains open as PID 54840. The previous app backup is preserved at `/private/tmp/photostore-reinstall-20261011.qcBCXU/previous-PhotoStore.app`.
- This release record does not claim full visual or Photoshop acceptance; the remaining Photoshop scope and active goal are unfinished.

## Historical 0.7.0 action-recording milestone

Action recording was the 0.7.0 milestone, ahead of further animation backfill. Its scope and
constraints are documented in [action-recording.md](action-recording.md). Compiler verification
passed with the locked offline native targets, and the prerelease is published and installed; broad UI
acceptance, native visual QA, and parity remain pending. The full Photoshop scope remains active and
unfinished.

The compiler check passed with `cargo check --locked --offline -j1 -p photostore -p photostore-cli
--features heif`; the final result is recorded in `/private/tmp/photostore-recording-check-20261011-final.log`.

The annotated tag `photostore-action-recording-2026-10-11` points to
`56aede3295b1e344b709c79043ef608a1b21e855`. The [PhotoStore 0.7.0 prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-action-recording-2026-10-11) is published with the ARM64 HEIF DMG, CLI ZIP, and two-entry `SHA256SUMS`; artifacts are ad hoc signed and not notarized. Package integrity passed for bundle ID, version, exact SHA, icons, licenses, strict signatures, and checksums.

The DMG is 26,112,401 bytes with SHA-256 `bb76f515c58434e7974db115ff9298893176d82179c543081d52b3ae76a9af2f`; the CLI ZIP is 16,145,049 bytes with SHA-256 `81f662080e85e988d41f69bdc38257a823db2651e9e5e4164ea961288f249d67`.

Installation record: canonical `/Applications/PhotoStore.app` is version 0.7.0, bundle `ai.photostore.app`, binary SHA-256 `84103a6c087c2ee786e4b785f48e72e9fa3c2424a21b3abc5aa8e2ce4d5067f`, and source SHA `56aede3295b1e344b709c79043ef608a1b21e855`; strict signature validation passed, the canonical executable started as PID 36046, and startup was confirmed. The retained old bundle is `/Applications/PhotoStore-0.6.1-recovery-20261011-111423.app`; the previous-app backup is `/private/tmp/photostore-reinstall-0.7.0-20261011-111423/previous-PhotoStore-0.6.1.app`.

The prior 0.6.1 process (PID 57432) remained alive with its artwork untouched after a native save stalled in the final directory. The attempted target `/Users/computer/Desktop/dev/PhotoStore/recovered-artwork/Untitled-1-before-0.7.0-20261011-110908.pcraft` has not appeared, so no artwork recovery or backup is claimed. The sample log is `/private/tmp/photostore-save-stall-20261011.sample.txt`; it shows the save blocked in `photostore-job-1 save_file → atomic_write_with → sync_dir → File.open → open`, with no TCC prompt.

## Historical 0.8.0 release pass

The completed 0.8.0 release combined Audio, Smart Filter Masks, and the New Document Fix. Status is explicit:

1. Recording release installed — complete as the historical 0.7.0 milestone; the 0.8.0 release below is also historical, with 0.9.0 now current.
2. Audio source integrated and pushed — complete (`405d102f1ddc8f13ed7ae8b6ce8779db0c0f28ec`).
3. New Document Fix — implemented, compiler-verified, and confirmed after reinstall with a normal 64×64 recording document creation.
4. Smart Filter mask engine — implemented, compiler-verified, and included in the 0.8.0 release.
5. Smart Filter mask UI — implemented, compiler-verified, and included in the 0.8.0 release.
6. Movie reload/audio reprobe and split-envelope fixes — implemented, compiler-verified, and included in the 0.8.0 release; arbitrary split-segment replacement remains open.
7. Native compiler gate — complete; the earlier gate exited 0 in 30.41 seconds, and the final live-mask/target-aware gate exited 0 in 4.27 seconds. Final log: `/private/tmp/photostore-smart-mask-newdoc-check-20261011-final.log`.
8. Documentation and version update — complete for 0.8.0.
9. Commit and push — complete at source commit `711e73995ac23b5a408a6a07e458ebfc3df91d12`.
10. Tag — complete: `photostore-animation-smart-masks-2026-10-11`.
11. Build and publish release — complete as the published 0.8.0 prerelease.
12. Reinstall and confirm New Document creation — complete. After reinstall, the native app showed `Untitled-1 — PhotoStore`, `64 px ×64 px (72ppi)`, Background on, and `Untitled-1 @200% (RGB/8)` with no unknown `recording` parameter error; the blank document was left visible.

The 0.8.0 source/tag is `711e73995ac23b5a408a6a07e458ebfc3df91d12`. The [published prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-animation-smart-masks-2026-10-11) contains the ARM64 DMG (26,309,368 bytes, SHA-256 `b91990e7164d6114199a6aa6fe7d258cecec5469765c1634ce05b6e0094f875e`) and CLI ZIP (16,277,058 bytes, SHA-256 `1d52d4c8a671cd8a2b16ea68401196db4738609458df739c30c0d8de7777c198`); `SHA256SUMS` is 206 bytes and contains only those two entries. The optimized build completed in 5m24s on 2026-10-11 UTC; its log is `/private/tmp/photostore-0.8.0-build.log`. Artifacts are ad hoc signed and not notarized.

The canonical `/Applications/PhotoStore.app` is version 0.8.0 with bundle `ai.photostore.app`, strict signature validation, binary SHA-256 `40d09d7cf3a0374a5debc933d268ccdc9ce0894304228d381e5e0657f88168fa`, and running PID 86439. The previous app backup is `/private/tmp/photostore-reinstall-0.8.0-20261011-115045/previous-PhotoStore-0.7.0.app`; the retained recovery sibling is `/Applications/PhotoStore-0.7.0-recovery-20261011-115045.app`. No artwork save or recovery is claimed.

Further Photoshop backfill is the next pass and is outside this twelve-step count. Exact UI parity,
unknown PSD filters, audio latency/master-clock behavior, and broader acceptance remain unverified;
the 0.9.0 mask transfer and Channels implementation is tracked below.

## Historical 0.9.0 mask, Channels, and keyframe pass

This source pass follows the installed and published 0.8.0 release. The 0.9.0 release pass is now
complete:

1. Public specification and architecture — complete.
2. Mask transfer engine — implemented: same-document ID guards, inherited locks, copy-on-write copy,
   default no-overwrite, one atomic undo edit, and self no-op.
3. Filter-mask channel sources — implemented for selection, Apply Image/Calculations, and duplicate
   alpha channels, with document-index channel sources and document-ID mask/property guards.
4. Mask transfer UI — implemented for plain move, Option copy, named destinations, overwrite prompt,
   and preserved thumbnail modifiers.
5. Channels UI — implemented for the temporary Filter Mask row, scoped normal target, overlay/gray
   eyes, footer load/delete, and ordinary-channel exit from filter view.
6. Video keyframe command/UI repairs — implemented for typed pending x/y/opacity values, document
   timeline bounds, target and lock guards, atomic Set/Delete, and no-op handling.
7. Compiler gate — complete; the initial rerun passed in 12.18 seconds and the final transfer-chooser gate passed in 3.71 seconds. Final log: `/private/tmp/photostore-mask-channels-keyframes-check-20261011-final.log`.
8. Version and documentation — complete; version 0.9.0 is prepared.
9. Commit and push — complete at source commit `3c905b47444cb71a9137572859d7a84cdffdb125`.
10. Tag — complete: `photostore-mask-channels-keyframes-2026-10-11`.
11. Build and publish — complete as the published 0.9.0 prerelease.
12. Reinstall and confirmation — complete; the canonical 0.9.0 app passed strict signature validation and started successfully. The prior 0.8.0 app was retained cleanly.

Empty Copy/Move menus, the palette, and assigned shortcuts open the named-destination chooser. A
captured document ID works across tabs; no-eligible-destination errors are explicit; transfer IDs
are limited to known values; overwrite defaults false; failures retain the form with an inline
`status_error`, and success closes it. Remaining scope includes general sparse tracks, stopwatches,
full transform/style controls, transitions, groups, exact audio master-clock synchronization,
coalesced external key-edit refresh, and broad UI/Photoshop acceptance. The 0.9.0
mask/Channels/keyframe release is historical. No new broad QA or Photoshop parity claim is made.
The prior 0.8.0 targeted 64×64 New Document confirmation remains historical
evidence.

The immutable 0.9.0 source/tag is `3c905b47444cb71a9137572859d7a84cdffdb125`. The [published
prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-mask-channels-keyframes-2026-10-11)
is an ARM64, ad hoc signed, not notarized build. The DMG is 26,334,832 bytes with SHA-256
`6441ca5f6636b2f517cdda0a7eff3201b6a02627ffe5a06e1998db14a13c7c2d`; the CLI ZIP is 16,278,953
bytes with SHA-256 `8662222462dcaf7d7bbdc0cab147f72badd07267864e57bc3c3c7405a50273f0`.
`SHA256SUMS` is 206 bytes and contains exactly those two assets. The build log is
`/private/tmp/photostore-0.9.0-build.log`.

The canonical `/Applications/PhotoStore.app` is version 0.9.0 with bundle `ai.photostore.app`,
binary SHA-256 `cecb44e45488e0f82c61c11b8579f1aef7a0a1655a00fa49872a955911b6d8bf`, strict signature
validation, and running PID 11312. The previous-app backup is
`/private/tmp/photostore-reinstall-0.9.0-20261011-121622/previous-PhotoStore-0.8.0.app`; the
retained recovery sibling is `/Applications/PhotoStore-0.8.0-recovery-20261011-121622.app`.
The old 0.8.0 `Untitled-1` was left cleanly and no artwork recovery is claimed.

## Completed 0.10.0 animation playback release pass

Version 0.10.0 is published and installed. The repeat/onion pass is documented in
[animation-playback.md](animation-playback.md). Its twelve steps:

1. Architecture and public specification — complete.
2. Persisted repeat/onion model — implemented.
3. Strict repeat/onion commands — implemented.
4. Finite playback, Smart Object, and GIF behavior — implemented.
5. Native UI editors and editable bindings — implemented.
6. Bounded canvas onion compositing — implemented.
7. Compiler gate — complete; the required check passed in 8.21 seconds with warnings and the existing 0.1.6 future-incompatibility notice. Final log: `/private/tmp/photostore-animation-repeat-onion-check-20261011-final.log`.
8. Version and documentation — prepared; implementation/compiler evidence is complete.
9. Commit and push — complete at source commit `f3a749978f72c91c0f0d4b4aa5d3e5e79ce7ec3e`.
10. Tag — complete: `photostore-animation-repeat-onion-2026-10-11`.
11. Build and publish — complete as the published 0.10.0 prerelease.
12. Reinstall and confirmation — complete; the canonical 0.10.0 app passed strict signature validation and started successfully. The prior 0.9.0 app was retained cleanly.

The New Document fix and historical 0.8.0 64×64 confirmation remain recorded above. No new broad
UI or Photoshop parity claim is made for 0.10.0.

The immutable 0.10.0 source/tag is `f3a749978f72c91c0f0d4b4aa5d3e5e79ce7ec3e`. The [published
prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-animation-repeat-onion-2026-10-11)
is an ARM64, ad hoc signed, not notarized build. The DMG is 26,361,008 bytes with SHA-256
`e94a86862a0157c22bf29db4789b67613a2dfb440441ad4650158c98fbd22137`; the CLI ZIP is 16,293,312
bytes with SHA-256 `834925e42a5613bdefea0157b9585e08957fd0e9bd293912ffec1fe5b4a8de6a`.
`SHA256SUMS` is 208 bytes and contains exactly those two assets. The UTC build log is
`/private/tmp/photostore-0.10.0-build.log`.

The canonical `/Applications/PhotoStore.app` is version 0.10.0 with bundle `ai.photostore.app`,
binary SHA-256 `90f90eae31f8043577323e4138cf694c2a4d3167b1f103f55d55e76c934bc969`, strict signature
validation, and running PID 24533. The previous-app backup is
`/private/tmp/photostore-reinstall-0.10.0-20261011-123258/previous-PhotoStore-0.9.0.app`; the
retained recovery sibling is `/Applications/PhotoStore-0.9.0-recovery-20261011-123258.app`.
The old 0.9.0 window had no document or unsaved marker and quit cleanly without a prompt.

## Completed 0.11.0 Smart Contents reliability release pass

Version 0.11.0 is published and installed. The scope is
documented in [smart-contents-editing.md](smart-contents-editing.md). Its twelve steps:

1. Architecture and public specification — complete.
2. Fallible close paths — implemented.
3. Batch/discard validation and parent-child guards — implemented.
4. Save/Don't Save/Cancel intent — implemented.
5. Contents context and parent navigation — implemented.
6. Integration — implemented in source.
7. Compiler gate — complete; the required check passed in 12.13 seconds. Final log: `/private/tmp/photostore-smart-contents-check-20261011-final.log`.
8. Version and documentation — prepared; implementation/compiler evidence is complete.
9. Commit and push — complete at source commit `3ebf38f77bd55bb57fc6c0f3cea3e9ead04667b9`.
10. Tag — complete: `photostore-smart-contents-2026-10-11`.
11. Build and publish — complete as the published 0.11.0 prerelease.
12. Reinstall and confirmation — complete; the canonical 0.11.0 app passed strict signature validation and started successfully. The prior 0.10.0 app was retained with its unsaved workspace preserved.

The recording, audio, repeat/onion, New Document, mask, Channels, and keyframe milestones remain
historical and preserved. The old 0.10.0 `Untitled-1` process remained alive after its quit prompt
was canceled; no artwork save or recovery file is claimed. Full Smart Object, linked external PSD,
and Photoshop parity scope remains unfinished.

The immutable 0.11.0 source/tag is `3ebf38f77bd55bb57fc6c0f3cea3e9ead04667b9`. The [published
prerelease](https://github.com/akiyamasho/PhotoStore/releases/tag/photostore-smart-contents-2026-10-11)
is an ARM64, ad hoc signed, not notarized build. The DMG is 26,373,589 bytes with SHA-256
`5921bf6b85934ecf82876dceb85c7086a74912eebb2ab4d04f10bf8d46252dd0`; the CLI ZIP is 16,299,958
bytes with SHA-256 `0e7c15286128056b0849aed9c923d754094b4d5b33fe2ab785b6a5985392073f`.
`SHA256SUMS` is 208 bytes and contains exactly those two assets. The UTC build log is
`/private/tmp/photostore-0.11.0-build.log`.

The canonical `/Applications/PhotoStore.app` is version 0.11.0 with bundle `ai.photostore.app`,
binary SHA-256 `2f386c3daa780d69304163dbce2ad469b02b92eef63b97f87abce7b0df027037`, strict signature
validation, and running PID 42277. The previous-app backup is
`/private/tmp/photostore-reinstall-0.11.0-20261011-125054/previous-PhotoStore-0.10.0.app`; the
retained recovery sibling is `/Applications/PhotoStore-0.10.0-recovery-20261011-125054.app`.

## Current 0.12.0 recording default-off pass

Version 0.12.0 is prepared in source; 0.11.0 remains the current installed and published release.
The current recording policy defaults New Document recording off and resets the checkbox on each
opening, while explicit API/preset opt-in and existing stored enabled documents remain supported.
The twelve-step status is:

1. Scope and plan — complete.
2. Default-off source behavior — complete.
3. Screenshot rules — complete.
4. Version preparation — complete.
5. Required compiler gate — complete; exit 0 in 33.99 seconds, log `/private/tmp/photostore-default-off-check-20261011.log`.
6. Preview build and screenshot evidence — complete; marker `preview-0.12.0-default-off`, preview build 5m33s, log `/private/tmp/photostore-0.12.0-preview-build.log`.
7. Actual screenshots — complete: approved compressed WebP captures and evidence are recorded under `docs/screenshots/0.12.0`.
8. README and release evidence — complete for the pretag state; publication remains pending.
9. Commit and push — pending.
10. Tag — pending; planned `photostore-recording-default-off-2026-10-11`.
11. Final build, exact SHA, and publish — pending.
12. Reinstall and confirmation — pending.

The approved captures are `new-document-recording-default-off.webp` (900×700, 51,972 bytes) and
`recording-settings-zero-frames.webp` (530×320, 24,036 bytes), with accompanying `evidence.md`.
Images show the unchecked New Document control and Settings Frames: 0. The capture flow confirmed
successful Create and observed the unchecked Time-lapse Recording menu item in accessibility data.
The global disable control is unchecked and is not evidence of per-document enabled state. The native preview instance quit normally without open user
documents. No broad UI QA claim is made; the 0.11.0 Smart Contents release remains current until
this pass is published and installed.

## Animation and timeline

### Implemented milestone

- Persisted video timelines and frame animation state in `.pcraft`, including timing, playback state, layer states, frame stacks, and native UI/CLI command integration.
- `FrameLayerState` now carries position, blend, fill opacity, and effects snapshots, while `Timeline.property_frames` preserves editable property tracks (`crates/doc/src/video.rs`, `crates/engine/src/video_cmds.rs`).
- Parent playback advances animated Smart Object children with time conversion and explicit frame-delay handling.
- Cumulative CFR time conversion and configurable tweening are wired through the timeline command/UI path (`crates/engine/src/video_cmds.rs`, `crates/engine/src/timeline_cmds.rs`, `crates/ui-egui/src/timeline_ui.rs`).
- Smart conversion keeps property-track ownership explicit when animated layers become embedded Smart Objects (`crates/engine/src/smart_cmds.rs`, `crates/engine/src/video_cmds.rs`).
- Optional Photoshop `.kys` import supports a primary set and alternates, stable command IDs including Japanese-labelled commands, and default preservation for unmapped or default-equivalent shortcuts (`crates/engine/src/kys.rs`, `crates/engine/src/prefs.rs`, `crates/ui-egui/src/kys_import.rs`).
- Native FFmpeg movie probing, RGBA8 frame decode/export, async job handling, and movie/timeline UI integration are present (`crates/engine/src/media_backend.rs`, `crates/engine/src/video_cmds.rs`, `crates/ui-egui/src/movie_ui.rs`, `crates/ui-egui/src/timeline_ui.rs`).
- Animated source conversion preserves the embedded child timeline, full document canvas, animated visibility/opacity ownership, and frame-qualified source caches.
- Smart Object refresh is bounded to depth 16, releases locks before decode/refresh, and preserves the last valid cached appearance on nested decode failure.

### Unverified or incomplete

- Persisted audio tracks/clips, linked movie audio, editing commands, shared FFmpeg preview/export, and streamed playback are present in the packaged 0.8.0 source. The locked offline native app/CLI HEIF compiler check passed in 7.88 seconds; its log is `/private/tmp/photostore-audio-clip-check-20261011-rerun2.log`. Sparse-track behavior, exact output latency/master-clock synchronization, and full UI/audio acceptance remain unverified.
- Transitions are not implemented or accepted.
- Stopwatches, transform tracks, clip groups, transitions, arbitrary split-segment replacement envelopes, and full Photoshop timeline/group/filter-mask scope remain pending.
- Complex tween interpolation is configurable and carries position/opacity/effects properties, but required acceptance evidence is pending.
- Rational source rate is probed by the media backend but stored as `f32` in the existing document model (`crates/doc/src/video.rs`).
- Eager decoded movie surfaces are capped at 512 MiB; the native import range is currently command parameters rather than a dedicated picker form.
- Movie decode/export pixel transfer is RGBA8. Long-video lazy decode, sustained memory behavior, render work-area behavior, and source-rate fidelity need acceptance evidence.
- Loop counts, onion skins, and advanced playback controls remain incomplete.
- Photoshop Video Timeline and Frame Animation need broader interaction coverage beyond the current command-driven foundation.
- Required integration checks passed: all-targets Cargo check, exact `.kys` API 3/3 plus library 4/4, 8×8 MP4/MOV/WebM smoke 1/1, Smart appearance regression 1/1, and the post-repair engine check. Broader tween, native UI, audio/timeline, and PSD acceptance remain unverified.

### Exact next steps

1. Author finite frame-animation repeat choices and configurable bounded onion skins; this is the next source pass and has no 0.10 compiler or release claim.
2. Continue the integrated audio/clip milestone: close arbitrary split-segment replacement envelopes, then address exact audio latency and shared-clock behavior; continue sparse tracks, clip groups, transitions, loop counts, onion skins, work-area rendering, and long-video memory behavior.
3. Add deterministic timing fixtures covering mismatched FPS, explicit frame delays, seek/playback boundaries, nested animated Smart Objects, and frame-animation export.
4. Add interaction acceptance for the Photoshop Video Timeline and Frame Animation panels after the engine contracts are stable.
5. Keep the alternate `.kys` mapping optional. If supplied at `/Users/computer/Desktop/dev/tools/Animation/photoshop_keys.kys`, use the primary/alternate API with stable Japanese IDs and preserve PhotoStore defaults for unmapped shortcuts.

Official Photoshop references for future workflow comparison:

- [Create timeline animations](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/use-keyframes/create-timeline-animation-workflow.html)
- [Edit video clips and audio in the timeline](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/organize-and-manage-video-groups/edit-video-clips-and-audio-in-the-timeline.html)

## Smart Objects

### Implemented milestone

- Embedded and linked source handling, persistent source identity, animated child timelines, source/cache invalidation, nested refresh limits, and undoable `Layer > Smart Objects > Reset Transform` exist in the native document model.
- PSD mapping infrastructure preserves embedded Smart Objects and Smart Filters in the inherited compatibility path; the scorecard records embedded Smart Object export as done.

### Unverified or incomplete

- Contents editing and shared contents identity workflows need product-level acceptance.
- Smart Filter rows now support collapse, context menu, move/reorder, blend options, delete, and the 0.9.0 mask transfer/Channels paths in the engine/UI; full acceptance remains open.
- Smart Object corner skew and perspective/homography are present in the current model; transform reset clears warp/perspective while preserving source contents and identity. Their external-file fidelity remains unverified.
- Linked-source workflows need explicit relink/update/missing-source behavior and acceptance.
- PSD external acceptance remains incomplete: fidelity is masked by filter math, unsupported/unmapped Camera Raw settings, and other format-specific gaps. Do not describe Smart Object PSD parity as complete.

### Exact next steps

1. Define source identity and contents-editing UX for embedded, linked, and shared sources, including stale/missing source behavior.
2. Continue post-0.9.0 keyframe UI coverage, sparse-track handling, editable parameter round trips, source refresh invalidation, and broader Smart appearance/animation coverage; retain mask transfer and Channels as implemented while broader acceptance remains open.
3. Exercise the existing perspective/homography and transform-reset paths against source contents, masks, filters, identity, undo, and external-file round trips.
4. Expand PSD acceptance fixtures for live Smart Objects, multiple instances, Smart Filters, masks, transforms, external sources, and Photoshop reopen/render checks.

## Remaining Photoshop feature families

The full Photoshop scope is tracked by family. Prioritize animation and Smart Objects first; use the historical scorecard as a signal source, then create PhotoStore-specific evidence for each family:

- Color and document foundations: color management, profiles, depth, HDR, channels, print, metadata, and document setup.
- Core editing: layers, groups, masks, selections, transforms, crop, canvas/view, undo/history, and layer comps/artboards.
- Adjustments and filters: adjustment layers, destructive adjustments, Smart Filters, blur/sharpen, distortion, noise, stylize, and filter galleries.
- Vectors and shapes: paths, pen tools, shape layers, fills/strokes, masks, and vector editing.
- Retouch and measurement: healing, clone, patch, remove, content-aware operations, rulers, guides, notes, and sampling.
- Brush dynamics and tablet behavior: pressure, tilt, rotation, smoothing, presets, deterministic replay, and native tablet edge cases.
- Typography: type on path, RTL and complex-script shaping, OpenType controls, variable-font axes, kerning, and PSD round trips.
- File formats and print workflows: PDF, RAW and XMP sidecars, spot plates, PSB/large documents, layered TIFF, and acceptance corpora.
- Automation and ecosystem: document-independent Actions, `.atn` import, richer batch workflows, plug-in ABI/panels, and automation acceptance.
- Native UI and preferences: floating/restored panels, scale and theme controls, remembered tool/preset settings, shortcut presentation, and elimination of unread preferences.
- Packaging and reliability: fallible allocation, large-file progress, cancellation, memory budgets, signing, and repeatable release artifacts.

## Ledger discipline

For each future entry, record the capability, status (`implemented`, `unverified`, or `pending`), evidence path or fixture, known limitation, and the next concrete action. Keep inherited history and licensing intact. Update status only after the corresponding implementation or acceptance evidence exists; never use the 627/627 menu-liveness figure as a substitute for behavior parity.
