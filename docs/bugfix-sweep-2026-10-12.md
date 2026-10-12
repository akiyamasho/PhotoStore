# PhotoStore open issue sweep — 2026-10-12

This document records the complete open-issue inventory fetched from
`akiyamasho/PhotoStore` on 2026-10-12. Pull requests were excluded. The snapshot is
`/tmp/photostore-sweep-inventory-20261012.jsonl` and contains five open issues, all with zero
comments.

## Inventory and disposition

| Issue | Report | Disposition | Status at this checkpoint |
| --- | --- | --- | --- |
| [#7](https://github.com/akiyamasho/PhotoStore/issues/7) | Give Fill, Pen, and Lasso distinct recognizable toolbar icons | Dedicated original icons are implemented, with tint and DPI-aware rasterization. | Source delivered on main; issue closed on GitHub. |
| [#8](https://github.com/akiyamasho/PhotoStore/issues/8) | Reset canvas rotation with Esc | Normal idle canvas Escape routes through the existing UI reset while preserving cancel, text-edit, and modal priority; zoom and center are unchanged. | Source delivered on main; issue closed on GitHub. |
| [#9](https://github.com/akiyamasho/PhotoStore/issues/9) | Clone Stamp source marker and preview after stroke release | Current hover/candidate sampled-region feedback is implemented with a canonical mapping and first-stroke source marker; destination overlay is hidden during Alt. | Source delivered on main; issue closed on GitHub. The user-confirmed reproduction has Aligned enabled, whose fixed source/destination offset intentionally makes the source region follow the pointer after release. Normal post-release slot math was equivalent before and after resolver cleanup; no changed Aligned behavior is claimed. |
| [#10](https://github.com/akiyamasho/PhotoStore/issues/10) | Enable timelapse recording by default when memory overhead is low | Closed after the accepted fallback review. Recording stays off by default; no capture, measurement, benchmark, or enablement is added. | Closed on GitHub with the memory assessment and accepted recording-off fallback. |
| [#11](https://github.com/akiyamasho/PhotoStore/issues/11) | Persist Auto-select Layer on/off across restarts | `tools.moveAutoSelect` is serialized with default `true`, persists across both theme bars and Move tool presets, and loads the saved value. | Source delivered on main; issue closed on GitHub. |

The source status is separate from verification and delivery. The integrated source commit
`d4cd1c035d1e43a74ef4b594056e7cc71199b78e` passed the required compiler command
`cargo check --locked --offline -p photostore --features dev-install` with the shared target,
single Cargo job, Rayon one, and `nice -n 10`; the final 0.16.2 app-plus-CLI release build then
passed in 12m12s from exact source/tag SHA `a7a2dd6c306d2f4ce33bd3a6fe87044cac03d520`. Stable
release v0.16.2 is published on GitHub and installed locally. Manual runtime acceptance was not
run; no screenshot or UI-verified claim is made. The prior 0.16.1 screenshots remain historical
and are not new 0.16.2 evidence.

## Manual acceptance instructions

These are short, sequential checks for the affected behavior. They are instructions for a later
manual pass; no checks were run while preparing this document.

1. Open a disposable document and inspect the toolbar. Confirm Fill, Pen, and Lasso each has its own
   recognizable original icon and that selecting each tool still selects the intended tool.
2. Rotate the canvas, then press Escape with no other cancelable interaction active. Confirm the
   active canvas view returns to its neutral rotation. Repeat while an ordinary preview, modal, or
   text-edit interaction is active and confirm that interaction's normal cancel/precedence remains
   intact.
3. In Clone Stamp, set a source with Alt and hover or move the brush. Confirm the selected/candidate
   source outline is transformed once from the canonical first sample and that the destination
   overlay is hidden while Alt is held. With Aligned (位置合わせあり) checked, confirm the fixed
   source/destination offset makes the sample follow the pointer after release. With Aligned
   unchecked, confirm the sample restarts at the chosen origin. No runtime verification was made
   during this documentation pass.
4. Change Auto-select Layer, quit PhotoStore normally, reopen it, and confirm the chosen on/off
   value is restored. On a fresh preference state, confirm the default is on.
5. For timelapse, create a new document without opting in and confirm recording remains off. This accepted fallback closes the conditional request; do not enable it automatically or infer a safe memory threshold from the pixel-size calculations alone.
