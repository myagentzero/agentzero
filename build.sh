#!/bin/bash
export CARGO_BUILD_JOBS=1
export PATH="/opt/gcc-7.3.0-arm-linux-musleabihf/bin:$PATH"
export CC_armv7_unknown_linux_musleabihf=arm-linux-musleabihf-gcc
export CXX_armv7_unknown_linux_musleabihf=arm-linux-musleabihf-g++
export AR_armv7_unknown_linux_musleabihf=arm-linux-musleabihf-ar
export CARGO_TARGET_ARMV7_UNKNOWN_LINUX_MUSLEABIHF_LINKER=arm-linux-musleabihf-gcc
rustup target add armv7-unknown-linux-musleabihf

cd web/
NODE_ENV=production npm run build
cd ..
cargo build --release --target armv7-unknown-linux-musleabihf --features "peripheral-rpi,hardware,browser-native,skill-creation"
