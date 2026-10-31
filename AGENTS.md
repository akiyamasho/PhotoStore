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
session, that authorization is sufficient; do not ask again. Before a release transaction, suppress
the local scheduler and verify the handoff: `launchctl print-disabled gui/$(id -u)` must report
`ai.photostore.local-ci` disabled, `launchctl list` must show no loaded local-ci job, and
`photostore-local-ci.py --status` must show no pending development or release job. A queue lock alone
is insufficient. Keep the LaunchAgent disabled and suppression active through the local build,
installation, push/tag/publication, and documentation-only follow-up pushes. Recheck all three signals
after delivery; with the scheduler still disabled and no pending work, this is the no-replay
acknowledgement. Do not wait for or close a running production app before building or installing.

For an ordinary authorized bug-fix release, build the app and CLI locally once from one exact source
SHA. That single local release build is the required compiler/build pass. Do not run a separate
compiler check, preview build, second release build, or any extra check/test/QA after required
verification. Check version, full source SHA, architecture, signature status, package integrity, and
checksums once against the final artifacts. Push the reviewed source, publish a GitHub release
targeting that exact source SHA (the release creates its version tag), then immediately install
locally from those same verified artifacts. Tagging, pushing, or creating/publishing a GitHub release
must never trigger another development or production build. There is no GitHub Actions or
GitHub-hosted CI build. Upload only the already-built and verified app/CLI artifacts; publication is
delivery, not a build trigger.

Queue ownership alone never justifies starting another development build. If an already queued or
running job is proven to be a redundant build of the exact release SHA, narrowly and gracefully
cancel or suppress only that duplicate; do not wait behind it. Never cancel or interrupt unrelated
jobs or applications. If an unrelated job owns the shared queue, wait for it to finish or report that
blocker without starting duplicate work. Never recompile identical executable inputs for
documentation-only changes; follow-up commits retain the immutable tag as the artifact source SHA
and require no binary rebuild or retag.

If a mount or packaging verification step partially fails, retry only that owned step with the same
artifacts; do not repeat a passed compiler or release build. Use a documented fallback after a hung
tool failure, and report concrete blockers rather than retrying the same path indefinitely. Avoid
narrating unchanged poll results while preserving higher-priority progress updates.

An explicitly requested reinstall may replace the installed app bundle while PhotoStore is running.
Stage and validate the replacement first, then install it atomically; do not force-quit PhotoStore or
delete files from a bundle its process may still use. Keep the previous bundle as a rollback copy
until its process has exited and the backup identity is verified. The running session may continue
using code already loaded in memory until restarted. Never modify the app's private preferences or
recovery state. Never resume the scheduler in a state that can replay already-built release source.
Keep scheduler suppression active until a no-build acknowledgement/baseline confirms the final main,
tag, release, and artifact state, including documentation-only pushes. If the LaunchAgent cannot be
proven disabled or a pending job remains, pause scheduling safely and leave it paused with a clear
report. Do not re-enable the local scheduler without an explicit user request and a proven no-replay
handoff.

## Required GPT Image design before feature/UI implementation

Before implementing any new feature or fixing any UI bug, generate a GPT Image mockup/design for the proposed user-facing result and save the image and exact prompt/provenance record under `docs/design/<YYYY-MM-DD>/`. Inspect the generated design against the request and current product context, then use it as a design reference during implementation. For a backend-only feature, the image must show a useful workflow/interface diagram for the affected user flow; it still requires the GPT Image design step. A pre-existing screenshot, text-only wireframe, or prompt without a generated image does not satisfy this requirement.

This applies before code implementation begins; it is a design workflow, not a user-approval or discretionary QA gate. If GPT Image generation is unavailable, continue independent research and specification work, record the blocker, and do not begin implementation of the affected feature/UI fix until its design can be generated. Pure non-UI bug fixes are outside this requirement. Do not use private artwork or known characters as image inputs unless their separate required authorization/reference rules are met. Preserve generated-asset provenance and identify mockups as design references, not product screenshots or verification evidence.

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

Legacy implementation note: the watcher was configured to build a development bundle for each
new `main` SHA and to build production after a published stable GitHub Release. Those legacy
triggers are not permitted for manual releases or as a way to promote locally built artifacts. Keep
the LaunchAgent paused; never let a push, tag, or release publication trigger a development or
production rebuild. Re-enable only after a separately authorized watcher change removes those
release rebuild triggers and a supported no-replay suppression/baseline handoff is proven. The prior
installer was designed to stage and atomically install only while production is not running, without
modifying production preferences/recovery or launching automatically; that behavior does not
supersede this paused-scheduler policy.
The existing native watcher file `/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json`
remains canonical.

The local CI LaunchAgent is paused by default and is not GitHub Actions or GitHub-hosted CI. The
existing watcher may poll `main` and GitHub Releases and contains build/install triggers; do not
reactivate it for a manual release. In particular, a `main` push, tag, or GitHub release must never
trigger redundant development or production builds. Stable releases use the local app-plus-CLI build
and immediate local installation described above. Keep downloadable assets for the three newest
published releases; the retention workflow deletes older release assets while preserving release
records and tags. Do not prune release assets manually.

Do not re-enable the scheduler without an explicit user request and a proven no-replay suppression
handoff. `--bootstrap` is first-use-only and `--observe-only` does not advance an existing baseline;
never claim either option suppresses or safely resumes the current watcher. Keep it paused if a
supported handoff is unavailable. After a manual release, suppression must cover final `main`, tag,
release, artifacts, and any documentation-only pushes through a no-build acknowledgement/baseline;
never resume a scheduler that could replay already-built source. Do not edit watcher code as an ad hoc
release workaround. This workflow does not imply a change to watcher code or its existing behavior.
