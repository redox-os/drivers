#!/usr/bin/bash

set -eo pipefail

if [[ "$CHECK_ONLY" -eq "1" ]]; then
    cargo fmt --check
else
    cargo fmt
fi