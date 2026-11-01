# PhotoStore

PhotoStore is a Rust-native image editor maintained as a personal fork of PhotoCraft. It is an independent project; no upstream sync relationship is configured.

![Rust 1.95+](https://img.shields.io/badge/Rust-1.95%2B-b7410e?style=flat-square&logo=rust)
![Platforms](https://img.shields.io/badge/platforms-macOS%20%7C%20Windows%20%7C%20Linux-2f7bf5?style=flat-square)
![License](https://img.shields.io/badge/license-MIT%20%2F%20Apache--2.0-3a3a3a?style=flat-square)

## Install

Download the macOS Apple Silicon package from [PhotoStore releases](https://github.com/akiyamasho/PhotoStore/releases). For a local install, use Rust stable 1.95+:

The 0.20.4 Apple Silicon macOS app aligns timeline rows with Layers, preserves authored video
opacity during overlapping playback and adds layer-owned audio. Stable-release checks are
available under **Help → Check for Updates…** using existing GitHub CLI (`gh`) authentication.
The app and CLI use the persistent local PhotoStore certificate, not Developer ID and not
notarized. See [in-app updates](docs/app-updates.md) and the [0.20.4 release record](docs/releases/0.20.4.md).

```sh
CARGO_BUILD_JOBS=1 cargo install --path apps/photostore --bin photostore --locked
CARGO_BUILD_JOBS=1 cargo install --path apps/photostore-cli --bin photostore-cli --locked
```

## Build and run

```sh
CARGO_BUILD_JOBS=1 cargo build --release -p photostore -p photostore-cli
./target/release/photostore
./target/release/photostore-cli --help
```

## Current release source

PhotoStore 0.20.4 makes timeline rows follow the Layers panel stack and preserves static video
opacity across playback snapshots. Movie audio belongs to its source layer; standalone audio can
be imported as a lightweight document layer. Bounded decoder-session reuse targets overlapping
4K source workloads without decoding whole movies into memory.
[Download 0.20.4](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.20.4).
See the [release/delivery record](docs/releases/0.20.4.md) and
[layered-media architecture and acceptance](docs/plans/layered-media-2026-11-01.md).
Native multi-source GPU/display/audio acceptance and CapCut performance parity remain unverified.
The prior [0.20.3 layer/input workflows](docs/releases/0.20.3.md) remain available.

The [0.20.2 performance/cadence release](docs/releases/0.20.2.md) retains persistent decoding,
shared COW frames/banks, complete fixture cadence and adaptive derived media. Fixed 64 MiB timeline
authoring thresholds use configured Memory Usage with background cancellation and atomic undo.
Core preparation benchmarks are separate from native displayed FPS. Native v3 archives retain
editable layers, Smart Objects and timeline data; older readers may ignore newer optional playback
intent. HDR conversion and full Photoshop/CapCut parity remain unsupported/unverified; distinct
codec, recording-archive, Smart-transform and configurable load quotas remain documented.

The previous [0.20.0 release](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.20.0) added
source identity/relinking, rational project rates, optional proxies, sample-clocked native audio,
affine Smart animation and original-source exports.
The prior [0.19.0 release](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.19.0) includes
rename text clipboard support, 1 px brush decrease, drawing, cursor and PSD timelapse fixes.

The previous [0.18.3 release](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.18.3) added the
selection-scoped Generative Fill context bar, retained in this release.

## Generative Fill

Select an area, choose **Edit → Generative Fill…**, enter a prompt, and choose **Generate**.
The full image provides context. Results arrive on an undoable layer, clipped to the selection
and matched to surrounding colors and lighting.

![Simple Generative Fill prompt](docs/screenshots/0.17.1/generative-fill-dialog.png)

Configure Local Codex or a custom provider in **Preferences → Generative Fill**. If local setup
is incomplete, **Open Settings** takes you directly there and keeps your prompt.

![Generative Fill settings](docs/screenshots/0.17.1/generative-fill-settings.png)

![Context-matched Codex sailboat](docs/screenshots/0.17.1/generative-fill-result.png)

See [setup and adapter contract](docs/generative-fill.md) and [capture evidence](docs/screenshots/0.17.1/evidence.md).
Initial scope: sRGB RGB documents up to 25 million pixels, with 8-bit PNG provider interchange.

## Historical preview screenshots

These 0.16.1 preview captures show a Lasso-selected pixel layer resized freely and then resized inward with Shift constraining its current aspect ratio. They remain historical preview evidence, not 0.16.2 release evidence. See the [0.16.1 release notes](docs/releases/0.16.1.md) and [capture evidence](docs/screenshots/0.16.1/evidence.md).

![Lasso selection resized freely](docs/screenshots/0.16.1/lasso-free-resize.png)

![Lasso selection resized inward with Shift constraining its aspect ratio](docs/screenshots/0.16.1/lasso-shift-resize.png)

Earlier 0.16.0 development-preview captures show raster and video Position/Opacity tracks. See the [capture evidence](docs/screenshots/0.16.0/evidence.md) and [v0.16.0 release](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.16.0).

![Raster Position and Opacity tracks](docs/screenshots/0.16.0/raster-position-opacity.webp)

![Video Position and Opacity tracks](docs/screenshots/0.16.0/video-position-opacity.webp)

Historical captures remain linked with their original provenance: [0.14.0 close and timeline evidence](docs/screenshots/0.14.0/evidence.md) and [0.13.1 toolbar/shortcut evidence](docs/screenshots/0.13.1/evidence.md).
