#!/usr/bin/bash

pushd virtio-core
cargo fmt
popd

pushd virtio-netd
cargo fmt
popd

pushd virtio-blkd
cargo fmt
popd
