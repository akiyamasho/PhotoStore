# Release playbook

The release playbook shared by every crafting app lives in craftrules:
[`../craftrules/release/playbook.md`](../../craftrules/release/playbook.md)
(`storytold/craftrules`, a sibling checkout; the repository is not public). It is the canonical copy; don't duplicate it here.

PhotoStore is its reference implementation (`.github/workflows/release.yml`, `packaging/`,
`cargo xtask version`). PhotoStore-specific details are in [releasing.md](releasing.md).
