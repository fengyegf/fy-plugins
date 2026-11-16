#!/usr/bin/env bash
# 离线回放冒烟（不接账号、不连 TDLib）：把本仓刚编出来的 6 个插件 + 宿主安装目录里的
# html2img（卡片渲染的硬依赖）装进临时插件目录，再用给定宿主跑一遍 --replay。
#
# 用法：scripts/replay-smoke.sh <fuyubot 安装目录> [replay.json]
#   <fuyubot 安装目录>  含 fuyubot.exe 与 plugins/ 的目录（宿主需 ABI v6 / 0.1.4+）
#   [replay.json]       默认 docs/replay/new_message.json
# 环境变量：REPLAY_WAIT=8000（回放后静默窗口，毫秒）
set -euo pipefail

HOST_DIR=${1:?用法: scripts/replay-smoke.sh <fuyubot 安装目录> [replay.json]}
# MSYS / Git Bash 会把 `/f/...` 形式的参数转成 `F:\...`；glob 不认反斜杠，统一成正斜杠。
HOST_DIR=${HOST_DIR//\\//}
REPLAY=${2:-docs/replay/new_message.json}
EXE="$HOST_DIR/fuyubot.exe"
[ -x "$EXE" ] || EXE="$HOST_DIR/fuyubot"
[ -x "$EXE" ] || { echo "找不到 fuyubot 可执行文件：$HOST_DIR"; exit 2; }
OUT=.replay
WAIT=${REPLAY_WAIT:-8000}

cd "$(dirname "$0")/.."

echo "== 构建（debug；CARGO_PROFILE_DEV_DEBUG=0 规避 Windows 下 zstd-sys 的 cl D8050）"
CARGO_PROFILE_DEV_DEBUG=0 cargo build --workspace --offline

echo "== 组装临时插件目录 $OUT/plugins"
rm -rf "$OUT"
mkdir -p "$OUT/plugins"
stage() { # $1=id $2=target/debug 里的 dll $3=plugin.json 声明的 entry
  mkdir -p "$OUT/plugins/$1/$(dirname "$3")"
  cp "crates/fuyubot-plugin-$1/plugin.json" "$OUT/plugins/$1/plugin.json"
  cp "target/debug/$2" "$OUT/plugins/$1/$3"
}
stage epic      epic.dll      epic.dll
stage kfc       kfc.dll       kfc.dll
stage musiccard musiccard.dll bin/musiccard.dll
stage ppxfy     ppxfy.dll     ppxfy.dll
stage system    system.dll    system.dll
stage whois     whois.dll     whois.dll

# html2img 是 epic / kfc / musiccard / system / whois 的硬依赖（不在本仓），从宿主目录借一份；
# fuyubot.mongo 是 ppxfy 的可选软依赖，一并带上，缺了也不影响启动。
for bundle in "$HOST_DIR"/plugins/fuyubot.html2img*/ "$HOST_DIR"/plugins/fuyubot.mongo/; do
  if [ -d "$bundle" ]; then cp -r "$bundle" "$OUT/plugins/"; fi
done
if ! ls -d "$OUT"/plugins/fuyubot.html2img* >/dev/null 2>&1; then
  echo "警告：宿主目录里没有 fuyubot.html2img，epic / kfc / musiccard / system / whois 的卡片渲染会失败"
fi

echo "== 回放 $REPLAY（不接 TDLib，请求只留档）"
cd "$OUT"
"$EXE" run --replay "../$REPLAY" --plugin-dir plugins --replay-wait "$WAIT" 2>&1 | tee replay.log
