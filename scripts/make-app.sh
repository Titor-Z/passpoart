#!/usr/bin/env bash
# 打包 macOS 应用：target/dist/Passpoart.app
#
# 用法：
#   scripts/make-app.sh            # release 构建
#   scripts/make-app.sh --debug    # debug 构建（出包快，体积大）
#
# 产物双击即用 LaunchServices 拉起，不再经由终端。
# 图标链路：assets/icon.png --sips--> 1024 PNG --sips--> iconset --iconutil--> icns。
# 源 PNG 由设计稿（assets/icon.svg 同形态）导出；保留 alpha 通道。
# （macOS 12 的 qlmanage 会把 SVG 合成到白底，故改读 PNG。）
set -euo pipefail
cd "$(dirname "$0")/.."

MODE=release
if [[ "${1:-}" == "--debug" ]]; then
  MODE=debug
fi

BIN="target/$MODE/passpoart"
APP="target/dist/Passpoart.app"
ICON_PNG="crates/vault-app/assets/icon.png"
VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/version = "\(.*\)"/\1/')"

echo "==> cargo build ($MODE)"
if [[ "$MODE" == "release" ]]; then
  cargo build -p vault-app --release
else
  cargo build -p vault-app
fi

echo "==> 准备 1024px 源 PNG"
rm -rf target/dist /tmp/pp-icon.iconset /tmp/pp-icon-1024.png
mkdir -p target/dist "$APP/Contents/MacOS" "$APP/Contents/Resources" /tmp/pp-icon.iconset
sips -z 1024 1024 "$ICON_PNG" --out /tmp/pp-icon-1024.png > /dev/null

echo "==> 烘焙软投影（Apple 模板风格：透明边距内，体底部微影）"
python3 - /tmp/pp-icon-1024.png << 'PY'
import sys
from PIL import Image, ImageFilter
p = sys.argv[1]
art = Image.open(p).convert("RGBA")
a = art.getchannel("A")
shadow = Image.new("RGBA", art.size, (94, 40, 80, 0))  # 品牌紫灰影
shadow.putalpha(a.point(lambda v: int(v * 0.22)))
shadow = shadow.filter(ImageFilter.GaussianBlur(14))
canvas = Image.new("RGBA", art.size, (0, 0, 0, 0))
canvas.alpha_composite(shadow, (0, 10))
canvas.alpha_composite(art)
canvas.save(p)
PY

echo "==> 生成 iconset"
for s in 16 32 128 256 512; do
  sips -z "$s" "$s" /tmp/pp-icon-1024.png --out "/tmp/pp-icon.iconset/icon_${s}x${s}.png" > /dev/null
done
sips -z 32 32   /tmp/pp-icon-1024.png --out /tmp/pp-icon.iconset/icon_16x16@2x.png   > /dev/null
sips -z 64 64   /tmp/pp-icon-1024.png --out /tmp/pp-icon.iconset/icon_32x32@2x.png   > /dev/null
sips -z 256 256 /tmp/pp-icon-1024.png --out /tmp/pp-icon.iconset/icon_128x128@2x.png > /dev/null
sips -z 512 512 /tmp/pp-icon-1024.png --out /tmp/pp-icon.iconset/icon_256x256@2x.png > /dev/null
sips -z 1024 1024 /tmp/pp-icon-1024.png --out /tmp/pp-icon.iconset/icon_512x512@2x.png > /dev/null
iconutil -c icns /tmp/pp-icon.iconset -o "$APP/Contents/Resources/AppIcon.icns"

echo "==> 组装 bundle (v$VERSION)"
cp "$BIN" "$APP/Contents/MacOS/passpoart"
cat > "$APP/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>              <string>Passpoart</string>
    <key>CFBundleDisplayName</key>       <string>Passpoart</string>
    <key>CFBundleIdentifier</key>        <string>com.passpoart.Passpoart</string>
    <key>CFBundleExecutable</key>        <string>passpoart</string>
    <key>CFBundleIconFile</key>          <string>AppIcon</string>
    <key>CFBundlePackageType</key>       <string>APPL</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>CFBundleVersion</key>           <string>$VERSION</string>
    <key>LSMinimumSystemVersion</key>    <string>10.13</string>
    <key>NSHighResolutionCapable</key>   <true/>
    <key>LSApplicationCategoryType</key> <string>public.app-category.productivity</string>
</dict>
</plist>
PLIST
printf 'APPL????' > "$APP/Contents/PkgInfo"

echo "==> 产物：$APP"
echo "    启动：open $APP （或访达双击；拖进「应用程序」目录即可常驻）"
