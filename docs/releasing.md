# Releasing PhotoStore

PhotoStore releases are prepared locally from the workspace and published manually. The current
release scope is the ARM64 macOS app and CLI; multi-platform workflow recipes remain optional and
are not enabled or claimed as built.

## Release evidence and screenshots

When the release evidence workflow requires screenshots, capture the real PhotoStore UI with
disposable demo documents; do not open or include private, unsaved, or unrelated artwork. Keep the
compressed source assets in [`docs/screenshots/`](screenshots/) under
`docs/screenshots/<version>/`. PNG is preferred for lossless UI text; readable WebP is also
permitted. Check that each image is legible at the size used in the release notes and README.

For each screenshot, record the feature or fix, disposable demo document, source version or SHA,
what is visibly verified, and any material limit. Use the exact committed image paths in release
Markdown and the README. For a private GitHub repository, use repository-relative links or
tag/commit-pinned raw links that resolve for authorized readers; do not use local file paths or
uncommitted preview URLs. A screenshot can establish the shown UI flow and state only. Menu wiring
or a visible control does not establish full Photoshop parity.

Capture requested or workflow-required screenshots from the final locally built release binary.
They may be committed in a documentation-only follow-up after publication; keep the release tag
immutable and do not rebuild or retag for screenshot documentation. Existing immutable tags remain
unchanged. The current `0.11` release may be supplemented with a screenshot link when its source
version is verified; this procedure does not rewrite older tags retroactively.

## Local release procedure

1. Set the workspace version with `cargo xtask version set <version>` and review the resulting
   `Cargo.toml` and `Cargo.lock` changes. Keep the workspace lockfile consistent.
2. Make the exact reviewed source commit the release reference and push it.
3. Build with the shared target directory, low resource settings, one job, and the locked offline
   dependency set when available. For the current macOS scope, package ARM64 with
   [`packaging/macos/package.sh`](../packaging/macos/package.sh).
   Production packaging uses the persistent local signing identity configured at
   `~/Library/Application Support/PhotoStore/signing/release-signing.json` by default. The public
   JSON has `schemaVersion: 1`, `kind: "local"`, `identitySha1`, `certificateSha256`, and
   `keychainPath`. The packager verifies the configured certificate and keychain identity before
   starting Cargo. It never creates or rotates a key. A missing or invalid identity is an error;
   use `MACOS_SIGN_IDENTITY` and optional `MACOS_KEYCHAIN` only to select another valid
   certificate-backed identity. `package-dev.sh` remains the ad-hoc path.
4. Inspect package contents and verify bundle identifier `ai.photostore.app`, version, baked
   source SHA/date, canonical icons, `NOTICE`, `ATTRIBUTION.md`, and license files. Generate
   `SHA256SUMS` and verify the listed artifact bytes and hashes.
5. Verify signatures with strict local checks. The default local certificate uses hardened runtime
   and a stable signing identity, but is not Developer ID and cannot be notarized; describe those
   packages as locally signed and not notarized. A valid Developer ID identity retains Apple's
   secure timestamp and may be notarized when the Apple credentials are configured. Do not
   describe a local-certificate package as notarized or universally trusted.
6. Publish a GitHub release targeting the exact source SHA with `gh`, including the DMG, CLI archive,
   and checksum file. Publishing creates the version tag for that source. Record the tag, commit,
   artifact hashes, and limitations in the goal ledger. The asset-retention workflow keeps assets for
   the three newest published releases and deletes assets from older releases while preserving their
   release records and tags.
7. Stage and validate the replacement app bundle and preserve unsaved documents. Atomically replace
   the installed app even if PhotoStore is running; do not force-quit it or delete files from a bundle
   its process may still use. Keep the previous bundle as a rollback copy until the process exits and
   the backup identity is verified. The running session may continue using already-loaded code until
   restarted. Read back the installed version and source SHA before reporting completion.

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
