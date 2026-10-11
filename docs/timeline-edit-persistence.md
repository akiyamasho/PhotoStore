# Timeline edit persistence

The engine now canonicalizes an ordinary edit while the edit transaction still has both the
pre-edit document and the edited display. For a stable playhead mapping (same current frame,
timeline mode, fps, duration, and layer ranges), a changed displayed raster surface is written
back only to its mapped video frame. The write uses copy-on-write tile identity, and it is skipped
when the underlying frame or video metadata changed. This preserves explicit frame replacement,
retime, navigation, and keyframe commands handled by their existing video/timeline code.

Current frame-animation or property-animation snapshots are updated field by field only when the
current snapshot vector itself did not change. Unrelated layer entries and range-hidden visibility
remain intact. Structural layer additions and removals are intentionally deferred because their
snapshot ordering and range semantics need command-specific handling.

`Session::edit` invokes this canonicalization after smart-filter refresh and color restriction, so
inline edits and the apply phase of background edit jobs share the same persistence boundary.
The helper does not perform full pixel comparisons: it relies on shallow surface metadata and
copy-on-write tile pointers. Keyframe sampling is allocation-free; sorted tracks use a lower-bound
lookup after the required O(n) sortedness check, and malformed unsorted tracks use deterministic
one-pass duplicate handling.

This is a bounded persistence pass. It does not claim full Photoshop timeline parity or cover
future timeline features such as stopwatch or structural snapshot editing.
