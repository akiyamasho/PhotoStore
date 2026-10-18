# Generative Fill

Select part of an image, choose **Edit → Generative Fill…**, describe the change, and choose
**Generate**. The provider receives the entire visible canvas as context, the selection mask,
and the selected bounding-box crop. PhotoStore adds the returned pixels as a new top-level
**Generative Fill** layer, clipped by the original selection, in one undo step. Original layers
remain editable. Feathered edges retain their selection coverage.

## Local Codex

The default provider runs the locally installed `codex` CLI using its existing sign-in and
image-generation tools. Sign in with `codex login` before using it. Expand **Provider settings**
to specify its executable, or choose a custom adapter. Credentials stay with the provider;
PhotoStore does not store API keys. Codex inference may use online services and account limits.
The image context is shared only when Generate is chosen.

PhotoStore invokes `codex exec` in a private temporary job directory, attaches `context.png`,
`mask.png`, and `crop.png`, and requests `$imagegen` to write `output.png`. A Codex installation
without image-editing access returns an error; PhotoStore does not substitute a simulated fill.
See the official [Codex image-generation guidance](https://learn.chatgpt.com/docs/image-generation)
and [CLI reference](https://learn.chatgpt.com/docs/cli/reference).

## Custom providers

Choose **Custom adapter**, set an executable, and enter its arguments as a JSON array of strings.
For example, an adapter you implement in Python can receive:

```json
["/absolute/path/to/my_adapter.py", "--request", "{request}", "--output", "{output}"]
```

Set Executable to the absolute Python interpreter path. Each array item is passed as one argument;
PhotoStore does not invoke a shell. Supported substitutions are `{request}`, `{output}`, `{context}`,
`{mask}`, `{crop}`, and `{prompt}`. The adapter can wrap a local model, an HTTP service, or another
image-editing tool. It owns service-specific authentication and model configuration. Provider
settings persist in the normal PhotoStore preferences under `generativeFill`.

The working directory contains a `request.json` with schema `photostore.generative-fill.v1`:

- `canvas`: full canvas width and height in pixels.
- `selectionRect`: `x`, `y`, `width`, and `height` of the edit in canvas coordinates.
- `prompt`: the user's requested change.
- `contextPath`: PNG of the composited full image.
- `maskPath`: full-canvas grayscale PNG; white edits, black preserves, gray feathers.
- `cropPath`: PNG crop at `selectionRect`, without rescaling.
- `outputPath`: the required output file (`output.png`).
- `colorContract`: the interchange color and output-size requirements.

The adapter must exit successfully after writing a real PNG at `outputPath`. Return either the
exact selected bounding-box dimensions or the exact full canvas dimensions. Full-canvas output is
cropped at the recorded coordinates; pixels outside the original selection are discarded. Wrong
sizes are rejected, avoiding silent stretching or coordinate shifts. An opaque patch replaces
selected visible content; transparency in a patch reveals the original layers underneath.

## Safety and limits

Generation runs as a cancellable background job against a document snapshot. The document stays
locked until the job completes, fails, or is cancelled. No result is committed on failure,
timeout, cancellation, or a stale document. Temporary image files are deleted when the job ends.
Custom adapters are trusted local executables and run with the user's normal process permissions.
Untrusted MCP/control commands cannot launch providers or change their executable settings;
trusted local CLI scripts use the same engine command `edit.generativeFill` as the UI.

The initial interchange supports sRGB RGB documents (including higher document bit depths), up
to 25 million canvas pixels. Convert other document modes/profiles to sRGB first. Provider output
must be 8-bit RGB/RGBA PNG, at most 128 MiB, with an sRGB profile or no embedded profile. Existing
higher-depth pixels are retained in their original layers; generated content has 8-bit precision.

Pixel placement and selection clipping are deterministic. Whether generated lighting, detail,
and edges look natural depends on the selected model and prompt; a seamless semantic match is
not guaranteed. Generated content is a raster layer, rather than an editable reconstruction of
text, vectors, Smart Objects, or video. This release targets native desktop use.
