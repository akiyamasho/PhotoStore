# PhotoStore 0.16.1 preview screenshot evidence

**Status:** Preview screenshots captured and legibility-checked. The user confirmed the resize fix works. This is not evidence of a tagged or published release.

## Preview source

- Source lineage: the preview was built from base `b39d2a6a1a46f4c2fbd18eae6e9aeb48b3ad89fd` plus the resize source/version patch that was later committed as `501739c5fdfd022270f130d6877f9b171d937007` (`Fix Lasso transform resizing and Shift aspect constraint`). The preview marker is `preview-0.16.1-lasso-resize`, not a full source commit SHA. The final tagged package must be verified separately against its full tagged source SHA.
- Preview: `/private/tmp/photostore-resize-preview-0.16.1/PhotoStore-Preview.app`, version 0.16.1, ARM64, bundle `ai.photostore.app.preview.resize-0-16-1`, build marker `preview-0.16.1-lasso-resize`.
- Preview executable SHA-256: `750ef5f3565a89a488f490e54c8ade65d7b9d09de4ca6577cb2b571592c02ec6`.
- Screenshot method: PhotoStore's `ui.screenshot` viewport API. Drags were driven by its synthetic `ui.pointer` control method, not an OS-native mouse. Thus the images do not independently verify native pointer delivery through the Lasso-specific canvas input route; the user separately confirmed the fix works.
- Recording was left off. The fixture was created in the isolated preview configuration and was not saved or derived from private artwork.

## Demo fixture and observed geometry

A disposable 480×320 white document contains layer 3, `Lasso Resize Demo`, filled teal inside a seven-point irregular Lasso polygon. Free Transform was active while the Lasso tool remained selected. The initial transform quad was `[[104,72],[335,72],[335,246],[104,246]]` (231×174 document units).

The free-resize drag moved the bottom-right handle to `[385,260]`. The returned quad was `[[104,72],[385,72],[385,260],[104,260]]` (281×188), showing width and height changed independently.

The Shift-constrained drag then moved the bottom-right handle horizontally inward from `[385,260]` to `[365,260]`. The returned quad was `[[104,72],[365,72],[365,246.6192],[104,246.6192]]` (261×174.6192); the aspect ratio remained about 1.4947 while both dimensions shrank. These values came from the preview control API after each synthetic drag.

## lasso-free-resize.png

- Feature: unconstrained Free Transform resize of Lasso-selected pixels.
- Demo: unsaved disposable 480×320 document; teal seven-point irregular Lasso fill on layer 3; Lasso remained the selected tool.
- Source: preview built from base `b39d2a6a1a46f4c2fbd18eae6e9aeb48b3ad89fd` plus the patch later committed as `501739c5fdfd022270f130d6877f9b171d937007`; executable hash and preview identity above. The build marker is not a source SHA.
- Capture: 1440×900 PNG, 98,355 bytes; SHA-256 `e8d5e9a9bda3809e91a6bfcfa3c7d23c323b9651cef66bf087321f33ba213430`.
- Known limits: synthetic pointer input; not independent native OS mouse-routing verification or broad transform acceptance.

## lasso-shift-resize.png

- Feature: Shift-constrained inward Free Transform resize, preserving the current aspect ratio during a one-axis handle drag.
- Demo: same unsaved disposable document and active Lasso tool as the free-resize image.
- Source: preview built from base `b39d2a6a1a46f4c2fbd18eae6e9aeb48b3ad89fd` plus the patch later committed as `501739c5fdfd022270f130d6877f9b171d937007`; executable hash and preview identity above. The build marker is not a source SHA.
- Capture: 1440×900 PNG, 97,929 bytes; SHA-256 `5f2bea43c7324721a611f68a44345b78b90adf91d1376ea2e9467bbe727427fd`.
- Known limits: synthetic pointer input; not independent native OS mouse-routing verification or broad transform acceptance.
