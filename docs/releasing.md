# Releasing PhotoStore

PhotoStore releases are prepared locally from the workspace and published manually. The current
release scope is the ARM64 macOS app and CLI; multi-platform workflow recipes remain optional and
are not enabled or claimed as built.

## Local release procedure

1. Set the workspace version with `cargo xtask version set <version>` and review the resulting
   `Cargo.toml` and `Cargo.lock` changes. Keep the workspace lockfile consistent.
2. Make the exact source commit the release reference. Create an annotated tag only after the
   source SHA, version metadata, and changelog/docs are ready.
3. Build with the shared target directory, low resource settings, one job, and the locked offline
   dependency set when available. For the current macOS scope, package ARM64 with
   [`packaging/macos/package.sh`](../packaging/macos/package.sh).
4. Inspect package contents and verify bundle identifier `ai.photostore.app`, version, baked
   source SHA/date, canonical icons, `NOTICE`, `ATTRIBUTION.md`, and license files. Generate
   `SHA256SUMS` and verify the listed artifact bytes and hashes.
5. Verify signatures with strict local checks. Local packages may be ad hoc signed and are not
   notarized; do not describe them as notarized or universally signed.
6. Publish the exact tag and artifacts as a GitHub prerelease with `gh`, including the DMG, CLI
   archive, and checksum file. Record the tag, commit, artifact hashes, and limitations in the
   goal ledger.
7. Before reinstalling, make a recoverable backup of the existing app bundle and preserve any
   unsaved documents or running process. Install the new app, confirm the canonical executable,
   bundle ID, version, source SHA, strict signature, and startup. Confirm release-specific feature
   flows only after installation is complete.

The current release procedure does not require an external playbook, upstream checkout, Discord
service, or automatic branch-triggered workflow. `.github/workflows/release.yml` and the platform
packaging scripts document optional manual multi-platform recipes; those recipes require their own
credentials, hosts, and evidence before they can be called a release.

## Fonts and licenses

Builds made with `CRAFT_FONTS_DIR` may embed the fonts listed in the [craft-fonts attribution
record](https://github.com/storytold/craft-fonts/blob/main/ATTRIBUTION.md). Their SIL Open Font
License texts ship as `OFL-<family>.txt` when those fonts are included. The web build does not
embed them. Preserve `NOTICE`, `ATTRIBUTION.md`, and every bundled asset license in each package.

## Packaging references

- macOS app and DMG: [`packaging/macos/package.sh`](../packaging/macos/package.sh)
- macOS verification: [`packaging/macos/verify.sh`](../packaging/macos/verify.sh)
- icon regeneration: [`packaging/icons.sh`](../packaging/icons.sh)
- web packaging: [`packaging/web/README.md`](../packaging/web/README.md)
