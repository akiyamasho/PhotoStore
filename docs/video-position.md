# Video Position development

This development delivery note records the 0.16.0 pass for independent Position ownership on
video layers. The candidate is `PhotoStore-dev-video-position`, bundle
`ai.photostore.app.dev.video-position`, on branch `codex/video-position`, based on
`2b93f6dd260291f9f49c8a5efda0698d3eb6cef8`.

The implementation persists a video clip's authored `base_translation` and last rendered
`applied_translation`. Frame snapshots carry `video_position`, separate from legacy content-bounds
position. Video Position and Opacity are independent owned tracks. Whole-pixel Linear and Hold
keys, navigation, and deletion are available; stopwatches are off by default. Animation tracks are
bounded by 100,000 keys, 256 tracks, frame 999,999, and coordinates ±1,000,000.

First enable performs bounded exact integer-frame migration of legacy video placement using Hold
keys, preserving visible samples and retaining raw XY for alpha-only editing. Dense snapshots must
contain canonical `video_position`; ambiguous legacy bbox snapshots are rejected. Unsupported owned
Position ancestors for video, malformed coordinates, excessive tracks or keys, and unrepresentable
data fail atomically. Source painting stores edits in raw frames without baking timeline motion.

Seek, save, reopen, and Video↔Frame conversion are intended to preserve the shown clip placement.
Destructive timeline operations freeze the shown pose into the base translation, clear ownership,
and preserve alpha. Full animated parents, Smart affine transforms, external PSD temporal fidelity,
and broader Photoshop parity remain future work. Source authoring, compiler, package, install, and
bounded native checks are complete for this candidate. The installed app is
`/Applications/PhotoStore-dev-video-position.app`; the staged package is
`/private/tmp/photostore-video-position-rc-0.16.0-2c3fd1099/PhotoStore-dev-video-position.app`.
The baked source SHA is `2c3fd1099d516587b5893d08f2734832cbcd8751`; the installed executable SHA-256
is `514f390dba78eb664a8ebd904db076c820eba1c15e5bd6b727f9c73ab3c7f632`. The final compiler/package
passed with exit 0; the Cargo build portion took 3m00s. Log
`/private/tmp/photostore-video-position-package-final-20261011.log`.
Installer evidence is `/private/tmp/photostore-video-position-install-20261011.log`.

The corrected native flow recorded 107 requests, 107 responses, 106 successes, one expected legacy
XY ownership rejection, and zero unexpected failures. It created three source frames inside the app:
frame 0 bounds [12,12,8,8], frame 1 bounds [16,12,8,8], and an empty frame 2. Owned Linear Position
was sampled at [10,6]; painted output reached [26,18,12,12]; opacity samples were 1.0, 0.4, and
0.5. Save/reopen, Video↔Frame conversion, empty-frame preservation, alpha preservation, and disable
freeze at [10,6] with zero keys passed. Recording remained off. The runtime receipt is
`/private/tmp/photostore-video-position-runtime-20261011-evidence/runtime-native-frames.jsonl`;
cleanup confirmed PID 81143 absent and listener 17663 closed. Artifact hashes are recorded by the
runtime generator: `ANIMATED.pcraft` `d092abc0323702a8548e4ae158a0fc03319941e7e6f8da5637fc23428f9d50e6`,
`CONVERTED.pcraft` `76db7cee07da519eaee5bdf7cca5b6ba56c7057288454ea3f6cf0e358fc7d1cb`, and
`DISABLED.pcraft` `83200711cd992ec381cf6dbe33e416638a384b08069e7fa588992158238dc932`.

No screenshot/UI-pixel, Smart visual, animated-parent, PSD, full-affine, release, or full-product
parity claim is made. Production, consolidated, and other goal applications remain untouched.

An earlier video-import attempt is retained as a separate ambient filesystem-capability restriction
receipt before the Position checks: it recorded 9 requests and clean closure. The corrected flow
created its three source frames inside PhotoStore; unused generated PNGs are not evidence.

Adobe's public [overview of animating layer properties](https://helpx.adobe.com/uk/photoshop/desktop/add-video-and-animation/use-keyframes/overview-of-animating-layer-properties.html)
describes keyframes as time-based changes to layer properties and distinguishes animated property
tracks from the layer's underlying content. It is a workflow reference for this bounded Position
pass, not evidence of PhotoStore parity.
