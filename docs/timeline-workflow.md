# Timeline workspace

> **Current delivery — 2026-11-02:** Stable [v0.20.7](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.20.7), exact binary source `7c17226b4e0989a55a32c963ae4f39b63afd1054`, is published and installed as the normal app/CLI. This delivers stable preview toolbar geometry, background paused refresh/completion wakeup, hidden-media readiness, bounded decoder ownership/buffer reuse, eligible placed-size video preview and pan/pen event routing fixes. [Release/evidence](releases/0.20.7.md). Native scanout/input/audio acceptance is manual; historical statuses below describe earlier releases, not the current delivery.

Issue #23 was reopened on 2026-10-27 after the prior timeline UI was rejected. This page records
the major-overhaul implementation in source commit
`5aa7c4a3566a3825bb1acb191e9279bd556dec21`, published to `main` on 2026-10-31 in the
user-approved push through `43f0e2b6fe0ac67037f32d967f49bf10c19cc3ea`.
Issue #23 remains open while manual visual, interaction and export acceptance are outstanding.
This is an original egui interface informed by public
references, with no CapCut or Photoshop assets copied.

## Current video workspace

- Video mode has a compact editing toolbar, one time ruler, timeline-wide horizontal time offset,
  fixed track labels/control gutter, and a shared vertical track scroll. Rows use persisted visual
  tracks where available; legacy documents derive one row per layer until an engine edit materializes
  track membership. A track may show more than one clip layer.
- Clip bars show names and types. Video frames and still surfaces use bounded cached thumbnails;
  linked movie thumbnails use the engine's asynchronous movie cache and source-frame mapping. Audio
  lanes request real decoded PCM waveform envelopes from `audio_waveform_cache`; while decoding,
  available bins are shown, and no fake waveform is generated.
- Still and Smart Object bars preview moves and trims before committing through timeline editor
  commands. Video bars retain the existing drag/snapshot commit path. Clicking a clip selects the
  same active layer used by the Properties dock. Main-track magnet reorders selected clips and
  repacks that row; ripple move is an independent optional mode. General snapping targets the
  playhead, markers and other clip edges. Smart Object conversion and Edit Contents remain available
  in clip context menus.
- The right Properties dock is enabled for video timelines in either workspace theme. Selected
  clips expose Position, Opacity and available Smart Transform controls with key diamonds and
  previous/next navigation. The dedicated selected-clip keyframe pane is closed by default and
  toggles from the toolbar or Alt+K. Its key markers can be multi-selected, dragged, deleted,
  copied/pasted, eased, added with Shift+Click, and edited together with the atomic key-value command.
  The pane also draws a selected-channel value curve for Position X/Y, Opacity, Transform Scale X/Y,
  and Rotation. It samples the engine animation evaluators at up to 512 points over the visible time
  interval, so Linear, Hold, and eased interpolation are shown from persisted track data. Graph key
  markers select and retime horizontally; Alt-drag edits value and time together in one
  `timeline.editor.key.moveValue` history operation. The inspector supports batch value editing for
  multiple selected keys. Audio clip details remain in the same Properties dock when audio is selected.
- The separate Media/Import sidebar was removed in 0.20.6. Layers/clip context, Place and drop are
  the media workflow; Relink, Replace Footage, Reload Frame and Rasterize remain on video layers.
  Linked audio is a thin band in its video row; standalone audio has its own compact row.
- The scoped shortcut table lives in the existing shortcut preference/export model. Most timeline keys
  are scoped to the timeline/inspector; Space transport is available from ordinary controls outside
  focused canvas interaction and text/modal entry. This preserves canvas shortcuts, typing, modal controls, and shortcut capture. The
  current table includes clip/key clipboard and attribute clipboard actions, transport, modes,
  markers, work area, Smart Object operations, media import, video render, audio extraction/volume,
  speed-curve controls, timeline zoom, key pane, key interpolation/navigation and player
  fullscreen/zoom/ruler/guide actions. A searchable 107-row reference view shows all 76 assigned
  observations, the six gesture mappings, and the 31 unassigned/no-default dispositions alongside
  the PhotoStore workflow each assigned row reaches.
- Frame Animation remains on its separate existing interface.

## Documented design references

- [CapCut desktop keyframe guide](https://www.capcut.com/resource/how-to-add-keyframes-in-capcut) describes property diamonds, moving keys, Alt+K, and Shift+Click.
- [CapCut keyframe animation](https://www.capcut.com/tools/keyframe-animation) describes automatic property keys and graph/ease refinement.
- [CapCut split workflow](https://www.capcut.com/features/how-to-split-a-video-capcut-workflow-comparison) describes selected-clip splitting. The installed 9.5.0 macOS Shortcut 1 profile was separately recorded in [the reference catalogue](plans/capcut-9.5-shortcuts.md); it is not claimed as universal defaults.
- [Adobe Photoshop timeline workflow](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/use-keyframes/create-timeline-animation-workflow.html) describes layer property animation.
- [Adobe Smart Object animation tutorial](https://www.adobe.com/learn/photoshop/web/animate-poster-text-images) describes editable Smart Object animation.

The plan and accounting source are [timeline overhaul](plans/timeline-overhaul.md) and the
[CapCut 9.5 shortcut catalogue](plans/capcut-9.5-shortcuts.md). The screenshot supplied during the
issue report establishes visual hierarchy only; native app observations establish that installed
profile's bindings; public documentation establishes reference workflows. PhotoStore behaviors and
shortcuts are design adaptations.

## 0.19.0 source follow-up

The later user authorization includes enhancements and normal production-identity installation. The source adds explicit mode menus, compact responsive overflow, adaptive ruler labels, larger lanes and visible-range media work, shared source-time movie sampling, prioritized/coalesced canvas requests, nested Smart Object request isolation, and explicit optional ripple deletion on non-Main visual/audio tracks. Ordinary deletion remains unchanged. See [0.19.0](releases/0.19.0.md) for exact compiler, artifact, publication and installation evidence. No source change or generated design establishes manual visual acceptance.

## Remaining requirements and limits

Main-track magnet reorders clips; with P enabled, Main-row deletion/trim/paste repacks the row and
updates later clips. Optional ripple moves on other rows are available. The observed native profile
does not assign a ripple-delete shortcut; 0.19.0 exposes an explicit Ripple Delete action for non-Main visual/audio rows without inventing a default key.
Variable-speed curves can be created and edited by dragging
interior graph points or changing their frame/speed fields; the engine maps those curves in preview
and export and synchronizes linked audio. The documented Alt+Drag text-center resize gesture is
implemented in the existing paragraph-text tool. Visual clip thumbnail sampling is bounded to
visible clip rectangles. The selected-clip inspector exposes ordinary source trim/speed and selected
audio properties.

Perspective/warp animation remains unsupported. Transform keys use the source-center pivot. The
0.19.0 limitation that guarded conversion while Transform animation was enabled is addressed in the
current source pass by sampling affine Smart Transform into editable frame snapshots while retaining
the Smart Object source. Its focused persistence test passed during the later 0.20.0/0.20.1 delivery passes. During the earlier development pass, the required PhotoStore-dev build passed in 6m03s from the source SHA above. Its
runtime binary was `5aa7c4a3566a3825bb1acb191e9279bd556dec21-216e5754fdbb8689`; its guarded reload
was deferred because the existing PhotoStore-dev process had open documents. The later consolidated
runtime `a387feb51d63b3749f892af046de592403105b2d-8184edd6c3af42a5` includes this overhaul plus the
smoothing, PSD recording, and cursor fixes. At that time it passed its required build and was staged;
the running process was left untouched. See [the historical receipt](bugfix-sweep-2026-10-31.md).
Normal 0.19.0 installation supersedes that development delivery; routine dev builds/watchers are now
retired by the user's policy. The old session was preserved while its owned watcher was stopped.
Seven focused implementation filters
passed 1/1 each during the implementation pass; they did not all run against the exact final source
SHA. Manual visual, interaction and export acceptance remain outstanding. Do not claim full
CapCut/Photoshop parity or close issue #23 until acceptance is complete.

## Current source: scalable playback and authoring (0.20.2 published and installed)

This current-source pass follows the published 0.20.0 app and CLI. The [scalable playback plan](plans/scalable-playback-2026-11-01.md)
and [GPT Image workflow reference](design/2026-11-01/smooth-playback/workflow.png), generated and
inspected before implementation, describe the intended architecture. The [0.20.2 cadence receipt](releases/0.20.2.md)
records source changes and current verification status. The generated board is synthetic design
material, not a product capture or measured result.

The working source now fast-paths identity `Surface::convert`, caches eligible video transfer
ramps, and uses persistent sequential FFmpeg preview streams with per-frame publication and
generation-aware requests. The transport clock drives visible timeline indicators independently
from the last decoded document frame; buffering is explicit, and playback avoids starting cold
filmstrip, animation-frame, or source-card thumbnail work. Existing project time mapping, exact
seeks, original-source export, editable layers/Smart Objects, drawing canvas, and color-management
contracts remain authoritative.

Retained derived media now uses a shared adaptive budget targeted at one quarter of the existing
Memory Usage allowance, capped at one eighth of physical RAM, with a 16 MiB floor. It replaces the
previous fixed movie/thumbnail/waveform partitions; it is not a total application memory cap. Four
timeline operations no longer reject estimates solely for exceeding 64 MiB. They use checked
allowance accounting, fallible reservation and cancellable atomic edit jobs. Frame metadata uses
copy-on-write banks while preserving the existing v3 serialized array form. Recording frame codec
allocation is geometry-based; the 256 MiB recording archive and 100,000-frame limits remain.

The separate 64-megapixel Smart affine-render guard remains pending a safe tiled/preflight renderer.
The decoder's 8-bit intermediate and applicable format, codec, archive and decompression limits
also remain. No total-RAM, zero-copy GPU, HDR, or achieved-FPS claim follows from these source
changes.

Eleven focused critical checks and the production 0.20.1 build/installation passed. Subsequent
production hash inspection confirmed a cadence defect, despite improved latency: 180 requests
contained only 120 distinct source frames. The corrected nanosecond-rounding boundary passed an
additional original-frame-by-frame regression at 30 and 30000/1001 fps, with seeks/tail/VFR holds.
The 0.20.2 production build and full-180-frame evidence passed, and the corrected normal app/CLI
were published and installed. See the [release/performance record](releases/0.20.2.md). The pre-change warm blocking
exact-seek baseline was 640.39 ms for 1080p and 1,929.93 ms for 4K (five noninitial requests each),
not native playback FPS. Native UI/audio acceptance remains pending; keep #23 open.
