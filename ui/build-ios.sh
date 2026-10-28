#!/bin/sh
# Builds one of chainv4's egui apps for iOS, into APP/target/ios: the .app, from the app's binary,
# an Info.plist and its icons, for the iOS Simulator or for an iPhone.
#
#   --sim       for the Simulator, installed and started in the one booted, or else a new iPhone
#   --ipa       for an iPhone, unsigned, packed as APP/target/ios/NAME.ipa: AltStore or Sideloadly
#               sign it with your Apple ID as they install it
#   --provision TEAM
#               once, with the iPhone connected: has Xcode make the Apple Development certificate
#               and the provisioning profile for the app's bundle id under the team TEAM, your Apple
#               ID's (Xcode, Settings, Accounts), building a stub app it signs itself
#   --install   for an iPhone, signed with that certificate and profile, and installed on the one
#               connected; a free Apple ID's profile lasts 7 days, then --provision again
#
#   ui/build-ios.sh APP [--debug] (--sim | --ipa | --provision TEAM | --install)...
#
# APP is the app's folder, wallet or explorer; wallet/build-ios.sh and explorer/build-ios.sh call
# this with their own.
set -eu

usage() {
    sed -n '2,18p' "$0" | cut -c3-
}

if [ $# -eq 0 ] || [ "$1" = -h ] || [ "$1" = --help ]; then
    usage
    exit 0
fi

app=$(cd "$1" && pwd)
shift
cd "$app"
# The binary the app runs as, from its [[bin]], and what it is called.
bin=$(sed -n '/^\[\[bin\]\]/,/^name/s/^name = "\(.*\)"$/\1/p' Cargo.toml | head -n 1)
name="chainv4 $bin"
bundle_id="org.chainv4.$bin"
profile=release
sim=
ipa=
install=
team=

while [ $# -gt 0 ]; do
    case "$1" in
        --debug) profile=debug ;;
        --sim) sim=1 ;;
        --ipa) ipa=1 ;;
        --install) install=1 ;;
        --provision) team=$2; shift ;;
        -h | --help) usage; exit 0 ;;
        *) echo "unknown argument $1; see --help" >&2; exit 1 ;;
    esac
    shift
done

if [ -z "$sim$ipa$install$team" ]; then
    echo "say --sim, --ipa, --provision TEAM or --install; see --help" >&2
    exit 1
fi

cargo_profile=--release
[ "$profile" = debug ] && cargo_profile=

# The .app for a platform: its binary, Info.plist and icons.
bundle() {
    triple=$1 platform=$2 out=$3
    echo "== building $bin for $triple"
    # shellcheck disable=SC2086
    cargo build $cargo_profile --target "$triple" --bin "$bin"
    rm -rf "$out"
    mkdir -p "$out"
    cp "target/$triple/$profile/$bin" "$out/$bin"

    for size in 120 180 152 167; do
        sips -z "$size" "$size" assets/icon.png --out "$out/AppIcon$size.png" > /dev/null
    done

    cat > "$out/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key><string>$bundle_id</string>
    <key>CFBundleExecutable</key><string>$bin</string>
    <key>CFBundleName</key><string>$name</string>
    <key>CFBundleDisplayName</key><string>$name</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleShortVersionString</key><string>0.1.0</string>
    <key>CFBundleVersion</key><string>1</string>
    <key>CFBundleSupportedPlatforms</key><array><string>$platform</string></array>
    <key>CFBundleIcons</key><dict><key>CFBundlePrimaryIcon</key><dict><key>CFBundleIconFiles</key><array><string>AppIcon120</string><string>AppIcon180</string></array></dict></dict>
    <key>CFBundleIcons~ipad</key><dict><key>CFBundlePrimaryIcon</key><dict><key>CFBundleIconFiles</key><array><string>AppIcon152</string><string>AppIcon167</string></array></dict></dict>
    <key>LSRequiresIPhoneOS</key><true/>
    <key>MinimumOSVersion</key><string>16.0</string>
    <key>UIDeviceFamily</key><array><integer>1</integer><integer>2</integer></array>
    <key>UIRequiredDeviceCapabilities</key><array><string>arm64</string></array>
    <key>UILaunchScreen</key><dict/>
    <key>UIApplicationSceneManifest</key><dict><key>UIApplicationSupportsMultipleScenes</key><false/><key>UISceneConfigurations</key><dict><key>UIWindowSceneSessionRoleApplication</key><array><dict><key>UISceneConfigurationName</key><string>Default</string><key>UISceneDelegateClassName</key><string>Chainv4SceneDelegate</string></dict></array></dict></dict>
    <key>UIStatusBarStyleDefault</key><string>UIStatusBarStyleDefault</string>
    <key>UISupportedInterfaceOrientations</key><array><string>UIInterfaceOrientationPortrait</string><string>UIInterfaceOrientationLandscapeLeft</string><string>UIInterfaceOrientationLandscapeRight</string></array>
    <key>NSCameraUsageDescription</key><string>To read the codes another device shows, such as a request to sign, and to take a photo you choose to send.</string>
    <key>NSFaceIDUsageDescription</key><string>To unlock the wallet, and to confirm what you send, without typing your password.</string>
    <key>NSLocalNetworkUsageDescription</key><string>To reach a chainv4 node on your own network.</string>
</dict>
</plist>
EOF
}

if [ -n "$sim" ]; then
    out="target/ios/simulator/$bin.app"
    bundle aarch64-apple-ios-sim iPhoneSimulator "$out"
    # A booted Simulator, or a new iPhone's.
    device=$(xcrun simctl list devices booted | sed -n 's/.*(\([0-9A-F-]\{36\}\)) (Booted).*/\1/p' | head -n 1)

    if [ -z "$device" ]; then
        device=$(xcrun simctl list devices available | grep -E '^ +iPhone' | head -n 1 | sed -n 's/.*(\([0-9A-F-]\{36\}\)).*/\1/p')
        [ -n "$device" ] || { echo "no iPhone Simulator: add one in Xcode, Window, Devices and Simulators" >&2; exit 1; }
        xcrun simctl boot "$device"
    fi

    open -b com.apple.iphonesimulator 2> /dev/null || true
    # A first boot takes a while: install once it is done.
    xcrun simctl bootstatus "$device" -b > /dev/null
    xcrun simctl install "$device" "$out"
    xcrun simctl launch "$device" "$bundle_id"
    echo "== $name runs in the Simulator $device"
fi

if [ -n "$team" ]; then
    # A stub app of the same bundle id, which Xcode signs itself, registering the phone connected
    # and making the certificate and profile on the way.
    command -v xcodegen > /dev/null || { echo "--provision needs XcodeGen: brew install xcodegen" >&2; exit 1; }
    stub=target/ios/provision
    rm -rf "$stub"
    mkdir -p "$stub/Stub"
    printf 'import UIKit\n@main class Stub: UIResponder, UIApplicationDelegate {}\n' > "$stub/Stub/Stub.swift"
    cat > "$stub/project.yml" <<SPEC
name: Provision
options: { bundleIdPrefix: org.chainv4 }
targets:
  Provision:
    type: application
    platform: iOS
    deploymentTarget: "16.0"
    sources: [Stub]
    settings:
      PRODUCT_BUNDLE_IDENTIFIER: $bundle_id
      DEVELOPMENT_TEAM: $team
      CODE_SIGN_STYLE: Automatic
      GENERATE_INFOPLIST_FILE: YES
SPEC
    (cd "$stub" && xcodegen generate --quiet)
    xcodebuild -project "$stub/Provision.xcodeproj" -scheme Provision -destination 'generic/platform=iOS' -allowProvisioningUpdates -quiet build
    echo "== provisioned $bundle_id under team $team: now --install"
fi

if [ -n "$ipa$install" ]; then
    out="target/ios/iphone/Payload/$bin.app"
    bundle aarch64-apple-ios iPhoneOS "$out"
fi

if [ -n "$ipa" ]; then
    rm -f "target/ios/$bin.ipa"
    (cd target/ios/iphone && zip -qr "../$bin.ipa" Payload)
    echo "== target/ios/$bin.ipa: unsigned; AltStore or Sideloadly sign it as they install it"
fi

if [ -n "$install" ]; then
    # The provisioning profile Xcode made for this bundle id, the newest, and the certificate it
    # names, in the login Keychain.
    profiles="$HOME/Library/Developer/Xcode/UserData/Provisioning Profiles"
    found=
    for candidate in "$profiles"/*.mobileprovision "$HOME/Library/MobileDevice/Provisioning Profiles"/*.mobileprovision; do
        [ -f "$candidate" ] || continue

        if security cms -D -i "$candidate" 2> /dev/null | grep -q "<string>[A-Z0-9]*\.$bundle_id</string>"; then
            found=$candidate
        fi
    done

    [ -n "$found" ] || {
        echo "no provisioning profile for $bundle_id: make one once in Xcode with your Apple ID (docs/wallet.md, iOS)" >&2
        exit 1
    }

    security cms -D -i "$found" > target/ios/profile.plist
    /usr/libexec/PlistBuddy -x -c 'Print :Entitlements' target/ios/profile.plist > target/ios/entitlements.plist
    identity=$(security find-identity -v -p codesigning | sed -n 's/.*) \([0-9A-F]\{40\}\) "Apple Development.*/\1/p' | head -n 1)
    [ -n "$identity" ] || { echo "no Apple Development certificate: sign in to Xcode with your Apple ID" >&2; exit 1; }
    cp "$found" "$out/embedded.mobileprovision"
    codesign --force --sign "$identity" --entitlements target/ios/entitlements.plist --timestamp=none "$out"
    phone=$(xcrun devicectl list devices 2> /dev/null | awk '/connected/ && /iPhone|iPad/ {print $3; exit}')
    [ -n "$phone" ] || { echo "no iPhone connected: plug it in, trust this Mac, and turn on Developer Mode" >&2; exit 1; }
    xcrun devicectl device install app --device "$phone" "$out"
    echo "== $name installed on $phone; open it from the home screen"
fi
