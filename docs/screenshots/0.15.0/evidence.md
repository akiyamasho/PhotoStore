# PhotoStore 0.15.0 opacity stopwatch evidence

This local development receipt has no screenshot asset. The focus=false native capture attempt timed
out while hidden and was occluded; no foreground fallback was used and no image was fabricated.
The sanitized runtime record is [evidence.json](evidence.json).

The focused native check used the fresh goal configuration with zero initial documents and recording
off. It confirmed Linear frame 1 = 0.625; Hold frame 1 = 1 and frame 2 = 0.25; native ZIP keys frame
0/Hold and frame 2/Linear 0.25; real close/reopen preservation; empty keys after disable with static
opacity 1.0; and
a clean app quit with the listener closed. The opacity key values and manifest keys were live
observations before disabling; the retained native artifact after disable has empty keys, so no
animated snapshot is claimed. Visual interaction acceptance remains unverified.

Source: `c57ee914f11cc410d1b1f13900e0104770462256`.
App: `/Applications/PhotoStore-dev-opacity-animation.app` (`ai.photostore.app.dev.opacity-animation`).
Retained native artifact: `/private/tmp/photostore-opacity-runtime-20261011-evidence/work/opacity-demo.pcraft`.
