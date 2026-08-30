#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoOrientation {
    #[default]
    Identity,
    R90,
    R180,
    R270,
    FlippedIdentity,
    FR90,
    FR180,
    FR270,
}

use VideoOrientation::*;

impl VideoOrientation {
    pub fn rotate_right(&self) -> Self {
        match self {
            Identity => R90,
            R90 => R180,
            R180 => R270,
            R270 => Identity,
            FlippedIdentity => FR90,
            FR90 => FR180,
            FR180 => FR270,
            FR270 => FlippedIdentity,
        }
    }

    pub fn rotate_left(&self) -> Self {
        match self {
            R90 => Identity,
            R180 => R90,
            R270 => R180,
            Identity => R270,
            FR90 => FlippedIdentity,
            FR180 => FR90,
            FR270 => FR180,
            FlippedIdentity => FR270,
        }
    }

    pub fn horizontal_flip(&self) -> Self {
        match self {
            Identity => FlippedIdentity,
            FlippedIdentity => Identity,
            R90 => FR270,
            FR270 => R90,
            R180 => FR180,
            FR180 => R180,
            R270 => FR90,
            FR90 => R270,
        }
    }

    pub fn vertical_flip(&self) -> Self {
        match self {
            Identity => FR180,
            FR180 => Identity,
            R90 => FR90,
            FR90 => R90,
            R180 => FlippedIdentity,
            FlippedIdentity => R180,
            R270 => FR270,
            FR270 => R270,
        }
    }

    pub fn is_width_height_swapped(&self) -> bool {
        matches!(self, R90 | R270 | FR90 | FR270)
    }

    /// Ordered ffmpeg video filters that reproduce this orientation. `transpose=1`
    /// is 90° clockwise, `transpose=2` is 90° counter-clockwise.
    pub fn ffmpeg_filters(&self) -> Vec<&'static str> {
        match self {
            Identity => vec![],
            R90 => vec!["transpose=1"],
            R180 => vec!["transpose=1", "transpose=1"],
            R270 => vec!["transpose=2"],
            FlippedIdentity => vec!["hflip"],
            FR90 => vec!["hflip", "transpose=1"],
            FR180 => vec!["vflip"],
            FR270 => vec!["hflip", "transpose=2"],
        }
    }
}
