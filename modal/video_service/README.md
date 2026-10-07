# video-service

The HTTP contract every Modal video service speaks with Studio's backend, the backend's
`crates/backend/src/curation/video_service.rs`, shared by [`warp-progress`](../warp_progress/README.md)
and [`object-tracking`](../object_tracking/README.md).

- `video_service/service.py`: `VideoService` serves one `POST` route for callers holding the bearer
  token, and an open `GET /health`. A request is an Arrow IPC stream of one episode's packets as
  `payload` binary, `decode_index` int64 and `pts_ns` int64, with schema metadata `codec=h264`,
  `framing=annex_b`, the service's `model`, and `implementation`, the revision the request was built
  for. Any revision but the service's gets a 409, so Studio never stores answers from another one; a
  video the service cannot answer for gets a 422, and a `RetryLater` failure a 503. The answer is one
  Arrow record batch, with the request's stages in a `Server-Timing` header and the same line logged.
- `video_service/episode.py`: `EpisodeVideo` decodes the packets with PyAV into display order, each
  picture carrying its packet's time, and refuses a stream that decodes to fewer pictures than it codes.
- `video_service/timing.py`: the per-stage timer behind `Server-Timing`.
- `video_service/testing.py`: a small synthetic episode and its request body, for the services' tests.

Each service takes it as an editable path dependency. Its Modal image skips that dependency in
`uv_sync` and copies the source in with `add_local_python_source`, so a deploy ships the code as it is.

```sh
uv sync
uv run pytest
```
