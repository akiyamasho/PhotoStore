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

## Recording and release evidence

Recording is off by default. Do not enable capture, tracing, or session recording as a side effect
of normal startup, development, release, or screenshot work.

Screenshots are captured when the user requests them or a specific product/evidence workflow
requires them; they are not a gate for an ordinary bug-fix release. When requested, use real
PhotoStore UI captures and disposable demo documents only. Never include private, unsaved, or
unrelated artwork. Store readable PNG or WebP files under `docs/screenshots/<version>/`. Record the
feature/fix, demo document, source version, capture method, and known limits. Release Markdown and
the README should link to the actual repository images when they are available. Distinguish
implemented, preview, verified, and published status, and do not claim full Photoshop parity from
menu wiring or an unverified UI path.

A requested screenshot may be captured from the final release binary and committed with the release
documentation. Verified, source-version-specific screenshots and their documentation may also be
added in a later documentation-only commit without retagging or rebuilding the binary. Documentation-
or workflow-only commits may follow an immutable source tag; they do not move that tag or change the
source SHA of its binary artifacts, and they do not require a binary rebuild or retag. Do not create
a provisional preview build solely to satisfy a screenshot gate. State when synthetic automation
input was used; it does not independently verify native OS input routing.

## Authorized bug-fix release transaction

When the user has explicitly authorized a stable release and production promotion in the active
session, that authorization is sufficient; do not ask again. For an ordinary authorized bug-fix
release, use one exact source commit and one app-plus-CLI release build. That release build is the
required compiler/build pass; do not add a separate preview build or preliminary compiler build.

Acquire the shared build/release queue lock before pushing and hold it through the release
transaction: build, push, tag, publish the stable release, and install the same verified bundle.
Do not push to `main` early and then wait for a redundant development rebuild of the release
commit. If the queue is occupied by work for the exact same source SHA, prove the match and narrowly
and gracefully cancel or suppress only that redundant duplicate; do not normalize a long wait behind
it. Never cancel or interrupt unrelated jobs or applications. If an unrelated job owns the shared
queue, wait for it to finish or report that blocker. Reuse the single verified app/CLI artifacts for
publication and installation.

Check the exact version, full source SHA, architecture, signature status, package integrity, and
checksums once against those artifacts. If a mount or packaging verification step partially fails,
retry only that owned step with the same artifacts; do not repeat a passed compiler or release
build. Do not run discretionary QA unless the user asks for it. Keep tool waits bounded, change to
a documented fallback after a repeated or hung tool failure, and report concrete blockers rather
than retrying the same path indefinitely. Avoid narrating unchanged poll results while preserving
higher-priority progress updates.

Never replace, restart, close, or kill an active production app; never modify its private
preferences or recovery state. Stage and install only after the exact release artifacts are
verified, and only when production is not running.

## Changes

Read the relevant crate and docs before editing. Keep manifests valid, preserve unrelated work, and make focused changes. Run only checks requested by the user or required to establish that an explicitly requested change is safe. Record significant product or format decisions in `docs/decisions.md`.

## Local development installation policy

Deliver milestone changes through the consolidated `PhotoStore-dev` runtime; use the repository-root
`./dev` watcher for local iteration, and reserve installed bundle refreshes for local CI/main
promotion. The normal edit loop is documented in [`docs/dev-install.md`](docs/dev-install.md) and
[`docs/dev-watch.md`](docs/dev-watch.md). It uses one shared Cargo target and incrementally builds
changed inputs; it does not require a new pull request or a fresh application reinstall for each
edit. An explicitly requested independent goal may use a separate `PhotoStore-dev-<goal>` identity,
but goal bundles are never the default workflow.

The watcher may close the owned, controlled dev app only when it has no open document tabs and no
background jobs or dialogs. Any open tab, including a saved tab, defers the queued reload; save and
close every tab to let it apply automatically. It never closes an unrelated owner process and never
touches `/Applications/PhotoStore.app`. Source preferences and configuration are preserved. The
loop is a safe process restart, not in-process hot module replacement. When launched by an agent,
use background mode so the app opens behind the user's active UI without blocking, focusing, or
raising that window.

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
export, unless a live owned dev watcher or its child process is active, in which case the CI install
is deferred. A production build is triggered only by a published, non-draft, non-prerelease GitHub
Release; the release tag is resolved to its exact commit SHA before building. A pushed tag,
draft, prerelease, PR, or release creation alone never promotes production. The verified
production bundle is staged and atomically installed at `/Applications/PhotoStore.app` only
while the app is not running; otherwise it is deferred. The watcher never kills, restarts,
focuses, closes, or overwrites an active production app, does not modify production
preferences/recovery, preserves an old-bundle backup, and does not launch apps automatically.
The existing native watcher file `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json`
remains canonical.

The local CI LaunchAgent remains opt-in. Use `--bootstrap` or `--observe-only` for safe
initialization, and do not enable it as a side effect of a release. The agent-led, user-authorized
release transaction above is an explicit workflow exception to the watcher's normal human-review
boundary; it does not imply that watcher code or its promotion policy changed. When that explicit
authorization is absent, human review remains required to create or publish releases. Automatic
installation by the watcher remains authorized only for published stable releases. The final
production safety rules above continue to apply to both workflows.
