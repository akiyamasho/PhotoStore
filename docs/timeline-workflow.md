# Timeline workspace

> **Current delivery — 2026-11-01:** Stable [v0.19.0](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.19.0), exact source `8aaacce50eccee3eb6d6261b21910cce7fe32545`, is published and installed as the normal PhotoStore app and CLI. It carries the existing major timeline overhaul plus layout, editing, and media enhancements. Issue #23 remains open for user visual/interaction/export acceptance. See the [release receipt](releases/0.19.0.md), [specification](plans/video-timeline-2026-11-01.md), and [GPT Image design provenance](design/2026-11-01/README.md). No computer-use, screenshot, native interaction, or visual-acceptance claim is made.

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
- The left Media bin is a view of sources already present in the current document, with cached media
  thumbnails and import actions. Add at Playhead and drag-to-timeline use the backend duplicate
  command, preserving the user's clip clipboard.
- The scoped shortcut table lives in the existing shortcut preference/export model. Timeline keys
  are dispatched only after clicking the timeline and are excluded from the global Photoshop
  dispatcher. This preserves canvas shortcuts, typing, modal controls, and shortcut capture. The
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

Perspective/warp animation is unsupported. Transform keys use the source-center pivot; conversion
to Frame Animation remains guarded while Transform animation is enabled because key baking is not
implemented. During the earlier development pass, the required PhotoStore-dev build passed in 6m03s from the source SHA above. Its
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
