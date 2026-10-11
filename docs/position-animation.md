# Position animation development

This document describes the current PhotoStore development version, kept at 0.16.0, for independent
layer Position animation. It is a development design and implementation record; compiler,
package, source-commit, install, runtime, and visual evidence are still pending.

## Product boundary

The candidate identity is `PhotoStore-dev-position-animation`, with bundle ID
`ai.photostore.app.dev.position-animation`. Its executable, settings, recovery process, and
configuration are separate from production and from the other goal applications. It registers no
file associations. Production and consolidated goal applications remain untouched. Version remains
0.16.0 for this development pass; no whole-workspace version rebuild is part of this pass.

The source commit SHA is pending the root delivery record and must be filled from the actual source
commit. The base planning reference is exactly `fd414d2ed15c27ff323b19fcf6ae84351b3a4557`; it is
not the candidate source SHA.

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
`timeline.setPosition`. Move, nudge, align, distribute, linked, group, and descendant operations
key at the playhead by applying the actual movement. Generic bounding-box painting does not create
keys. First enable migrates legacy dense Position snapshots to bounded Hold keys and reconciles the
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

The current bounded pass has three milestones: engine (2 steps), UI (2 steps), and delivery (4
steps). The source is frozen, implemented, and root-reviewed; engine and UI are complete, while
delivery remains pending. Its four delivery steps are the source checkpoint, mandatory
`package-dev.sh` compile/package gate, own install, and the focused background test plus receipt and
private branch push. Standalone checks did not compile the candidate: the earlier check ended after
405.76 seconds on a foreign lock, and the final queued check was stopped with SIGTERM exit 143
without diagnostics. The required package-driven compile has not passed. Package, install, runtime,
and UI visual evidence remain pending; no screenshot or release is claimed. The inspector exposes
numeric `document.id` and `session.documents[].id`; document selectors use the documented index
versus timeline `actualDocId` distinction in `control-protocol`. Native PSD temporal and
full-source-transform acceptance is not part of this development pass.

The public workflow reference is [Adobe's overview of animating layer properties](https://helpx.adobe.com/uk/photoshop/desktop/add-video-and-animation/use-keyframes/overview-of-animating-layer-properties.html).
