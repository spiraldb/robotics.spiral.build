import argparse
import ipaddress

import uvicorn


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Serve Spiral Robotics ACT training and inference over HTTP + Arrow IPC"
    )
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", default=50051, type=int)
    parser.add_argument("--log-level", default="info")
    parser.add_argument(
        "--allow-remote-access",
        action="store_true",
        help="explicitly permit this unauthenticated runtime to listen beyond loopback",
    )
    args = parser.parse_args()
    if not _is_loopback_host(args.host) and not args.allow_remote_access:
        parser.error(
            "refusing to expose the unauthenticated runtime beyond loopback; "
            "pass --allow-remote-access explicitly"
        )
    uvicorn.run(
        "spiral_training_runtime.service:app",
        host=args.host,
        port=args.port,
        log_level=args.log_level,
        workers=1,
    )


def _is_loopback_host(host: str) -> bool:
    if host.casefold() == "localhost":
        return True
    try:
        return ipaddress.ip_address(host.strip("[]")).is_loopback
    except ValueError:
        return False


if __name__ == "__main__":
    main()
