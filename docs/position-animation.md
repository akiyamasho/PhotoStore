# Position animation development

This document describes the current PhotoStore development version, kept at 0.16.0, for independent
layer Position animation. It is a development design and implementation record. Local
compile/package/install/native-runtime evidence passed; visual acceptance remains pending.

## Product boundary

The candidate identity is `PhotoStore-dev-position-animation`, with bundle ID
`ai.photostore.app.dev.position-animation`. Its executable, settings, recovery process, and
configuration are separate from production and from the other goal applications. It registers no
file associations. Production and consolidated goal applications remain untouched. Version remains
0.16.0 for this development pass; no whole-workspace version rebuild is part of this pass.

The baked source commit SHA is `a4c52a3f2ec39e122ab5d145b6dd8b6b398b4316`, read from
`/Applications/PhotoStore-dev-position-animation.app/Contents/Info.plist` as `PhotoStoreBuildCommit`.
The base planning reference is exactly `fd414d2ed15c27ff323b19fcf6ae84351b3a4557`; it is not the
candidate source SHA.

## Contract

`Layer.PositionAnimation` is a native optional property. Missing data remains backward-compatible
with no Position track. A track stores an applied Position anchor and its applied value, rather
than deriving state from a layer bounding box. Keys are whole-pixel signed X/Y values, with frame
indices from 0 through 999,999, with signed X/Y bounds of ±1,000,000. Interpolation is `Linear`
with rounded f64 results or `Hold`.
Position owns a document budget of 100,000 total keys and 256 tracks; Opacity has its own budget.
Ordinary Raster, Text, Shape, and Smart layers are authorable. Independent video Position and a
full Smart affine transform remain future scope. Existing legacy video XY and independent Opacity
behavior remain supported unchanged.

The engine exposes `set/togglePositionAnimation`, `set/deletePositionKeyframe`, and
`timeline.setPosition`. Movement updates the first owned Position ancestor, adding or updating a key
when enabled and changing static Position when disabled; unowned groups recurse, and a direct child
move keys that child. This prevents a parent wrapper and child curve from receiving the same movement
twice. Generic
bounding-box painting does not create keys. First enable migrates legacy dense Position snapshots to
bounded Hold keys and reconciles the
baseline with the current pose; future keys are retained. Enabling an already-enabled track is a
no-op. Disabling or deleting the last key freezes the playhead pose and clears owned keys; a later
enable starts a fresh static point.

The UI retains captured document ID, layer ID, frame, track, and history guards. The engine keeps
document ID, layer, and frame bounds at the command boundary; static typed setters do not require
callers to supply history or a frame. XY values are typed. Previous/next diamond markers and
Linear/Hold controls are exposed, and unassigned defaults remain editable through the existing
shortcut registry.

## Smart Object and conversion rules

Smart conversion preserves one root curve on the curve parent and clears the inner curve. For
multi-child conversion, child curves are retained. Unpacking a parent track requires a group
wrapper and preserves enabled or disabled state.

Video-to-Frame Animation samples every frame from the actual document, after owned rendering, and
stores Position states with a 64 MiB estimated snapshot budget. Empty or clipped positional
geometry, including affected wrapper descendants, fails atomically when it cannot be represented;
the operation does not silently discard data. Owned curves are cleared only after a successful bake.
Frame-to-Video keeps property snapshots and clears inactive owned Position tracks. Destructive
timeline creation in either mode, `makeFramesFromLayers`, and deletion freeze the current displayed
pose and clear tracks.

## Current development status

The source is frozen, implemented, root-reviewed, compiled, packaged, installed, and runtime-tested
for this local delivery. Version is 0.16.0 development at `/Applications/PhotoStore-dev-position-animation.app`,
bundle `ai.photostore.app.dev.position-animation`, baked source
`a4c52a3f2ec39e122ab5d145b6dd8b6b398b4316`, and binary SHA-256
`6e9238d8d04954b4a1b611ebae16f711ceae74c47d719aa2df223cd218112b1d`. The final compiler/package
passed in 216.29 seconds; log: `/private/tmp/photostore-position-animation-package-hierarchy-final-20261011.log`.
The runtime receipt recorded 69 requests, 69 responses, and 0 failures from
`/private/tmp/photostore-position-runtime-20261011-evidence/runtime.jsonl`; launch was
hidden/inactive/non-frontmost after helper-confirmed finished launching, and cleanup closed the
listener. Seek/reopen preserved painted 8×8 bounds at [12,12], [20,16], and [28,20]; disable froze
[20,16] with zero keys; Video↔Frame conversion preserved geometry and cleared owned Position;
recording was off. The Smart hierarchy source fix compiled but was not runtime-tested. Development
branch checkpoint: `codex/position-stopwatch`. No screenshot/UI-pixel, Smart visual, external PSD,
or full-affine acceptance is claimed. Native PSD temporal acceptance remains outside this pass.

The public workflow reference is [Adobe's overview of animating layer properties](https://helpx.adobe.com/uk/photoshop/desktop/add-video-and-animation/use-keyframes/overview-of-animating-layer-properties.html).
