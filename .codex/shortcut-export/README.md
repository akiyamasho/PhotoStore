# PhotoStore shortcut export

This one-way exporter mirrors the saved PhotoStore preferences into
`/Users/computer/Desktop/dev/tools/Animation/photostore_keys.json` using the
native `photostore-shortcuts` JSON format, version 1. The preferences file is
the source of truth; external edits to the exported file are not imported.

The LaunchAgent runs at login, watches the PhotoStore preferences directory,
and checks every 30 seconds as a fallback. Errors go to the LaunchAgent log
and never overwrite the destination with incomplete data.
