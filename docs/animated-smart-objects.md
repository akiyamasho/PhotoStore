# Animated Smart Objects

PhotoStore keeps the native timeline when selected animated layers are converted to an embedded
Smart Object. Video frame stacks, per-frame delays, keyframes, frame-animation visibility and
layer ranges are serialized in the embedded `.pcraft` source. When one selected root layer is
converted, its animated visibility and opacity move to the replacement Smart Object; the child
root is normalized so those values are not applied twice. A video layer with opacity keyframes
keeps opacity in the child video stack and leaves the replacement Smart Object at neutral opacity.
For a multi-layer selection, selected layer snapshots remain in the child document.

During timeline navigation the parent playhead is converted to seconds, including explicit frame
delays, and the embedded timeline selects its own frame using its FPS, delays and loop policy.
Animated conversions retain the full document canvas in the source so later frame positions cannot
be clipped by the current frame's alpha bounds.
Nested Smart Objects are refreshed with a depth limit of 16. A failed nested decode leaves the
last valid cached appearance in place; commands that explicitly refresh a source return an
actionable error. Timeline synchronization does not create an undo record per frame.

Decoded source documents use a small content-hash LRU, while flattened source images use the
existing bounded pixel LRU with frame-qualified keys for animated sources. Locks are released
before decoding or recursively refreshing a child. Static raster and vector sources retain their
existing fast paths, and linked source files are never rewritten by playback.

`Layer > Smart Objects > Reset Transform` is undoable and preserves source contents, Smart
Filters, masks, identity and the current visual centre. It restores the source's original scale
and orientation by clearing perspective and warp and placing the source unscaled at that centre.
The command does not unwrap the object or attempt full Photoshop transform-history parity.

Known gaps remain audio, transitions, complex transform/tween interpolation, export fidelity for
all external animated formats, and exact Photoshop parity for unusual timeline metadata.
