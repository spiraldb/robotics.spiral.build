# syntax=docker/dockerfile:1
#
# Spiral Robotics Studio as a Linux server: the backend, its web interface, and
# a Spiral driver with the video runtime it loads. Build it from the Studio
# checkout with two more named contexts:
#
#   spiraldb  a SpiralDB checkout, the driver's source
#   web       the `public/` directory `dx bundle --web` writes
#
# README.md beside this file has the command; the Hosted image workflow runs it.

# LGPL-only FFmpeg, built as SpiralDB's own images build it.
FROM debian:trixie-slim AS ffmpeg
COPY --from=spiraldb scripts/setup-ffmpeg-lgpl.sh scripts/verify-ffmpeg-lgpl.sh /opt/build/scripts/
ENV FFMPEG_PREFIX=/opt/ffmpeg
ENV FFMPEG_X86ASM=1
RUN bash /opt/build/scripts/setup-ffmpeg-lgpl.sh \
    && mkdir /ffmpeg-runtime \
    && cp -a /opt/ffmpeg/lib/libavcodec.so.* /opt/ffmpeg/lib/libavformat.so.* \
        /opt/ffmpeg/lib/libavutil.so.* /opt/ffmpeg/lib/libswscale.so.* /ffmpeg-runtime/

# The driver, with the toolchain and schema compilers SpiralDB's CLI image uses.
FROM rust:trixie AS driver
RUN apt-get update && apt-get install -y --no-install-recommends \
    autoconf automake build-essential curl git \
    libclang-dev libssl-dev libtool pkg-config \
    cmake unzip \
    && rm -rf /var/lib/apt/lists/*
COPY --from=ffmpeg /opt/ffmpeg /opt/ffmpeg
ENV FFMPEG_DIR=/opt/ffmpeg
ENV PKG_CONFIG_PATH=/opt/ffmpeg/lib/pkgconfig
ENV LD_LIBRARY_PATH=/opt/ffmpeg/lib
COPY --from=spiraldb scripts/setup-flatc.sh /tmp/setup-flatc.sh
RUN bash /tmp/setup-flatc.sh /usr/local && rm /tmp/setup-flatc.sh
ARG PROTOC_VERSION=26.1
RUN case "$(dpkg --print-architecture)" in \
        amd64) PROTOC_ARCH=x86_64 ;; \
        arm64) PROTOC_ARCH=aarch_64 ;; \
        *) echo "Unsupported Protobuf architecture." >&2; exit 1 ;; \
    esac \
    && curl -L --proto '=https' --tlsv1.2 -sSf \
        --output /tmp/protoc.zip \
        "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-${PROTOC_ARCH}.zip" \
    && unzip /tmp/protoc.zip -d /usr/local \
    && test -f /usr/local/include/google/protobuf/struct.proto \
    && rm /tmp/protoc.zip
ENV PROTOC=/usr/local/bin/protoc
ENV PROTOC_INCLUDE=/usr/local/include
WORKDIR /spiraldb
COPY --from=spiraldb rust-toolchain.toml .
RUN rustup toolchain install
COPY --from=spiraldb . .
# The video runtime is a cdylib the driver loads from its own directory at run
# time; the smoke example checks that loading in the final image.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/spiraldb/target \
    cargo build --locked --release \
        --package adbc-driver-spiraldb --package spiral-video-runtime --package spiral-video \
        --lib --example video_runtime_smoke \
    && mkdir /driver \
    && cp target/release/libadbc_driver_spiraldb.so target/release/libspiral_video_runtime.so \
        target/release/examples/video_runtime_smoke /driver/

# The Studio backend, with the toolchain rust-toolchain.toml pins.
FROM rust:trixie AS server
RUN apt-get update && apt-get install -y --no-install-recommends libclang-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /studio
COPY rust-toolchain.toml .
RUN rustup toolchain install
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/studio/target \
    cargo build --locked --release --package spiral-robotics-backend \
    && cp target/release/spiral-robotics-backend /usr/local/bin/

FROM debian:trixie-slim AS run
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3t64 libstdc++6 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 studio \
    && useradd --uid 10001 --gid 10001 --home-dir /var/lib/spiral-robotics --create-home \
        --shell /sbin/nologin studio
COPY --from=ffmpeg /ffmpeg-runtime/ /opt/ffmpeg/lib/
COPY --from=driver /driver/libadbc_driver_spiraldb.so /driver/libspiral_video_runtime.so /opt/spiral-robotics/driver/
# The backend serves the web interface from `public/` beside its executable.
COPY --from=server /usr/local/bin/spiral-robotics-backend /opt/spiral-robotics/
COPY --from=web . /opt/spiral-robotics/public/
# Studio's SQLite state and the Spiral sign-in (under ~/.config) live in HOME,
# the one volume.
ENV HOME=/var/lib/spiral-robotics
ENV SPIRAL_ROBOTICS_STUDIO_DB=/var/lib/spiral-robotics/studio.sqlite3
ENV SPIRALDB_ADBC_DRIVER=/opt/spiral-robotics/driver/libadbc_driver_spiraldb.so
ENV LD_LIBRARY_PATH=/opt/ffmpeg/lib
VOLUME /var/lib/spiral-robotics
USER 10001:10001
RUN --mount=type=bind,from=driver,source=/driver/video_runtime_smoke,target=/opt/spiral-robotics/driver/video_runtime_smoke \
    ["/opt/spiral-robotics/driver/video_runtime_smoke"]
EXPOSE 8080
# Studio drains and releases its resources on Ctrl+C.
STOPSIGNAL SIGINT
ENTRYPOINT ["/opt/spiral-robotics/spiral-robotics-backend", "--bind", "0.0.0.0:8080", "--allow-remote-access", "--no-cameras"]
