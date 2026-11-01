# Smooth preview and input design

Generated before implementation on 2026-11-02 (JST) with the built-in GPT Image tool. No reference images were attached. Original geometric scenery, no private media or known characters. Output: `workflow.png`; source output `exec-f435a020-fc17-4b86-9b40-dea03fb47511.png` in the session's generated-images folder. This is a design reference, not a product screenshot or performance evidence.

## Exact prompt

```text
Use case: ui-mockup
Asset type: PhotoStore implementation design reference, not a product screenshot.
Primary request: A polished dark desktop drawing and multilayer video editor design sheet for smooth interactive preview, stable transport, canvas panning, and pen brush selection.
Composition: wide landscape sheet. Upper two thirds: original PhotoStore editor with drawing toolbar left, Layers list right, landscape preview made only of abstract geometric scenery (no people or characters), two video layers at 50% opacity, compact integrated audio waveform band inside each video clip. One compact fixed timeline toolbar/ruler and aligned layer rows at bottom. Keep layout stable during playback: reserve a small fixed status area with the word "Preparing" rather than shifting transport buttons. No Media/Import sidebar. Show unobtrusive Auto preview quality control, original media retained.
Lower third: clear workflow diagram "Input → latest requested time → persistent decoders → completed frame → GPU preview". Show a nonblocking UI lane, bounded lookahead, display-sized preview and full-quality originals/export on separate line. Small interaction callouts: "Hold Space + drag: pan canvas", "Space outside canvas: play / pause", "Pen secondary press: brush picker". Show a compact brush picker anchored at the pen tip and a hand cursor on canvas.
Constraints: original generic application design, crisp readable UI hierarchy. No private artwork, filenames, brands beyond PhotoStore, known characters, fake FPS measurements, parity claims or external logos. Mockup describes intended implementation, not evidence.
```

## Inspection

The generated sheet supports the requested stable toolbar, integrated audio, Layers workflow, pointer-owned panning and pen picker. Its large top transport and illustrative extra audio lane are not a requirement to relocate existing UI. Implement a fixed status slot in the existing compact toolbar. The Auto quality selector is a design proposal only until a real display-sized preview path exists. Original media/export must remain separate from derived preview pixels. No measured responsiveness or CapCut parity is implied.

