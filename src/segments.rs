//! Multiple non-destructive source ranges and how they are exported.

use std::path::{Path, PathBuf};

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

/// One item in the single-track sequence.  The source is kept alongside its
/// own non-destructive range so clips from different files can be edited and
/// exported without copying media into a project directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipSegment {
    pub source: PathBuf,
    pub range: ClipRange,
}

impl ClipSegment {
    pub fn new(source: impl Into<PathBuf>, start_ms: u64, end_ms: u64) -> Self {
        Self {
            source: source.into(),
            range: ClipRange::new(start_ms, end_ms),
        }
    }

    pub fn from_range(source: impl Into<PathBuf>, range: ClipRange) -> Self {
        Self {
            source: source.into(),
            range,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.range.duration_ms()
    }

    pub fn source_name(&self) -> &str {
        self.source
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_else(|| self.source.to_str().unwrap_or("Video"))
    }

    pub fn source_path(&self) -> &Path {
        &self.source
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

    #[test]
    fn segment_keeps_source_and_range_together() {
        let segment = ClipSegment::new("second.mp4", 4_000, 1_000);
        assert_eq!(segment.source_name(), "second.mp4");
        assert_eq!(segment.range, ClipRange::new(1_000, 4_000));
        assert_eq!(segment.duration_ms(), 3_000);
    }
}
