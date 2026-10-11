# Video Position development

This source-stage note records the 0.16.0 development pass for independent Position ownership on
video layers. The candidate is `PhotoStore-dev-video-position`, bundle
`ai.photostore.app.dev.video-position`, on branch `codex/video-position`, based on
`2b93f6dd260291f9f49c8a5efda0698d3eb6cef8`.

The implementation persists a video clip's authored `base_translation` and last rendered
`applied_translation`. Frame snapshots carry `video_position`, separate from legacy content-bounds
position. Video Position and Opacity are independent owned tracks. Whole-pixel Linear and Hold
keys, navigation, deletion, and stopwatch defaults are bounded by 100,000 keys, 256 tracks,
frame 999,999, and coordinates ±1,000,000.

First enable performs bounded exact integer-frame migration of legacy video placement using Hold
keys, preserving visible samples and retaining raw XY for alpha-only editing. Dense snapshots must
contain canonical `video_position`; ambiguous legacy bbox snapshots are rejected. Unsupported owned
Position ancestors for video, malformed coordinates, excessive tracks or keys, and unrepresentable
data fail atomically. Source painting stores edits in raw frames without baking timeline motion.

Seek, save, reopen, and Video↔Frame conversion are intended to preserve the shown clip placement.
Destructive timeline operations freeze the shown pose into the base translation, clear ownership,
and preserve alpha. Full animated parents, Smart affine transforms, external PSD temporal fidelity,
and broader Photoshop parity remain future work. No screenshot, UI-pixel, Smart visual, compiler,
package, install, native-runtime, or private-push evidence is claimed at this source stage; the
full product backfill remains unfinished.

Adobe's public [overview of animating layer properties](https://helpx.adobe.com/uk/photoshop/desktop/add-video-and-animation/use-keyframes/overview-of-animating-layer-properties.html)
describes keyframes as time-based changes to layer properties and distinguishes animated property
tracks from the layer's underlying content. It is a workflow reference for this bounded Position
pass, not evidence of PhotoStore parity.
