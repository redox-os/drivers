#!/usr/bin/bash

set -eo pipefail

function fmt() {
    for dir in "$@"
    do
        pushd $dir
        printf "\e[1;32mFormatting\e[0m $dir\n"
        if [[ "$CHECK_ONLY" -eq "1" ]]; then
            cargo fmt --check
        else
            cargo fmt
        fi
        popd
    done
}

fmt common \
    graphics/bgad \
    graphics/fbcond \
    graphics/vesad \
    graphics/virtio-gpud \
    inputd \
    net/driver-network \
    net/virtio-netd \
    pcid \
    storage/driver-block \
    virtio-core
