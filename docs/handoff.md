# PhotoStore handoff — 2026-10-11

## Current baseline

- **Published and installed:** PhotoStore 0.16.0, immutable tag `v0.16.0`, exact source `a87fd1049844a41d456cbdce9d5fb0fa5038ab3c` on `main`.
- **Release:** [v0.16.0](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.16.0), release ID `409665038`.
- **Installed app:** `/Applications/PhotoStore.app`, ARM64, ad hoc signed and not notarized; executable SHA-256 `9bf8fea3b609532c6ef2e08959e17477166b4aa485701384e667945ab190faf6`.
- **Artifacts:** DMG SHA-256 `396c880ec9b4c6a092a2dbee54b872e8f2936aac0f445758102e8ea5c143ccb8`; CLI ZIP SHA-256 `5a6ab30230490c8b5e0c1b9df58126605a9b7cc96b0a28be9f3c46029ce143b5`.
- Mounted DMG app, CLI, signatures, full source, licenses, architecture, and checksums passed. The DMG correction reused the verified final binaries without another Rust build, source edit, or retag.
- Previous bundle backup: `/Applications/.PhotoStore.backup.v0.16.0.1791723317.97155.app`. Preferences, recovery, and artwork were untouched; PhotoStore was not launched during promotion.
- The detailed release receipt and screenshot evidence are in [0.16.0 release notes](releases/0.16.0.md) and [0.16.0 screenshot evidence](screenshots/0.16.0/evidence.md).

## Remaining work, in priority order

1. **Smart Object workflow depth and animation.** Deepen existing shared/embedded/linked contents editing and identity workflows; linked child “Save Contents” currently embeds native content instead of updating the external file. Define and implement missing/relink/update behavior, then cover nested timing/ownership/cache, transforms/styles, Smart Filters, and external PSD round trips. The 0.11 contents-edit reliability work is implemented; broader acceptance remains open. See [animated Smart Objects](animated-smart-objects.md) and [Smart Contents editing](smart-contents-editing.md).
2. **Timeline and media acceptance.** Position and Opacity stopwatches are implemented for ordinary layers and video; bounded RPC and real preview screenshots exist. Loop counts and onion skins are implemented in 0.10. Continue timing/UI acceptance, full affine and style tracks, clip groups/transitions, arbitrary split-segment replacement envelopes, sparse audio, output latency/master-clock sync, rational frame rates (currently `f32`), and lazy long-video decoding (currently capped at 512 MiB RGBA8 eager surfaces). See [animation playback](animation-playback.md) and [audio timeline](audio-timeline.md).
3. **Photoshop workflow evidence.** Measure the remaining families in the ledger with PhotoStore-specific evidence: color/depth/HDR; layers, masks, transforms; filters and RAW; vectors and text; retouch/tablet behavior; formats/print; actions/plugins; native UI and reliability. `627/627` menu liveness is command dispatch evidence only.
4. **Packaging follow-up.** Persist APFS-safe image creation in `packaging` and make the release verifier mount the DMG and inspect the bundled app. The published artifact was corrected and verified, but `package.sh` still creates an HFS hybrid image and `verify.sh` does not mount-check the app.
5. **Optional workflow depth.** Shortcut sorting/import-cancel/alternate editing and broader interaction acceptance remain open if selected as the next scope. Recording remains off by default; deterministic replay, tablet pressure, and parity acceptance remain open.

## Working rules for the next pass

- Use the consolidated `./dev` watcher and `PhotoStore-dev` runtime for iteration. Launch in background without focusing the user’s active UI. Reload only when the owned dev app has no tabs, jobs, or dialogs; never touch an active production app.
- Share `/Users/computer/Desktop/dev/PhotoStore/target`; use `CARGO_BUILD_JOBS=1`, `RAYON_NUM_THREADS=1`, `nice -n 10`, and one heavy Rust job at a time. Keep document behavior in the shared command registry and persistent document model.
- Delegate execution to the newest available Luna model at medium effort, using bounded subtasks with disjoint write scopes; report milestones and current-step totals and completion percentages.
- Start new work from remote `main` in a clean worktree. The shared checkout may contain another agent’s dirty work; do not reset, stash, or overwrite it.
- Keep recording disabled unless explicitly enabled by the user. Avoid discretionary QA unless requested. Record status as implemented, preview, verified, or published with its actual evidence.
- For future releases, commit real screenshot evidence before tagging, tag the exact source SHA, then build and verify that exact SHA and package. Human review remains required before creating/publishing a release or tag.
- The full Photoshop backfill is unfinished. No overall completion percentage is asserted.
