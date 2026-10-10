# Release playbook

Use the repository's [release procedure](releasing.md) as the source of truth. Before a release,
update the version with `cargo xtask version`, review the generated metadata and changelog, and
build the packages described in `packaging/`. Verify package contents, checksums, signatures,
bundle identifiers, and baked version/SHA values before publishing the GitHub release. Record the
exact tag, commit, artifacts, and local install evidence in `docs/photostore-goal-ledger.md`.

This repository owns its release procedure and does not depend on an external playbook or upstream
community service.
