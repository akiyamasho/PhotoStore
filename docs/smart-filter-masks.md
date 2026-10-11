# Smart Filter Masks milestone

PhotoStore's 0.8.0 pass adds Smart Filter mask targeting and controls. The public behavior reference is Adobe's [Smart Filter mask documentation](https://helpx.adobe.com/photoshop/desktop/effects-filters/smart-filters/mask-smart-filter-effects.html).

The source implementation supports mask targeting and filtering, grayscale mask view, density and feather controls, invert, automatic white-mask creation, and corresponding UI controls. Live previews target the mask; the composite filtered effect refreshes at a committed stroke. This does not claim full live effect recomposition. It preserves the distinction between the filter target and the mask target. Exact Photoshop UI parity, mask copy/move, channel selection, unknown PSD filter behavior, and full acceptance remain open.

The related New Document fix accepts the recording field through strict parameter validation and keeps the form with an inline error when Create fails. Movie reload preserves interpretation and playback settings, while canonical audio reprobe and valid split envelopes are retained. The 0.8.0 native app/CLI HEIF compiler check passed; the final gate is recorded in `/private/tmp/photostore-smart-mask-newdoc-check-20261011-final.log` (exit 0 in 4.27s). This source was packaged and installed in the 0.8.0 prerelease; exact UI parity and broad visual acceptance remain unverified.
