#!/bin/sh
# Builds the validity proofs' programs for SP1 into elf/ (chainv4-proof-program,
# chainv4-finality-program and chainv4-headers-program), as every prover must for the same program
# ids: SP1's toolchain from sp1up --c-toolchain under $SP1_DIR (~/.sp1), its
# RISC-V GCC for Luau's C++, climits included (Luau leaves it to Apple's clang), and unwind tables
# for Rust frames, which a Lua error raised from a Rust callback unwinds through.
set -e
SP1_DIR="${SP1_DIR:-$HOME/.sp1}"
export PATH="$PATH:$SP1_DIR/bin:$SP1_DIR/riscv/bin"
export CC_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-gcc
export CXX_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-g++
export AR_riscv64im_succinct_zkvm_elf=riscv64-unknown-elf-ar
export LUAU_CXXFLAGS="-include climits"
cd "$(dirname "$0")/program"
cargo prove build --rustflags="-Cforce-unwind-tables=yes" --output-directory ../elf
