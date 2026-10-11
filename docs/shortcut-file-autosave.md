# Native shortcut-file autosave

PhotoStore can keep the accepted keyboard and toolbar shortcut set in a separate native JSON file.
Open Edit → Keyboard Shortcuts and Menus and enter an absolute path ending in `.json` in
“Automatically save shortcuts to”. Leave it empty to disable the feature. The setting is stored in
PhotoStore preferences and is not hard-coded to a personal folder, so it can point to an iCloud
directory or a Git-managed configuration file on a native install.

The file uses the existing version-1 `photostore-shortcuts` format and includes primary overrides,
empty primary tombstones, and alternate bindings. Draft changes are exported only after OK; Cancel
does not write either preferences or the shortcut file. The parent directory must already exist.
PhotoStore writes the native preferences atomically first, then writes the shortcut file atomically.
If the export fails, preferences remain saved and the status message identifies the configured path
and error; the export remains pending for the existing bounded retry. Unrelated preference changes
do not rewrite a shortcut file whose path and encoded contents have already succeeded.

The engine validates the value without filesystem access so it remains usable by the web and WASM
builds. Automation cannot set `shortcutFile`, because it is an ambient native filesystem path;
interactive native settings are still allowed.
