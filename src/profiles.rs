#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ContainerFormat {
    Best,
    Same,
    Matroska,
    Mpeg,
    WebM,
    GifContainer,
}

#[derive(Debug, Copy, Clone)]
pub enum VideoEncoding {
    Av1,
    Vp8,
    Vp9,
    H264,
    H265,
    Gif,
}

#[derive(Debug, Copy, Clone)]
pub enum AudioEncoding {
    Aac,
    Ac3,
    Opus,
    Vorbis,
    Flac,
}

use gettextrs::gettext;
use AudioEncoding::*;
use ContainerFormat::*;
use VideoEncoding::*;

impl ContainerFormat {
    pub fn get_all() -> Vec<ContainerFormat> {
        vec![Best, Same, Matroska, Mpeg, WebM, GifContainer]
    }

    pub fn viable_matchings(&self) -> (Vec<VideoEncoding>, Vec<AudioEncoding>) {
        match self {
            Best => (vec![Av1], vec![Opus]),
            Same => (vec![], vec![]),
            Matroska => (
                vec![Av1, Vp9, Vp8, H264, H265],
                vec![Vorbis, Opus, Aac, Ac3, Flac],
            ),
            Mpeg => (vec![Av1, Vp9, Vp8, H264, H265], vec![Opus, Aac, Ac3, Flac]),
            WebM => (vec![Av1, Vp8, Vp9], vec![Vorbis, Opus]),
            GifContainer => (vec![VideoEncoding::Gif], vec![]),
        }
    }

    pub fn format(&self) -> &str {
        match self {
            Best => "video/webm",
            Matroska => "video/x-matroska",
            Mpeg => "video/quicktime",
            WebM => "video/webm",
            GifContainer => "image/gif",
            Same => unreachable!(),
        }
    }

    pub fn extension(&self) -> &str {
        match self {
            Best => "webm",
            Matroska => "mkv",
            Mpeg => "mp4",
            WebM => "webm",
            GifContainer => "gif",
            Same => unreachable!(),
        }
    }

    pub fn for_display(&self) -> String {
        match self {
            Best => gettext("Recommended (WEBM, AV1, Opus)"),
            Same => gettext("Keep as-is"),
            Matroska => "MKV".to_owned(),
            Mpeg => "MP4".to_owned(),
            WebM => "WEBM".to_owned(),
            GifContainer => "GIF".to_owned(),
        }
    }
}

impl VideoEncoding {
    pub fn get_format(&self) -> &str {
        match self {
            Av1 => "video/x-av1",
            Vp8 => "video/x-vp8",
            Vp9 => "video/x-vp9",
            H264 => "video/x-h264",
            H265 => "video/x-h265",
            Gif => "image/gif",
        }
    }

    pub fn get_preset_name(&self) -> &str {
        match self {
            Av1 => "svtav1enc",
            Vp8 => "vp8enc",
            Vp9 => "vp9enc",
            H264 => "x264enc",
            H265 => "x265enc",
            Gif => "gifenc",
        }
    }

    /// Returns (property_name, unit_multiplier) for bitrate.
    /// The value to pass = bitrate_kbps * multiplier.
    /// x264/x265 use kbit/s (multiplier 1), svtav1enc uses bits/s (multiplier 1000),
    /// vp8/vp9 use bits/s via "target-bitrate" (multiplier 1000).
    pub fn bitrate_property(&self) -> (&'static str, u32) {
        match self {
            H264 | H265 => ("bitrate", 1),
            Av1 => ("bitrate", 1000),
            Vp8 | Vp9 => ("target-bitrate", 1000),
            Gif => ("", 0),
        }
    }

    pub fn for_display(&self) -> &str {
        match self {
            Av1 => "AV1",
            Vp8 => "VP8",
            Vp9 => "VP9",
            H264 => "H264",
            H265 => "H265",
            Gif => "GIF",
        }
    }
}

impl AudioEncoding {
    pub fn get_format(&self) -> &str {
        match self {
            Aac => "audio/mpeg",
            Ac3 => "audio/x-ac3",
            Opus => "audio/x-opus",
            Vorbis => "audio/x-vorbis",
            Flac => "audio/x-flac",
        }
    }

    pub fn for_display(&self) -> &str {
        match self {
            Aac => "AAC",
            Ac3 => "AC3",
            Opus => "Opus",
            Vorbis => "Vorbis",
            Flac => "FLAC",
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Quality {
    Low,
    Medium,
    High,
    Unchanged,
}

impl Quality {
    /// Returns the bitrate in kbps for this quality level.
    /// None means "unchanged" (use encoder defaults).
    pub fn bitrate_kbps(&self) -> Option<u32> {
        match self {
            Quality::Low => Some(2000),
            Quality::Medium => Some(5000),
            Quality::High => Some(15000),
            Quality::Unchanged => None,
        }
    }

    pub fn from_index(index: u32) -> Self {
        match index {
            0 => Quality::Low,
            1 => Quality::Medium,
            2 => Quality::High,
            _ => Quality::Unchanged,
        }
    }
}

#[derive(Debug)]
pub struct OutputFormat {
    pub container_format: ContainerFormat,
    pub video_encoding: Option<VideoEncoding>,
    pub audio_encoding: Option<AudioEncoding>,
    pub quality: Quality,
}
