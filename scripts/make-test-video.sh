#!/usr/bin/env bash
# Generates the short WebM clip used by scripts/ui-smoke.mjs to test the
# video keyboard shortcuts (←/→ seek 5s, Space play/pause). Requires ffmpeg.
set -euo pipefail

output="${1:-/tmp/agentgod-ui-test.webm}"

ffmpeg -y -loglevel error \
  -f lavfi -i "testsrc2=size=160x120:rate=10" \
  -t 6 -c:v libvpx -b:v 200k -an \
  "$output"

echo "wrote $output"
