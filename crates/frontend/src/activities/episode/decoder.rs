//! Native WebCodecs bindings and playhead-following playback for every
//! recorded camera. Kept in Rust so decode cancellation and frame disposal
//! follow component lifetime.
use super::messages::MessageBuffer;
use base64::Engine as _;
use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use spiral_robotics_common::datasets::{EpisodeVideo, MessageRead};
use spiral_robotics_common::relations::EpisodeQueryRequest;
use std::{cell::RefCell, ops::Range, rc::Rc};
use wasm_bindgen::{JsCast as _, prelude::*};

// web-sys currently gates WebCodecs behind a global unstable cfg. These narrow
// IDL bindings use the standard browser API without changing workspace flags.
#[wasm_bindgen]
extern "C" {
    type VideoDecoder;
    #[wasm_bindgen(constructor, catch)]
    fn new(options: &Object) -> Result<VideoDecoder, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn configure(this: &VideoDecoder, config: &Object) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn decode(this: &VideoDecoder, chunk: &EncodedVideoChunk) -> Result<(), JsValue>;
    #[wasm_bindgen(method, catch)]
    fn flush(this: &VideoDecoder) -> Result<Promise, JsValue>;
    #[wasm_bindgen(method, catch)]
    fn close(this: &VideoDecoder) -> Result<(), JsValue>;
    #[wasm_bindgen(method, getter, js_name = decodeQueueSize)]
    fn queue_size(this: &VideoDecoder) -> u32;

    type EncodedVideoChunk;
    #[wasm_bindgen(constructor, catch)]
    fn new(options: &Object) -> Result<EncodedVideoChunk, JsValue>;

    type VideoFrame;
    #[wasm_bindgen(method)]
    fn close(this: &VideoFrame);
    #[wasm_bindgen(method, getter)]
    fn timestamp(this: &VideoFrame) -> f64;
    #[wasm_bindgen(method, getter, js_name = displayWidth)]
    fn width(this: &VideoFrame) -> u32;
    #[wasm_bindgen(method, getter, js_name = displayHeight)]
    fn height(this: &VideoFrame) -> u32;
    #[wasm_bindgen(method, getter, js_name = codedWidth)]
    fn coded_width(this: &VideoFrame) -> u32;
    #[wasm_bindgen(method, getter, js_name = codedHeight)]
    fn coded_height(this: &VideoFrame) -> u32;

    #[wasm_bindgen(extends = web_sys::CanvasRenderingContext2d)]
    type FrameCanvas;
    #[wasm_bindgen(method, structural, catch, js_name = drawImage)]
    fn draw_frame(this: &FrameCanvas, frame: &VideoFrame, x: f64, y: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(method, structural, catch, js_name = drawImage)]
    fn draw_frame_scaled(
        this: &FrameCanvas,
        frame: &VideoFrame,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<(), JsValue>;
}

struct CameraPacket {
    microseconds: i64,
    bytes: Vec<u8>,
    keyframe: bool,
}

pub(crate) struct EncodedCamera {
    codec: String,
    packets: Vec<CameraPacket>,
}
impl EncodedCamera {
    pub(crate) fn new(source: &EpisodeVideo, start_ns: u64) -> Result<Self, String> {
        let packets = source
            .packets()
            .iter()
            .map(|packet| {
                let relative = (i128::from(packet.timestamp_ns()) - i128::from(start_ns)) / 1_000;
                Ok(CameraPacket {
                    microseconds: i64::try_from(relative)
                        .map_err(|_| "Camera timestamp exceeds playback range")?,
                    bytes: base64::engine::general_purpose::STANDARD
                        .decode(packet.data())
                        .map_err(|_| "Invalid encoded camera packet")?,
                    keyframe: packet.keyframe(),
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self {
            codec: source.codec().to_owned(),
            packets,
        })
    }

    /// Every packet `request` reads, on a clock that starts at `origin`.
    pub(super) async fn read(request: MessageRead, origin: u64) -> Result<Self, String> {
        let messages = MessageBuffer::load(request).await?;
        let camera = Self::from_messages(messages.batches(), origin)?;
        if camera.packets.is_empty() {
            return Err("No recorded video in this episode".into());
        }
        Ok(camera)
    }

    /// Every packet one episode query lists, on a clock that starts at
    /// `origin`: a read its statement bounds, such as the group of pictures
    /// one still decodes. A query that lists none reads an empty camera.
    pub(super) async fn query(request: &EpisodeQueryRequest, origin: u64) -> Result<Self, String> {
        let messages = MessageBuffer::query(request).await?;
        Self::from_messages(messages.batches(), origin)
    }

    /// The packets `batches` hold, read as one episode query's rows, on a
    /// clock that starts at `origin`.
    pub(super) fn from_messages(
        batches: &[arrow_array::RecordBatch],
        origin: u64,
    ) -> Result<Self, String> {
        use arrow_array::Array as _;
        use spiral_robotics_common::datasets::H264AccessUnit;
        let mut camera = Self {
            codec: String::new(),
            packets: vec![],
        };
        for batch in batches {
            let time = batch
                .column_by_name("log_time")
                .and_then(|a| a.as_any().downcast_ref::<arrow_array::UInt64Array>())
                .ok_or("Missing video time")?;
            let format = batch
                .column_by_name("field_0")
                .ok_or("Missing video format")?;
            let data = batch
                .column_by_name("field_1")
                .ok_or("Missing video data")?;
            for row in 0..batch.num_rows() {
                if time.is_null(row) || data.is_null(row) {
                    return Err("Video packet is null".into());
                }
                if arrow_cast::display::array_value_to_string(format.as_ref(), row)
                    .map_err(|e| e.to_string())?
                    != "h264"
                {
                    return Err("Camera renderer requires H.264".into());
                }
                let bytes = if let Some(a) =
                    data.as_any().downcast_ref::<arrow_array::BinaryArray>()
                {
                    a.value(row)
                } else if let Some(a) = data
                    .as_any()
                    .downcast_ref::<arrow_array::LargeBinaryArray>()
                {
                    a.value(row)
                } else if let Some(a) = data.as_any().downcast_ref::<arrow_array::BinaryViewArray>()
                {
                    a.value(row)
                } else {
                    return Err("Video data must be binary".into());
                };
                let access = H264AccessUnit::parse(bytes)?;
                if camera.packets.is_empty() && !access.keyframe() {
                    return Err("Video must begin with a keyframe".into());
                }
                if let Some(codec) = access.codec() {
                    if !camera.codec.is_empty() && camera.codec != codec {
                        return Err("Video codec changes within this window".into());
                    }
                    camera.codec = codec.into();
                }
                let microseconds =
                    i64::try_from((i128::from(time.value(row)) - i128::from(origin)) / 1000)
                        .map_err(|e| e.to_string())?;
                if camera
                    .packets
                    .last()
                    .is_some_and(|p| p.microseconds >= microseconds)
                {
                    return Err("Video packet timestamps must increase".into());
                }
                camera.packets.push(CameraPacket {
                    microseconds,
                    bytes: bytes.to_vec(),
                    keyframe: access.keyframe(),
                });
            }
        }
        Ok(camera)
    }

    /// The frame on screen at `seconds`: the last one logged by then, or the
    /// first before it.
    pub(super) fn frame_at(&self, seconds: f64) -> Option<usize> {
        let logged = self
            .packets
            .partition_point(|packet| packet.microseconds as f64 <= seconds * 1e6);
        (!self.packets.is_empty()).then(|| logged.saturating_sub(1))
    }
    /// When `frame` was logged, in seconds from the episode's start.
    pub(super) fn frame_time(&self, frame: usize) -> f64 {
        self.packets[frame].microseconds as f64 / 1e6
    }

    pub(super) fn frame_seconds(&self, seconds: f64) -> Option<f64> {
        let index = self
            .packets
            .partition_point(|packet| packet.microseconds as f64 <= seconds * 1e6)
            .checked_sub(1)?;
        Some(self.packets[index].microseconds as f64 / 1e6)
    }
    fn first_frame_seconds(&self) -> Option<f64> {
        self.packets
            .first()
            .map(|packet| packet.microseconds as f64 / 1e6)
    }
    /// When the last packet was logged: the frame a read that ends on it,
    /// such as a still's, is for.
    pub(super) fn last_frame_seconds(&self) -> Option<f64> {
        self.packets
            .last()
            .map(|packet| packet.microseconds as f64 / 1e6)
    }
    /// The first recorded frame when `seconds` precedes it: the picture a
    /// camera holds at the start of an episode instead of an empty mat.
    pub(super) fn first_frame_after(&self, seconds: f64) -> Option<f64> {
        self.first_frame_seconds().filter(|first| seconds < *first)
    }
    /// The moment to decode for `seconds`, never earlier than the first frame.
    pub(super) fn held_frame_seconds(&self, seconds: f64) -> f64 {
        self.first_frame_after(seconds).unwrap_or(seconds)
    }

    fn group_at(&self, seconds: f64) -> Option<Range<usize>> {
        let timestamp = (seconds * 1e6).round() as i64;
        let index = self
            .packets
            .partition_point(|packet| packet.microseconds <= timestamp)
            .checked_sub(1)?;
        let start = self.packets[..=index]
            .iter()
            .rposition(|packet| packet.keyframe)?;
        let end = self.packets[start + 1..]
            .iter()
            .position(|packet| packet.keyframe)
            .map(|offset| start + 1 + offset)
            .unwrap_or(self.packets.len());
        Some(start..end)
    }
}

/// One camera's packets are read once per open episode; a component compares
/// the buffer it was handed by identity rather than by every packet in it.
impl PartialEq for EncodedCamera {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

/// A decoded frame and the bytes it occupies, released together.
struct DecodedFrame(VideoFrame, u64);
impl Drop for DecodedFrame {
    fn drop(&mut self) {
        self.0.close();
    }
}

const RETAIN_BEFORE_US: f64 = 250_000.0;
const LOOKAHEAD_PACKETS: usize = 6;
const FRAME_WAIT_ROUNDS: usize = 60;
/// A keyframe interval whose decoded frames exceed this cannot be held in
/// memory at once, so the camera says so rather than crashing the tab.
const DECODED_INTERVAL_LIMIT: u64 = 128 * 1024 * 1024;

/// Follows a playhead through one camera's packets: forward
/// playback decodes each frame once, a jump restarts from the preceding
/// keyframe, and only frames near the playhead stay in memory. That is what
/// lets every camera of an episode stay visible at once.
pub(crate) struct CameraPlayback {
    camera: Rc<EncodedCamera>,
    output: Rc<RefCell<DecoderOutput>>,
    session: Option<DecoderSession>,
    group: Range<usize>,
    fed: usize,
    flushed: bool,
}

impl CameraPlayback {
    pub(crate) fn new(camera: Rc<EncodedCamera>) -> Self {
        Self {
            camera,
            output: Rc::new(RefCell::new(DecoderOutput::default())),
            session: None,
            group: 0..0,
            fed: 0,
            flushed: false,
        }
    }

    /// The packet whose frame shows at `seconds`, within its keyframe interval.
    fn target(&self, seconds: f64) -> Option<(Range<usize>, usize)> {
        let range = self.camera.group_at(seconds)?;
        let timestamp = (seconds * 1e6).round() as i64;
        let within = self.camera.packets[range.clone()]
            .partition_point(|packet| packet.microseconds <= timestamp);
        Some((range.clone(), range.start + within.checked_sub(1)?))
    }

    fn needs_restart(&self, range: &Range<usize>, target_index: usize) -> bool {
        let target_timestamp = self.camera.packets[target_index].microseconds as f64;
        self.session.is_none()
            || *range != self.group
            || (target_index < self.fed && !self.output.borrow().has_frame_at(target_timestamp))
    }

    /// How many packets a move to `seconds` must decode before its frame,
    /// or `None` when the decoder can simply continue.
    pub(super) fn restart_cost(&self, seconds: f64) -> Option<usize> {
        let (range, target_index) = self.target(seconds)?;
        self.needs_restart(&range, target_index)
            .then_some(target_index - range.start)
    }

    fn restart(&mut self, range: Range<usize>) -> Result<(), String> {
        self.session = None;
        self.output.borrow_mut().clear();
        let session = DecoderSession::new(self.output.clone()).map_err(DecoderSession::error)?;
        let config = Object::new();
        Reflect::set(&config, &"codec".into(), &self.camera.codec.clone().into())
            .map_err(DecoderSession::error)?;
        Reflect::set(&config, &"optimizeForLatency".into(), &true.into())
            .map_err(DecoderSession::error)?;
        session
            .decoder
            .configure(&config)
            .map_err(DecoderSession::error)?;
        self.session = Some(session);
        self.fed = range.start;
        self.group = range;
        self.flushed = false;
        Ok(())
    }

    /// Decode up to the frame for `seconds`, releasing frames well behind it.
    pub(crate) async fn prepare(&mut self, seconds: f64) -> Result<(), String> {
        let Some((range, target_index)) = self.target(seconds) else {
            self.session = None;
            self.output.borrow_mut().clear();
            return Ok(());
        };
        if self.needs_restart(&range, target_index) {
            self.restart(range.clone())?;
        }
        let target_timestamp = self.camera.packets[target_index].microseconds as f64;
        self.output
            .borrow_mut()
            .release_before(target_timestamp - RETAIN_BEFORE_US);
        let feed_until = (target_index + 1 + LOOKAHEAD_PACKETS).min(range.end);
        while self.fed < feed_until {
            let session = self.session.as_ref().ok_or("Camera decoder is not open")?;
            session
                .push(&self.camera.packets[self.fed])
                .map_err(DecoderSession::error)?;
            self.fed += 1;
            while session.decoder.queue_size() > 8 {
                gloo_timers::future::TimeoutFuture::new(1).await;
                self.output.borrow().check()?;
            }
        }
        for _ in 0..FRAME_WAIT_ROUNDS {
            self.output.borrow().check()?;
            if self.output.borrow().has_frame_at(target_timestamp) {
                return Ok(());
            }
            if self.fed >= range.end && !self.flushed {
                // The last frames of an interval only leave the decoder on flush;
                // afterwards it needs a keyframe, which the next interval starts with.
                self.flushed = true;
                let session = self.session.as_ref().ok_or("Camera decoder is not open")?;
                let flush = session.decoder.flush().map_err(DecoderSession::error)?;
                wasm_bindgen_futures::JsFuture::from(flush)
                    .await
                    .map_err(DecoderSession::error)?;
                continue;
            }
            gloo_timers::future::TimeoutFuture::new(4).await;
        }
        self.output.borrow().check()
    }

    /// Paint the latest retained frame at or before `seconds`; `false` clears.
    pub(crate) fn draw(
        &self,
        canvas: &web_sys::HtmlCanvasElement,
        seconds: f64,
    ) -> Result<bool, String> {
        match self.with_frame(seconds, |frame| Self::paint(canvas, frame)) {
            Some(painted) => painted.map(|()| true),
            None => Self::clear(canvas).map(|()| false),
        }
    }

    /// The frame on screen at `seconds` as a still covering `cover`, a width
    /// and height in CSS pixels, at the frame's own aspect.
    pub(super) async fn still(
        &mut self,
        seconds: f64,
        cover: (f64, f64),
    ) -> Result<CameraStill, String> {
        self.prepare(seconds).await?;
        self.with_frame(seconds, |frame| CameraStill::of(frame, cover))
            .unwrap_or_else(|| Err("No frame at this time".into()))
    }

    /// `paint` applied to the latest retained frame at or before `seconds`.
    fn with_frame<R>(&self, seconds: f64, paint: impl FnOnce(&VideoFrame) -> R) -> Option<R> {
        let timestamp = (seconds * 1e6).round();
        let output = self.output.borrow();
        output
            .frames
            .iter()
            .rev()
            .find(|frame| frame.0.timestamp() <= timestamp)
            .map(|frame| paint(&frame.0))
    }

    fn paint(canvas: &web_sys::HtmlCanvasElement, frame: &VideoFrame) -> Result<(), String> {
        if canvas.width() != frame.width() {
            canvas.set_width(frame.width());
        }
        if canvas.height() != frame.height() {
            canvas.set_height(frame.height());
        }
        Self::context(canvas)?
            .draw_frame(frame, 0.0, 0.0)
            .map_err(DecoderSession::error)
    }

    fn clear(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
        Self::context(canvas)?.clear_rect(0.0, 0.0, canvas.width().into(), canvas.height().into());
        Ok(())
    }
    fn context(canvas: &web_sys::HtmlCanvasElement) -> Result<FrameCanvas, String> {
        canvas
            .get_context("2d")
            .map_err(DecoderSession::error)?
            .ok_or_else(|| "Camera canvas is unavailable".to_owned())
            .map(JsCast::unchecked_into)
    }
}

/// One frame drawn small, such as a cell of a camera's filmstrip or the still
/// a row of episodes leads with.
pub(super) struct CameraStill {
    /// The picture as a JPEG `data:` URL.
    url: String,
    /// The frame's own width and height, in pixels, as it displays.
    frame_size: (u32, u32),
}

impl CameraStill {
    /// `frame` drawn at its own aspect, just large enough to cover `cover`,
    /// in CSS pixels, at the screen's density so it is sharp there. A box of
    /// no width is covered by the still's height alone.
    fn of(frame: &VideoFrame, (cover_width, cover_height): (f64, f64)) -> Result<Self, String> {
        let window = web_sys::window().ok_or("No window to draw a still in")?;
        let frame_size = (frame.width().max(1), frame.height().max(1));
        let (frame_width, frame_height) = (f64::from(frame_size.0), f64::from(frame_size.1));
        let scale = window.device_pixel_ratio().clamp(1.0, 3.0)
            * (cover_width / frame_width).max(cover_height / frame_height);
        let width = (frame_width * scale).round().max(1.0);
        let height = (frame_height * scale).round().max(1.0);
        let canvas = window
            .document()
            .ok_or("No document to draw a still in")?
            .create_element("canvas")
            .map_err(DecoderSession::error)?
            .unchecked_into::<web_sys::HtmlCanvasElement>();
        canvas.set_width(width as u32);
        canvas.set_height(height as u32);
        CameraPlayback::context(&canvas)?
            .draw_frame_scaled(frame, 0.0, 0.0, width, height)
            .map_err(DecoderSession::error)?;
        let url = canvas
            .to_data_url_with_type("image/jpeg")
            .map_err(DecoderSession::error)?;
        Ok(Self { url, frame_size })
    }
    pub(super) fn url(&self) -> &str {
        &self.url
    }
    /// The frame's width over its height.
    pub(super) fn aspect(&self) -> f64 {
        f64::from(self.frame_size.0) / f64::from(self.frame_size.1)
    }
    pub(super) fn frame_size(&self) -> (u32, u32) {
        self.frame_size
    }
}

struct DecoderOutput {
    frames: Vec<DecodedFrame>,
    bytes: u64,
    error: Option<String>,
    /// Frames stamped before this moment are released as soon as they decode.
    retain_after: f64,
}
impl Default for DecoderOutput {
    fn default() -> Self {
        Self {
            frames: Vec::new(),
            bytes: 0,
            error: None,
            retain_after: f64::NEG_INFINITY,
        }
    }
}
impl DecoderOutput {
    fn push(&mut self, frame: VideoFrame) {
        if frame.timestamp() < self.retain_after {
            frame.close();
            return;
        }
        // Four bytes per coded pixel conservatively bounds common H.264 surfaces.
        let size = u64::from(frame.coded_width()) * u64::from(frame.coded_height()) * 4;
        let frame = DecodedFrame(frame, size);
        self.bytes = self.bytes.saturating_add(size);
        if self.bytes > DECODED_INTERVAL_LIMIT {
            self.error =
                Some("Camera keyframe interval exceeds the 128 MiB decoded playback limit".into());
        } else if self.error.is_none() {
            self.frames.push(frame);
        }
    }
    fn release_before(&mut self, cutoff: f64) {
        self.retain_after = cutoff;
        let released = self
            .frames
            .iter()
            .filter(|frame| frame.0.timestamp() < cutoff)
            .map(|frame| frame.1)
            .sum::<u64>();
        self.frames.retain(|frame| frame.0.timestamp() >= cutoff);
        self.bytes = self.bytes.saturating_sub(released);
    }
    fn clear(&mut self) {
        self.frames.clear();
        self.bytes = 0;
        self.error = None;
        self.retain_after = f64::NEG_INFINITY;
    }
    fn has_frame_at(&self, timestamp: f64) -> bool {
        self.frames
            .iter()
            .any(|frame| frame.0.timestamp() == timestamp)
    }
    fn check(&self) -> Result<(), String> {
        self.error.clone().map_or(Ok(()), Err)
    }
}

struct DecoderSession {
    decoder: VideoDecoder,
    _output: Closure<dyn FnMut(VideoFrame)>,
    _error: Closure<dyn FnMut(JsValue)>,
}
impl DecoderSession {
    fn new(output: Rc<RefCell<DecoderOutput>>) -> Result<Self, JsValue> {
        if !Reflect::get(&js_sys::global(), &"VideoDecoder".into())?.is_function() {
            return Err(JsValue::from_str(
                "H.264 playback needs a browser with WebCodecs over HTTPS or localhost",
            ));
        }
        let errors = output.clone();
        let on_output =
            Closure::<dyn FnMut(VideoFrame)>::new(move |frame| output.borrow_mut().push(frame));
        let on_error = Closure::<dyn FnMut(JsValue)>::new(move |error| {
            errors.borrow_mut().error = Some(Self::error(error))
        });
        let options = Object::new();
        Reflect::set(
            &options,
            &"output".into(),
            on_output.as_ref().unchecked_ref::<Function>(),
        )?;
        Reflect::set(
            &options,
            &"error".into(),
            on_error.as_ref().unchecked_ref::<Function>(),
        )?;
        let decoder = VideoDecoder::new(&options)?;
        Ok(Self {
            decoder,
            _output: on_output,
            _error: on_error,
        })
    }
    fn push(&self, packet: &CameraPacket) -> Result<(), JsValue> {
        let options = Object::new();
        Reflect::set(
            &options,
            &"type".into(),
            &if packet.keyframe { "key" } else { "delta" }.into(),
        )?;
        Reflect::set(
            &options,
            &"timestamp".into(),
            &JsValue::from_f64(packet.microseconds as f64),
        )?;
        Reflect::set(
            &options,
            &"data".into(),
            &Uint8Array::from(packet.bytes.as_slice()),
        )?;
        self.decoder.decode(&EncodedVideoChunk::new(&options)?)
    }
    fn error(error: JsValue) -> String {
        error
            .as_string()
            .or_else(|| {
                Reflect::get(&error, &"message".into())
                    .ok()
                    .and_then(|value| value.as_string())
            })
            .unwrap_or_else(|| format!("Browser video decoder is unavailable: {error:?}"))
    }
}
impl Drop for DecoderSession {
    fn drop(&mut self) {
        let _ = self.decoder.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiral_robotics_common::datasets::VideoPacket;

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn recorded_video_messages_become_one_packet_each() {
        use arrow_array::{ArrayRef, BinaryArray, RecordBatch, StringArray, UInt64Array};
        use std::sync::Arc;
        let bytes = include_bytes!("fixtures/seek.h264");
        let batch = RecordBatch::try_from_iter(vec![
            (
                "log_time",
                Arc::new(UInt64Array::from(vec![1000])) as ArrayRef,
            ),
            ("field_0", Arc::new(StringArray::from(vec!["h264"]))),
            (
                "field_1",
                Arc::new(BinaryArray::from(vec![bytes.as_slice()])),
            ),
        ])
        .unwrap();
        let camera = EncodedCamera::from_messages(&[batch], 1000).unwrap();
        assert_eq!(camera.packets[0].bytes, bytes);
        assert_eq!(camera.packets[0].microseconds, 0);
        assert_eq!(camera.last_frame_seconds(), Some(0.0));
        // A read that lists no packet is an empty camera, not a failure.
        assert_eq!(
            EncodedCamera::from_messages(&[], 1000)
                .unwrap()
                .last_frame_seconds(),
            None
        );
    }

    /// The fixture's eight 32×32 access units, a frame every 33 ms, seekable
    /// at every `keyframe_every`th one.
    fn seek_fixture(keyframe_every: usize) -> EncodedCamera {
        let bytes = include_bytes!("fixtures/seek.h264");
        let mut offsets = bytes
            .windows(5)
            .enumerate()
            .filter_map(|(index, bytes)| (bytes == [0, 0, 0, 1, 9]).then_some(index))
            .collect::<Vec<_>>();
        offsets.push(bytes.len());
        assert_eq!(offsets.len(), 9);
        let packets = offsets
            .windows(2)
            .enumerate()
            .map(|(index, range)| CameraPacket {
                microseconds: index as i64 * 33_333,
                bytes: bytes[range[0]..range[1]].to_vec(),
                keyframe: index % keyframe_every == 0,
            })
            .collect();
        EncodedCamera {
            codec: "avc1.42c00a".into(),
            packets,
        }
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn independent_keyframe_decode_matches_continuous_decode() {
        let canvas = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .create_element("canvas")
            .unwrap()
            .unchecked_into::<web_sys::HtmlCanvasElement>();
        let mut pixels = Vec::new();
        // The same eight access units, seekable at every fourth one or only
        // from the first, which decodes them in one run.
        for keyframe_every in [8, 4] {
            let mut playback = CameraPlayback::new(Rc::new(seek_fixture(keyframe_every)));
            playback.prepare(0.2).await.unwrap();
            assert!(playback.draw(&canvas, 0.2).unwrap());
            pixels.push(
                CameraPlayback::context(&canvas)
                    .unwrap()
                    .get_image_data(0.0, 0.0, 32.0, 32.0)
                    .unwrap()
                    .data(),
            );
        }
        assert_eq!(pixels[0], pixels[1]);
        assert!(pixels[1].iter().any(|value| *value > 0));
        assert_eq!(canvas.width(), 32);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn a_still_decodes_from_the_keyframe_before_its_frame_alone() {
        // A still's read: its frame and the packets back to the keyframe
        // before it, which draw what the whole recording draws there.
        let mut group = seek_fixture(4);
        group.packets.drain(..4);
        group.packets.truncate(3);
        let shown = group.last_frame_seconds().unwrap();
        assert_eq!(shown, 0.199998);
        let slot = (96.0, 54.0);
        let alone = CameraPlayback::new(Rc::new(group))
            .still(shown, slot)
            .await
            .unwrap();
        let whole = CameraPlayback::new(Rc::new(seek_fixture(8)))
            .still(shown, slot)
            .await
            .unwrap();
        assert_eq!(alone.frame_size(), (32, 32));
        assert_eq!(alone.aspect(), 1.0);
        assert!(alone.url().starts_with("data:image/jpeg;base64,"));
        assert_eq!(alone.url(), whole.url());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    fn seeking_uses_the_preceding_keyframe_and_preserves_epoch_precision() {
        let epoch = 1_786_639_635_890_000_000;
        let packets = (0..90)
            .map(|index| {
                VideoPacket::new(epoch + index * 33_333_333, "AA==".into(), index % 30 == 0)
            })
            .collect();
        let camera =
            EncodedCamera::new(&EpisodeVideo::new("avc1.64001e".into(), packets), epoch).unwrap();
        assert_eq!(camera.group_at(-0.001), None);
        assert_eq!(camera.group_at(0.0), Some(0..30));
        assert_eq!(camera.group_at(1.8), Some(30..60));
        assert_eq!(camera.group_at(0.5), Some(0..30));
        assert_eq!(camera.group_at(3.0), Some(60..90));
        assert_eq!(camera.packets[1].microseconds, 33_333);
    }
}
