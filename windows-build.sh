#!/bin/bash
# should not use my nix store, please
CROSS_CONTAINER_ENGINE=podman NIX_STORE="/var/empty" cross build --release --target x86_64-pc-windows-gnu