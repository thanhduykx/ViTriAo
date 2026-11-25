#!/usr/bin/env bash
# Build app và đóng gói thành IPA chưa ký (SideStore/iloader/Sideloadly sẽ ký lại khi cài).
set -euo pipefail
cd "$(dirname "$0")/.."

ROOT="$PWD"
OUT="$ROOT/build"
rm -rf "$OUT"
mkdir -p "$OUT"

xcodebuild \
  -project ViTriAo.xcodeproj \
  -scheme ViTriAo \
  -configuration Release \
  -sdk iphoneos \
  -destination 'generic/platform=iOS' \
  -derivedDataPath "$OUT/DerivedData" \
  CODE_SIGNING_ALLOWED=NO \
  CODE_SIGNING_REQUIRED=NO \
  CODE_SIGN_IDENTITY="" \
  build

APP="$OUT/DerivedData/Build/Products/Release-iphoneos/ViTriAo.app"
test -d "$APP"

mkdir -p "$OUT/Payload"
cp -R "$APP" "$OUT/Payload/"
(cd "$OUT" && zip -qry ViTriAo.ipa Payload)
rm -rf "$OUT/Payload"
ls -lh "$OUT/ViTriAo.ipa"
