//! Multiple non-destructive source ranges and how they are exported.

use gettextrs::gettext;

/// A half-open source range in milliseconds: `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipRange {
    pub start_ms: u64,
    pub end_ms: u64,
}

impl ClipRange {
    pub fn new(start_ms: u64, end_ms: u64) -> Self {
        Self {
            start_ms: start_ms.min(end_ms),
            end_ms: end_ms.max(start_ms),
        }
    }

    pub fn duration_ms(self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

/// Whether multiple ranges become one continuous file or one file per range.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SegmentExportMode {
    #[default]
    Join,
    Separate,
}

impl SegmentExportMode {
    pub fn get_all() -> [Self; 2] {
        [Self::Join, Self::Separate]
    }

    pub fn from_index(index: u32) -> Self {
        Self::get_all()
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }

    pub fn for_display(self) -> String {
        match self {
            Self::Join => gettext("Join into one clip"),
            Self::Separate => gettext("Export individual clips"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_normalized() {
        assert_eq!(ClipRange::new(4_000, 1_000), ClipRange::new(1_000, 4_000));
        assert_eq!(ClipRange::new(1_000, 4_000).duration_ms(), 3_000);
    }
}
