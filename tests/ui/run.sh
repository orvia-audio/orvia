#!/bin/sh
# Run the screenshot comparison with its own X11 authority file and display.
set -eu
auth_file=$(mktemp)
trap 'rm -f -- "$auth_file"' EXIT
xvfb-run -a -f "$auth_file" -s "-screen 0 1280x1024x24 -dpi 96" \
    dbus-run-session -- /usr/bin/python3 "$(dirname "$0")/snapshots.py" "$@"
