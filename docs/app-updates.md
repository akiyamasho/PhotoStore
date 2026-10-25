# In-app updates

The updater is included in the published PhotoStore 0.18.0 Apple Silicon macOS app. **Help → Check
for Updates…** checks the fixed private repository `akiyamasho/PhotoStore` for its newest stable
semantic version. GitHub authentication comes from the user's installed GitHub CLI (`gh`) and existing
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
signing identity. Production packages now use the persistent PhotoStore signing identity by
default, so updates and reinstalls can retain the same code-signing identity and designated
requirement. macOS still decides whether a privacy prompt is needed; moving from an older ad-hoc
signed installation to the persistent identity may require consent again for individual protected
folders or services. Native prompt behavior was not manually tested for this migration. Keep the
same certificate, bundle identifier, and designated requirement for later releases. Replacing the
certificate can cause macOS to treat the app as a different requester and ask for access again.

The default local certificate is a stable signing identity, not a Developer ID certificate. Local
packages signed with it are not notarized and are intended for this machine. The release packager
does not create or rotate the certificate automatically; it fails before building if its public
configuration or code-signing identity is unavailable.

Intel macOS, Windows, and Linux do not support in-app installation. The published PhotoStore 0.17.1
package does not include the updater. Version 0.18.0 was built, verified, installed locally, and
published from source commit
[`1da682ebd22981c39ef60b9df1d7ef12c3a4f04a`](https://github.com/akiyamasho/PhotoStore/tree/1da682ebd22981c39ef60b9df1d7ef12c3a4f04a).
The install used the local release artifact; GitHub browser download and the in-app self-update
flow have not been exercised. See the [0.18.0 release record](releases/0.18.0.md) for artifact
digests and verification details.
