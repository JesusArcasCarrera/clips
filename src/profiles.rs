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

    pub fn get_preset_name(&self) -> &'static str {
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
            // x264enc / x265enc: "bitrate" in kbit/s.
            H264 | H265 => ("bitrate", 1),
            // svtav1enc: "target-bitrate" in kbit/s (it has no "bitrate" property).
            Av1 => ("target-bitrate", 1),
            // vp8enc / vp9enc: "target-bitrate" in bits/s.
            Vp8 | Vp9 => ("target-bitrate", 1000),
            Gif => ("", 0),
        }
    }

    /// NVENC hardware encoder factory and its bitrate property (in kbit/s), if one
    /// exists for this codec. AV1 NVENC requires an RTX 40-series GPU or newer.
    fn hardware_encoder(&self) -> Option<(&'static str, (&'static str, u32))> {
        match self {
            H264 => Some(("nvh264enc", ("bitrate", 1))),
            H265 => Some(("nvh265enc", ("bitrate", 1))),
            Av1 => Some(("nvav1enc", ("bitrate", 1))),
            Vp8 | Vp9 | Gif => None,
        }
    }

    /// Resolves the actual GStreamer encoder element to use. When `prefer_gpu` is set
    /// and a NVENC encoder for this codec is present in the registry, it is forced via
    /// the encoding profile's preset-name (which selects the encoder factory); otherwise
    /// it falls back to the software encoder.
    pub fn resolve_encoder(&self, prefer_gpu: bool) -> ResolvedVideoEncoder {
        if prefer_gpu {
            if let Some((element, bitrate_property)) = self.hardware_encoder() {
                if gst::ElementFactory::find(element).is_some() {
                    return ResolvedVideoEncoder {
                        element,
                        hardware: true,
                        bitrate_property,
                    };
                }
            }
        }

        ResolvedVideoEncoder {
            element: self.get_preset_name(),
            hardware: false,
            bitrate_property: self.bitrate_property(),
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

    /// ffmpeg encoders for this codec: (software, optional NVENC hardware).
    /// AV1 NVENC (`av1_nvenc`) requires an RTX 40-series GPU or newer.
    pub fn ffmpeg_encoders(&self) -> (&'static str, Option<&'static str>) {
        match self {
            H264 => ("libx264", Some("h264_nvenc")),
            H265 => ("libx265", Some("hevc_nvenc")),
            Av1 => ("libsvtav1", Some("av1_nvenc")),
            Vp8 => ("libvpx", None),
            Vp9 => ("libvpx-vp9", None),
            Gif => ("gif", None),
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

    /// ffmpeg audio encoder name for this codec.
    pub fn ffmpeg_codec(&self) -> &'static str {
        match self {
            Aac => "aac",
            Ac3 => "ac3",
            Opus => "libopus",
            Vorbis => "libvorbis",
            Flac => "flac",
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
    /// Bits-per-pixel-per-frame budget for this quality level. The actual target
    /// bitrate scales with the output resolution and framerate, so a small crop gets
    /// a small file and a full-resolution export gets a large one — instead of a
    /// fixed bitrate that bloats tiny crops or starves large frames.
    fn bits_per_pixel(&self) -> Option<f64> {
        match self {
            Quality::High => Some(0.10),
            Quality::Medium => Some(0.06),
            Quality::Low => Some(0.035),
            Quality::Unchanged => None,
        }
    }

    /// Resolution- and framerate-aware target bitrate in kbps. `None` means
    /// "unchanged" (use encoder defaults).
    pub fn target_kbps(&self, width: u32, height: u32, fps: f64) -> Option<u32> {
        let bpp = self.bits_per_pixel()?;
        let bits = bpp * width as f64 * height as f64 * fps.max(1.0);
        // Floor at 200 kbps so tiny/low-fps clips stay watchable.
        Some(((bits / 1000.0).round() as u32).max(200))
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

/// The concrete encoder chosen for an export, after deciding between GPU and CPU.
#[derive(Debug, Clone)]
pub struct ResolvedVideoEncoder {
    /// GStreamer element factory name (e.g. `nvh264enc`, `x264enc`).
    pub element: &'static str,
    /// Whether this is a hardware (GPU) encoder.
    pub hardware: bool,
    /// (property_name, unit_multiplier) for setting the target bitrate.
    pub bitrate_property: (&'static str, u32),
}

impl ResolvedVideoEncoder {
    /// Short human-readable label for the UI, e.g. "GPU · NVENC (nvh264enc)".
    pub fn label(&self) -> String {
        if self.hardware {
            format!("GPU · NVENC ({})", self.element)
        } else {
            format!("CPU · {} (software)", self.element)
        }
    }
}
