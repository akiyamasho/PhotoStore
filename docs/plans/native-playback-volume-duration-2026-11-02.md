# Native playback, row gain and timeline duration

User request: actual native playback with many 4K layers must stay responsive to drawing and panning; provide row volume, draggable object lengths, content-following timeline end and explicit extra time. A CLI compositor result alone does not establish the native result.

## Implementation outcomes in this pass

1. Native canvas pipeline: keep all Video Timeline previews, including live strokes, on the background compositor. Publish textures inside renderer callback preparation without GPU rebind, and retire displaced snapshots/resources on the worker. Use stable stroke-family identity and merge only the live paint target over the newest prepared video frame. Same-frame partial redraw uses cumulative gesture bounds; a movie frame change forces full redraw. The input loop must never synchronously composite the video stack.
2. Row gain and scalable composition (independent work): persist audio-track gain separately from clip gain and mute, with a backward-compatible default. Both preview and export use the product. Isolate legacy linked tracks by owner when needed. Stream recognized Normal RGBA8 stacks beyond eight layers in bounded source batches, including an eight-video-plus-drawing case. Preserve portable fallbacks for other effects/formats.
3. Duration editing (independent of renderer work): show actual edge handles and duration feedback; trim video within source limits and extend still/Smart Object holds. Reconcile timeline end after timing edits, retaining deliberately authored tail padding. Legacy slack is inferred before the edit rather than silently discarded. Fit-to-content clears padding; explicit added time reserves it. Content growth consumes reserved padding, so extending a clip does not move the target end; shrink preserves any remaining reservation. Off-clip keyframes, source media and markers remain stored. Frame Animation retains its own rules.

## Concrete findings

- `canvas::ensure_gpu` previously admitted only raw preview key zero to the video worker. A live stroke used synchronous `gpu.refresh` instead.
- A live stroke was invalidated by every presentation revision, despite no authored edit. It must follow content revision while its painted target is composited over current media.
- Publishing/replacing video resources involved egui renderer access and UI-side drops. Worker-private resources and callback-side map swaps remove those costs from app update.
- The fused Normal stack stopped at eight inputs. Adding a drawing layer to eight movies changed the rendering path. Source batching must remain bounded and preserve alpha/color order.
- Audio tracks had mute but no independent row gain. Clip-gain fanout would destroy relative levels and fail at zero.
- Existing trim commands did not reconcile total duration; UI right edges were clamped to the current global end and poorly indicated.

## Design and evidence policy

Built-in GPT Image designs were generated and inspected before implementation: [native workflow/volume](../design/2026-11-02/native-playback-volume/workflow.png), [duration](../design/2026-11-02/native-playback-volume/timeline-duration.png), with exact prompts alongside them. These are original design references, not application captures.

Build the final normal app and CLI once from one exact source SHA, with the shared single-job target and scheduler suppressed. The required follow-up is a focused native-window run using owned 4K fixtures and isolated preferences/recovery, observing actual callback publications and UI response while synthetic drawing/panning runs. No screenshots, capture or private artwork. Synthetic input does not verify OS pen routing or subjective CapCut equivalence. Check the changed persistence/trim/gain boundaries and bounded GPU pixels outside timed playback, then verify/package/push/release/install the same artifacts. No broad QA suites or repeat builds for documentation.

Primary references: [wgpu submission callbacks](https://docs.rs/wgpu/30.0.1/wgpu/struct.Queue.html#method.on_submitted_work_done), [polling semantics](https://docs.rs/wgpu/30.0.1/wgpu/type.PollType.html), [CapCut audio level workflow](https://www.capcut.com/resource/volume-booster-download). CapCut documents dB controls; the particular row UI and limits here are PhotoStore decisions, not a copied product specification.

The [c150 native window candidate](../performance/0.20.8-native-c150.json) improved decoder reuse and composition, but still failed cadence. The scheduling cause is separate from the cache-size policy: consumer-affine foreground workers also popped thumbnail jobs and could block behind the single background-decoder permit, leaving a required movie owner unable to run. Thumbnails now have one dedicated background worker and queued thumbnail work is suspended while native transport is active. The same shared decoder admission (at most four heavyweight decodes, one background) and memory allowance remain authoritative. In-flight thumbnail work finishes safely without occupying a foreground worker. Active transport suppresses new thumbnail work; pause/stop wakes it. This avoids creating a late-frame catch-up cycle during preroll/early playback.
