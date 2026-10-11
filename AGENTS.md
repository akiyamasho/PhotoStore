# PhotoStore contributor guide

PhotoStore is an independent Rust-native image editor. Its GitHub repository is private by default and has no upstream sync relationship. Preserve existing history, licences, NOTICE, and attribution records.

## Product identity

Use **PhotoStore** in user-facing text, window titles, About/home screens, OS menus, installers, release artifacts, and executable names. All internal crates use the `photostore-*` naming. Exposed commands are `photostore` and `photostore-cli`. The native app uses a PhotoStore-specific bundle identifier and configuration directory so it never writes inherited PhotoCraft preferences. PhotoStore is maintained as a personal fork for the owner's workflows, with no upstream sync relationship.

## Architecture

Keep the Rust layering intact: foundation and format crates at the bottom; document and operations above them; algorithms, paint, text, and vector services above that; compose/gpu/format and I/O above the document; engine and automation behind the UI; native egui/wgpu, CLI, and web shells at the top. User-visible behaviour belongs in the command registry so UI, CLI, automation, and persistence share one implementation. Smart Objects, timeline state, and frame animation must remain serializable document data.

## Reliability

Input from files, commands, automation, and users is untrusted. Return actionable errors, validate bounds and sizes, and avoid panic paths in non-test code. Do not assume 8-bit RGB, one allocation, or a single platform. Keep the clean-room boundary: use public specifications and original implementations, and preserve a licence/provenance row for every third-party or generated asset.

## Resource use

Use `CARGO_BUILD_JOBS=1` for builds and tests. Use the shared Cargo target directory; do not create per-agent 10 GB target directories. Avoid fetching corpora or running heavy builds unless the task requires it. Do not run discretionary QA or validation passes unless the user explicitly opts in.

## Recording and release documentation

Recording is off by default. Do not enable capture, tracing, or session recording as a
side effect of normal startup, development, release, or screenshot work.

Every release must document each released feature or fix with a real PhotoStore screenshot.
Capture the relevant flow with disposable demo documents only; never include the owner's
private, unsaved, or unrelated artwork. Store compressed, readable assets under
`docs/screenshots/<version>/` as lossless PNG or readable WebP. The assets must be committed
before the final release tag. Release Markdown and the README must reference the actual
committed images using links that remain valid for this private GitHub repository. Keep a
short evidence record for each image (feature/fix, demo document, source version, and known
limits). Statuses must distinguish implemented, preview, verified, and published. Do not
claim full Photoshop parity from menu wiring or an unverified UI path.

When a release needs new screenshots, use this order: implement and run the required
compiler; make one provisional preview build, labeled as a preview and treated as one
shared-target job; capture and legibility-check the UI screenshots; commit the docs and
README image references; tag the exact source SHA; then perform the incremental final
rebuild and verify its exact SHA and package integrity before publishing. Never retag or
publish provisional binaries. Older immutable tags remain unchanged. A current release
may be supplemented with a screenshot link only when its source version is verified.

## Changes

Read the relevant crate and docs before editing. Keep manifests valid, preserve unrelated work, and make focused changes. Run only checks requested by the user or required to establish that an explicitly requested change is safe. Record significant product or format decisions in `docs/decisions.md`.

## Local development installation policy

Every completed milestone is delivered to the isolated consolidated `PhotoStore-dev` installation
before the milestone is considered complete. The normal edit loop is the repository-root `./dev`
watcher documented in [`docs/dev-install.md`](docs/dev-install.md) and
[`docs/dev-watch.md`](docs/dev-watch.md). It uses one shared Cargo target and incrementally builds
changed inputs; it does not require a new pull request or a fresh application reinstall for each
edit. An explicitly requested independent goal may use a separate `PhotoStore-dev-<goal>` identity,
but goal bundles are never the default workflow.

The watcher may close the owned, controlled dev app only when it has no open document tabs and no
background jobs or dialogs. Any open tab, including a saved tab, defers the queued reload; save and
close every tab to let it apply automatically. It never closes an unrelated owner process and never
touches `/Applications/PhotoStore.app`. Source preferences and configuration are preserved. The
loop is a safe process restart, not in-process hot module replacement. When launched by an agent,
use background mode so the user's active UI is not blocked.

## Local CI and release watcher

Development and release automation is an opt-in local macOS LaunchAgent in
`packaging/local-ci/`, never a generic GitHub self-hosted runner. It polls the fixed
`akiyamasho/PhotoStore` repository's trusted `main` branch and GitHub Releases API at about
60-second intervals (`RunAtLoad`); it records the current main/release baseline before first
use so enabling it cannot replay stale work. A single queue/lock permits one heavy Rust job at
a time, using persistent checkouts outside this changing worktree, the shared Cargo target
`/Users/computer/Desktop/dev/PhotoStore/target`, `CARGO_BUILD_JOBS=1`, `RAYON_NUM_THREADS=1`,
and `nice -n 10`. Polling uses `gh` authentication without copying or logging credentials and
validates all refs before checkout.

Every new main SHA builds and installs the isolated `PhotoStore-dev` bundle with the
`dev-install` feature and the separate `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json`
export. A production build is triggered only by a published, non-draft, non-prerelease GitHub
Release; the release tag is resolved to its exact commit SHA before building. A pushed tag,
draft, prerelease, PR, or release creation alone never promotes production. The verified
production bundle is staged and atomically installed at `/Applications/PhotoStore.app` only
while the app is not running; otherwise it is deferred. The watcher never kills, restarts,
focuses, closes, or overwrites an active production app, does not modify production
preferences/recovery, preserves an old-bundle backup, and does not launch apps automatically.
The existing native watcher file `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json`
remains canonical.

Do not enable the LaunchAgent until authoring/review and initial promotion are complete. Use
`--bootstrap` or `--observe-only` for safe initialization. Human review remains required to
create/publish releases and tags; automatic installation is authorized only for published
stable releases. Every released feature/fix still requires the screenshot evidence rules above.
