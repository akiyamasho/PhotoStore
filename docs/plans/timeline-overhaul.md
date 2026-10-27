# Issue #23 — major timeline workspace overhaul

Status: researched and posted to issue #23 before the major-overhaul sweep began; implementation is now in progress. The previous implementation was rejected by the user on 2026-10-27. Its build receipt does not verify this replacement. Current build and acceptance evidence belongs in [the workflow record](../timeline-workflow.md).

## Requirement and evidence

The user supplied a CapCut screenshot and said: “wrong, look how clean capcut is, and include all keyboard shortcuts and keyframing setup”, then explicitly required research, planning, an issue update with a major overhaul in mind, and only then the bugfix sweep.

Reference evidence:

1. The supplied 2662×1470 screenshot: narrow track-control gutter, compact single icon toolbar, one ruler, uninterrupted horizontal clip lanes, subdued separators, teal footage/orange text clips, thumbnails and audio indication inside clips, one continuous playhead, transport near the preview, and a separate right inspector. No property forms or repeated layer/keyframe headings occupy the ordinary timeline.
2. Installed CapCut **9.5.0/macOS**, Help → Shortcut, profile **Shortcut 1**: **107 action/gesture entries across four categories, 76 assigned and 31 unassigned**. Every visible row was transcribed in [the reference catalogue](capcut-9.5-shortcuts.md). This profile has customized or unusual bindings, so it is not called a universal default. Settings/project contents were not changed.
3. [CapCut keyframes](https://www.capcut.com/resource/how-to-add-keyframes-in-capcut), [keyframe animation](https://www.capcut.com/tools/keyframe-animation), and [split workflow](https://www.capcut.com/features/how-to-split-a-video-capcut-workflow-comparison) establish property diamonds, initial-key then automatic property keys, draggable timing, graph/ease controls, and selected-clip cutting. [Adobe's timeline workflow](https://helpx.adobe.com/photoshop/desktop/add-video-and-animation/use-keyframes/create-timeline-animation-workflow.html) and [Smart Object tutorial](https://www.adobe.com/learn/photoshop/web/animate-poster-text-images) establish retained editable contents and layer-property animation.

The screenshot establishes visual organization. Native shortcut observations establish that profile's keys. Public documentation establishes workflows. The architecture and bindings below are PhotoStore design decisions, not statements that every observed CapCut label has identical semantics in PhotoStore.

## Why the previous pass is insufficient

- `timeline_ui.rs` still puts multiple heading/mode/transport/settings rows ahead of a short nested scroll viewport. The ruler scrolls away with the tracks, and the label gutter scrolls horizontally.
- `video_clip_ui.rs` still treats every layer as its own track, paints largely empty solid clips, and expands multiple property rows beneath each layer. Split segments cannot behave as neighboring clips on a single persistent track.
- Animation values are edited through popup forms and Apply buttons. There is no proper selected-clip inspector, selected-key editor, drag retiming, easing setup, or keyframe editing keyboard context.
- Most timeline operations have no defaults. The global dispatcher can send A/B/V/S/Q/W, Space, arrows, Tab, Delete, and Cmd+B to Photoshop canvas commands instead. Merely exposing command IDs in Keyboard Shortcuts does not implement a coherent video-editing keymap.
- Clipboard operations, markers, ripple/magnet behavior, linked media selection, and multiple clips per track need engine behavior, not decorative buttons.

## Target visual hierarchy

```text
Existing PhotoStore editor / canvas-preview               Selection inspector
                                                         Transform  ◇  < >
                                                         Position / Scale / Rotation
                                                         Opacity    ◇  < >
                                                         Clip timing / Speed / Audio
──────────────────── compact timeline editing toolbar ──────────────────────
fixed track controls │ fixed time ruler / work area / marker row
─────────────────────┼─────────────────────────────────────────────────────
text  lock eye mute  │ [text clip]     [text clip]
video lock eye mute  │ [thumbnail clip][thumbnail clip]      [clip]
video lock eye mute  │         [overlay clip]
audio lock    mute  │ [actual media waveform / envelope]
                     │ selected-clip keyframe lanes ONLY when requested
─────────────────────┴──────────────── horizontal time scrollbar ────────────
```

The ordinary empty or unselected state is spacious and clip-focused. The toolbar uses consistent small icons, separators and tooltips with effective keys; zoom controls sit at the right. Put transport/time display adjacent to the preview or compactly outside the clip lanes. Settings are a single overflow menu. The default timeline has no mode sentence, frame counter paragraph, per-layer Properties headings, value forms, or duplicate audio/video headers. Frame Animation remains a distinct existing view.

The gutter and ruler must stay fixed under the appropriate scroll axis: vertical scroll moves track bodies and headers together while the ruler remains visible; horizontal scroll moves time content without moving headers. Compute a single shared viewport geometry before drawing. Adaptive ticks are based on zoom, frame rate and visible range. Clip labels are clipped, selection borders are legible, trim handles are predictable, and drag preview does not mutate history until release.

## Engine and document design

Keep the existing Rust layering and one native document/command system. Existing Position, Opacity, Smart Object source data and video frame stacks remain valid. No separate project database or ad-hoc UI-only timing model.

- Add backward-compatible **visual track metadata** to Timeline, with stable track IDs, names, order, kind, member layer IDs, and track lock/mute state. A visible clip is identified by its stable layer ID and canonical range; a track can contain multiple clips. A split creates the new layer/clip on the same track. Legacy documents derive one track per eligible layer until an edit materializes metadata. Stale references, duplicates, mixed group parents and limits are handled deterministically; do not silently reparent nested artwork.
- `layer_ranges` remains the canonical start/end timing. Add optional common clip playback metadata to a range only where necessary for new source offset, reverse or variable speed; existing VideoData playback is the legacy fallback, not a second independently writable authority. Shared source-time helpers must drive seek, preview, linked audio, nested Smart Objects and export.
- Multi-clip operations accept explicit document and visual/audio selections through engine commands, use one edit transaction, honor locks/source bounds, and return actionable errors. UI focus and inspector selection are shell state; media edits and key data are document state.
- Moving a clip carries its animation timing and linked media according to the linkage setting. Trim retains off-range source/key data; split preserves source continuity and each resulting clip's animation. Use one canonical animation-retiming helper; do not let UI code independently rewrite Position, Opacity, Transform and legacy keys.
- Retiming must retain representable off-range keys. If a hidden pre-clip anchor would move before frame zero, materialize the clip-start sample and preserve a cut eased boundary segment at integer frames, within the existing key-count limit. Reject the edit atomically if preservation exceeds that limit. Undo retains the original document. Legacy Smart clips capture their original absolute source clock before moving to clip-local playback, so the first edit does not jump their contents.
- Add persisted document markers and work-area commands. Main-track magnet/ripple, geometric snapping, linked selection and preview skimming are separate controls with separate behavior. A magnet icon must not merely toggle rounding of audio times.
- Copy/cut/paste clips and attributes operate on a typed engine clipboard containing complete editable media/animation snapshots, not raster-only clipboard pixels. Paste remaps IDs and times; undo restores the whole edit. Layer/Smart Object grouping is reused where it preserves contents and timing.
- Audio and nested Smart Object playback follow the same source-time policy, including trim/split/reverse/speed where supported. Curve speed needs real persisted retiming and matching preview/export mapping; a button that opens an empty page is not completion.

## Keyframing setup

1. Selecting a clip populates the existing right Properties dock with a video selection inspector (dock it in the video workspace even when the ordinary theme floats Properties). The timeline remains clean. A single Inspector control restores a hidden inspector.
2. Show Position, Scale X/Y (with linked scale option), Rotation, Opacity, clip timing/speed and relevant audio controls in clear groups. Each supported property has its own diamond and previous/next controls. Smart Object Convert/Edit Contents remains available from inspector and clip context menu.
3. The first diamond enables that property's animation and records the playhead value. Moving the playhead and directly editing the inspector value records a new key. Drag values preview live; commit once on release/Enter, not one undo step per pixel. No Apply popup is required for ordinary changes.
4. Alt+K toggles a dedicated keyframe pane for the selected clip. Alt+Shift+K adds/toggles a key for the selected property. Shift+Click adds a key at the clicked time. Diamonds support click/Shift selection, drag retiming, Delete, copy/paste, previous/next navigation, and context interpolation. Playhead and horizontal scale match the main ruler.
5. Provide Linear, Hold, Ease In, Ease Out, and Ease In/Out with a visible curve/value graph and editable timing. Interpolation is persisted and sampled by the engine for Position/Opacity/Smart Transform, not only painted as a curved line. Prevent accidental key overwrites; batch moves/deletes are one history entry.
6. Smart Object placement remains non-destructive. Ordinary canvas translation and supported affine scale/rotation should author the active animation track rather than being globally blocked. Unsupported projective/warp operations retain explicit guards; never silently rasterize or drop animation.
7. Selected keys and selected clips are distinct contexts. Delete affects selected keys while the key editor owns focus, selected clips in the timeline, and text inside a text field. Escape clears the transient context; undo/redo stays chronological.

## Complete shortcut coverage contract

Use the 107-row native reference catalogue as the accounting source. It includes 76 assigned keys/gestures, not just Play and Split. Every assigned entry must have an implementation mapping or a clearly documented unresolved semantic/out-of-scope disposition; missing core timeline actions keep this issue open. Unassigned AI/caption/voice/multicamera/services are not new feature requirements solely because their action names occur in the catalogue. They must not receive invented keys or dead controls.

Provide a **Timeline/CapCut editing preset**, an editable/searchable shortcut setup page grouped as Timeline, Keyframes, Player, Basic, Other, and a reference/cheat sheet generated from the same command table used to dispatch. Show primary and alternate keys, unbound actions, scope, conflicts and Reset-to-preset. Existing user overrides/export remain intact. Support the existing native shortcut JSON format and canonical watcher workflow; do not overwrite `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json` or global preferences as a side effect.

Required operation families and defaults:

| Family | Required setup |
| --- | --- |
| Selection and tools | A select, B razor; Shift/additive and marquee selection; [ / ] directional selection; explicit clip/work-area selection; Escape/deselect |
| Transport | Space play/pause; J/K/L reverse/stop/forward shuttle; Left/Right frame steps; Shift+Left/Right multi-frame steps; Home/End; Up/Down edit points. Keep the observed profile's customized frame keys as editable aliases rather than reversing arrow semantics. |
| Cutting | Cmd+B split selected, Cmd+Shift+B split all eligible unlocked clips, Q/W trim/delete to playhead, Backspace/Delete remove; selection-sensitive behavior for audio, stills, footage and Smart Objects |
| Clipboard/history | Cmd+C/X/V complete clips or selected keys; Cmd+Shift+C/V attributes; Cmd+Z / Cmd+Shift+Z undo/redo |
| Timeline navigation | Cmd+plus/minus, Cmd+wheel anchored zoom, Shift+Z fit, wheel vertical and Alt+wheel horizontal pan |
| Track/edit modes | P main-track magnet, N snapping, tilde linkage, S preview skimming, V enable/disable clip, Ctrl+Shift+S extract/restore linked audio |
| Markers/work area | M marker, Alt+M alternate color, Shift+M next, Alt+Shift+M previous, I/O work-area boundaries; Shift+X work area to selection and Alt+X clear work-area selection as explicit PhotoStore adaptations |
| Group/compound | Cmd+G/Cmd+Shift+G group/ungroup; Alt+G/Alt+Shift+G convert/opened Smart compound and unpack; Shift+Enter enter selected compound contents (the native `In` label remains marked semantically uncertain) |
| Speed/audio | Cmd+R speed inspector; Shift+B curve speed; Cmd+period/comma ±0.1dB with actual dB conversion and selected audio target |
| Keyframes | Alt+K pane, Alt+Shift+K add/toggle active-property key, Shift+Click key addition, key selection/drag/Delete/copy/paste, interpolation and previous/next controls |
| Player/basic | The observed Play/fullscreen/zoom-fit/rulers/guides/held alignment and text-center resize gestures; import, export, new, tab-to-media, return-to-editor, quit and Escape map to actual PhotoStore equivalents. Differences from CapCut workspace navigation are labeled as adaptations. |

Timeline context has priority only after interaction with the timeline, its inspector or preview. Typing, numeric/text editing, modal dialogs, rename, menus, shortcut capture and active canvas tools retain their keys. Closing/hiding the editor relinquishes context. Do not install a global A/B/V/S/Q/W/Space/Cmd+B override. Modifier matching must be exact and platform-aware. File operations remain the existing application commands. Shortcuts must report disabled-state reasons rather than silently do nothing.

## Ordered implementation and handoffs

1. **Research and issue correction.** Save this plan and the full observed catalogue; update/reopen #23 with the rejected prior outcome and major-overhaul acceptance. Only after this live issue update, snapshot the complete paginated open non-PR inventory and begin the sweep.
2. **Track/edit substrate.** Backend author owns doc/format/engine: persisted track membership, source-time helpers, clip selection command parameters, clip editing/clipboard/markers/magnet/linkage, and animation retiming. Publish explicit command/response interfaces before UI wiring.
3. **Clean workspace.** UI author owns the timeline renderer and inspector: pinned gutter/ruler, multiple clips per row, filmstrips/real cached waveforms, clean toolbar, selection/drag gestures, docked inspector. Replace the prior video rendering structure; preserve Frame Animation separately. This can run alongside backend implementation once interfaces are agreed.
4. **Keyframe and speed editor.** Backend author implements atomic key operations, persisted easing/retiming and source-time integration. UI author implements the selected-clip keyframe pane, graph controls and direct property editing against these commands. This step depends on the track/time contracts.
5. **Full shortcut setup.** UI author wires the complete scoped preset/catalogue/editor and gesture map through the existing dispatcher/preferences/export; reuse the same entries for tooltips and the cheat sheet. Backend author supplies real operation implementations. Missing assigned core operations are gaps, not documentation-only completion.
6. **Integrate native behavior.** Primary reviews the bounded handoffs and updates product/format decisions. Preserve native save/reopen, layers, original source data, compound contents, frame animation, existing key tracks and all user changes. No separate QA/reviewer/auditor agents.
7. **Required delivery.** Run the single required consolidated PhotoStore-dev compiler/build after source integration. Add/run only narrow evidence for actual data-preservation boundaries changed here (e.g. clip/key move/split plus native roundtrip, clipboard ID remapping, shortcut focus preventing deletion). No broad suites, screenshot passes or browser QA by default. Use background guarded dev reload, preserve running documents, keep production scheduler disabled, commit/push reviewed main and post truthful evidence.

## Acceptance / completion gate

- At the user's reference proportions, the ordinary video timeline consists of one compact toolbar, fixed ruler/header gutter, clean media lanes and actual clips; no per-layer property forms, duplicate headers or nested independent audio scroll areas.
- Panning/zooming never moves the track controls offscreen or changes alignment. Split pieces remain neighboring clips on a persisted track. Clip thumbnails/real audio envelopes are media-derived, bounded and cached; never fabricate a waveform.
- Inspector and selected-clip keyframe pane provide the full setup above. Keys can be added, selected, moved, deleted, eased and edited directly; Smart contents and media remain editable and synchronized.
- The 107 native entries are all accounted for, all 76 assigned entries have explicit mappings/dispositions, and all core timeline/keyframe bindings execute real operations through the command system with working shortcut customization. No hidden global conflicts or key stealing while typing.
- Save/reopen, undo/redo, split/move/trim/ripple, linked audio, compound editing and export retain canonical timing and stable identities. New persisted metadata has backward-compatible defaults and bounded validation.
- The previous compiler/persistence receipt is historical evidence only. Keep #23 open while this overhaul is partial or a required core shortcut/keyframe capability remains unimplemented. Report implemented, compiler-verified and manually accepted states separately; compilation alone does not establish visual matching.
