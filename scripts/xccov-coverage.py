#!/usr/bin/env python3
"""Fail if total line coverage in an .xcresult bundle is below a threshold.

Usage: xccov-coverage.py <Result.xcresult> <min_percent>
Parses `xcrun xccov view --report --json` and sums executable/covered lines
across all targets.
"""

import json
import subprocess
import sys


def main() -> int:
    bundle, threshold = sys.argv[1], float(sys.argv[2])
    raw = subprocess.run(
        ["xcrun", "xccov", "view", "--report", "--json", bundle],
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    executable = covered = 0
    for target in json.loads(raw).get("targets", []):
        for file in target.get("files", []):
            executable += file.get("executableLines", 0)
            covered += file.get("coveredLines", 0)
    percent = 100.0 * covered / executable if executable else 100.0
    print(
        f"Swift line coverage: {covered}/{executable} = {percent:.2f}% (min {threshold:.2f}%)"
    )
    return 0 if percent >= threshold else 1


if __name__ == "__main__":
    sys.exit(main())
