#!/usr/bin/env bash
# ar-OCG-Router 安装/卸载脚本（systemd，code name: ar-ocg-router）
#
# 用法：
#   sudo ./install.sh install      安装并启动服务
#   sudo ./install.sh uninstall    停止并删除服务（保留配置与日志）
#   sudo ./install.sh start|stop|restart|status|logs
#   sudo ./install.sh check        自检（--selftest，会真实访问上游）
#
# 可用环境变量覆盖路径：
#   PREFIX=/usr/local  CONFIG_DIR=/etc/ar-ocg-router  LOG_DIR=/var/log/ar-ocg-router
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SERVICE=ar-ocg-router
PREFIX=${PREFIX:-/usr/local}
BIN_DIR=$PREFIX/bin
BIN=$BIN_DIR/ar-ocg-router
CONFIG_DIR=${CONFIG_DIR:-/etc/ar-ocg-router}
CONFIG=$CONFIG_DIR/config.yaml
LOG_DIR=${LOG_DIR:-/var/log/ar-ocg-router}
UNIT=/etc/systemd/system/$SERVICE.service
SRC_BIN=$SCRIPT_DIR/ar-ocg-router
SRC_UNIT=$SCRIPT_DIR/ar-ocg-router.service
SRC_EXAMPLE=$SCRIPT_DIR/config.example.yaml

log() { printf "[%s] %s\n" "$(date '+%H:%M:%S')" "$*"; }
die() { printf "错误：%s\n" "$*" >&2; exit 1; }
need_root() { [ "$(id -u)" = 0 ] || die "需要 root：sudo $0 $*"; }
have_systemd() { command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; }

usage() { sed -n '2,20p' "$0"; }

cmd_install() {
  need_root install
  [ -f "$SRC_BIN" ] || die "找不到二进制 $SRC_BIN（请把 install.sh 与 ar-ocg-router 放同一目录）"
  have_systemd || log "警告：当前系统没有运行 systemd，服务文件已安装但不会自动生效"
  install -d -m 0755 "$BIN_DIR" "$CONFIG_DIR" "$LOG_DIR"
  install -m 0755 "$SRC_BIN" "$BIN"
  if [ ! -f "$CONFIG" ]; then
    if [ -f "$SRC_EXAMPLE" ]; then install -m 0640 "$SRC_EXAMPLE" "$CONFIG"; else die "缺少 $SRC_EXAMPLE"; fi
    log "已写入模板配置 $CONFIG —— 请填入 OpenCode Go / DeepSeek 的 key"
  else
    log "保留已有配置 $CONFIG"
  fi
  sed -e "s|@BIN@|$BIN|g" -e "s|@CONFIG@|$CONFIG|g" -e "s|@LOGDIR@|$LOG_DIR|g" \
      -e "s|@CONFIGDIR@|$CONFIG_DIR|g" -e "s|@WORKDIR@|$CONFIG_DIR|g" "$SRC_UNIT" > "$UNIT"
  chmod 0644 "$UNIT"
  if have_systemd; then
    systemctl daemon-reload
    systemctl enable --now "$SERVICE"
    sleep 1
    systemctl --no-pager --full status "$SERVICE" || true
  fi
  log "完成。配置：$CONFIG  日志：journalctl -u $SERVICE -f"
  log "客户端 base_url = http://127.0.0.1:8787/v1（端口见配置文件）"
}

cmd_uninstall() {
  need_root uninstall
  if command -v systemctl >/dev/null 2>&1; then systemctl disable --now "$SERVICE" >/dev/null 2>&1 || true; fi
  rm -f "$UNIT"
  command -v systemctl >/dev/null 2>&1 && systemctl daemon-reload || true
  rm -f "$BIN"
  log "已卸载（配置 $CONFIG_DIR 与日志 $LOG_DIR 保留，如需删除请手动 rm -rf）"
}

cmd_ctl() {
  local action=$1
  have_systemd || die "没有 systemd，无法 $action"
  systemctl "$action" "$SERVICE"
}

cmd_logs() { journalctl -u "$SERVICE" -n ${2:-80} --no-pager; }

cmd_check() { "$BIN" --config "$CONFIG" --selftest; }

case "${1:-}" in
  install)   cmd_install ;;
  uninstall) cmd_uninstall ;;
  start|stop|restart|status) cmd_ctl "$1" ;;
  logs)      cmd_logs "$1" ;;
  check)     cmd_check ;;
  *)         usage; exit 2 ;;
esac
