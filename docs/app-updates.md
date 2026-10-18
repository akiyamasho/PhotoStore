# In-app updates

The source implementation described here is intended for the next macOS package; the published
PhotoStore 0.17.1 app does not contain this updater. In that package, **Help → Check for Updates…**
checks the fixed private repository `akiyamasho/PhotoStore` for its newest stable semantic
version. GitHub authentication comes from the user's installed GitHub CLI (`gh`) and existing
`gh auth login` session. The updater searches PATH and common Homebrew locations; it never stores,
requests, prints, or sends a token itself. The current updater implementation supports the
published Apple Silicon (`aarch64`) package only. A
development build, a build without the production `ai.photostore.app` identity, a disk-image
launch, and a translocated app cannot update themselves.

When a release is available, **Download Update** downloads only the architecture-matched
PhotoStore DMG. The repository and asset name are fixed in the app. Before the UI reports the
update ready, the download size and GitHub-provided SHA-256 digest are checked, the DMG is mounted
read-only, the bundle identity and version are checked, and macOS strict code-signature
verification passes. The app is staged beside private updater data. Installation is offered only
after this work and is refused while any document tab or background job is open.

Choosing **Install and Quit** starts a small helper from the running executable. It waits for the
app process to exit naturally, revalidates the staged bundle and the current install metadata,
then renames the original bundle to a recoverable sibling backup and activates the staged app on
the same filesystem. Failed activation restores the original when possible. It never force-kills
PhotoStore, signs or modifies the new app after verification, or replaces a bundle on a mounted
volume. The helper requests a fresh app launch and retains the sibling backup; the OS accepting
that launch request does not prove PhotoStore completed startup. Helper failures are shown in the
updater status on the next launch, with the recovery location when rollback could not restore the
original path.

For paths present in both versions, the updater preserves the installed bundle's POSIX mode,
owner and group, file flags, ACL state, and extended attributes, including the absence of
`com.apple.quarantine`. It excludes code-signature attributes because those belong to the new
signed files. If it cannot establish or preserve metadata safely, installation stops before the
original bundle is renamed. The updater does not migrate macOS TCC privacy grants, keychain
access, or other OS-managed identity state; those permissions are controlled by macOS and code
signing identity.

Intel macOS, Windows, and Linux do not support in-app installation. The published PhotoStore
0.17.1 package does not include the updater; this documentation describes the source change for a
future macOS package.
