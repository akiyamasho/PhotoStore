# PhotoStore development watcher

The development watcher is the small feedback loop around the isolated `PhotoStore-dev`
installation. It is intended for local source iteration and does not change the production app.

From the repository root, the supported commands are:

```sh
./dev                 # foreground incremental build and safe owned-app restart
./dev --background    # start the watcher without taking over the user's UI
./dev --once          # build once and perform one guarded reload attempt
./dev --status        # print watcher state and owned runtime count
```

`--background` is for agent-driven work that must leave the user's foreground application alone.
The watcher is a process restart with a guarded control request, not in-process hot module
replacement.

## State and scope

The watcher uses the shared repository Cargo target and `CARGO_BUILD_JOBS=1`; it does not create a
per-agent target directory. It watches Rust, asset, and packaging inputs that affect the dev
bundle, then runs one incremental build at a time. Build failures leave the currently running
dev bundle in place and preserve its logs and recovery state.

After a successful build, the watcher requests a safe restart of the isolated dev process. It
waits whenever any document tab is open, even when that tab is saved, or whenever background jobs
or dialogs are active. Save and close every tab and resolve jobs or dialogs; the queued reload then
applies automatically. The watcher may close its own controlled blank process only after that guard
passes. It never closes an unrelated owner process and never touches `/Applications/PhotoStore.app`.
The implementation preserves source preferences and configuration; it does not claim additional
recovery-state protection beyond the open-tab, jobs, and dialog guards.

The consolidated identity is `PhotoStore-dev`. An independent goal may use
`PhotoStore-dev-<goal>` only when explicitly requested, with separate bundle, config, recovery,
presets, logs, UI state, process, and shortcut-export paths. The canonical production shortcut export remains
`/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json`; development uses the `.dev.json`
counterpart.

## Relationship to local CI

The optional local CI LaunchAgent consumes trusted `main` and published GitHub Release metadata.
Each new `main` SHA updates the consolidated dev installation. Production installation is
eligible only for the latest published release whose `draft` and `prerelease` flags are false and
whose tag resolves to the release's exact commit. A pushed tag, draft, prerelease, pull request,
or release creation alone cannot promote production. If the active production app or a protected
dev process would be overwritten, installation remains staged until a later poll.

Enablement and bootstrap remain deliberate operator actions. See [`docs/local-ci.md`](local-ci.md)
and [`docs/dev-install.md`](dev-install.md) for commands and installation boundaries.
