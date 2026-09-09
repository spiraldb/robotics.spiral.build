//! Native WebCodecs bindings and ownership of one decoded keyframe interval.
//! Kept in Rust so decode cancellation and frame disposal follow component lifetime.
use base64::Engine as _;
use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use spiral_robotics_common::datasets::EpisodeVideo;
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
                let timestamp = packet
                    .timestamp_ns()
                    .parse::<u64>()
                    .map_err(|_| "Invalid camera timestamp")?;
                let relative = (i128::from(timestamp) - i128::from(start_ns)) / 1_000;
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

    pub(crate) fn first_frame_seconds(&self) -> f64 {
        self.packets
            .first()
            .map(|packet| packet.microseconds.max(0) as f64 / 1e6)
            .unwrap_or_default()
    }

    pub(crate) fn group_at(&self, seconds: f64) -> Option<Range<usize>> {
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

struct DecodedFrame(VideoFrame);
impl Drop for DecodedFrame {
    fn drop(&mut self) {
        self.0.close();
    }
}

pub(crate) struct DecodedGroup {
    frames: Vec<DecodedFrame>,
}
impl DecodedGroup {
    pub(crate) async fn decode(
        camera: &EncodedCamera,
        range: Range<usize>,
    ) -> Result<Self, String> {
        let output = Rc::new(RefCell::new(DecoderOutput::default()));
        let session = DecoderSession::new(output.clone()).map_err(DecoderSession::error)?;
        let config = Object::new();
        Reflect::set(&config, &"codec".into(), &camera.codec.clone().into())
            .map_err(DecoderSession::error)?;
        Reflect::set(&config, &"optimizeForLatency".into(), &true.into())
            .map_err(DecoderSession::error)?;
        session
            .decoder
            .configure(&config)
            .map_err(DecoderSession::error)?;
        for packet in &camera.packets[range] {
            session.push(packet).map_err(DecoderSession::error)?;
            while session.decoder.queue_size() > 8 {
                gloo_timers::future::TimeoutFuture::new(1).await;
                output.borrow().check()?;
            }
        }
        wasm_bindgen_futures::JsFuture::from(
            session.decoder.flush().map_err(DecoderSession::error)?,
        )
        .await
        .map_err(DecoderSession::error)?;
        output.borrow().check()?;
        let mut frames = std::mem::take(&mut output.borrow_mut().frames);
        frames.sort_by(|left, right| left.0.timestamp().total_cmp(&right.0.timestamp()));
        if frames.is_empty() {
            return Err("The camera decoder returned no frames".into());
        }
        Ok(Self { frames })
    }

    pub(crate) fn draw(
        &self,
        canvas: &web_sys::HtmlCanvasElement,
        seconds: f64,
    ) -> Result<(), String> {
        let timestamp = (seconds * 1e6).round();
        let Some(frame) = self
            .frames
            .iter()
            .rev()
            .find(|frame| frame.0.timestamp() <= timestamp)
        else {
            return Self::clear(canvas);
        };
        if canvas.width() != frame.0.width() {
            canvas.set_width(frame.0.width());
        }
        if canvas.height() != frame.0.height() {
            canvas.set_height(frame.0.height());
        }
        let context = Self::context(canvas)?;
        context
            .draw_frame(&frame.0, 0.0, 0.0)
            .map_err(DecoderSession::error)
    }

    pub(crate) fn clear(canvas: &web_sys::HtmlCanvasElement) -> Result<(), String> {
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

#[derive(Default)]
struct DecoderOutput {
    frames: Vec<DecodedFrame>,
    bytes: u64,
    error: Option<String>,
}
impl DecoderOutput {
    fn push(&mut self, frame: VideoFrame) {
        let frame = DecodedFrame(frame);
        // Four bytes per coded pixel conservatively bounds common H.264 surfaces.
        self.bytes = self.bytes.saturating_add(
            u64::from(frame.0.coded_width()) * u64::from(frame.0.coded_height()) * 4,
        );
        if self.bytes > 128 * 1024 * 1024 {
            self.error =
                Some("Camera keyframe interval exceeds the 128 MiB decoded playback limit".into());
        } else if self.error.is_none() {
            self.frames.push(frame);
        }
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
    async fn independent_keyframe_decode_matches_continuous_decode() {
        let bytes = include_bytes!("fixtures/seek.h264");
        let mut offsets = bytes
            .windows(5)
            .enumerate()
            .filter_map(|(index, bytes)| (bytes == [0, 0, 0, 1, 9]).then_some(index))
            .collect::<Vec<_>>();
        offsets.push(bytes.len());
        let packets = offsets
            .windows(2)
            .enumerate()
            .map(|(index, range)| CameraPacket {
                microseconds: index as i64 * 33_333,
                bytes: bytes[range[0]..range[1]].to_vec(),
                keyframe: index % 4 == 0,
            })
            .collect::<Vec<_>>();
        assert_eq!(packets.len(), 8);
        let camera = EncodedCamera {
            codec: "avc1.42c00a".into(),
            packets,
        };
        let canvas = web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .create_element("canvas")
            .unwrap()
            .unchecked_into::<web_sys::HtmlCanvasElement>();
        let continuous = DecodedGroup::decode(&camera, 0..8).await.unwrap();
        continuous.draw(&canvas, 0.2).unwrap();
        let expected = DecodedGroup::context(&canvas)
            .unwrap()
            .get_image_data(0.0, 0.0, 32.0, 32.0)
            .unwrap()
            .data();
        let seek = DecodedGroup::decode(&camera, 4..8).await.unwrap();
        seek.draw(&canvas, 0.2).unwrap();
        let actual = DecodedGroup::context(&canvas)
            .unwrap()
            .get_image_data(0.0, 0.0, 32.0, 32.0)
            .unwrap()
            .data();
        assert_eq!(actual, expected);
        assert!(actual.iter().any(|value| *value > 0));
        assert_eq!(canvas.width(), 32);
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
