#!/bin/sh
# Builds the wallet's iOS app into wallet/target/ios: see ui/build-ios.sh, which builds both apps the
# same way.
#
#   wallet/build-ios.sh [--debug] [--store] (--sim | --ipa | --install)...
exec "$(dirname "$0")/../ui/build-ios.sh" "$(dirname "$0")" "$@"
