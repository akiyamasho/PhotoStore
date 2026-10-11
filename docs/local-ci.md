# Local CI and release watcher

Status: implemented in `packaging/local-ci/`; not enabled by default. The watcher is a
background macOS LaunchAgent for the fixed `akiyamasho/PhotoStore` repository. It uses `gh`
for read-only metadata, persistent checkouts outside this worktree, one `fcntl.flock` lock,
and one shared Cargo target with one build job. It does not create tags, releases, pull
requests, or self-hosted runner jobs.

## Enable

After authoring/review and initial promotion are complete, run:

```sh
packaging/local-ci/enable-launchagent.sh
```

The script first copies the poller to `~/Library/Application Support/PhotoStore/local-ci/runtime`,
then bootstraps a baseline of current `main` and the latest published stable release without
building, and only then registers the LaunchAgent. Re-enabling preserves state. The agent runs
at load and approximately every 60 seconds with explicit `HOME` and `PATH`.

## Status and disable

The state directory is `~/Library/Application Support/PhotoStore/local-ci/state`; logs are in
`local-ci.log`. Read-only status is:

```sh
packaging/local-ci/photostore-local-ci.sh --status
```

`--observe-only` performs metadata reads only and performs zero builds, installs, or state
mutations. To disable, unload `ai.photostore.local-ci` with `launchctl bootout` and leave the
state directory intact for later inspection. Status reports observed baselines separately from
installed SHAs, so bootstrap cannot silently claim an app was installed.

## Triggers and safety

New `main` commits build the dev-install + heif variant and assemble then stage or install
`/Applications/PhotoStore-dev.app`. Production builds run only for the latest release sorted by
`published_at` where `draft` and `prerelease` are false. The release tag is resolved through
annotated tags to its exact commit SHA. A pushed tag, draft, prerelease, PR, or release creation
alone cannot promote production. Builds embed and verify `PHOTOSTORE_BUILD_SHA` and date;
production uses the official `heif` feature and requires `CRAFT_FONTS_DIR` for licenses.

Bundles are frozen into a staging directory and codesign-verified before promotion. If the
corresponding app identity is active, the bundle is retained as a pending artifact and installed
on a later poll after exit. The watcher never kills, restarts, focuses, closes, or overwrites an
active app, preserves an old production bundle backup, and keeps failure metadata with a
five-minute retry backoff. Missing network/authentication/JSON metadata is an error and never
advances a baseline.

The native shortcut export remains
`/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json`; development uses its
separate `.dev.json` counterpart. Production preferences and recovery data are not modified.
A release is eligible only when published, non-draft, and non-prerelease. A pushed tag or
retargeted tag is never a promotion signal; release-ID tag drift is recorded as a failure for
human review.

The GitHub Releases API fields used here are documented at
<https://docs.github.com/en/rest/releases/releases>.
