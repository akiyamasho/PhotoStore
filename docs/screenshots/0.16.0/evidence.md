# PhotoStore 0.16.0 screenshot evidence

Status: provisional-preview captures are present and root-reviewed for legibility and demo content. These images document the isolated `0.16.0dev` preview, not a final production build or a published release. Both demo documents were saved disposable fixtures; recording was disabled, a fresh configuration had zero documents before capture, and the owned preview process was inactive and non-frontmost before and after each screenshot. The screenshots contain no private artwork.

The native CUA screenshot API returned JPEG bytes. The committed lossless WebP files are pixel-identical to the decoded JPEG captures; they are not described as lossless camera captures. The preview source was `8b07e3bfd067f27c396e5475646d33b0c58e2e59`, built as `/private/tmp/photostore-production-preview-0.16.0-20261011/PhotoStore-dev-release-0-16.app`, bundle `ai.photostore.app.dev.release-0-16`, binary SHA-256 `75e54f59bf87f1fc73fbba40055af2e97b65e80e7a70b9d930e2084b3e1b200b`.

## raster-position-opacity.webp

- Feature: ordinary raster-layer Position and Opacity animation controls, with typed XY values and visible track/key controls.
- Demo: saved disposable `release-evidence/raster-motion.pcraft`; document 1, layer 3 (`Raster Motion`), displayed frame 2 of 3 (zero-based index 1).
- Visible values: opacity 72.5%, Position (115, 85), Linear interpolation, two keys.
- Capture: 1440×900, 158,848 bytes; SHA-256 `20d18a9d2f5c0faa9597c531e66734f0e66855f21a5fa923f7a46bd72c25d86a`.
- Known limits: preview UI evidence for this raster track only; it does not establish broad interaction acceptance or external-format parity.

## video-position-opacity.webp

- Feature: independent video Position and Opacity tracks; Linear/Hold selectors, legacy-keyframe ownership text, and visible source content.
- Demo: saved disposable `release-evidence/video-motion.pcraft`; document 4, layer 6; displayed frame 2 of 3 (zero-based capture index 1). Source frames 0 and 1 are painted; frame 2 is empty.
- Visible values: opacity 75%, Position (6, 4), three migrated Position keys with Hold active, and Linear Opacity interpolation.
- Capture: 2880×1800, 338,704 bytes; SHA-256 `8b4bde0632205a136c774577a8c2e48b3a64c53c898a2c7568550778f9a918c0`.
- Known limits: the screenshot documents this candidate UI state and visible track ownership only. It does not establish full timeline, Smart Object, PSD, audio, or Photoshop parity.

The preview bundle was PID 51523 and remained inactive/non-frontmost before and after capture; the app was quit after the captures. Recording was checked off for the fresh configuration and the demo documents. No production package, publication, or production installation is evidenced here.

Historical captures keep their original source and capture facts: [0.14.0 close-guard and timeline evidence](../0.14.0/evidence.md), and [0.13.1 toolbar/shortcut-autosave evidence](../0.13.1/evidence.md). They are not represented as 0.16.0 captures.

## Historical video-opacity development receipt

This is a local development delivery receipt for the independent video-opacity pass. It records the
compiler, package, install, and focused native runtime facts. Visual interaction acceptance remains
open, and the development branch checkpoint is prepared on `codex/video-opacity`.

### Source and compiler

- Baked source: `d66ddbcbed3989f66cb568b1f6d8f7f30fc9d464`.
- Combined compiler: passed in 45.07 seconds with exit 0.
- Compiler log: `/private/tmp/photostore-video-opacity-check-20261011.log`.
- Compiler output: 7 engine warnings and 1 existing UI warning, with no errors.

### Package and install

- Development package: completed in 6m13s.
- Package log: `/private/tmp/photostore-video-opacity-rc-build-20261011.log`.
- Installed app: `/Applications/PhotoStore-dev-video-opacity.app`.
- Bundle ID: `ai.photostore.app.dev.video-opacity`.
- Executable SHA-256: `597d2e4da6e32cbaf12f0bcfb6db592ac70dc5dfbcdd9670f82250d0bcad89c6`.
- Installer verification, as reported by root: the same hash, signature, ARM64 architecture, absent
  document association, absent goal process, absent default configuration, and no other applications
  touched. The installed hash and bundle identity were independently read by root. No standalone
  installer log was retained.

### Focused runtime and screenshots

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
