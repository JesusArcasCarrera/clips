use std::{path::Path, process::Command};

use itertools::Itertools;

#[derive(Debug, Clone, Copy)]
pub struct Framerate {
    pub nominator: u32,
    pub denominator: u32,
}

impl Framerate {
    pub fn value(&self) -> f64 {
        self.nominator as f64 / self.denominator as f64
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Dimensions<T> {
    pub width: T,
    pub height: T,
}

impl Dimensions<u32> {
    pub fn width_f64(&self) -> f64 {
        self.width as f64
    }

    pub fn height_f64(&self) -> f64 {
        self.height as f64
    }
}
impl<T: Copy> Dimensions<T> {
    pub fn swap(&self) -> Dimensions<T> {
        Dimensions {
            width: self.height,
            height: self.width,
        }
    }
}

impl From<Dimensions<f64>> for Dimensions<u32> {
    fn from(value: Dimensions<f64>) -> Self {
        Dimensions {
            width: value.width as u32,
            height: value.height as u32,
        }
    }
}

impl From<Dimensions<u32>> for Dimensions<f64> {
    fn from(value: Dimensions<u32>) -> Self {
        Dimensions {
            width: value.width as f64,
            height: value.height as f64,
        }
    }
}

pub fn get_info(path: String) -> Option<(Dimensions<u32>, Option<Framerate>, bool)> {
    let video_info = get_video_info(path.clone())?;
    let audio_info = get_audio_info(path)?;
    Some((video_info.0, video_info.1, audio_info))
}

/// Read a media duration without opening a decoder.  Keeping this probe next
/// to the existing stream metadata probe makes adding sources cheap and lets
/// the UI reject empty or malformed ranges before export.
pub fn get_duration_ms(path: &Path) -> Option<u64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    let seconds = std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()?;
    if seconds.is_finite() && seconds > 0.0 {
        Some((seconds * 1_000.0).round() as u64)
    } else {
        None
    }
}

fn get_audio_info(path: String) -> Option<bool> {
    let o = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-show_entries", "stream=codec_type"])
        .args(["-of", "csv=p=0"])
        .arg(path)
        .output()
        .ok()?;

    let s = std::str::from_utf8(&o.stdout).ok()?;

    Some(s.trim().split('\n').any(|x| x == "audio"))
}

fn get_video_info(path: String) -> Option<(Dimensions<u32>, Option<Framerate>)> {
    let o = Command::new("ffprobe")
        .args(["-v", "error"])
        .args(["-select_streams", "v:0"])
        .args(["-show_entries", "stream=width,height,r_frame_rate"])
        .args(["-of", "csv=s=x:p=0"])
        .arg(path)
        .output()
        .ok()?;

    let s = std::str::from_utf8(&o.stdout).ok()?;

    match s.trim().split('x').collect_vec()[..] {
        [a, b, c] => Some((
            Dimensions {
                width: a.trim().parse().ok()?,
                height: b.trim().parse().ok()?,
            },
            {
                let (x, y) = c.split('/').collect_tuple()?;
                Some(Framerate {
                    nominator: x.trim().parse().ok()?,
                    denominator: y.trim().parse().ok()?,
                })
            },
        )),
        [a, b] => Some((
            Dimensions {
                width: a.trim().parse().ok()?,
                height: b.trim().parse().ok()?,
            },
            None,
        )),
        _ => None,
    }
}

pub fn get_debug_info() {
    let o = Command::new("gst-inspect-1.0").output().unwrap();

    let s = std::str::from_utf8(&o.stdout).unwrap();

    println!("{}", s);
}
