# Brush curve smoothing

When brush smoothing is enabled, PhotoStore first applies the selected exponential or pulled-string
smoothing, then reconstructs a centripetal cubic path between the resulting samples before placing
dabs. This removes the visible straight joins that can remain when sparse mouse or pen samples are
only jitter-filtered. Pressure, tilt, rotation, wheel, and time are interpolated between samples;
stroke endpoints are retained.

Curve flattening targets 0.25 screen pixels of deviation and is capped at 256 chords for each
sample interval. The cap bounds work for malformed or unusually large coordinates; it can leave a
small amount of visible faceting on extreme paths. The live preview uses the same incremental
generator and finish-tail calculation as the committed stroke. A one-sample lookahead is used for
interior tangents, so the newest portion is provisional until more input arrives or the preview
draws the finish tail.

The curve stage is disabled when smoothing is off and for aliased Pencil strokes, which retain
their pixel-aligned path. This change does not add hardware acceleration, proxy rendering, tablet
driver coalescing, or parity claims with other painting applications. It does not increase the
existing smoothing amount or its lag.
