#!/bin/bash
# Container entry point: release-build, then run the given script with the rest of the arguments.
set -eu
cargo build --release 2>&1 | tail -3
exec "$@"
