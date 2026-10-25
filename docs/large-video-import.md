# Large video import

PhotoStore has two different movie workflows.

## Linked movie footage

Use **File > Open** to open a movie as a document, or **Layer > Video Layers > New Video Layer from File** to add a movie to an existing document. These paths probe the original file and decode only its first frame during import. The document stores the canonical source path, source frame count and frame rate, selected source trim, and a resident preview. Native format version 2 preserves that metadata. Later frames are decoded on demand into a bounded in-memory cache; exports and other full-quality engine work decode on their worker thread. The movie remains linked to its original local file, so it must remain available for later frames and export.

The timeline duration uses the logical source length, not the number of resident frames. Linked movie pixels are read-only in this implementation. Raster paint, filters, and frame-stack insert, duplicate, delete, or restore operations are rejected with an instruction to rasterize or convert first. Timeline retiming, position, and opacity remain document metadata; convert the footage to a Smart Object for other nondestructive transforms. If the source is offline or a requested frame cannot be decoded, PhotoStore keeps the last valid canvas appearance where possible; export reports the source error rather than exporting that fallback frame.

UI frame requests are asynchronous and coalesced through a bounded 160 MiB cache. Timeline thumbnails use only cached movie frames and do not synchronously decode on the UI thread. Preview may temporarily show the last valid frame while the requested one is loading. A single resident frame above 160 MiB (estimated from allocated 256-pixel tiles) is rejected at import. This path does not implement proxy controls, hardware acceleration, or a promise of performance parity with dedicated video editors.

## Frames to Layers

**Frames to Layers** creates an editable raster layer for each decoded frame, so it intentionally materializes the selected range in memory. It retains a 512 MiB decoded-frame limit. For a large movie, use one of the linked workflows above instead of increasing this eager limit.

Existing image-sequence imports and blank video layers continue to use resident frames and their existing bounds.
