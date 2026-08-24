#!/usr/bin/env python3
"""Enforce the workspace's library-first dependency direction."""

import json
import subprocess
import sys


def main() -> int:
    result = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--no-deps"],
        check=True,
        capture_output=True,
        text=True,
    )
    packages = {package["name"]: package for package in json.loads(result.stdout)["packages"]}
    library = packages.get("music-sync")
    cli = packages.get("music-sync-cli")
    if library is None or cli is None:
        print("architecture check failed: expected workspace packages are missing", file=sys.stderr)
        return 1

    library_dependencies = {dependency["name"] for dependency in library["dependencies"]}
    cli_dependencies = {dependency["name"] for dependency in cli["dependencies"]}
    if "music-sync-cli" in library_dependencies:
        print("architecture check failed: music-sync depends on music-sync-cli", file=sys.stderr)
        return 1
    if "music-sync" not in cli_dependencies:
        print("architecture check failed: music-sync-cli must depend on music-sync", file=sys.stderr)
        return 1

    print("architecture check passed: music-sync-cli -> music-sync")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

