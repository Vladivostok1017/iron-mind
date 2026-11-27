#!/usr/bin/env bash
#
# build-mac.sh —— 一条命令出 macOS 的 .app。
#
# 和 Windows 那边是**同一个外壳**（`crates/sim-desktop`：tao + wry），
# 只是这台机器上它从来没有被打成 macOS 的样子。差别只有打包方式：
#
#   Windows  IronMind.exe + sim-server.exe + modules\  → 一个 zip
#   macOS    IronMind.app/Contents/MacOS/{IronMind, sim-server}
#                      /Contents/Resources/{AppIcon.icns, modules/*.imod}
#
# 为什么必须是 `.app` 而不是一个裸二进制：
#   · 没有 bundle 就没有 Dock 图标、双击启动、cmd+tab 条目；
#   · 裸二进制从 Finder 启动时工作目录是 `/`，存档会写不下去
#     （外壳已经把这种情况兜到 ~/Library/Application Support/IronMind 了，
#      见 crates/sim-desktop/src/main.rs 的 mac_data_root）。
#
# 用法：
#   tools/build-mac.sh              # 出 dist/IronMind.app 和 dist/IronMind-<版本>-mac.zip
#   tools/build-mac.sh --no-zip     # 只要 .app，方便直接跑
#
# 注意：签的是**本地 ad-hoc 签名**（`codesign -s -`），够本机双击运行。
# 分发给别人要开发者证书 + 公证，那不是这个项目要做的事（和 iOS 那个未签名 ipa 同理）。

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DO_ZIP=1
for a in "$@"; do
  case "$a" in
    --no-zip) DO_ZIP=0 ;;
    -h|--help) sed -n '2,26p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "不认识的参数：$a" >&2; exit 2 ;;
  esac
done

[ "$(uname -s)" = "Darwin" ] || { echo "这是 macOS 打包脚本（Windows 用 tools/package.ps1）" >&2; exit 2; }

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
[ -n "$VERSION" ] || VERSION="0.0.0"

echo "▸ 编 release（sim-desktop + sim-server）"
cargo build --release -p sim-desktop -p sim-server

APP="$ROOT/dist/IronMind.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources/modules"

echo "▸ 装进 bundle"
cp -f target/release/sim-desktop "$APP/Contents/MacOS/IronMind"
cp -f target/release/sim-server  "$APP/Contents/MacOS/sim-server"

# 引擎自带的演示模块（目录形态，内核两种都认）。
# 放在 bundle 里是**只读**的：导入的模块会落到 ~/Library/Application Support/IronMind/modules。
for m in content/scenarios/*.imod; do
  [ -d "$m" ] && cp -R "$m" "$APP/Contents/Resources/modules/" && echo "  + modules/$(basename "$m")"
done

# ── 图标：用 iOS 那套 1024，转成 .icns ────────────────────────────────────
# 同一个徽记、同一段画图代码（tools/make-icon-ios.py），不另画一套省得两处漂移。
SRC_PNG="ios/IronMind/Assets.xcassets/AppIcon.appiconset/icon-ios-marketing-1024@1x.png"
if [ -f "$SRC_PNG" ] && command -v iconutil >/dev/null 2>&1; then
  ICONSET="$ROOT/build/AppIcon.iconset"
  rm -rf "$ICONSET"; mkdir -p "$ICONSET"
  # iconutil 认这些**固定名字**，别自作主张改
  for pair in "16:16x16" "32:16x16@2x" "32:32x32" "64:32x32@2x" \
              "128:128x128" "256:128x128@2x" "256:256x256" "512:256x256@2x" \
              "512:512x512" "1024:512x512@2x"; do
    px="${pair%%:*}"; name="${pair##*:}"
    sips -z "$px" "$px" "$SRC_PNG" --out "$ICONSET/icon_${name}.png" >/dev/null 2>&1
  done
  iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
  echo "  + AppIcon.icns"
else
  echo "  ⚠️ 跳过图标（没有 $SRC_PNG 或没有 iconutil）"
fi

echo "▸ Info.plist"
cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- 钢心 / Iron Mind —— macOS bundle。
     NSAllowsLocalNetworking：页面和内核对的是 http://127.0.0.1:<port>，
     ATS 默认只放行 https，不开这一项 WKWebView 直接白屏（和 iOS 那边同一个坑）。
     LSMinimumSystemVersion 11.0：tao 0.37 / wry 0.57 的下限。 -->
<plist version="1.0">
<dict>
	<key>CFBundleName</key><string>钢心</string>
	<key>CFBundleDisplayName</key><string>钢心</string>
	<key>CFBundleIdentifier</key><string>org.ironmind.steelheart.mac</string>
	<key>CFBundleExecutable</key><string>IronMind</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>CFBundleShortVersionString</key><string>$VERSION</string>
	<key>CFBundleVersion</key><string>$VERSION</string>
	<key>CFBundleInfoDictionaryVersion</key><string>6.0</string>
	<key>LSMinimumSystemVersion</key><string>11.0</string>
	<key>LSApplicationCategoryType</key><string>public.app-category.strategy-games</string>
	<key>NSHighResolutionCapable</key><true/>
	<!-- 声明 .imod 是文档类型：系统层面认得它，WKWebView 的文件面板才不会
	     因为"未知类型"把文件丢掉（iOS 那边同样的声明在 ios/IronMind/Info.plist）。 -->
	<key>UTImportedTypeDeclarations</key>
	<array>
		<dict>
			<key>UTTypeIdentifier</key><string>org.ironmind.imod</string>
			<key>UTTypeDescription</key><string>钢心战役模块</string>
			<key>UTTypeConformsTo</key>
			<array><string>public.data</string><string>public.archive</string></array>
			<key>UTTypeTagSpecification</key>
			<dict>
				<key>public.filename-extension</key><array><string>imod</string></array>
				<key>public.mime-type</key><array><string>application/x-ironmind-module</string></array>
			</dict>
		</dict>
	</array>
	<key>NSAppTransportSecurity</key>
	<dict>
		<key>NSAllowsLocalNetworking</key><true/>
	</dict>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" >/dev/null

# ── 签名 ────────────────────────────────────────────────────────────────
# ad-hoc：本机双击能跑。**必须在所有文件就位之后签**，之后再加文件就失效了。
echo "▸ ad-hoc 签名"
codesign --force --deep --sign - "$APP" 2>&1 | tail -2
codesign --verify --deep --strict "$APP" && echo "  ✅ 签名校验通过"

echo "▸ 大小：$(du -sh "$APP" | cut -f1)"

if [ "$DO_ZIP" = 1 ]; then
  ZIP="$ROOT/dist/IronMind-$VERSION-mac.zip"
  rm -f "$ZIP"
  (cd "$ROOT/dist" && zip -qry "IronMind-$VERSION-mac.zip" "IronMind.app")
  echo "▸ $ZIP（$(du -h "$ZIP" | cut -f1)）"
fi

echo
echo "跑：open dist/IronMind.app"
echo "存档：~/Library/Application Support/IronMind/"
