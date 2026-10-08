#!/usr/bin/env bash
unset CODEG_DATA_DIR CODEG_HOME CODEG_STATIC_DIR CODEG_ACP_TMP_ROOT CODEG_TOKEN CODEG_PORT CODEG_HOST
if env | grep '^CODEG'; then echo 'Unexpected inherited CODEG environment' >&2; exit 1; fi
set -euo pipefail
exec python3 "$(cd "$(dirname "$0")" && pwd -P)/environment.py" up "$@"
