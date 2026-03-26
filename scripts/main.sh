#!/usr/bin/env sh

set -eu

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
PID_DIR="${ROOT_DIR}/.run"
PID_FILE="${PID_DIR}/main.pid"
BIN_PATH="${ROOT_DIR}/target/debug/polymarket-ltf"

usage() {
  cat <<'EOF'
usage:
  ./scripts/main.sh start [args...]
  ./scripts/main.sh stop
  ./scripts/main.sh status

examples:
  ./scripts/main.sh start
  ./scripts/main.sh start --help
  ./scripts/main.sh stop
EOF
}

is_running() {
  [ -f "${PID_FILE}" ] || return 1
  pid="$(cat "${PID_FILE}")"
  kill -0 "${pid}" 2>/dev/null
}

start() {
  if is_running; then
    echo "main is already running: pid=$(cat "${PID_FILE}")"
    exit 1
  fi

  mkdir -p "${PID_DIR}"

  echo "building main binary..."
  cargo build

  echo "starting main in background..."
  nohup "${BIN_PATH}" "$@" > /dev/null 2>&1 &

  pid="$!"
  echo "${pid}" > "${PID_FILE}"

  echo "started: pid=${pid}"
}

stop() {
  if ! [ -f "${PID_FILE}" ]; then
    echo "main is not running"
    exit 0
  fi

  pid="$(cat "${PID_FILE}")"
  if kill -0 "${pid}" 2>/dev/null; then
    echo "stopping main: pid=${pid}"
    kill "${pid}"
  else
    echo "main pid file exists but process is not running: pid=${pid}"
  fi

  rm -f "${PID_FILE}"
}

status() {
  if is_running; then
    echo "main is running: pid=$(cat "${PID_FILE}")"
  else
    echo "main is not running"
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
