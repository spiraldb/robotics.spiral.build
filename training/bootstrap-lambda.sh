#!/usr/bin/env bash
set -euo pipefail

runtime_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
uv_tool_dir="${runtime_dir}/.uv-tool"
bind_host="${SPIRAL_RUNTIME_HOST:-0.0.0.0}"
bind_port="${SPIRAL_RUNTIME_PORT:-50051}"

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "bootstrap-lambda.sh requires a Linux Lambda Stack host" >&2
    exit 1
fi

python3 -c 'import sys; assert (3, 12) <= sys.version_info[:2] < (3, 13), sys.version'

if command -v uv >/dev/null 2>&1; then
    uv_bin="$(command -v uv)"
else
    if [[ ! -x "${uv_tool_dir}/bin/uv" ]]; then
        python3 -m venv "${uv_tool_dir}"
        "${uv_tool_dir}/bin/python" -m pip install "uv==0.11.30"
    fi
    uv_bin="${uv_tool_dir}/bin/uv"
fi

"${uv_bin}" sync \
    --project "${runtime_dir}" \
    --frozen \
    --no-dev \
    --no-managed-python

"${runtime_dir}/.venv/bin/python" -c '
import torch
if not torch.cuda.is_available():
    raise SystemExit("uv sync succeeded, but PyTorch cannot see a CUDA GPU")
print(f"Ready on {torch.cuda.get_device_name(0)}")
'

public_host="${SPIRAL_RUNTIME_PUBLIC_HOST:-}"
if [[ -z "${public_host}" ]]; then
    public_host="$(
        python3 -c '
import ipaddress
import urllib.request

response = urllib.request.urlopen("https://api.ipify.org", timeout=5)
print(ipaddress.IPv4Address(response.read().decode().strip()))
' 2>/dev/null || true
    )"
fi
if [[ -z "${public_host}" ]]; then
    public_host="$(
        python3 -c '
import socket

with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as connection:
    connection.connect(("1.1.1.1", 80))
    print(connection.getsockname()[0])
' 2>/dev/null || true
    )"
fi
if [[ -z "${public_host}" ]]; then
    public_host="${bind_host}"
fi

printf '\nStudio runtime endpoint: http://%s:%s\n\n' "${public_host}" "${bind_port}"

exec "${runtime_dir}/.venv/bin/spiral-training-runtime" \
    --host "${bind_host}" \
    --port "${bind_port}"
