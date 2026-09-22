//! Contact sheets: the moments one episode is sampled at, and where each
//! sampled frame sits on the sheet the model reads.
//!
//! The geometry lives apart from the canvas so the rules the model depends on
//! — 224 px-wide tiles at the recording's own aspect ratio, five columns by
//! four rows, chronological left to right then top to bottom, and the
//! episode-relative timestamp burned into every tile — are checked without a
//! browser.

/// Tiles across one sheet.
pub(super) const COLUMNS: usize = 5;
/// Tile rows down one sheet.
pub(super) const ROWS: usize = 4;
/// Frames one sheet carries: about ten seconds at the default half-second
/// sampling interval.
pub(super) const FRAMES_PER_SHEET: usize = COLUMNS * ROWS;

const TILE_WIDTH: f64 = 224.0;
/// The band at a tile's foot that carries its timestamp.
const STAMP_HEIGHT: f64 = 16.0;
/// The shape a tile falls back to before any frame has been decoded.
const FALLBACK_ASPECT: f64 = 16.0 / 9.0;

/// The moments one episode is sampled at, in order.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SamplePlan {
    seconds: Vec<f64>,
}

impl SamplePlan {
    /// Every `interval` second from the start of the episode while the
    /// recording lasts. An episode shorter than one interval is still sampled
    /// once, at its start, so a short recording is annotated rather than
    /// skipped.
    pub(super) fn new(duration: f64, interval: f64) -> Result<Self, String> {
        if !duration.is_finite() || duration <= 0.0 {
            return Err("This episode has no recorded duration.".into());
        }
        if !interval.is_finite() || interval <= 0.0 {
            return Err("The sample interval must be a positive number of seconds.".into());
        }
        let count = (duration / interval).ceil().min(u16::MAX as f64) as usize;
        let seconds = (0..count.max(1))
            .map(|index| index as f64 * interval)
            .collect();
        Ok(Self { seconds })
    }
    pub(super) fn len(&self) -> usize {
        self.seconds.len()
    }
    /// The moments themselves, in order.
    pub(super) fn times(&self) -> &[f64] {
        &self.seconds
    }
    /// What one tile's timestamp reads: the seconds the model must answer in.
    pub(super) fn stamp_label(seconds: f64) -> String {
        format!("{seconds:.1}s")
    }
}

/// One rendered sheet: the span of the episode it covers and the encoded
/// picture the model reads.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct ContactSheet {
    span: (f64, f64),
    image: String,
}

impl ContactSheet {
    pub(super) fn new(span: (f64, f64), image: String) -> Self {
        Self { span, image }
    }
    pub(super) fn image(&self) -> &str {
        &self.image
    }
    /// What the request spends on this sheet, as it is actually sent.
    pub(super) fn bytes(&self) -> usize {
        self.image.len()
    }
    /// What the model is told about this sheet before it sees it.
    pub(super) fn caption(&self, index: usize, total: usize) -> String {
        format!(
            "Contact sheet {} of {total}, covering {} to {} of this episode.",
            index + 1,
            Self::moment(self.span.0),
            Self::moment(self.span.1)
        )
    }
    fn moment(seconds: f64) -> String {
        format!("{seconds:.1} s")
    }
}

/// Where each tile of one sheet is drawn, in sheet pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct SheetLayout {
    tile: (f64, f64),
    tiles: usize,
}

impl SheetLayout {
    /// A sheet for `tiles` frames of a recording whose pictures are
    /// `frame` pixels. Tiles keep the recording's aspect ratio at a fixed
    /// width, and every tile reserves a band for its timestamp.
    pub(super) fn new(frame: (u32, u32), tiles: usize) -> Self {
        let aspect = match (frame.0, frame.1) {
            (width, height) if width > 0 && height > 0 => f64::from(width) / f64::from(height),
            _ => FALLBACK_ASPECT,
        };
        let picture = (TILE_WIDTH / aspect).round().max(1.0);
        Self {
            tile: (TILE_WIDTH, picture + STAMP_HEIGHT),
            tiles: tiles.min(FRAMES_PER_SHEET),
        }
    }
    /// The sheet's pixel size: full width, and only the rows the tiles fill.
    pub(super) fn size(self) -> (u32, u32) {
        let rows = self.tiles.div_ceil(COLUMNS).clamp(1, ROWS);
        (
            (TILE_WIDTH * COLUMNS as f64) as u32,
            (self.tile.1 * rows as f64) as u32,
        )
    }
    /// The top-left corner of tile `index`, chronological left to right.
    pub(super) fn origin(self, index: usize) -> (f64, f64) {
        (
            (index % COLUMNS) as f64 * self.tile.0,
            (index / COLUMNS) as f64 * self.tile.1,
        )
    }
    /// Where tile `index` draws its decoded picture, above its timestamp band.
    pub(super) fn picture(self, index: usize) -> (f64, f64, f64, f64) {
        let (x, y) = self.origin(index);
        (x, y, self.tile.0, self.tile.1 - STAMP_HEIGHT)
    }
    /// Where tile `index` writes its timestamp: the band and its text baseline.
    pub(super) fn stamp(self, index: usize) -> (f64, f64, f64, f64) {
        let (x, y) = self.origin(index);
        (x, y + self.tile.1 - STAMP_HEIGHT, self.tile.0, STAMP_HEIGHT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_start_at_zero_and_stop_with_the_recording() {
        let plan = SamplePlan::new(3.0, 0.5).unwrap();
        assert_eq!(plan.seconds, [0.0, 0.5, 1.0, 1.5, 2.0, 2.5]);
        assert_eq!(plan.len(), 6);
        // A recording shorter than one interval is still worth one frame.
        assert_eq!(SamplePlan::new(0.2, 0.5).unwrap().seconds, [0.0]);
        assert!(SamplePlan::new(0.0, 0.5).is_err());
        assert!(SamplePlan::new(f64::NAN, 0.5).is_err());
        assert!(SamplePlan::new(10.0, 0.0).is_err());
        assert!(SamplePlan::new(10.0, f64::INFINITY).is_err());
        assert_eq!(SamplePlan::stamp_label(8.25), "8.2s");
        assert_eq!(SamplePlan::stamp_label(0.0), "0.0s");
    }

    #[test]
    fn sheets_hold_twenty_frames_in_chronological_order() {
        let plan = SamplePlan::new(25.0, 0.5).unwrap();
        assert_eq!(plan.len(), 50);
        let sheets = plan.times().chunks(FRAMES_PER_SHEET).collect::<Vec<_>>();
        assert_eq!(sheets.len(), 3);
        assert_eq!(sheets[0].len(), FRAMES_PER_SHEET);
        assert_eq!(sheets[0][0], 0.0);
        assert_eq!(sheets[1][0], 10.0);
        assert_eq!(
            sheets[2],
            [20.0, 20.5, 21.0, 21.5, 22.0, 22.5, 23.0, 23.5, 24.0, 24.5]
        );
    }

    #[test]
    fn tiles_keep_the_recording_shape_and_reserve_a_timestamp_band() {
        let layout = SheetLayout::new((640, 360), FRAMES_PER_SHEET);
        assert_eq!(layout.size(), (1120, (126.0 + 16.0) as u32 * 4));
        assert_eq!(layout.origin(0), (0.0, 0.0));
        assert_eq!(layout.origin(4), (896.0, 0.0));
        assert_eq!(layout.origin(5), (0.0, 142.0));
        assert_eq!(layout.origin(19), (896.0, 426.0));
        assert_eq!(layout.picture(5), (0.0, 142.0, 224.0, 126.0));
        assert_eq!(layout.stamp(5), (0.0, 268.0, 224.0, 16.0));
        // A square camera makes taller tiles; an unknown one falls back to 16:9.
        assert_eq!(SheetLayout::new((512, 512), 1).size(), (1120, 240));
        assert_eq!(SheetLayout::new((0, 0), 1).size(), (1120, 142));
    }

    #[test]
    fn a_sheet_announces_the_span_it_covers() {
        let sheet = ContactSheet::new((10.0, 19.5), "data:image/jpeg;base64,AAAA".into());
        assert_eq!(
            sheet.caption(1, 3),
            "Contact sheet 2 of 3, covering 10.0 s to 19.5 s of this episode."
        );
        assert_eq!(sheet.bytes(), sheet.image().len());
    }

    #[test]
    fn a_short_sheet_only_draws_the_rows_it_fills() {
        assert_eq!(SheetLayout::new((640, 360), 3).size(), (1120, 142));
        assert_eq!(SheetLayout::new((640, 360), 6).size(), (1120, 284));
        assert_eq!(SheetLayout::new((640, 360), 0).size(), (1120, 142));
        // A sheet never grows past its twenty tiles even if handed more.
        assert_eq!(SheetLayout::new((640, 360), 99).size(), (1120, 568));
    }
}
