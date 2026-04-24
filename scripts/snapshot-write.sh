#!/usr/bin/env sh

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

if [ -f "${SCRIPT_DIR}/snapshot-write" ]; then
  APP_DIR="${SCRIPT_DIR}"
elif [ -f "${ROOT_DIR}/snapshot-write" ]; then
  APP_DIR="${ROOT_DIR}"
else
  APP_DIR="${SCRIPT_DIR}"
fi

PID_DIR="${APP_DIR}/.run"
PID_FILE="${PID_DIR}/snapshot-write.pid"
BIN_PATH="${APP_DIR}/snapshot-write"
LOG_DIR="${APP_DIR}/logs"

usage() {
  cat <<'EOF'
usage:
  ./scripts/snapshot-write.sh start [symbols] [5m|15m|both] [output_dir]
  ./scripts/snapshot-write.sh stop
  ./scripts/snapshot-write.sh status

examples:
  ./scripts/snapshot-write.sh start
  ./scripts/snapshot-write.sh start btc,eth,sol,xrp both data/snapshots
  ./scripts/snapshot-write.sh stop
EOF
}

is_running() {
  [ -f "${PID_FILE}" ] || return 1
  pid="$(cat "${PID_FILE}")"
  kill -0 "${pid}" 2>/dev/null
}

start() {
  SYMBOLS="${1:-btc,eth,sol,xrp}"
  INTERVAL="${2:-both}"
  OUTPUT_DIR="${3:-${APP_DIR}/data/snapshots}"

  if is_running; then
    echo "snapshot-write is already running: pid=$(cat "${PID_FILE}")"
    exit 1
  fi

  if ! [ -f "${BIN_PATH}" ]; then
    echo "binary not found: ${SCRIPT_DIR}/snapshot-write or ${ROOT_DIR}/snapshot-write"
    exit 1
  fi

  if ! [ -x "${BIN_PATH}" ]; then
    echo "binary is not executable: ${BIN_PATH}"
    exit 1
  fi

  mkdir -p "${PID_DIR}"
  mkdir -p "${LOG_DIR}"
  mkdir -p "${OUTPUT_DIR}"

  # 提高文件描述符上限，避免 WS 重连 + reqwest 连接池 + 日志 + CSV 瞬时打开撞到默认 1024
  ulimit -n 65535 2>/dev/null || true

  echo "starting snapshot-write in background..."
  POLYMARKET_LTF_LOG_DIR="${LOG_DIR}" \
    nohup "${BIN_PATH}" "${SYMBOLS}" "${INTERVAL}" "${OUTPUT_DIR}" \
    > /dev/null 2>&1 &

  pid="$!"
  echo "${pid}" > "${PID_FILE}"

  echo "started: pid=${pid}"
}

stop() {
  if ! [ -f "${PID_FILE}" ]; then
    echo "snapshot-write is not running"
    exit 0
  fi

  pid="$(cat "${PID_FILE}")"
  if kill -0 "${pid}" 2>/dev/null; then
    echo "stopping snapshot-write: pid=${pid}"
    kill "${pid}"
  else
    echo "snapshot-write pid file exists but process is not running: pid=${pid}"
  fi

  rm -f "${PID_FILE}"
}

status() {
  if is_running; then
    echo "snapshot-write is running: pid=$(cat "${PID_FILE}")"
  else
    echo "snapshot-write is not running"
    [ -f "${PID_FILE}" ] && rm -f "${PID_FILE}"
  fi
}

cd "${APP_DIR}"

COMMAND="${1:-}"
case "${COMMAND}" in
  start)
    shift
    start "$@"
    ;;
  stop)
    stop
    ;;
  status)
    status
    ;;
  *)
    usage
    exit 1
    ;;
esac
