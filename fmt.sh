#!/usr/bin/bash

set -eo pipefail

printf "\e[1;32mFormatting\e[0m $dir\n"
if [[ "$CHECK_ONLY" -eq "1" ]]; then
    cargo fmt --all --check
else
    cargo fmt --all
fi
