#!/usr/bin/env bash
# S6 验收沙箱环境包（bash 侧）：source 本文件后再运行任何沙箱二进制。
export S6_ROOT="D:/coding/hutao-music-player/.worktrees/s6-verify/verify"
export XDG_CONFIG_HOME="$S6_ROOT/sandbox/config"
export XDG_DATA_HOME="$S6_ROOT/sandbox/data"
export XDG_CACHE_HOME="$S6_ROOT/sandbox/cache"
export HMP_IPC_ENDPOINT='\\.\pipe\hmp-s6-verify'
export S6_HMP="D:/coding/hutao-music-player/.worktrees/s6-verify/target/release/hmp.exe"
export S6_DESKTOP="D:/coding/hutao-music-player/.worktrees/s6-verify/target/release/hmp-desktop.exe"
export S6_SHOT_MENU="D:/coding/hutao-music-player/.worktrees/s6-verify/target/release/examples/shot-menu.exe"
