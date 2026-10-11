# Smart Contents editing and close safety

PhotoStore's 0.11.0 source pass makes Smart Object contents editing and close decisions fallible
and explicit. `Session::try_close` and `try_close_with_discard` preserve failed tabs and links when
compatibility callers do not request discard; command close errors remain visible. A dirty child
commits atomically to its parent, and commit errors propagate before link, tab, or job cleanup. A
dirty open child blocks its parent. Batch close processes children before parents, caps discard IDs at
4096, and strictly validates integer, duplicate, affected-live-document IDs. Partial batch success
can remain when a later document fails.

The UI defers a Don't Save decision with document IDs and revisions. Cancel preserves edits and
links; a document changed after the choice is re-prompted; newly dirty parents are reconciled after
child Save. Save/Don't Save/Cancel intent is retained across contents navigation. The Contents
context bar queues Save and Return after the canvas frame, while Info is on-demand and read-only,
showing source kind and shared-instance count. Missing origin layers fall back through the engine's
contents-ID lookup, retaining the parent name when available. New Info/Return registry entries use
no shortcut by default and remain editable.

Contents Save validates the live parent and instances before a dirty save and uses the existing
atomic contents update. Linked-child Save Contents embeds native contents into the parent; it does
not write an external linked file or claim full Photoshop linked-file fidelity. Source changes or
remapping can still produce independent children, some legacy Option-close callers do not surface
errors, and broader UI/PSD parity remains unverified.

Public references: [Adobe Smart Objects overview](https://helpx.adobe.com/photoshop/desktop/create-manage-layers/smart-objects/smart-objects-overview-and-benefits.html)
and [updating linked Smart Objects](https://helpx.adobe.com/sg/photoshop/desktop/create-manage-layers/smart-objects/update-linked-smart-objects.html).

The required native app/CLI HEIF compiler check passed in 12.13 seconds; final log:
`/private/tmp/photostore-smart-contents-check-20261011-final.log`. Null and `{}` no-argument
Smart commands retain compatibility behavior, and clean Save Contents is a no-op when the document
is already clean. The source uses `document` as a `DocId` for Smart commands; `file.close` keeps
`document` as a document index.
