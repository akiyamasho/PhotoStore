# PhotoStore-dev local installation

`PhotoStore-dev.app` is the consolidated review installation for development. Build it with the
`dev-install` Cargo feature so the native app uses the consolidated identity and window name, with
isolated per-user state (`PhotoStore-dev` on macOS and Windows, `photostore-dev` on Linux).
Portable dev bundles use `PhotoStoreDevData` and dev-only portable markers, so a production marker
cannot select the production data folder.

The installer copies the shared icon and licence records, removes all document and URL declarations
from the dev `Info.plist`, signs the bundle ad hoc, and installs atomically at the selected
`/Applications/PhotoStore-dev*.app`. It never modifies or removes `/Applications/PhotoStore.app`,
does not claim file associations, and refuses to replace a running dev process. Launching is
opt-in; `--launch-background` uses macOS background opening behind the active window so an agent
does not take over the user's UI.

From the repository root, the normal consolidated install command is:

```sh
packaging/macos/install-dev.sh --shortcut-file /Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json
```

To install a previously verified feature build without rebuilding:

```sh
packaging/macos/install-dev.sh --skip-build --binary /Users/computer/Desktop/dev/PhotoStore/target/debug/photostore
```

The script requires the binary's embedded `PhotoStore-dev build` marker, records the source SHA
and working-tree hash in the bundle metadata, and uses one shared Cargo target with one build job.
It does not launch the app by default. During installation, the script may seed a missing dev
`preferences.json` from the read-only production preferences file into the selected goal directory.
Pass `--shortcut-file /Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json` to seed the
non-racing shortcut exporter; the seed also forces `disableRecording` true. Existing dev
preferences are retained; production preferences and recovery/private artwork are never copied or
changed. If `CRAFT_FONTS_DIR` points to the existing local craft-fonts checkout, its OFL records
are copied with the bundle. Promotion to production is a separate, explicit user-requested release
step after review.

## Consolidated daily edit loop

The default loop uses the single consolidated `PhotoStore-dev` identity and the shared Cargo target:

```sh
./dev --watch
```

The watcher builds changed inputs incrementally and asks the isolated dev process to close only
when no document tabs, background jobs, or dialogs are open. A failed build leaves the current
process alive. Any open tab, including a saved tab, defers reload; save and close every tab and
resolve jobs or dialogs so the queued reload applies automatically. The watcher does not force-kill
or discard state and does not perform hot module replacement. Each edit uses this same dev
installation and does not require a new pull request or bundle reinstall.

An explicitly requested independent goal may use a distinct `PhotoStore-dev-<goal>` bundle,
process, configuration, recovery, presets, logs, UI state, and shortcut export. Development uses
`/Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json`; the production
`photostore_keys.json` remains untouched.

Production remains at `/Applications/PhotoStore.app` until an explicitly reviewed release is
tagged, published, and verified. The optional local CI watcher may install a published stable
release, but a branch, pushed tag, draft, prerelease, or release creation alone never promotes it.
