//! Contact sheets: the pictures of one episode the model reads, sampled at a
//! fixed interval and tiled chronologically, left to right then top to
//! bottom, five across and four down.
//!
//! Every tile is 224 px wide at the recording's own aspect ratio, and a band
//! at its foot carries the picture's time in seconds from the first picture,
//! which is the time the model answers in.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::{FilterType, replace, resize};
use image::{Rgb, RgbImage};

/// Tiles across one sheet.
const COLUMNS: u32 = 5;
/// Tile rows down one sheet.
const ROWS: u32 = 4;
/// Pictures one sheet carries: ten seconds at the default half-second
/// interval.
pub(super) const FRAMES_PER_SHEET: usize = (COLUMNS * ROWS) as usize;
const TILE_WIDTH: u32 = 224;
/// The band at a tile's foot that carries its time.
const STAMP_HEIGHT: u32 = 16;
const STAMP_INSET: u32 = 4;
/// Enough to read a gripper against cloth, small enough that an episode fits
/// in one request.
const JPEG_QUALITY: u8 = 70;
const SHEET_BACKGROUND: Rgb<u8> = Rgb([0x0f, 0x17, 0x2a]);
const STAMP_BACKGROUND: Rgb<u8> = Rgb([0, 0, 0]);
const STAMP_INK: Rgb<u8> = Rgb([0xff, 0xff, 0xff]);

/// Where one video's pictures sit on its presentation timeline.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct VideoTimeline {
    first_ns: u64,
    duration_ns: u64,
}

impl VideoTimeline {
    /// The timeline of pictures presented at `times_ns`, ascending. It runs
    /// from the first picture for one mean picture period past the last, so
    /// the last picture is inside it.
    pub(super) fn new(times_ns: &[i128]) -> Result<Self, String> {
        let (Some(first), Some(last)) = (times_ns.first(), times_ns.last()) else {
            return Err("the video has no pictures".to_owned());
        };
        if times_ns.len() < 2 || last <= first {
            return Err("the video has a single moment, so there is nothing to break down".into());
        }
        let first_ns = u64::try_from(*first)
            .map_err(|_| format!("picture time {first} ns precedes the video's timeline"))?;
        let span = u64::try_from(last - first)
            .map_err(|_| "the video's pictures span more than u64 nanoseconds".to_owned())?;
        let period = span / (times_ns.len() as u64 - 1);
        Ok(Self {
            first_ns,
            duration_ns: span.saturating_add(period),
        })
    }

    /// Seconds from the first picture to the end of the timeline.
    pub(super) fn duration_seconds(self) -> f64 {
        self.duration_ns as f64 / 1e9
    }

    /// The presentation time, in nanoseconds, `seconds` after the first
    /// picture, kept on the timeline.
    pub(super) fn presentation_ns(self, seconds: f64) -> u64 {
        let offset = (seconds * 1e9).round().clamp(0.0, self.duration_ns as f64) as u64;
        self.first_ns + offset
    }

    fn seconds_since_first(self, time_ns: i128) -> f64 {
        (time_ns - i128::from(self.first_ns)) as f64 / 1e9
    }
}

/// The pictures one episode is sampled at: every `interval` from the first
/// picture while the timeline lasts, each landing on the nearest picture,
/// each picture taken at most once.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SamplePlan {
    positions: Vec<usize>,
    seconds: Vec<f64>,
}

impl SamplePlan {
    pub(super) fn new(
        timeline: VideoTimeline,
        times_ns: &[i128],
        interval_seconds: f64,
    ) -> Result<Self, String> {
        if !interval_seconds.is_finite() || interval_seconds <= 0.0 {
            return Err("the sample interval must be a positive number of seconds".to_owned());
        }
        let count = (timeline.duration_seconds() / interval_seconds)
            .ceil()
            .clamp(1.0, f64::from(u16::MAX)) as usize;
        let mut plan = Self {
            positions: Vec::with_capacity(count),
            seconds: Vec::with_capacity(count),
        };
        for sample in 0..count {
            let wanted =
                i128::from(timeline.first_ns) + (sample as f64 * interval_seconds * 1e9) as i128;
            let after = times_ns.partition_point(|time| *time < wanted);
            let position = match (after.checked_sub(1), times_ns.get(after)) {
                (Some(before), Some(next)) if wanted - times_ns[before] <= next - wanted => before,
                (Some(before), None) => before,
                _ => after,
            };
            // Samples only move forward, so a repeat is the picture just taken.
            if plan.positions.last() == Some(&position) {
                continue;
            }
            plan.positions.push(position);
            plan.seconds
                .push(timeline.seconds_since_first(times_ns[position]));
        }
        Ok(plan)
    }

    /// The sampled pictures' display-order positions, ascending.
    pub(super) fn positions(&self) -> &[usize] {
        &self.positions
    }

    /// Each sampled picture's time, in seconds from the first picture.
    pub(super) fn seconds(&self) -> &[f64] {
        &self.seconds
    }
}

/// One rendered sheet: the span of the episode it covers and the JPEG the
/// model reads.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ContactSheet {
    span: (f64, f64),
    data_url: String,
}

impl ContactSheet {
    /// The sheet as a `data:image/jpeg;base64,…` URL.
    pub(super) fn data_url(&self) -> &str {
        &self.data_url
    }

    /// What the request spends on this sheet, as it is sent.
    pub(super) fn bytes(&self) -> usize {
        self.data_url.len()
    }

    /// What the model is told about this sheet before it sees it.
    pub(super) fn caption(&self, index: usize, total: usize) -> String {
        format!(
            "Contact sheet {} of {total}, covering {:.1} s to {:.1} s of this episode.",
            index + 1,
            self.span.0,
            self.span.1
        )
    }

    /// What one tile's band reads: the seconds the model answers in.
    fn stamp_label(seconds: f64) -> String {
        format!("{seconds:.1}s")
    }
}

/// The sheets of one episode's sampled pictures, each drawn as its picture
/// is decoded, in order, onto sheets of twenty tiles. The first picture
/// settles the tile shape, so every sheet keeps the recording's aspect
/// ratio; a sheet is encoded once its last tile is drawn, so only the sheet
/// being drawn is held as pixels.
pub(super) struct ContactSheets {
    /// Each sampled picture's time, in seconds from the first picture.
    seconds: Vec<f64>,
    shape: Option<(u32, u32)>,
    canvas: Option<SheetCanvas>,
    drawn: usize,
    sheets: Vec<ContactSheet>,
}

impl ContactSheets {
    /// The sheets for pictures sampled at `seconds`.
    pub(super) fn new(seconds: Vec<f64>) -> Self {
        Self {
            seconds,
            shape: None,
            canvas: None,
            drawn: 0,
            sheets: Vec::new(),
        }
    }

    /// Draw the next sampled picture into its tile.
    pub(super) fn draw(&mut self, picture: &RgbImage) -> Result<(), String> {
        let index = self.drawn;
        let seconds = *self
            .seconds
            .get(index)
            .ok_or("more pictures were decoded than were sampled")?;
        let shape = *self
            .shape
            .get_or_insert((picture.width(), picture.height()));
        let tile = index % FRAMES_PER_SHEET;
        let mut canvas = match self.canvas.take() {
            Some(canvas) if tile > 0 => canvas,
            _ => {
                let tiles = (self.seconds.len() - index).min(FRAMES_PER_SHEET);
                SheetCanvas::open(SheetLayout::new(shape, tiles))
            }
        };
        canvas.tile(tile, picture, seconds);
        self.drawn += 1;
        if tile + 1 < canvas.layout.tiles {
            self.canvas = Some(canvas);
            return Ok(());
        }
        self.sheets.push(ContactSheet {
            span: (self.seconds[index - tile], seconds),
            data_url: canvas.encode()?,
        });
        Ok(())
    }

    /// Every sheet, once each sampled picture is drawn.
    pub(super) fn finish(self) -> Result<Vec<ContactSheet>, String> {
        if self.seconds.is_empty() {
            return Err("no pictures could be sampled from the video".to_owned());
        }
        if self.drawn < self.seconds.len() {
            return Err(format!(
                "only {} of the {} sampled pictures were decoded",
                self.drawn,
                self.seconds.len()
            ));
        }
        Ok(self.sheets)
    }
}

/// Where each tile of one sheet is drawn, in sheet pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct SheetLayout {
    picture_height: u32,
    tiles: usize,
}

impl SheetLayout {
    /// A sheet for `tiles` pictures of `frame` pixels: tiles keep the
    /// recording's aspect ratio at a fixed width, and each reserves a band
    /// for its time.
    fn new(frame: (u32, u32), tiles: usize) -> Self {
        let picture_height = match frame {
            (width, height) if width > 0 && height > 0 => {
                (f64::from(TILE_WIDTH) * f64::from(height) / f64::from(width)).round() as u32
            }
            _ => TILE_WIDTH * 9 / 16,
        };
        Self {
            picture_height: picture_height.max(1),
            tiles: tiles.min(FRAMES_PER_SHEET),
        }
    }

    fn tile_height(self) -> u32 {
        self.picture_height + STAMP_HEIGHT
    }

    /// The sheet's pixel size: full width, and only the rows the tiles fill.
    fn size(self) -> (u32, u32) {
        let rows = (self.tiles as u32).div_ceil(COLUMNS).clamp(1, ROWS);
        (TILE_WIDTH * COLUMNS, self.tile_height() * rows)
    }

    /// The top-left corner of tile `index`.
    fn origin(self, index: usize) -> (u32, u32) {
        let index = index as u32;
        (
            (index % COLUMNS) * TILE_WIDTH,
            (index / COLUMNS) * self.tile_height(),
        )
    }
}

/// One sheet being drawn.
struct SheetCanvas {
    layout: SheetLayout,
    image: RgbImage,
}

impl SheetCanvas {
    /// A blank sheet, so a tile that never arrives leaves an obvious gap.
    fn open(layout: SheetLayout) -> Self {
        let (width, height) = layout.size();
        Self {
            layout,
            image: RgbImage::from_pixel(width, height, SHEET_BACKGROUND),
        }
    }

    /// Place one picture into tile `index` and burn its time into the band
    /// below it.
    fn tile(&mut self, index: usize, picture: &RgbImage, seconds: f64) {
        let (x, y) = self.layout.origin(index);
        let scaled = resize(
            picture,
            TILE_WIDTH,
            self.layout.picture_height,
            FilterType::Triangle,
        );
        replace(&mut self.image, &scaled, i64::from(x), i64::from(y));
        let band = y + self.layout.picture_height;
        for row in band..band + STAMP_HEIGHT {
            for column in x..x + TILE_WIDTH {
                self.image.put_pixel(column, row, STAMP_BACKGROUND);
            }
        }
        StampFont::draw(
            &mut self.image,
            &ContactSheet::stamp_label(seconds),
            (
                x + STAMP_INSET,
                band + (STAMP_HEIGHT - StampFont::HEIGHT) / 2,
            ),
        );
    }

    fn encode(self) -> Result<String, String> {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
            .encode_image(&self.image)
            .map_err(|error| format!("encoding a contact sheet failed: {error}"))?;
        Ok(format!("data:image/jpeg;base64,{}", BASE64.encode(jpeg)))
    }
}

/// A 5×7 bitmap font drawn at twice its size, holding only what a time
/// stamp needs.
struct StampFont;

impl StampFont {
    const SCALE: u32 = 2;
    const WIDTH: u32 = 5 * Self::SCALE;
    const HEIGHT: u32 = 7 * Self::SCALE;
    const ADVANCE: u32 = Self::WIDTH + Self::SCALE;

    /// Each glyph row, top to bottom; the high bit of five is the leftmost
    /// pixel.
    fn glyph(character: char) -> Option<[u8; 7]> {
        Some(match character {
            '0' => [0x0e, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0e],
            '1' => [0x04, 0x0c, 0x04, 0x04, 0x04, 0x04, 0x0e],
            '2' => [0x0e, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1f],
            '3' => [0x1f, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0e],
            '4' => [0x02, 0x06, 0x0a, 0x12, 0x1f, 0x02, 0x02],
            '5' => [0x1f, 0x10, 0x1e, 0x01, 0x01, 0x11, 0x0e],
            '6' => [0x06, 0x08, 0x10, 0x1e, 0x11, 0x11, 0x0e],
            '7' => [0x1f, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
            '8' => [0x0e, 0x11, 0x11, 0x0e, 0x11, 0x11, 0x0e],
            '9' => [0x0e, 0x11, 0x11, 0x0f, 0x01, 0x02, 0x0c],
            '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0c, 0x0c],
            's' => [0x00, 0x00, 0x0f, 0x10, 0x0e, 0x01, 0x1e],
            _ => return None,
        })
    }

    /// Draw `text` with its top-left corner at `origin`, clipped to the
    /// image.
    fn draw(image: &mut RgbImage, text: &str, origin: (u32, u32)) {
        for (slot, character) in text.chars().enumerate() {
            let Some(rows) = Self::glyph(character) else {
                continue;
            };
            let left = origin.0 + slot as u32 * Self::ADVANCE;
            for (row, bits) in rows.into_iter().enumerate() {
                for column in 0..5 {
                    if bits & (0x10 >> column) == 0 {
                        continue;
                    }
                    for dy in 0..Self::SCALE {
                        for dx in 0..Self::SCALE {
                            let x = left + column * Self::SCALE + dx;
                            let y = origin.1 + row as u32 * Self::SCALE + dy;
                            if x < image.width() && y < image.height() {
                                image.put_pixel(x, y, STAMP_INK);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timeline(times: &[i128]) -> VideoTimeline {
        VideoTimeline::new(times).unwrap()
    }

    #[test]
    fn a_timeline_runs_one_picture_period_past_its_last_picture() {
        let times = [2_000_000_000, 2_500_000_000, 3_000_000_000];
        let timeline = timeline(&times);
        assert_eq!(timeline.duration_seconds(), 1.5);
        assert_eq!(timeline.presentation_ns(1.25), 3_250_000_000);
        // Answers are kept on the timeline.
        assert_eq!(timeline.presentation_ns(9.0), 3_500_000_000);
        assert!(VideoTimeline::new(&[0]).is_err());
        assert!(VideoTimeline::new(&[-1, 5]).is_err());
    }

    #[test]
    fn samples_start_at_the_first_picture_and_take_the_nearest_picture_once() {
        // A 10 Hz camera sampled every quarter second, with one picture late.
        let times = (0..10)
            .map(|index| index * 100_000_000 + if index == 5 { 40_000_000 } else { 0 })
            .collect::<Vec<i128>>();
        let plan = SamplePlan::new(timeline(&times), &times, 0.25).unwrap();
        assert_eq!(plan.positions(), [0, 2, 5, 7]);
        assert_eq!(plan.seconds(), [0.0, 0.2, 0.54, 0.7]);
        // A camera sparser than the interval gives each of its pictures once.
        let sparse = [0, 1_000_000_000];
        let plan = SamplePlan::new(timeline(&sparse), &sparse, 0.1).unwrap();
        assert_eq!(plan.positions(), [0, 1]);
        assert!(SamplePlan::new(timeline(&sparse), &sparse, 0.0).is_err());
    }

    #[test]
    fn sheets_hold_twenty_stamped_tiles_at_the_recording_shape() {
        let layout = SheetLayout::new((640, 360), FRAMES_PER_SHEET);
        assert_eq!(layout.size(), (1120, (126 + 16) * 4));
        assert_eq!(layout.origin(4), (896, 0));
        assert_eq!(layout.origin(5), (0, 142));
        assert_eq!(SheetLayout::new((640, 360), 3).size(), (1120, 142));
        assert_eq!(SheetLayout::new((512, 512), 1).size(), (1120, 240));

        let picture = RgbImage::from_pixel(64, 36, Rgb([200, 200, 200]));
        let mut sheets = ContactSheets::new((0..21).map(|index| f64::from(index) * 0.5).collect());
        for _ in 0..21 {
            sheets.draw(&picture).unwrap();
        }
        assert!(sheets.draw(&picture).is_err(), "only what was sampled");
        let sheets = sheets.finish().unwrap();
        assert_eq!(sheets.len(), 2);
        assert_eq!(
            sheets[0].caption(0, 2),
            "Contact sheet 1 of 2, covering 0.0 s to 9.5 s of this episode."
        );
        assert_eq!(
            sheets[1].caption(1, 2),
            "Contact sheet 2 of 2, covering 10.0 s to 10.0 s of this episode."
        );
        let jpeg = BASE64
            .decode(
                sheets[0]
                    .data_url()
                    .trim_start_matches("data:image/jpeg;base64,"),
            )
            .unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap().into_rgb8();
        assert_eq!(decoded.dimensions(), (1120, 568));
        // The stamp band is dark with light ink, below a light picture.
        let ink = (0..60)
            .filter(|x| decoded.get_pixel(STAMP_INSET + x, 126 + 8)[0] > 128)
            .count();
        assert!(ink > 0 && ink < 60, "{ink}");
        assert!(decoded.get_pixel(200, 60)[0] > 150);
        assert_eq!(ContactSheet::stamp_label(8.25), "8.2s");
    }

    #[test]
    fn sheets_missing_a_sampled_picture_are_refused() {
        let mut short = ContactSheets::new(vec![0.0, 0.5, 1.0]);
        short
            .draw(&RgbImage::from_pixel(8, 8, Rgb([0, 0, 0])))
            .unwrap();
        assert_eq!(
            short.finish().unwrap_err(),
            "only 1 of the 3 sampled pictures were decoded"
        );
        assert!(ContactSheets::new(Vec::new()).finish().is_err());
    }
}
