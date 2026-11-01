# Compact audio and scalable preview reference

Built-in GPT Image, generate mode, 2026-11-01. Actual input reference paths: [] (no input images).
Original output: exec-509941ce-531b-4c77-b6c8-7e64340eaf8a.png. Saved asset: workflow.png.
Synthetic design reference, not a screenshot or performance evidence.

## Exact prompt

Use case: ui-mockup. Original synthetic PhotoStore design board for smooth many-source 4K editing and compact integrated audio. Dark gray desktop image/video editor. No input images, characters, private artwork, or logos. Canvas shows translucent blue and orange geometric shapes layered together. Bottom Video Timeline has eight compact video rows 48px tall, each a filmstrip with a thin 10px waveform band INSIDE its bottom edge, not a separate audio track or repeated audio label. One row header per video with eye, lock and tiny speaker controls; top-to-bottom order matches Layers list. Selected row has a subtle blue outline. Standalone music is a single compact waveform row. Video labels 'Video 8' through 'Video 1', and 'Music'. A preview quality dropdown 'Auto' with options Original/Half/Quarter; status 'Originals preserved for export'. Small lower architecture diagram: linked 4K originals -> bounded decode / reusable cache -> viewport-sized preview -> GPU layer composition -> canvas. Separate branch Original sources -> full-quality export. Caption 'Preview resolution adapts; authored pixels, timing, masks and Smart Objects remain editable'. No claimed measured FPS or hardware acceleration results. Footer 'Synthetic design reference — not a product screenshot'. Precise readable layout, calm practical controls.

## Inspection before implementation

The generated board has a single compact row per video with a thin internal waveform band,
one standalone music row, a preview-quality control and an originals-to-export branch. It adds
synthetic scenery behind the requested geometry and has duplicated eye controls; implementation
must keep the established PhotoStore layout and one eye control, using the audio strip and workflow
diagram as references. No known characters, private artwork or input images were used. FPS,
hardware acceleration and native smoothness must be established independently.
