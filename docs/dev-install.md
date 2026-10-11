# PhotoStore-dev local installation

`PhotoStore-dev.app` is the review installation for completed development milestones. Build it
with the `dev-install` Cargo feature so the native app uses the `ai.photostore.app.dev` identity,
the `PhotoStore-dev` window name, and isolated per-user state (`PhotoStore-dev` on macOS and
Windows, `photostore-dev` on Linux). Portable dev bundles use `PhotoStoreDevData` and dev-only
portable markers, so a production marker cannot select the production data folder.

The installer copies the shared icon and licence records, removes all document and URL declarations
from the dev `Info.plist`, signs the bundle ad hoc, and installs atomically at
`/Applications/PhotoStore-dev.app`. It never modifies or removes `/Applications/PhotoStore.app`,
does not claim file associations, and refuses to replace a running dev process. Launching is
opt-in; `--launch-background` uses macOS background/hidden opening so an agent does not take over
the user's UI.

From the repository root, the normal later command is:

```sh
packaging/macos/install-dev.sh --shortcut-file /Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json
```

To install a previously verified feature build without rebuilding:

```sh
packaging/macos/install-dev.sh --skip-build --binary target/debug/photostore
```

The script requires the binary's embedded `PhotoStore-dev build` marker, records the source SHA
and working-tree hash in the bundle metadata, and uses one shared Cargo target with one build job.
It does not launch the app by default. During installation, the script may seed a missing dev
`preferences.json` from the read-only production preferences file. Pass
`--shortcut-file /Users/computer/Desktop/dev/tools/Animation/photostore_keys.dev.json` to seed the
non-racing shortcut exporter; the seed also forces `disableRecording` true. Existing dev
preferences are retained; production preferences and recovery/private artwork are never copied or
changed. If `CRAFT_FONTS_DIR` points to the existing local craft-fonts checkout, its OFL records
are copied with the bundle. Promotion to production is a separate, explicit user-requested release
step after review.
