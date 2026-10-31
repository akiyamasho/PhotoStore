# macOS DMG

The PhotoStore DMG is intentionally plain: it contains `PhotoStore.app` and an `Applications`
symlink and is created by `packaging/macos/package.sh` with `hdiutil create`, an APFS filesystem,
and compressed UDZO output. APFS avoids synthesized FinderInfo attributes that can invalidate
strict bundle signatures on a mounted HFS hybrid. No custom Finder background, generated
`.DS_Store`, or external artwork is required.

The image volume is explicitly sized to twice the staged allocation plus 256 MiB, with a 512 MiB
minimum. This leaves room for APFS metadata; automatic sizing failed with ENOSPC during 0.19.0
packaging despite ample host disk space. Empty space is compressed and is not a download-size
floor. That release retried only the image step at 512 MiB using the already signed app, without
recompiling; this script change applies to future packages and does not change its published assets.

Verify the image signature and the mounted app's strict signature, identity, source SHA and
licence contents. If legacy `hdiutil verify` fails while DiskImages2 recognizes the image, use
`diskutil image info` and `diskutil image attach --readOnly --nobrowse --mountPoint <owned-directory>`
for the same artifact, then eject that mount. See the [0.19.0 receipt](../../../docs/releases/0.19.0.md)
for the exact fallback and asset checksums. A packaging or mounting retry must not rebuild passed
executable inputs.
