#!/usr/bin/env bash
# 按需拉取 Python 参考实现到 .deps/（差分测试 Oracle，已 gitignore）
# 用法: ./scripts/fetch-python-ref.sh
set -euo pipefail

REF_URL="https://github.com/L-1124/QQMusicApi.git"
REF_COMMIT="108617ffe80abefec6358717b9f4d3677550db10"
DEST=".deps/qqmusic-api-python"

if [ -d "$DEST/.git" ]; then
  echo "参考源已存在: $DEST"
  exit 0
fi

mkdir -p .deps
git clone "$REF_URL" "$DEST"
git -C "$DEST" checkout --quiet "$REF_COMMIT"
echo "已拉取参考源到 $DEST (commit $REF_COMMIT)"
