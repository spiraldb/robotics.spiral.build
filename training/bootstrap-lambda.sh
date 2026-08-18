#!/usr/bin/env bash
set -euo pipefail

runtime_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
uv_tool_dir="${runtime_dir}/.uv-tool"
bind_host="${SPIRAL_RUNTIME_HOST:-127.0.0.1}"
bind_port="${SPIRAL_RUNTIME_PORT:-50051}"
ssh_user="${SPIRAL_RUNTIME_SSH_USER:-ubuntu}"

if [[ "$(uname -s)" != "Linux" ]]; then
    echo "bootstrap-lambda.sh requires a Linux Lambda Stack host" >&2
    exit 1
fi

python3 -c 'import sys; assert (3, 12) <= sys.version_info[:2] < (3, 13), sys.version'

if ! command -v ffmpeg >/dev/null 2>&1; then
    echo "bootstrap-lambda.sh requires ffmpeg for Spiral MP4 fallback decoding" >&2
    exit 1
fi

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

bundled_adbc_driver="${runtime_dir}/libadbc_driver_spiraldb.so"
if [[ -z "${SPIRALDB_ADBC_DRIVER:-}" && -f "${bundled_adbc_driver}" ]]; then
    export SPIRALDB_ADBC_DRIVER="${bundled_adbc_driver}"
fi
if [[ -n "${SPIRALDB_ADBC_DRIVER:-}" && ! -f "${SPIRALDB_ADBC_DRIVER}" ]]; then
    echo "SpiralDB ADBC driver is missing: ${SPIRALDB_ADBC_DRIVER}" >&2
    exit 1
fi

spiral_video_runtime="$(
    "${runtime_dir}/.venv/bin/python" -c '
from pathlib import Path
import spiraldb

package = Path(spiraldb.__file__).resolve().parent
runtime = package / "libspiral_video_runtime.so"
if not runtime.is_file():
    raise SystemExit(f"SpiralDB video runtime is missing: {runtime}")
print(runtime)
'
)"
video_support_dir="${runtime_dir}/.spiral-video-runtime"
video_library_dir="${video_support_dir}/av.libs"

if ldd "${spiral_video_runtime}" | grep -q 'not found'; then
    if ! compgen -G "${video_library_dir}/libavcodec-*.so.62.*" >/dev/null; then
        "${uv_bin}" pip install \
            --python "${runtime_dir}/.venv/bin/python" \
            --target "${video_support_dir}" \
            --no-deps \
            "av==18.1.0"
    fi

    shopt -s nullglob
    link_video_library() {
        local soname="$1"
        local pattern="$2"
        local matches=("${video_library_dir}"/${pattern})
        if [[ "${#matches[@]}" -ne 1 ]]; then
            echo "expected exactly one ${pattern} in ${video_library_dir}" >&2
            exit 1
        fi
        ln -sfn "$(basename "${matches[0]}")" "${video_library_dir}/${soname}"
    }
    link_video_library "libavcodec.so.62" 'libavcodec-*.so.62.*'
    link_video_library "libavutil.so.60" 'libavutil-*.so.60.*'
    link_video_library "libswscale.so.9" 'libswscale-*.so.9.*'
    shopt -u nullglob
    export LD_LIBRARY_PATH="${video_library_dir}${LD_LIBRARY_PATH:+:${LD_LIBRARY_PATH}}"
fi
export SPIRAL_VIDEO_RUNTIME="${spiral_video_runtime}"

"${runtime_dir}/.venv/bin/python" -c '
import ctypes
import os

ctypes.CDLL(os.environ["SPIRAL_VIDEO_RUNTIME"])
print("SpiralDB FFmpeg video runtime ready")
'

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

printf '\nRuntime listening privately at http://%s:%s\n' "${bind_host}" "${bind_port}"
printf 'On the Studio machine, start this SSH tunnel:\n'
printf '  ssh -N -L %s:127.0.0.1:%s %s@%s\n' \
    "${bind_port}" "${bind_port}" "${ssh_user}" "${public_host}"
printf 'Then use this Studio runtime endpoint:\n'
printf '  http://127.0.0.1:%s\n\n' "${bind_port}"

exec "${runtime_dir}/.venv/bin/spiral-training-runtime" \
    --host "${bind_host}" \
    --port "${bind_port}"
