//! Image correction, playback speed and repeat (loop/boomerang) effects.
//!
//! Colour values use GStreamer's ranges (`videobalance` + `gamma`) so the live
//! preview can be driven by setting element properties directly; the ffmpeg
//! exporter translates them into `eq`/`hue` filters, which follow the same model
//! (luma offset/scale, chroma scale, gamma curve) with matching ranges.

use gettextrs::gettext;

/// Colour correction applied live to the preview and baked into the export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorAdjustments {
    /// Luma offset, `-1.0`…`1.0`. `0.0` leaves the image untouched.
    pub brightness: f64,
    /// Luma scale, `0.0`…`2.0`. `1.0` leaves the image untouched.
    pub contrast: f64,
    /// Chroma scale, `0.0`…`2.0`. `1.0` leaves the image untouched.
    pub saturation: f64,
    /// Hue rotation in degrees, `-180.0`…`180.0`. `0.0` leaves the image untouched.
    pub hue: f64,
    /// Gamma curve, `0.1`…`4.0`. `1.0` leaves the image untouched.
    pub gamma: f64,
    /// Unsharp-mask strength, `0.0`…`1.0`. `0.0` leaves the image untouched.
    pub sharpness: f64,
}

impl Default for ColorAdjustments {
    fn default() -> Self {
        Self::NEUTRAL
    }
}

impl ColorAdjustments {
    /// GES effect used for live sharpness and its fallback renderer.
    ///
    /// `gaussianblur` only accepts AYUV. Leaving the conversion implicit can make
    /// GES negotiate decoder-specific buffers all the way into the effect; VP8/VP9
    /// WebM streams are particularly prone to producing corrupted preview frames.
    /// The explicit caps also bring hardware-decoded frames back into system memory.
    pub const GST_SHARPEN_EFFECT: &'static str =
        "videoconvert ! video/x-raw,format=AYUV ! gaussianblur sigma=0 ! videoconvert";

    /// The identity adjustment: every knob at its no-op position.
    pub const NEUTRAL: Self = Self {
        brightness: 0.0,
        contrast: 1.0,
        saturation: 1.0,
        hue: 0.0,
        gamma: 1.0,
        sharpness: 0.0,
    };

    /// Whether these values would leave the image unchanged, so the exporter can
    /// skip the (CPU-bound) colour filters entirely.
    pub fn is_neutral(&self) -> bool {
        const EPS: f64 = 0.0005;
        (self.brightness - Self::NEUTRAL.brightness).abs() < EPS
            && (self.contrast - Self::NEUTRAL.contrast).abs() < EPS
            && (self.saturation - Self::NEUTRAL.saturation).abs() < EPS
            && self.hue.abs() < 0.5
            && (self.gamma - Self::NEUTRAL.gamma).abs() < EPS
            && self.sharpness.abs() < EPS
    }

    /// Hue in `videobalance`'s units, where `±1.0` is a full `±180°` rotation.
    pub fn gst_hue(&self) -> f64 {
        (self.hue / 180.0).clamp(-1.0, 1.0)
    }

    /// Negative sigma makes GStreamer's `gaussianblur` sharpen the image. The
    /// deliberately moderate maximum avoids obvious ringing around hard edges.
    pub fn gst_sharpen_sigma(&self) -> f64 {
        -5.0 * self.sharpness.clamp(0.0, 1.0)
    }

    /// ffmpeg filters reproducing these adjustments, in order. `eq` covers
    /// brightness/contrast/saturation/gamma, `hue` rotates colour and `unsharp`
    /// applies the requested edge contrast.
    pub fn ffmpeg_filters(&self) -> Vec<String> {
        if self.is_neutral() {
            return vec![];
        }

        let mut filters = Vec::with_capacity(2);

        filters.push(format!(
            "eq=brightness={:.4}:contrast={:.4}:saturation={:.4}:gamma={:.4}",
            self.brightness, self.contrast, self.saturation, self.gamma
        ));

        if self.hue.abs() >= 0.5 {
            filters.push(format!("hue=h={:.2}", self.hue));
        }

        if self.sharpness >= 0.0005 {
            filters.push(format!(
                "unsharp=5:5:{:.3}:5:5:0",
                1.5 * self.sharpness.clamp(0.0, 1.0)
            ));
        }

        filters
    }
}

/// Playback rate applied to both video and audio during preview and export.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlaybackSpeed {
    Normal,
    ThreeQuarter,
    Half,
    Quarter,
}

impl PlaybackSpeed {
    pub fn get_all() -> Vec<Self> {
        vec![Self::Normal, Self::ThreeQuarter, Self::Half, Self::Quarter]
    }

    pub fn from_index(index: u32) -> Self {
        Self::get_all()
            .get(index as usize)
            .copied()
            .unwrap_or(Self::Normal)
    }

    pub fn for_display(&self) -> String {
        match self {
            Self::Normal => gettext("Normal"),
            Self::ThreeQuarter => gettext("0.75×"),
            Self::Half => gettext("0.5×"),
            Self::Quarter => gettext("0.25×"),
        }
    }

    pub fn factor(&self) -> f64 {
        match self {
            Self::Normal => 1.0,
            Self::ThreeQuarter => 0.75,
            Self::Half => 0.5,
            Self::Quarter => 0.25,
        }
    }

    pub fn is_normal(&self) -> bool {
        *self == Self::Normal
    }

    pub fn output_duration_ms(&self, source_ms: u64) -> u64 {
        (source_ms as f64 / self.factor()).round() as u64
    }

    /// FFmpeg's `atempo` accepts factors down to 0.5, so quarter speed needs two
    /// stages. Keeping this here guarantees the video and audio rate agree.
    pub fn ffmpeg_audio_filter(&self) -> Option<&'static str> {
        match self {
            Self::Normal => None,
            Self::ThreeQuarter => Some("atempo=0.75"),
            Self::Half => Some("atempo=0.5"),
            Self::Quarter => Some("atempo=0.5,atempo=0.5"),
        }
    }
}

/// How the trimmed selection is repeated in the exported file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatMode {
    /// Export the selection once.
    Off,
    /// Play the selection back to back.
    Loop,
    /// Play the selection forwards, then backwards.
    Boomerang,
}

impl RepeatMode {
    pub fn get_all() -> Vec<RepeatMode> {
        vec![RepeatMode::Off, RepeatMode::Loop, RepeatMode::Boomerang]
    }

    pub fn from_index(index: u32) -> Self {
        Self::get_all()
            .get(index as usize)
            .copied()
            .unwrap_or(RepeatMode::Off)
    }

    pub fn for_display(&self) -> String {
        match self {
            RepeatMode::Off => gettext("None"),
            RepeatMode::Loop => gettext("Loop"),
            RepeatMode::Boomerang => gettext("Boomerang"),
        }
    }
}

/// A repeat effect: a mode plus how many cycles of it to emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Repeat {
    pub mode: RepeatMode,
    /// Number of cycles, at least 1. One cycle is the selection for
    /// [`RepeatMode::Loop`], or forwards+backwards for [`RepeatMode::Boomerang`].
    pub count: u32,
}

impl Default for Repeat {
    fn default() -> Self {
        Self::OFF
    }
}

impl Repeat {
    pub const OFF: Self = Self {
        mode: RepeatMode::Off,
        count: 1,
    };

    /// Whether this would produce exactly the trimmed selection, so the exporter
    /// can take the single-pass path.
    pub fn is_off(&self) -> bool {
        match self.mode {
            RepeatMode::Off => true,
            RepeatMode::Loop => self.cycles() <= 1,
            RepeatMode::Boomerang => false,
        }
    }

    /// The cycle count, floored at 1.
    pub fn cycles(&self) -> u32 {
        self.count.max(1)
    }

    /// Whether the reversed pass is needed.
    pub fn is_boomerang(&self) -> bool {
        self.mode == RepeatMode::Boomerang
    }

    /// Length of the exported file for a selection of `source_ms`.
    pub fn output_duration_ms(&self, source_ms: u64) -> u64 {
        match self.mode {
            RepeatMode::Off => source_ms,
            RepeatMode::Loop => source_ms * self.cycles() as u64,
            RepeatMode::Boomerang => source_ms * 2 * self.cycles() as u64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_speed_keeps_audio_and_duration_in_sync() {
        let speed = PlaybackSpeed::Quarter;

        assert_eq!(speed.output_duration_ms(1_000), 4_000);
        assert_eq!(speed.ffmpeg_audio_filter(), Some("atempo=0.5,atempo=0.5"));
    }

    #[test]
    fn sharpness_is_neutral_at_zero_and_adds_unsharp_when_enabled() {
        assert!(ColorAdjustments::NEUTRAL.is_neutral());

        let adjusted = ColorAdjustments {
            sharpness: 0.5,
            ..ColorAdjustments::NEUTRAL
        };
        assert!(!adjusted.is_neutral());
        assert!(adjusted
            .ffmpeg_filters()
            .iter()
            .any(|filter| filter.starts_with("unsharp=")));
        assert!(adjusted.gst_sharpen_sigma() < 0.0);
        assert!(ColorAdjustments::GST_SHARPEN_EFFECT.contains("format=AYUV"));
    }
}
