#!/usr/bin/env python3
"""Keep assets for the three newest published PhotoStore releases."""

import json
import os
import re
import subprocess
import sys


MAX_RELEASES_TO_SCAN = 2_000
KEEP_RELEASES = 3


def gh_json(endpoint: str) -> object:
    result = subprocess.run(["gh", "api", endpoint], check=True, capture_output=True, text=True)
    return json.loads(result.stdout)


def main() -> int:
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    if not re.fullmatch(r"[^/]+/[^/]+", repository):
        print("GITHUB_REPOSITORY must be owner/repository", file=sys.stderr)
        return 2

    releases = []
    for page in range(1, (MAX_RELEASES_TO_SCAN // 100) + 1):
        batch = gh_json(f"repos/{repository}/releases?per_page=100&page={page}")
        if not isinstance(batch, list):
            print("GitHub returned an unexpected releases response", file=sys.stderr)
            return 2
        releases.extend(batch)
        if len(batch) < 100:
            break
    else:
        print(f"Refusing cleanup: reached scan limit of {MAX_RELEASES_TO_SCAN} releases", file=sys.stderr)
        return 2

    published = [
        release
        for release in releases
        if isinstance(release, dict) and release.get("draft") is False and isinstance(release.get("published_at"), str)
    ]
    published.sort(key=lambda release: release["published_at"], reverse=True)
    old_releases = published[KEEP_RELEASES:]
    assets = [
        (release.get("tag_name", "(unknown tag)"), asset)
        for release in old_releases
        for asset in release.get("assets", [])
        if isinstance(asset, dict) and isinstance(asset.get("id"), int)
    ]

    print(f"Keeping {min(KEEP_RELEASES, len(published))} newest published releases; removing {len(assets)} assets from {len(old_releases)} older releases.")
    for tag, asset in assets:
        asset_id = asset["id"]
        name = asset.get("name", "(unnamed asset)")
        print(f"Deleting asset {name!r} from {tag} (asset id {asset_id})")
        subprocess.run(["gh", "api", "--method", "DELETE", f"repos/{repository}/releases/assets/{asset_id}"], check=True)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except subprocess.CalledProcessError as error:
        print(f"GitHub CLI request failed with exit status {error.returncode}", file=sys.stderr)
        raise SystemExit(error.returncode or 1)
    except json.JSONDecodeError:
        print("GitHub returned invalid JSON", file=sys.stderr)
        raise SystemExit(2)
