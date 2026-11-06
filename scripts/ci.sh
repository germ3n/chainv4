#!/bin/bash
# Runs what .github/workflows/test.yml runs, on this machine, as the workflows are started by hand to
# save runner minutes, plus the wallet's store build and the iOS checks. It never runs a GPU kernel,
# as the workflow's runners cannot and a laptop would: chainv4-gpu stays out of the workspace's tests,
# and the matrix's row with the gpu feature is linted and compiled but not run.
#
#   scripts/ci.sh [--list] [--from N]
#
# It goes on past a step that fails, says each one's result and where its output is, and ends with the
# ones that failed. --from N picks up at step N of --list, as the matrix is long. CI_LOGS names the
# folder for each step's output. CI_MATRIX_ONLY=1 and CI_NO_MATRIX=1 narrow it to or from the matrix.
set -u

cd "$(dirname "$0")/.."

export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR=never

logs=${CI_LOGS:-$(mktemp -d)}
mkdir -p "$logs"
host=$(rustc -vV | sed -n 's/^host: //p')

# The one test of chainv4-gpu that runs no kernel.
gpu_test=tests::a_gpu_is_chosen_once_natively_and_once_through_opengl

# What test.yml's features job lists, and the row that would run kernels here.
matrix=(
    sha256,ed25519
    blake3,edblake
    keccak256,blake2s,blake2b,ed25519
    sha256,secp256k1
    blake3,ed25519,mnemonic
    keccak256,edblake,secp256k1,mnemonic
    blake3,bitcoin
    sha256,ed25519,bitcoin,mnemonic
    blake3,swap
    lvm,sha256,ed25519,swap
    lvm,sha256,ed25519
    wasm,keccak256,ed25519
    lvm,wasm,blake2s,edblake
    node,blake2b,ed25519
    http,sha256,edblake
    stream,blake2b,edblake
    client,keccak256,edblake
    node,blake3,secp256k1
    http,client,blake3,ed25519,finality-ffg
    http,client,sha256,edblake,finality-authority
    http,client,keccak256,ed25519,finality-depth
    http,client,blake3,ed25519,finality-ffg,governance-electorate
    http,client,sha256,edblake,finality-authority,governance-council
    node,blake3,ed25519,gpu
    release,blake3,ed25519
    updates,http,sha256,edblake
    http,blake3,ed25519,finality-ffg,anchoring-btc
    http,sha256,ed25519,finality-ffg,anchoring-eth
)

names=()
commands=()

step() {
    names+=("$1")
    commands+=("$2")
}

if [ -z "${CI_MATRIX_ONLY:-}" ]; then
    step "root clippy" "cargo clippy --workspace --all-targets --locked -- -D warnings"
    step "root tests" "cargo test --workspace --exclude chainv4-gpu --locked"
    step "gpu's test with no kernel" "cargo test -p chainv4-gpu --lib --locked -- --exact $gpu_test"
    step "contracts clippy" "cargo clippy --manifest-path contracts/Cargo.toml --target wasm32-unknown-unknown --locked -- -D warnings"
    step "sdk tests" "cargo test --manifest-path contracts/Cargo.toml -p chainv4-sdk --locked --target $host"

    step "ui clippy" "cargo clippy --manifest-path ui/Cargo.toml --all-targets --features blocks --locked -- -D warnings"
    step "ui tests" "cargo test --manifest-path ui/Cargo.toml --features camera,blocks --locked"
    step "ui clippy, web" "cargo clippy --manifest-path ui/Cargo.toml --target wasm32-unknown-unknown --features blocks --locked -- -D warnings"
    step "ui clippy, android" "cargo clippy --manifest-path ui/Cargo.toml --target aarch64-linux-android --no-default-features --features camera --locked -- -D warnings"

    step "explorer clippy" "cargo clippy --manifest-path explorer/Cargo.toml --all-targets --locked -- -D warnings"
    step "explorer tests" "cargo test --manifest-path explorer/Cargo.toml --locked"
    step "explorer clippy, web" "cargo clippy --manifest-path explorer/Cargo.toml --target wasm32-unknown-unknown --locked -- -D warnings"
    step "explorer web bundle" "(cd explorer && trunk build --release --locked)"
    step "explorer check, ios" "cargo check --manifest-path explorer/Cargo.toml --target aarch64-apple-ios --locked"

    step "wallet clippy" "cargo clippy --manifest-path wallet/Cargo.toml --all-targets --locked -- -D warnings"
    step "wallet tests" "cargo test --manifest-path wallet/Cargo.toml --locked"
    step "wallet clippy, no default features" "cargo clippy --manifest-path wallet/Cargo.toml --all-targets --locked --no-default-features -- -D warnings"
    step "wallet clippy, store" "cargo clippy --manifest-path wallet/Cargo.toml --all-targets --locked --no-default-features --features store -- -D warnings"
    step "wallet clippy, web" "cargo clippy --manifest-path wallet/Cargo.toml --target wasm32-unknown-unknown --locked -- -D warnings"
    step "wallet clippy, web store" "cargo clippy --manifest-path wallet/Cargo.toml --target wasm32-unknown-unknown --locked --no-default-features --features store -- -D warnings"
    step "wallet web bundle" "(cd wallet && trunk build --release --locked)"
    step "wallet check, ios" "cargo check --manifest-path wallet/Cargo.toml --target aarch64-apple-ios --locked"
    step "wallet check, ios simulator" "cargo check --manifest-path wallet/Cargo.toml --target aarch64-apple-ios-sim --locked"
fi

if [ -z "${CI_NO_MATRIX:-}" ]; then
    for features in "${matrix[@]}"; do
        step "features $features: clippy" "cargo clippy --all-targets --locked --no-default-features --features $features -- -D warnings"

        case ",$features," in
            *,gpu,*) step "features $features: tests compiled, not run, for its kernels" "cargo test --no-run --locked --no-default-features --features $features" ;;
            *) step "features $features: tests" "cargo test --locked --no-default-features --features $features" ;;
        esac
    done
fi

from=1

while [ $# -gt 0 ]; do
    case "$1" in
        --list)
            for at in "${!names[@]}"; do echo "$((at + 1)). ${names[$at]}"; done
            exit 0
            ;;
        --from) from=$2; shift 2 ;;
        -h | --help) sed -n '2,12p' "$0" | cut -c3-; exit 0 ;;
        *) echo "unknown argument $1; see --help" >&2; exit 2 ;;
    esac
done

failed=()
total=${#names[@]}

for at in "${!names[@]}"; do
    number=$((at + 1))
    [ "$number" -lt "$from" ] && continue

    # The matrix builds far more than it can keep on a tight disk.
    free=$(df -g . | awk 'NR == 2 { print $4 }')
    if [ "$free" -lt 6 ]; then
        echo "   under 6 GB free: removing target/debug"
        rm -rf target/debug
    fi

    name=${names[$at]}
    log="$logs/$(printf '%02d' "$number").log"
    began=$SECONDS

    if ${commands[$at]} > "$log" 2>&1; then
        result=ok
    else
        result=FAILED
        failed+=("$number. $name: $log")
    fi

    passed=$(grep -E '^test result: ' "$log" | awk '{ sum += $4 } END { if (NR) print sum }')
    echo "$number/$total $result $name ($((SECONDS - began)) s${passed:+, $passed tests passed})"
done

echo
echo "output in $logs"

if [ ${#failed[@]} -gt 0 ]; then
    echo "${#failed[@]} failed:"
    printf '  %s\n' "${failed[@]}"
    exit 1
fi

echo "all passed"
