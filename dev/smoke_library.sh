#!/usr/bin/env bash
# Use the shared compiled Gradle fixture and artifact capture for the library smoke.
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
exec "$repo/dev/ktlsp-harness.sh" library "$@"
