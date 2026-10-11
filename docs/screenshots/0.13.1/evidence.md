# PhotoStore 0.13.1 screenshot evidence

Status: verified preview capture; publication follows the exact tagged source and final package
checks. The screenshot uses a disposable blank PhotoStore document and contains no private or
unsaved artwork.

## toolbar-shortcut-autosave.png

- Feature/fix: identifiable toolbar tool icons and the native shortcut-file autosave field used by
  the consolidated `PhotoStore-dev` workflow.
- Demo document: blank 8×8 RGB document created for the capture; no artwork or private files.
- Preview source: `34c70481c3a1602bc80931d302ce8e5def2c5f6e`.
- Capture result: 2880×1800 RGBA PNG, SHA-256
  `093995a69d20bfa93e3a00b018d8e6774bd47383f30aad58ddf77a6fee54039e`.
- Runtime evidence: the dev shortcut export path was exercised at
  `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json`. The continuous watcher
  also deferred an 8×8 demo reload while its tab was open, then reloaded after the tab closed.
- Known limits: the image shows the PhotoStore product header and relevant controls, but the OS
  dev title, full build SHA, and full path tail are clipped. It documents the toolbar/autosave UI,
  not the later published-release event or the local CI production promotion.

Statuses remain distinct: implemented in source, preview in this capture, verified after exact-tag
package checks, and published only after the GitHub release is created.
