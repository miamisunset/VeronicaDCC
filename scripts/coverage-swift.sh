#!/bin/bash
# Enforce Swift line coverage >= 85% for the Veronica app + tests.
# Usage: ./scripts/coverage-swift.sh
set -euo pipefail
cd "$(dirname "$0")/.."
RESULT_BUNDLE="$(mktemp -d)/Veronica.xcresult"
xcodebuild -project Veronica.xcodeproj -scheme Veronica \
  -destination 'platform=macOS' -configuration Debug \
  -resultBundlePath "$RESULT_BUNDLE" -enableCodeCoverage YES test
python3 scripts/xccov-coverage.py "$RESULT_BUNDLE" 85
