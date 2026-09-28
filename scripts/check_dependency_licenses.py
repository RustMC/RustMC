#!/usr/bin/env python3
"""Fail CI when a locked dependency's declared license needs review.

This checks package metadata, not source archives or release notice completeness.
"""

import json
import subprocess
import sys

# Names and SPDX expressions reviewed for the current lockfile. New packages or
# changed declarations require a human review and an update to docs/PROVENANCE.md.
REVIEWED = {
    "equivalent": "Apache-2.0 OR MIT",
    "errno": "MIT OR Apache-2.0",
    "hashbrown": "MIT OR Apache-2.0",
    "indexmap": "Apache-2.0 OR MIT",
    "libc": "MIT OR Apache-2.0",
    "memchr": "Unlicense OR MIT",
    "proc-macro2": "MIT OR Apache-2.0",
    "quote": "MIT OR Apache-2.0",
    "serde": "MIT OR Apache-2.0",
    "serde_core": "MIT OR Apache-2.0",
    "serde_derive": "MIT OR Apache-2.0",
    "serde_spanned": "MIT OR Apache-2.0",
    "signal-hook": "MIT OR Apache-2.0",
    "signal-hook-registry": "MIT OR Apache-2.0",
    "syn": "MIT OR Apache-2.0",
    "toml": "MIT OR Apache-2.0",
    "toml_datetime": "MIT OR Apache-2.0",
    "toml_edit": "MIT OR Apache-2.0",
    "toml_write": "MIT OR Apache-2.0",
    "unicode-ident": "(MIT OR Apache-2.0) AND Unicode-3.0",
    "windows-link": "MIT OR Apache-2.0",
    "windows-sys": "MIT OR Apache-2.0",
    "winnow": "MIT",
}


def review_packages(packages: list[dict], reviewed: dict[str, str] = REVIEWED) -> list[str]:
    problems = []
    seen = set()
    first_party_seen = False
    for package in packages:
        name = package["name"]
        expression = package.get("license")
        if name == "rustmc-server":
            first_party_seen = True
            if expression != "Apache-2.0":
                problems.append(f"{name}: expected Apache-2.0, found {expression!r}")
            continue
        seen.add(name)
        expected = reviewed.get(name)
        if expected is None:
            problems.append(f"{name} {package['version']}: unreviewed dependency, license {expression!r}")
        elif expression != expected:
            problems.append(
                f"{name} {package['version']}: license changed from {expected!r} to {expression!r}"
            )
    if not first_party_seen:
        problems.append("rustmc-server: first-party package missing from metadata")
    for name in sorted(reviewed.keys() - seen):
        problems.append(f"{name}: reviewed entry is no longer in Cargo.lock")
    return problems


def main() -> int:
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode:
        print(result.stderr, file=sys.stderr)
        return result.returncode
    packages = json.loads(result.stdout)["packages"]
    problems = review_packages(packages)
    if problems:
        print("Dependency license review required:", file=sys.stderr)
        for problem in problems:
            print(f"- {problem}", file=sys.stderr)
        return 1
    print(f"Reviewed declared licenses for {len(packages) - 1} locked third-party packages.")
    print("This does not replace notice/source review before binary distribution.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
