#!/usr/bin/bash

function fmt() {
    for dir in "$@"
    do
        pushd $dir
        printf "\e[1;32mFormatting\e[0m $dir\n"
        cargo fmt
        popd
    done
}

fmt virtio-core \
    virtio-netd \
    virtio-blkd \
    virtio-gpud \
    inputd
