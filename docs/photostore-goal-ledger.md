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
- Audio/video source commit `405d102f1ddc8f13ed7ae8b6ce8779db0c0f28ec` is pushed on `origin/main`; the earlier `287f7c94c065d37d3bdc177c3e44f28719490e84` is archived WIP. The integrated audio/clip milestone is documented in [audio-timeline.md](audio-timeline.md); it is compiler-verified but not packaged or installed.
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

## Next priority: action recording

Action recording is the current 0.7.0 milestone, ahead of further animation backfill. Its scope and
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

## Current 0.8.0 release pass

The upcoming release combines Audio, Smart Filter Masks, and the New Document Fix. Status is explicit:

1. Recording release installed — complete (`0.7.0`, current installed release).
2. Audio source integrated and pushed — complete (`405d102f1ddc8f13ed7ae8b6ce8779db0c0f28ec`).
3. New Document Fix — implemented and compiler-verified.
4. Smart Filter mask engine — implemented and compiler-verified.
5. Smart Filter mask UI — implemented and compiler-verified.
6. Movie reload/audio reprobe and split-envelope fixes — implemented and compiler-verified; arbitrary split-segment replacement remains open.
7. Native compiler gate — complete; exit 0 in 30.41 seconds, log `/private/tmp/photostore-smart-mask-newdoc-check-20261011-rerun.log`.
8. Documentation and version update — complete; 0.8.0 version/docs preparation is ready.
9. Commit and push — pending.
10. Tag — pending.
11. Build and publish release — pending.
12. Reinstall and confirm New Document creation — pending. The installed 0.7.0 app visibly confirmed the prior `file.new` invalid-parameter error (`recording` unknown) through read-only accessibility inspection; that specific cause is fixed, and confirmation is planned after the 0.8.0 reinstall.

Further Photoshop backfill is the next pass and is outside this twelve-step count. Exact UI parity,
mask copy/move/channel selection, unknown PSD filters, audio latency/master-clock behavior, and
broader acceptance remain unverified.

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

- Persisted audio tracks/clips, linked movie audio, editing commands, shared FFmpeg preview/export, and streamed playback are present in source. The locked offline native app/CLI HEIF compiler check passed in 7.88 seconds; its log is `/private/tmp/photostore-audio-clip-check-20261011-rerun2.log`. Full UI/audio acceptance remains unverified.
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

1. Complete acceptance of the native media boundary: verify decode ownership, cancellation, tiny MP4/MOV/WebM cases, range selection, fractional-FPS timestamps, and UI failure behavior. Record supported containers/codecs before calling import/export supported.
2. Continue the integrated audio/clip milestone: finish body/trim commit controls, reload-footage and split-segment replacement behavior, then address exact audio latency and shared-clock acceptance; continue clip groups, transitions, loop counts, onion skins, work-area rendering, and long-video memory behavior.
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
- Smart Filter rows now support collapse, context menu, move/reorder, blend options, and delete in the engine/UI path (`crates/engine/src/smart_cmds.rs`, `crates/ui-egui/src/smart_ui.rs`); direct mask painting and full acceptance remain open.
- Smart Object corner skew and perspective/homography are present in the current model; transform reset clears warp/perspective while preserving source contents and identity. Their external-file fidelity remains unverified.
- Linked-source workflows need explicit relink/update/missing-source behavior and acceptance.
- PSD external acceptance remains incomplete: fidelity is masked by filter math, unsupported/unmapped Camera Raw settings, and other format-specific gaps. Do not describe Smart Object PSD parity as complete.

### Exact next steps

1. Define source identity and contents-editing UX for embedded, linked, and shared sources, including stale/missing source behavior.
2. Add direct filter-mask painting, editable parameter round trips, source refresh invalidation, and broader Smart appearance/animation coverage now that the targeted regression has passed.
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
