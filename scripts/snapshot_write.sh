#!/usr/bin/env sh

set -eu

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PID_DIR="${ROOT_DIR}/.run"
PID_FILE="${PID_DIR}/snapshot_write.pid"
BIN_PATH="${ROOT_DIR}/target/debug/examples/snapshot_write"

usage() {
  cat <<'EOF'
usage:
  ./scripts/snapshot_write.sh start [symbol] [5m|15m|both] [output_dir]
  ./scripts/snapshot_write.sh stop
  ./scripts/snapshot_write.sh status

examples:
  ./scripts/snapshot_write.sh start
  ./scripts/snapshot_write.sh start btc both data/snapshots
  ./scripts/snapshot_write.sh stop
EOF
}

is_running() {
  [ -f "${PID_FILE}" ] || return 1
  pid="$(cat "${PID_FILE}")"
  kill -0 "${pid}" 2>/dev/null
}

start() {
  SYMBOL="${1:-btc}"
  INTERVAL="${2:-both}"
  OUTPUT_DIR="${3:-data/snapshots}"

  if is_running; then
    echo "snapshot_write is already running: pid=$(cat "${PID_FILE}")"
    exit 1
  fi

  mkdir -p "${PID_DIR}"

  echo "building snapshot_write example..."
  cargo build --example snapshot_write

  echo "starting snapshot_write in background..."
  nohup "${BIN_PATH}" "${SYMBOL}" "${INTERVAL}" "${OUTPUT_DIR}" \
    > /dev/null 2>&1 &

  pid="$!"
  echo "${pid}" > "${PID_FILE}"

  echo "started: pid=${pid}"
}

stop() {
  if ! [ -f "${PID_FILE}" ]; then
    echo "snapshot_write is not running"
    exit 0
  fi

  pid="$(cat "${PID_FILE}")"
  if kill -0 "${pid}" 2>/dev/null; then
    echo "stopping snapshot_write: pid=${pid}"
    kill "${pid}"
  else
    echo "snapshot_write pid file exists but process is not running: pid=${pid}"
  fi

  rm -f "${PID_FILE}"
}

status() {
  if is_running; then
    echo "snapshot_write is running: pid=$(cat "${PID_FILE}")"
  else
    echo "snapshot_write is not running"
    [ -f "${PID_FILE}" ] && rm -f "${PID_FILE}"
  fi
}

cd "${ROOT_DIR}"

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
