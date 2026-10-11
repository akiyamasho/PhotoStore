# Animation playback and onion skins

PhotoStore's released 0.10.0 milestone makes timeline repeat and onion-skin settings explicit in native
documents. It keeps the existing `TimelineLoop` model and serde defaults: `Once`, `Loop`, and
`PingPong`, with `loop_count` meaning finite total plays (`Some(1..=65_536)`) or `None` for
forever. Strict commands are document-ID guarded; finite loops end on the last frame, while finite
PingPong ends on the first frame. Animated Smart Object children honor finite counts while retaining
legacy unbounded palindrome timing. GIF export encodes total plays as restart count `N - 1`; Once
omits the loop extension and the existing wrapper is preserved.

The native UI exposes Once, 3 Times, Forever, Other, and a PingPong extension. Invalid settings
keep their forms open; opacity values are normalized as percentages. Empty menu, palette, and
assigned-shortcut entries open the native editors, while existing default bindings remain unchanged.

Onion-skin settings persist in the native document. Settings support 0–8 frames before and after,
spacing 1–1000, minimum and maximum opacity, and Normal, Multiply, Screen, or Difference blending.
Preview pixels are bounded to 16 ghosts, a 1024-pixel long edge, two working float buffers, and one
texture; they are not written into exports or action-recording captures. Cloned snapshots stay synchronized; the CPU
composite replaces the ordinary canvas once for alpha-correct display color management. Endpoints
do not wrap. Onion previews are suspended during live and temporary previews and are not included in
documents, exports, or action-recording captures.

Known limits: the 1024-pixel preview can appear soft with onion skins enabled; up to 16 synchronous
renders may add unmeasured latency; only four blend modes are implemented; native PingPong endpoint
timing and Smart Object/GIF palindrome behavior differ; and exact Photoshop UI/PSD acceptance is
unverified. General property tracks, stopwatches, Smart transform/style tracks, clip transitions and
groups, and exact audio synchronization remain future work.

Public references: [Adobe loop choices](https://helpx.adobe.com/ca/photoshop/desktop/add-video-and-animation/create-animation-frames/specify-looping-in-frame-animations.html),
[Adobe's CS6 reference](https://helpx.adobe.com/pdf/cs6/photoshop_reference.pdf) for onion controls,
and the [Go GIF package documentation](https://pkg.go.dev/image/gif) for positive loop-count
semantics.
