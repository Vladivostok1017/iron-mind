#!/usr/bin/env bash
#
# build-ios.sh —— 一条命令出 iOS 包（或在模拟器里跑起来）。
#
# 和安卓那边 tools/build-android.ps1 一一对应：
#
#   安卓：NDK → .so → jniLibs → APK（自己签一张固定钥匙）
#   iOS ：cargo → .a → Xcode → .app → Payload/*.ipa（**不签**，见下）
#
# 为什么 iOS 的产物不签：签名要用**你的** Apple ID 和描述文件，那是人和机器的
# 东西，不该进仓库。安卓那张自签钥匙只证明"同一个 APK 的后续版本"，
# iOS 没有等价物——每台设备上装东西都要 Apple 发一张证书。
# 所以这里出的是**未签名 .ipa**，交给 Xcode 或侧载工具去签。
#
# 用法：
#
#   tools/build-ios.sh                     # 出未签名 .ipa → dist/
#   tools/build-ios.sh --simulator         # 编到模拟器并启动（不需要 Apple ID）
#   tools/build-ios.sh --module ../nanxun-1938/nanxun_1938.imod
#   tools/build-ios.sh --signed --install  # 用你的开发证书签，并装到连着的设备
#
# 需要：macOS + 完整 Xcode（不是 Command Line Tools）、Rust、
#       `rustup target add aarch64-apple-ios aarch64-apple-ios-sim`

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

MODULE=""
SIMULATOR=""
DO_SIGN=0
DO_INSTALL=0
SIM_NAME="iPhone 15"

while [ $# -gt 0 ]; do
  case "$1" in
    --module)    MODULE="${2:-}"; shift 2 ;;
    --module=*)  MODULE="${1#*=}"; shift ;;
    --simulator) SIMULATOR=1; shift ;;
    --simulator=*) SIMULATOR=1; SIM_NAME="${1#*=}"; shift ;;
    --signed)    DO_SIGN=1; shift ;;
    --install)   DO_INSTALL=1; DO_SIGN=1; shift ;;
    -h|--help)   sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "不认识的参数：$1（--help 看用法）" >&2; exit 2 ;;
  esac
done

# ── 前置检查：缺什么就直说，别让 Xcode 报一串天书 ──────────────────────────
if ! command -v xcodebuild >/dev/null 2>&1; then
  cat >&2 <<'MSG'
找不到 xcodebuild。装一个完整的 Xcode（App Store），然后：

  sudo xcode-select -s /Applications/Xcode.app
  xcodebuild -runFirstLaunch

只有 Command Line Tools 是**编不了 iOS 的**（没有 iOS SDK）。
MSG
  exit 1
fi
# xcode-select 还指着 Command Line Tools？那就用 DEVELOPER_DIR 指到 Xcode 上。
# 好处：**不需要 sudo**（xcode-select -s 要 root，DEVELOPER_DIR 不要）。
# 也认挂在别的卷上的 Xcode（/Volumes/*/Xcode*.app），和 docs/25 里"装到外盘"一致。
if ! xcodebuild -showsdks 2>/dev/null | grep -q "iphoneos"; then
  for cand in /Applications/Xcode*.app /Volumes/*/Xcode*.app; do
    if [ -d "$cand/Contents/Developer/Platforms/iPhoneOS.platform" ]; then
      export DEVELOPER_DIR="$cand/Contents/Developer"
      echo "▸ DEVELOPER_DIR=$DEVELOPER_DIR"
      break
    fi
  done
fi

if ! xcodebuild -showsdks 2>/dev/null | grep -q "iphoneos"; then
  echo "这台机器上没有 iOS SDK。先装完整 Xcode 并运行 xcodebuild -runFirstLaunch。" >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1 && [ ! -x "$HOME/.cargo/bin/cargo" ]; then
  echo "找不到 cargo。先装 Rust：https://rustup.rs" >&2
  exit 1
fi

TARGET_TRIPLE="aarch64-apple-ios"
[ -n "$SIMULATOR" ] && TARGET_TRIPLE="aarch64-apple-ios-sim"
if command -v rustup >/dev/null 2>&1; then
  rustup target add "$TARGET_TRIPLE" >/dev/null 2>&1 || true
fi

# 内嵌哪个战役：交给 cargo 的 build.rs（sim-bundle）去读
if [ -n "$MODULE" ]; then
  MODULE_ABS="$(cd "$(dirname "$MODULE")" && pwd)/$(basename "$MODULE")"
  export IRONMIND_BUNDLE_MODULE="$MODULE_ABS"
  echo "▸ 内嵌模块：$IRONMIND_BUNDLE_MODULE"
fi

# 版本号只有一个来源：Cargo.toml 的 workspace.package.version
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || VERSION="0.0.0"

# 工程是生成的（ios/project.yml）。装了 xcodegen 就顺手刷新一遍，
# 没装就用仓库里提交的那份 pbxproj——玩家不需要装 XcodeGen。
if command -v xcodegen >/dev/null 2>&1; then
  echo "▸ 刷新 Xcode 工程（xcodegen）"
  (cd ios && xcodegen generate --spec project.yml --project . >/dev/null)
fi

CONFIG="Release"
DEST="generic/platform=iOS"
SDK="iphoneos"
SIGN_ARGS=(CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY="")
if [ -n "$SIMULATOR" ]; then
  CONFIG="Debug"
  SDK="iphonesimulator"
  # 用 generic 而不是点名设备：点名的话，机器上没有那台模拟器就直接报错退出，
  # 而这台机器上装的是哪一代运行时（18.5 / 26.x）事先并不知道。
  DEST="generic/platform=iOS Simulator"
  SIGN_ARGS=()
elif [ "$DO_SIGN" = 1 ]; then
  SIGN_ARGS=(-allowProvisioningUpdates)
fi

# DerivedData 也可以指到别的卷（默认在仓库里，几 GB）：
#   IRONMIND_DERIVED=/Volumes/Dev/ironmind-xcode tools/build-ios.sh
DERIVED="${IRONMIND_DERIVED:-$ROOT/build/ios-$CONFIG}"

# App Store 版 Xcode 装完之后，iOS **平台包**（设备支持 + 模拟器运行时）是另下的。
# 没下的时候 `-scheme 加 -destination` 会被"iOS x.y is not installed"挡下来，
# 但 `-target -sdk iphoneos` 仍能编（actool 除外，它要模拟器运行时）。
# 这里先探一下，免得让人对着一条看不懂的 destination 报错发愣。
NEEDS_TARGET_MODE=0
if xcodebuild -showdestinations -project ios/IronMind.xcodeproj -scheme IronMind 2>&1 \
   | grep -q "is not installed. To use with Xcode"; then
  NEEDS_TARGET_MODE=1
  echo "⚠️  iOS 平台包还没装（xcodebuild -downloadPlatform iOS），改用 -target 方式"
fi

echo "▸ xcodebuild（$CONFIG / $SDK）"
if [ "$NEEDS_TARGET_MODE" = 1 ]; then
  # -target 模式不接受 -derivedDataPath，产物落在 OBJROOT/SYMROOT 下
  set -x
  xcodebuild \
    -project ios/IronMind.xcodeproj \
    -target IronMind \
    -configuration "$CONFIG" \
    -sdk "$SDK" \
    OBJROOT="$ROOT/ios/build/obj-$CONFIG" \
    SYMROOT="$ROOT/ios/build/sym-$CONFIG" \
    ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"} \
    build
  set +x
  APP="$ROOT/ios/build/sym-$CONFIG/$CONFIG-$SDK/IronMind.app"
else
  set -x
  xcodebuild \
    -project ios/IronMind.xcodeproj \
    -scheme IronMind \
    -configuration "$CONFIG" \
    -sdk "$SDK" \
    -destination "$DEST" \
    -derivedDataPath "$DERIVED" \
    ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"} \
    build
  set +x
  APP="$DERIVED/Build/Products/$CONFIG-$SDK/IronMind.app"
fi
if [ ! -d "$APP" ]; then
  APP="$(find "$ROOT/build" "$ROOT/ios/build" -maxdepth 5 -name "IronMind.app" -type d 2>/dev/null | head -1)"
fi
if [ -z "${APP:-}" ] || [ ! -d "$APP" ]; then
  echo "没找到产物 IronMind.app" >&2
  exit 1
fi

# ── 模拟器：直接装上去跑 ───────────────────────────────────────────────────
if [ -n "$SIMULATOR" ]; then
  DEVICE="$SIM_NAME"
  if ! xcrun simctl list devices available 2>/dev/null | grep -q "$DEVICE"; then
    # 这台机器上装了哪一代运行时并不知道，挑第一台可用的 iPhone
    DEVICE="$(xcrun simctl list devices available 2>/dev/null \
      | awk -F'[()]' '/iPhone/ {gsub(/^[ \t]+|[ \t]+$/,"",$1); print $1; exit}')"
    echo "▸ 没找到 \"$SIM_NAME\"，改用：$DEVICE"
  fi
  if [ -z "$DEVICE" ]; then
    echo "没有可用的 iPhone 模拟器。先跑：xcodebuild -downloadPlatform iOS" >&2
    exit 1
  fi
  echo "▸ 装到模拟器：$DEVICE"
  xcrun simctl boot "$DEVICE" 2>/dev/null || true
  # 注意：`open -a Simulator` 在刚解压完 Xcode 的机器上会**静默失败**——
  # LaunchServices 还没登记这个名字，于是只有设备在后台 booted、GUI 根本没起，
  # 人对着空屏幕不知道去哪儿找。所以用全路径，并显式挂到这台设备上。
  SIM_APP="$(xcode-select -p 2>/dev/null)/Applications/Simulator.app"
  [ -d "$SIM_APP" ] || SIM_APP=/Applications/Xcode.app/Contents/Developer/Applications/Simulator.app
  SIM_UDID="$(xcrun simctl list devices booted 2>/dev/null \
    | awk -F'[()]' -v n="$DEVICE" 'index($0, n) {print $2; exit}')"
  if [ -d "$SIM_APP" ]; then
    open -a "$SIM_APP" ${SIM_UDID:+--args -CurrentDeviceUDID "$SIM_UDID"} || true
  else
    open -a Simulator || true
  fi
  # 先杀掉再装：simctl install 只换 bundle，**不会**重启已在跑的进程，
  # 于是截出来的图还是上一版的界面（这个坑我踩过一次才明白）。
  xcrun simctl terminate "$DEVICE" com.ironmind.steelheart 2>/dev/null || true
  xcrun simctl install "$DEVICE" "$APP"
  xcrun simctl launch "$DEVICE" com.ironmind.steelheart
  echo "▸ 已启动。改 Swift 直接再跑一次；改 Rust 也会自动重编（preBuildScript）。"
  exit 0
fi

# ── 真机：装上去（需要 --signed 和已经配好的开发团队）──────────────────────
if [ "$DO_INSTALL" = 1 ]; then
  echo "▸ 装到连着的设备"
  # Xcode 15 起用 devicectl；老一点的环境退回 ios-deploy
  if xcrun devicectl list devices >/dev/null 2>&1; then
    xcrun devicectl device install app --device "$(xcrun devicectl list devices | awk '/connected/ {print $1; exit}')" "$APP"
  elif command -v ios-deploy >/dev/null 2>&1; then
    ios-deploy --bundle "$APP"
  else
    echo "装不了：没有 devicectl（Xcode 15+）也没有 ios-deploy。用 Xcode 直接 Run 也行。" >&2
    exit 1
  fi
fi

# ── 打包成未签名 .ipa ──────────────────────────────────────────────────────
mkdir -p dist
rm -rf dist/Payload
mkdir -p dist/Payload
cp -R "$APP" dist/Payload/
IPA="dist/IronMind-ios-$VERSION.ipa"
rm -f "$IPA"
(cd dist && zip -qry "IronMind-ios-$VERSION.ipa" Payload && rm -rf Payload)
echo
echo "▸ $IPA（$(du -h "$IPA" | cut -f1)）"
echo
cat <<MSG
怎么装到手机上（三选一）：

  1. Xcode 直接 Run（最省事，自动签名）：
     打开 ios/IronMind.xcodeproj → Signing & Capabilities 里选你的 Team →
     选中你的 iPhone → ⌘R

  2. 侧载工具（AltStore / Sideloadly）：把上面这个 .ipa 拖进去，用你的 Apple ID 重签。
     免费账号的 App 有效期 7 天，过期要重签。

  3. 自己的签名流程：
     xcodebuild -exportArchive 或者 plutil 改完后用 codesign 手工签。

游戏本体完全离线：装上之后不需要电脑、不需要联网（接了模型才会用网络）。
MSG
