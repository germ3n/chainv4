#!/bin/sh
# Builds the explorer's iOS app into explorer/target/ios: see ui/build-ios.sh, which builds both apps the
# same way.
#
#   explorer/build-ios.sh [--debug] (--sim | --ipa | --install)...
exec "$(dirname "$0")/../ui/build-ios.sh" "$(dirname "$0")" "$@"
