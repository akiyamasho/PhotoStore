# Smart Filter Masks milestone

PhotoStore's next 0.8.0 pass adds Smart Filter mask targeting and controls. The public behavior reference is Adobe's [Smart Filter mask documentation](https://helpx.adobe.com/photoshop/desktop/effects-filters/smart-filters/mask-smart-filter-effects.html).

The source implementation supports mask targeting and filtering, grayscale mask view, density and feather controls, invert, automatic white-mask creation, and corresponding UI controls. It preserves the distinction between the filter target and the mask target. Exact Photoshop UI parity, mask copy/move, channel selection, unknown PSD filter behavior, and full acceptance remain open.

The related New Document fix accepts the recording field through strict parameter validation and keeps the form with an inline error when Create fails. Movie reload preserves interpretation and playback settings, while canonical audio reprobe and valid split envelopes are retained. The 0.8.0 native app/CLI HEIF compiler check passed; exact UI parity and broad visual acceptance remain unverified.
