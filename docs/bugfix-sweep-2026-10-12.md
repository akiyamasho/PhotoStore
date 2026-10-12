# PhotoStore open issue sweep — 2026-10-12

This document records the complete open-issue inventory fetched from
`akiyamasho/PhotoStore` on 2026-10-12. Pull requests were excluded. The snapshot is
`/tmp/photostore-sweep-inventory-20261012.jsonl` and contains five open issues, all with zero
comments.

## Inventory and disposition

| Issue | Report | Disposition | Status at this checkpoint |
| --- | --- | --- | --- |
| [#7](https://github.com/akiyamasho/PhotoStore/issues/7) | Give Fill, Pen, and Lasso distinct recognizable toolbar icons | Dedicated original icons are in source work. | Implementation in progress; intended to close after the landed commit reaches `main`. |
| [#8](https://github.com/akiyamasho/PhotoStore/issues/8) | Reset canvas rotation with Esc | Escape resets the active canvas view after ordinary cancel, modal, and text-edit precedence. | Implementation in progress; intended to close after the landed commit reaches `main`. |
| [#9](https://github.com/akiyamasho/PhotoStore/issues/9) | Clone Stamp source marker and preview after stroke release | Selected/candidate source marker and transformed brush-region outline are implemented while hovering or selecting with Alt; destination overlay is hidden during Alt and engine mapping is canonical. | Source implementation complete; intended to close after the landed commit reaches `main`. The supplied reproduction has Aligned enabled, whose fixed source/destination offset intentionally makes the source region follow the pointer after release. Normal post-release slot math was equivalent before and after resolver cleanup; no stale-source fix is claimed. |
| [#10](https://github.com/akiyamasho/PhotoStore/issues/10) | Enable timelapse by default when memory overhead is low | Deferred. Recording stays off by default; no capture, measurement, benchmark, or enablement is added. | Open pending an explicit memory budget and representative measurement if the default should change. |
| [#11](https://github.com/akiyamasho/PhotoStore/issues/11) | Persist Auto-select Layer on/off across restarts | `tools.moveAutoSelect` is serialized and restores the last value; the default is `true`. | Implementation in progress; intended to close after the landed commit reaches `main`. |

The source status is separate from verification and delivery. At this checkpoint the required
compiler pass is pending and the `main` push is pending. No issue is closed, no version bump or
release is created, and no reinstall or screenshot is implied. A later delivery update may revise
this file's status lines after the compiler and push complete.

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
5. For timelapse, create a new document without opting in and confirm recording remains off. Do not
   enable it automatically or infer a safe memory threshold from the pixel-size calculations alone.
