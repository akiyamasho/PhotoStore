# Notebook View release captures

Both screenshots show the Apple Silicon app extracted from the published 0.21.1 disk image. Its source commit is `d33595b5549c89ce6aa05a9d330cb1ca9943b9ab`. [Release and downloads](https://github.com/akiyamasho/PhotoStore/releases/tag/v0.21.1).

| Capture | What it shows |
| --- | --- |
| [Notebook View](notebook-view.webp) | Page 2 of 4 displays the Cup study group on the editable canvas with the Brush tool selected. |
| [Pages dialog](notebook-pages.webp) | The four root entries appear in layer-stack order. The dialog highlights Cup study and provides search and move controls. |

The disposable document, `Studio notebook.psd`, contains three root layers and one root group. Cup study contains a Watercolor layer. The document was saved with Notebook View active and reopened to check the root order and group contents. In the native app, synthetic shortcut input exercised previous and next page, opened Pages and jumped to page 3 by number. Moving a page through the notebook command and undoing the move restored the original order. Physical keyboard and stylus input and video recording were outside the capture pass.

The native screenshot command captured a 1240 × 800 logical window at 2× scale using isolated demo settings. The README uses 1600-pixel-wide WebP derivatives at quality 93. Recording stayed off.

## Demo artwork

OpenAI GPT Image generated the watercolor for this disposable document from the following prompt with no image inputs. The surrounding editor and dialog are native PhotoStore UI.

> An original artists' field notebook watercolor plate, landscape 3:2. On warm off-white cold press paper, a single cobalt blue ceramic teacup with a cream rim and loop handle, seen from slightly above and sitting on a faded pale peach cloth. Loose transparent ultramarine and muted ochre watercolor washes, visible granulation, subtle graphite construction hatching, hand-painted gestural edges. Cup sits a little to the right of center with generous uncluttered paper margin. A small trio of painted blue color samples at lower left. The entire image is the flat scanned artwork, edge to edge paper. No text, no people, no brands, no devices, no interface. Original disposable sample art for an image editor notebook demonstration.
