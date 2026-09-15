#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum WorkflowSide {
    #[default]
    Left,
    Right,
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct ComparisonEvidence {
    cohort: WorkflowSide,
    bin: Option<(usize, usize)>,
    offset: usize,
}
impl ComparisonEvidence {
    pub(super) fn new(cohort: WorkflowSide) -> Self {
        Self {
            cohort,
            ..Self::default()
        }
    }
    pub(super) fn with_bin(mut self, bin: Option<(usize, usize)>) -> Self {
        self.bin = bin;
        self.offset = 0;
        self
    }
    pub(super) fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }
    pub(super) fn offset(&self) -> usize {
        self.offset
    }
    pub(super) fn cohort(&self) -> WorkflowSide {
        self.cohort
    }
    pub(super) fn bin(&self) -> Option<(usize, usize)> {
        self.bin
    }
}
