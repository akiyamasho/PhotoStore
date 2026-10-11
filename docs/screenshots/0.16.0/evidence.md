# PhotoStore 0.16.0 video-opacity delivery evidence

This is a local development delivery receipt for the independent video-opacity pass. It records the
compiler, package, install, and focused native runtime facts. Visual interaction acceptance remains
open, and the development branch checkpoint is prepared on `codex/video-opacity`.

## Source and compiler

- Baked source: `d66ddbcbed3989f66cb568b1f6d8f7f30fc9d464`.
- Combined compiler: passed in 45.07 seconds with exit 0.
- Compiler log: `/private/tmp/photostore-video-opacity-check-20261011.log`.
- Compiler output: 7 engine warnings and 1 existing UI warning, with no errors.

## Package and install

- Development package: completed in 6m13s.
- Package log: `/private/tmp/photostore-video-opacity-rc-build-20261011.log`.
- Installed app: `/Applications/PhotoStore-dev-video-opacity.app`.
- Bundle ID: `ai.photostore.app.dev.video-opacity`.
- Executable SHA-256: `597d2e4da6e32cbaf12f0bcfb6db592ac70dc5dfbcdd9670f82250d0bcad89c6`.
- Installer verification, as reported by root: the same hash, signature, ARM64 architecture, absent
  document association, absent goal process, absent default configuration, and no other applications
  touched. The installed hash and bundle identity were independently read by root. No standalone
  installer log was retained.

## Focused runtime and screenshots

The focused native runtime receipt passed in the fresh, distinct working directory
`/private/tmp/photostore-video-opacity-runtime-20261011-evidence`. The blank native document was
64×64 with a three-frame, 3 fps video timeline and recording disabled. Legacy opacity was frame 0 =
1.0 and frame 2 = 0.25; first enable preserved frame 1 = 0.625. Setting layer properties at frame 2
to opacity 0.5 succeeded; an XY-only update to x=20, y=6 omitted alpha and preserved legacy 0.25;
an explicit `timeline.addKeyframe` opacity 0.9 was rejected with reason `OWNED OPACITY`. The final
sample was frame 1 = 0.75. The saved `ANIMATED.pcraft` reopened with frames 0/1/2 = 1.0/0.75/0.5.
Disabling at frame 1 froze static opacity at 0.75 with zero keys; the separately saved
`DISABLED.pcraft` records that state. Both native artifacts were read by root and use
`photostore-format 0.16.0`. The final flow retained 42 RPC responses, including one expected rejection
(`id18`, `timeline.addKeyframe` with owned opacity) and 41 successful responses, and ended with a clean
quit and closed listener.

The fresh hidden launch reported PID 61599 and enforced finished launching, hidden, inactive, and
non-frontmost state. No dedicated fresh launch log was retained. Existing launch/listener logs from
the stale first attempt are not used as successful provenance. A later empty-row `STABLE_CHECKS` reset
in the launcher helper was source-reviewed after this runtime and was not rerun; it is a helper-script
correction and does not change the baked app.

No new UI screenshot is recorded. The hidden 0.15.0 capture timed out and produced no image; visual
GUI acceptance and broader dense or duplicate/future migration acceptance remain unverified. The
historical 0.15.0 receipt is available at [opacity stopwatch evidence](../0.15.0/evidence.md).

No image or UI interaction/canvas-pixel evidence was captured. The blank-video flow and hidden
screenshot limitation leave visual acceptance, dense or duplicate/future migration acceptance, and
production release acceptance unverified. No release, publication, promotion, or production
installation is claimed.
