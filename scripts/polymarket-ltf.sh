#!/usr/bin/env sh

set -eu

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"

if [ -f "${SCRIPT_DIR}/polymarket-ltf" ]; then
  APP_DIR="${SCRIPT_DIR}"
elif [ -f "${ROOT_DIR}/polymarket-ltf" ]; then
  APP_DIR="${ROOT_DIR}"
else
  APP_DIR="${SCRIPT_DIR}"
fi

PID_DIR="${APP_DIR}/.run"
PID_FILE="${PID_DIR}/polymarket-ltf.pid"
BIN_PATH="${APP_DIR}/polymarket-ltf"

usage() {
  cat <<'EOF'
usage:
  ./scripts/polymarket-ltf.sh start [args...]
  ./scripts/polymarket-ltf.sh stop
  ./scripts/polymarket-ltf.sh status

examples:
  ./scripts/polymarket-ltf.sh start
  ./scripts/polymarket-ltf.sh start --help
  ./scripts/polymarket-ltf.sh stop
EOF
}

is_running() {
  [ -f "${PID_FILE}" ] || return 1
  pid="$(cat "${PID_FILE}")"
  kill -0 "${pid}" 2>/dev/null
}

start() {
  if is_running; then
    echo "polymarket-ltf is already running: pid=$(cat "${PID_FILE}")"
    exit 1
  fi

  if ! [ -f "${BIN_PATH}" ]; then
    echo "binary not found: ${SCRIPT_DIR}/polymarket-ltf or ${ROOT_DIR}/polymarket-ltf"
    exit 1
  fi

  if ! [ -x "${BIN_PATH}" ]; then
    echo "binary is not executable: ${BIN_PATH}"
    exit 1
  fi

  mkdir -p "${PID_DIR}"

  echo "starting polymarket-ltf in background..."
  nohup "${BIN_PATH}" "$@" > /dev/null 2>&1 &

  pid="$!"
  echo "${pid}" > "${PID_FILE}"

  echo "started: pid=${pid}"
}

stop() {
  if ! [ -f "${PID_FILE}" ]; then
    echo "polymarket-ltf is not running"
    exit 0
  fi

  pid="$(cat "${PID_FILE}")"
  if kill -0 "${pid}" 2>/dev/null; then
    echo "stopping polymarket-ltf: pid=${pid}"
    kill "${pid}"
  else
    echo "polymarket-ltf pid file exists but process is not running: pid=${pid}"
  fi

  rm -f "${PID_FILE}"
}

status() {
  if is_running; then
    echo "polymarket-ltf is running: pid=$(cat "${PID_FILE}")"
  else
    echo "polymarket-ltf is not running"
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
