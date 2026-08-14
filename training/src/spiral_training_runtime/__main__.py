import argparse

import uvicorn


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Serve Spiral Robotics ACT training and inference over HTTP + Arrow IPC"
    )
    parser.add_argument("--host", default="0.0.0.0")
    parser.add_argument("--port", default=50051, type=int)
    parser.add_argument("--log-level", default="info")
    args = parser.parse_args()
    uvicorn.run(
        "spiral_training_runtime.service:app",
        host=args.host,
        port=args.port,
        log_level=args.log_level,
        workers=1,
    )


if __name__ == "__main__":
    main()
