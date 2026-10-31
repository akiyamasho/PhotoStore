# PhotoStore local CI watcher

> **Paused local scheduler — 2026-11-01:** Routine delivery overwrites the normal PhotoStore app directly. Keep this legacy scheduler disabled and unloaded; push, tag, or release publication must not trigger a build. `--bootstrap` is first-use-only; `--observe-only` does not advance an existing baseline. Do not resume without explicit user authorization and a supported no-replay handoff. This release does not change watcher code.

This opt-in macOS LaunchAgent polls the fixed `akiyamasho/PhotoStore` repository with `gh` every 60 seconds. It watches trusted `main` and the newest published, non-draft, non-prerelease GitHub release. Tags, drafts, prereleases, pull requests, and release creation never trigger production promotion.

The watcher lives outside the changing checkout, uses persistent checkouts under `~/Library/Application Support/PhotoStore/local-ci/checkouts`, one `fcntl.flock`, one heavy Rust job, the shared target `/Users/computer/Desktop/dev/PhotoStore/target`, and `CARGO_BUILD_JOBS=1 RAYON_NUM_THREADS=1 nice -n 10`. `--bootstrap` records current baselines without building, preventing stale replay. `--observe-only` reads metadata only and performs zero builds and installs. It does not launch apps or copy/log credentials.

Each new main SHA builds with `--features dev-install,heif` and assembles `/Applications/PhotoStore-dev.app` using the isolated shortcut export `photostore_keys.dev.json`. A production build runs only for a published stable release and resolves its tag to an exact SHA. The signed bundle is staged and installed at `/Applications/PhotoStore.app` only while the app is not running; otherwise it is deferred. The watcher never kills, restarts, focuses, closes, or overwrites an active app, never modifies production preferences/recovery, and keeps an old-bundle backup. The native shortcut file `photostore_keys.json` remains canonical.

Run `enable-launchagent.sh` only after reviewing and completing initial promotion. Human review remains required to create/publish releases and tags. GitHub Releases API reference: <https://docs.github.com/en/rest/releases/releases>.
