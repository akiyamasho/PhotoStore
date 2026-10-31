# PhotoStore issue sweep — 2026-10-31

The complete paginated open-issue inventory for `akiyamasho/PhotoStore`, fetched after filing the
user's timelapse report and reopening the matching smoothing issue, contains three issues after
excluding pull requests. The local full snapshot
is `/private/tmp/photostore-open-issues.json`.

The later cursor focus report added [#25](https://github.com/akiyamasho/PhotoStore/issues/25).
The refreshed complete paginated inventory contains four non-PR open issues, stored locally at
`/private/tmp/photostore-open-issues-cursor.json`. The earlier three-issue snapshot and build receipt
below describe the first implementation pass.

| Issue | Disposition | Delivery state |
| --- | --- | --- |
| [#20 — Drawing strokes remain angular despite smoothing with tablet pen and mouse](https://github.com/akiyamasho/PhotoStore/issues/20) | Reopened after the user's 35% smoothing report and inability to draw circles. Corrected the timed filter to follow moving input analytically, preserving interpolated time and pressure before curve reconstruction. | Local implementation; six smoothing regressions and one live/committed chunked-rendering regression passed. Native mouse/tablet acceptance remains outstanding; issue stays open. |
| [#24 — Save timelapse recording history with the PSD](https://github.com/akiyamasho/PhotoStore/issues/24) | Persist recording settings, encoded frames, action labels, pause/error state, and layer recording exclusions in a namespaced PSD/PSB image resource. | Local implementation; five focused persistence regressions passed. Photoshop open/re-save acceptance remains unverified; issue stays open. |
| [#23 — Major overhaul: CapCut timeline workspace, full shortcuts, and keyframing](https://github.com/akiyamasho/PhotoStore/issues/23) | Existing implementation and development-build receipts are recorded in [timeline workflow](timeline-workflow.md). Manual visual, interaction, and export acceptance remain outstanding. | Remains open; this timelapse change does not establish timeline acceptance. |
| [#25 — Active tool cursor sometimes resets after switching apps](https://github.com/akiyamasho/PhotoStore/issues/25) | User reports intermittent arrow cursor after Alt-Tab while a tool remains selected. Added per-viewport focus recovery that refreshes native cursor visibility after the final cursor is resolved. Tool, build, OS, input device, and exact switch-key mapping remain Unknown. | Local implementation; required development build pending. Native app-switch acceptance remains outstanding; issue stays open. |

The report creation and duplicate report update used the explicitly invoked PhotoStore bug-report
workflow. Unreported build, device, reproduction, and error details remain Unknown. A reviewed still
from the user's 4.88-second screen recording was attached to #20; the original video was preserved.
No application screenshot or unrelated artwork was captured.
The timelapse change required narrow save/reopen evidence because it addresses loss of recorded
history and preservation of private-layer exclusions. The smoothing checks address the reported
severe drawing blocker. No broad test suite, screenshot pass, or manual application/Photoshop
acceptance is implied.

Local `main` already contains three unpublished timeline commits before this change. Publishing
those commits has not been authorized in this session. Both fixes remain reviewable locally;
neither issue may be closed on the strength of a local-only fix. No issues were closed: two local
fixes and one separate acceptance-dependent timeline issue remained open at the first checkpoint.
The subsequent cursor report raises the current inventory to four open issues; no issues have
been closed by this workflow.

## Focused evidence

All Rust work used the shared repository target directory, `CARGO_BUILD_JOBS=1`, and
`RAYON_NUM_THREADS=1`. Commands used `nice -n 10`; the sandbox denied the priority change, but
the compiler and tests completed.

- `cargo test --locked --offline -p photostore-io --test psd_recording`: 5 passed. Includes real
  encoded 1920×1080 JPEG frames saved and reopened through PSD and PSB, enabled and paused states,
  private-only metadata, purge/stale data, foreign resources, invalid frame rejection, and opaque
  malformed/future metadata. Log: `/private/tmp/photostore-psd-recording-check.log`.
- `cargo test --locked --offline -p photostore-paint --lib smoothing`: 6 passed. Includes the
  analytic timed-motion response at 35%, irregular partitions of a circular path, time/pressure
  interpolation, stationary catch-up, and existing smoothing paths.
- `cargo test --locked --offline -p photostore-paint --lib chunked_rendering_matches_one_shot`:
  1 passed, comparing incremental and committed rendering with smoothing enabled.
  Paint log: `/private/tmp/photostore-smoothing-check.log`.

The isolated old-filter reproduction showed 87.75 px versus 81.4078 px at the end of equivalent
0→100 px timed motion sampled differently, and 6.655 px endpoint drift between equivalent circular
paths. The new tests require matching analytic response and equivalent-path endpoints. These are
synthetic input checks; the user's native input sequence was not recorded as pointer events.

## Development delivery

The combined source commit is `4f653e04a1bb3820bb67ef64a7c60d6481d703ba`. One consolidated
PhotoStore-dev build passed in 179.582 seconds using the existing development helper's `build()`
and `save_pending()` functions. It produced runtime
`4f653e04a1bb3820bb67ef64a7c60d6481d703ba-97040010455e8515`, with source fingerprint
`97040010455e851529d7bb8d7534af698d8b7f70166cb9a6a4e2b4d56350e78f`.
Build log: `/private/tmp/photostore-combined-dev-build.log`.

The old controlled dev process remained running while the new binary was staged. Because a live
child lease prevents a second watcher from attaching, this pass acquired the watcher owner lock
without blocking, used the existing shared build lock and helper build path, and staged the result
without starting a watcher, rotating its token, requesting a reload, or changing the app session.
The previous pending runtime's binary remains available. The production app was not rebuilt,
reinstalled, or launched.

To load the staged runtime, save work and quit PhotoStore-dev normally, then run `./dev --background`
from the repository root. The watcher consumes the pending binary. Native manual acceptance remains
outstanding. This documentation receipt does not change the binary's source commit or require
another build.

After staging, `ai.photostore.local-ci` remained disabled and unloaded, and local-CI status reported
no pending development or release job. Historical failure flags remained present; the scheduler
was not enabled or otherwise changed by this pass.

Suggested manual acceptance, not performed by this pass:

1. On a disposable canvas, use the same Brush preset, size, zoom, input device, and 35% smoothing
   as the report. Draw circles and curved strokes, checking both live feedback and the final stroke.
2. Explicitly enable recording on a disposable document, make several edits, save PSD, close and
   reopen it, and check replay/history and private-layer exclusions. Purge, save, and reopen again
   to confirm the old frames do not return. Recording remains off by default.
3. If a Photoshop round trip is required, use a copy of that PSD and compare artwork, layers, and
   retained PhotoStore recording after Photoshop saves it. Preservation by Photoshop is unverified.

## Follow-up: cursor focus recovery (#25)

The native integration caches the selected cursor. On macOS, winit also caches the cursor icon
and its visibility, while app switching can reset the actual OS cursor to the arrow. Re-selecting
an unchanged cursor can therefore skip the native refresh. The repository-owned cursor lifecycle
now tracks focus loss and return separately for each viewport. Once focused with a current pointer
position, it resolves tool images and widget cursor precedence, then queues an opposite-to-final
visibility transition in the completed UI pass. This forces native cursor rectangle refresh while
retaining the current cursor's final visibility. It does not change the selected tool or synthesize
pointer movement. If pointer hover was cleared, recovery waits for actual pointer re-entry.

The fix stays in PhotoStore's UI integration, without editing Cargo registry sources or upgrading
dependencies. No discretionary cursor tests or screenshot passes were added or run; native manual
acceptance is the default. On a disposable document, select a Brush or another tool, switch to
another application and back, and check that the cursor returns correctly over the canvas and that
toolbar, text, resize, and dialog cursors still take precedence. This manual pass has not been run.
