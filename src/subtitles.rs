//! Single-track subtitle parsing, validation and non-destructive retiming.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::segments::ClipRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubtitleFormat {
    Srt,
    WebVtt,
    Ass,
}

impl SubtitleFormat {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "srt" => Some(Self::Srt),
            "vtt" => Some(Self::WebVtt),
            "ass" | "ssa" => Some(Self::Ass),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleCue {
    pub id: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub identifier: Option<String>,
    pub settings: Option<String>,
    pub style: Option<String>,
    pub speaker: Option<String>,
}

impl SubtitleCue {
    pub fn new(id: u64, start_ms: u64, end_ms: u64, text: impl Into<String>) -> Self {
        Self {
            id,
            start_ms,
            end_ms,
            text: text.into(),
            identifier: None,
            settings: None,
            style: None,
            speaker: None,
        }
    }

    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleTrack {
    pub format: SubtitleFormat,
    pub language: Option<String>,
    pub title: Option<String>,
    pub metadata: BTreeMap<String, String>,
    cues: Vec<SubtitleCue>,
    next_id: u64,
}

impl SubtitleTrack {
    pub fn new(format: SubtitleFormat) -> Self {
        Self {
            format,
            language: None,
            title: None,
            metadata: BTreeMap::new(),
            cues: Vec::new(),
            next_id: 1,
        }
    }

    pub fn parse(format: SubtitleFormat, input: &str) -> Result<Self, SubtitleParseError> {
        match format {
            SubtitleFormat::Srt => parse_srt(input),
            SubtitleFormat::WebVtt => parse_webvtt(input),
            SubtitleFormat::Ass => parse_ass(input),
        }
    }

    pub fn from_file(path: &Path) -> Result<Self, SubtitleLoadError> {
        let format = SubtitleFormat::from_path(path).ok_or(SubtitleLoadError::UnsupportedFormat)?;
        let input = fs::read_to_string(path)?;
        Self::parse(format, &input).map_err(SubtitleLoadError::Parse)
    }

    pub fn cues(&self) -> &[SubtitleCue] {
        &self.cues
    }

    pub fn cue(&self, id: u64) -> Option<&SubtitleCue> {
        self.cues.iter().find(|cue| cue.id == id)
    }

    pub fn add_cue(&mut self, start_ms: u64, end_ms: u64, text: impl Into<String>) -> u64 {
        let id = self.allocate_id();
        self.cues.push(SubtitleCue::new(id, start_ms, end_ms, text));
        self.sort_cues();
        id
    }

    pub fn update_cue(
        &mut self,
        id: u64,
        start_ms: u64,
        end_ms: u64,
        text: impl Into<String>,
    ) -> bool {
        let Some(cue) = self.cues.iter_mut().find(|cue| cue.id == id) else {
            return false;
        };
        cue.start_ms = start_ms;
        cue.end_ms = end_ms;
        cue.text = text.into();
        self.sort_cues();
        true
    }

    pub fn remove_cue(&mut self, id: u64) -> bool {
        let before = self.cues.len();
        self.cues.retain(|cue| cue.id != id);
        self.cues.len() != before
    }

    pub fn diagnostics(&self, media_duration_ms: u64) -> Vec<SubtitleDiagnostic> {
        let mut diagnostics = Vec::new();
        for (index, cue) in self.cues.iter().enumerate() {
            if cue.start_ms >= cue.end_ms {
                diagnostics.push(SubtitleDiagnostic::InvalidDuration { cue_id: cue.id });
            }
            if cue.end_ms > media_duration_ms {
                diagnostics.push(SubtitleDiagnostic::OutsideMedia {
                    cue_id: cue.id,
                    media_duration_ms,
                });
            }
            if cue.text.trim().is_empty() {
                diagnostics.push(SubtitleDiagnostic::EmptyText { cue_id: cue.id });
            }
            for previous in &self.cues[..index] {
                if cue.start_ms < previous.end_ms {
                    diagnostics.push(SubtitleDiagnostic::Overlap {
                        first_cue_id: previous.id,
                        second_cue_id: cue.id,
                    });
                }
            }
        }
        diagnostics
    }

    /// Produces the subtitle timeline for joined source ranges. Cues crossing
    /// a cut are clipped to it, then shifted to the corresponding output time.
    pub fn retimed_for_ranges(&self, ranges: &[ClipRange]) -> Self {
        let mut output = Self::new(self.format);
        output.language.clone_from(&self.language);
        output.title.clone_from(&self.title);
        output.metadata.clone_from(&self.metadata);

        let mut output_offset = 0_u64;
        for range in ranges {
            for cue in &self.cues {
                let intersection_start = cue.start_ms.max(range.start_ms);
                let intersection_end = cue.end_ms.min(range.end_ms);
                if intersection_start >= intersection_end {
                    continue;
                }

                let mut retimed = cue.clone();
                retimed.id = output.allocate_id();
                retimed.start_ms = output_offset + intersection_start - range.start_ms;
                retimed.end_ms = output_offset + intersection_end - range.start_ms;
                output.cues.push(retimed);
            }
            output_offset = output_offset.saturating_add(range.duration_ms());
        }
        output.sort_cues();
        output
    }

    fn push_imported_cue(&mut self, mut cue: SubtitleCue) {
        cue.id = self.allocate_id();
        self.cues.push(cue);
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }

    fn sort_cues(&mut self) {
        self.cues
            .sort_by_key(|cue| (cue.start_ms, cue.end_ms, cue.id));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubtitleDiagnostic {
    InvalidDuration {
        cue_id: u64,
    },
    OutsideMedia {
        cue_id: u64,
        media_duration_ms: u64,
    },
    EmptyText {
        cue_id: u64,
    },
    Overlap {
        first_cue_id: u64,
        second_cue_id: u64,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SubtitleLoadError {
    #[error("unsupported subtitle format")]
    UnsupportedFormat,
    #[error("could not read subtitle file: {0}")]
    Io(#[from] io::Error),
    #[error(transparent)]
    Parse(#[from] SubtitleParseError),
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SubtitleParseError {
    #[error("missing WebVTT header")]
    MissingWebVttHeader,
    #[error("invalid timestamp at line {line}: {value}")]
    InvalidTimestamp { line: usize, value: String },
    #[error("subtitle cue at line {line} has no text")]
    MissingText { line: usize },
    #[error("ASS file has no usable Events format")]
    MissingAssEventsFormat,
}

pub fn discover_sidecar_subtitles(video: &Path) -> io::Result<Vec<PathBuf>> {
    let Some(parent) = video.parent() else {
        return Ok(Vec::new());
    };
    let Some(stem) = video.file_stem().and_then(|stem| stem.to_str()) else {
        return Ok(Vec::new());
    };
    let stem_lower = stem.to_ascii_lowercase();

    let mut matches = Vec::new();
    for entry in fs::read_dir(parent)? {
        let path = entry?.path();
        if SubtitleFormat::from_path(&path).is_none() {
            continue;
        }
        let Some(candidate_stem) = path.file_stem().and_then(|name| name.to_str()) else {
            continue;
        };
        let candidate_lower = candidate_stem.to_ascii_lowercase();
        let compatible = candidate_lower == stem_lower
            || candidate_lower
                .strip_prefix(&stem_lower)
                .is_some_and(|suffix| suffix.starts_with('.') || suffix.starts_with('-'));
        if compatible {
            matches.push(path);
        }
    }
    matches.sort_by(|left, right| {
        left.file_name()
            .cmp(&right.file_name())
            .then_with(|| left.cmp(right))
    });
    Ok(matches)
}

fn parse_srt(input: &str) -> Result<SubtitleTrack, SubtitleParseError> {
    let mut track = SubtitleTrack::new(SubtitleFormat::Srt);
    let lines = normalized_lines(input);
    let mut index = 0;

    while index < lines.len() {
        if lines[index].trim().is_empty() {
            index += 1;
            continue;
        }

        let block_line = index + 1;
        let identifier = if lines[index].contains("-->") {
            None
        } else {
            let value = lines[index]
                .trim()
                .trim_start_matches('\u{feff}')
                .to_string();
            index += 1;
            Some(value)
        };
        if index >= lines.len() {
            return Err(SubtitleParseError::InvalidTimestamp {
                line: block_line,
                value: String::new(),
            });
        }

        let (start_ms, end_ms, settings) = parse_timing_line(lines[index], index + 1, ',')?;
        index += 1;
        let text_start = index;
        while index < lines.len() && !lines[index].trim().is_empty() {
            index += 1;
        }
        if index == text_start {
            return Err(SubtitleParseError::MissingText { line: block_line });
        }
        let mut cue = SubtitleCue::new(0, start_ms, end_ms, lines[text_start..index].join("\n"));
        cue.identifier = identifier;
        cue.settings = settings;
        track.push_imported_cue(cue);
    }
    track.sort_cues();
    Ok(track)
}

fn parse_webvtt(input: &str) -> Result<SubtitleTrack, SubtitleParseError> {
    let lines = normalized_lines(input);
    if !lines
        .first()
        .is_some_and(|line| line.trim_start_matches('\u{feff}').starts_with("WEBVTT"))
    {
        return Err(SubtitleParseError::MissingWebVttHeader);
    }

    let mut track = SubtitleTrack::new(SubtitleFormat::WebVtt);
    let mut index = 1;
    while index < lines.len() && !lines[index].trim().is_empty() {
        if let Some((key, value)) = lines[index].split_once(':') {
            track
                .metadata
                .insert(key.trim().to_string(), value.trim().to_string());
        }
        index += 1;
    }

    while index < lines.len() {
        while index < lines.len() && lines[index].trim().is_empty() {
            index += 1;
        }
        if index >= lines.len() {
            break;
        }
        if lines[index].starts_with("NOTE") || lines[index] == "STYLE" || lines[index] == "REGION" {
            while index < lines.len() && !lines[index].trim().is_empty() {
                index += 1;
            }
            continue;
        }

        let block_line = index + 1;
        let identifier = if lines[index].contains("-->") {
            None
        } else {
            let identifier = Some(lines[index].trim().to_string());
            index += 1;
            identifier
        };
        if index >= lines.len() {
            return Err(SubtitleParseError::InvalidTimestamp {
                line: block_line,
                value: String::new(),
            });
        }

        let (start_ms, end_ms, settings) = parse_timing_line(lines[index], index + 1, '.')?;
        index += 1;
        let text_start = index;
        while index < lines.len() && !lines[index].trim().is_empty() {
            index += 1;
        }
        if index == text_start {
            return Err(SubtitleParseError::MissingText { line: block_line });
        }
        let mut cue = SubtitleCue::new(0, start_ms, end_ms, lines[text_start..index].join("\n"));
        cue.identifier = identifier;
        cue.settings = settings;
        track.push_imported_cue(cue);
    }
    track.sort_cues();
    Ok(track)
}

fn parse_ass(input: &str) -> Result<SubtitleTrack, SubtitleParseError> {
    let lines = normalized_lines(input);
    let mut track = SubtitleTrack::new(SubtitleFormat::Ass);
    let mut section = "";
    let mut fields: Option<Vec<String>> = None;

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed;
            continue;
        }
        if section.eq_ignore_ascii_case("[Script Info]") {
            if let Some((key, value)) = trimmed.split_once(':') {
                let key = key.trim().to_string();
                let value = value.trim().to_string();
                if key.eq_ignore_ascii_case("Title") {
                    track.title = Some(value.clone());
                }
                if key.eq_ignore_ascii_case("Language") {
                    track.language = Some(value.clone());
                }
                track.metadata.insert(key, value);
            }
        } else if section.eq_ignore_ascii_case("[Events]") {
            if let Some(value) = trimmed.strip_prefix("Format:") {
                fields = Some(
                    value
                        .split(',')
                        .map(|field| field.trim().to_ascii_lowercase())
                        .collect(),
                );
                continue;
            }
            let Some(dialogue) = trimmed.strip_prefix("Dialogue:") else {
                continue;
            };
            let Some(fields) = fields.as_ref() else {
                return Err(SubtitleParseError::MissingAssEventsFormat);
            };
            let values = dialogue
                .splitn(fields.len(), ',')
                .map(str::trim)
                .collect::<Vec<_>>();
            if values.len() != fields.len() {
                continue;
            }
            let value = |name: &str| {
                fields
                    .iter()
                    .position(|field| field == name)
                    .and_then(|position| values.get(position).copied())
            };
            let start_text = value("start").unwrap_or_default();
            let end_text = value("end").unwrap_or_default();
            let start_ms = parse_ass_timestamp(start_text).ok_or_else(|| {
                SubtitleParseError::InvalidTimestamp {
                    line: index + 1,
                    value: start_text.to_string(),
                }
            })?;
            let end_ms = parse_ass_timestamp(end_text).ok_or_else(|| {
                SubtitleParseError::InvalidTimestamp {
                    line: index + 1,
                    value: end_text.to_string(),
                }
            })?;
            let text = value("text")
                .unwrap_or_default()
                .replace("\\N", "\n")
                .replace("\\n", "\n");
            if text.trim().is_empty() {
                return Err(SubtitleParseError::MissingText { line: index + 1 });
            }
            let mut cue = SubtitleCue::new(0, start_ms, end_ms, text);
            cue.style = value("style")
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            cue.speaker = value("name")
                .filter(|value| !value.is_empty())
                .map(str::to_string);
            track.push_imported_cue(cue);
        }
    }

    if fields.is_none() {
        return Err(SubtitleParseError::MissingAssEventsFormat);
    }
    track.sort_cues();
    Ok(track)
}

fn normalized_lines(input: &str) -> Vec<&str> {
    input
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .collect()
}

fn parse_timing_line(
    line: &str,
    line_number: usize,
    decimal_separator: char,
) -> Result<(u64, u64, Option<String>), SubtitleParseError> {
    let Some((start, end_and_settings)) = line.split_once("-->") else {
        return Err(SubtitleParseError::InvalidTimestamp {
            line: line_number,
            value: line.to_string(),
        });
    };
    let mut end_parts = end_and_settings.split_whitespace();
    let end = end_parts.next().unwrap_or_default();
    let settings = {
        let value = end_parts.collect::<Vec<_>>().join(" ");
        (!value.is_empty()).then_some(value)
    };
    let parse = |value: &str| {
        parse_timestamp(value.trim(), decimal_separator).ok_or_else(|| {
            SubtitleParseError::InvalidTimestamp {
                line: line_number,
                value: value.trim().to_string(),
            }
        })
    };
    Ok((parse(start)?, parse(end)?, settings))
}

fn parse_timestamp(value: &str, decimal_separator: char) -> Option<u64> {
    let (clock, fraction) = value.rsplit_once(decimal_separator)?;
    let parts = clock.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (
            0,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        [hours, minutes, seconds] => (
            hours.parse::<u64>().ok()?,
            minutes.parse::<u64>().ok()?,
            seconds.parse::<u64>().ok()?,
        ),
        _ => return None,
    };
    if minutes >= 60 || seconds >= 60 {
        return None;
    }
    let fraction = match fraction.len() {
        1 => fraction.parse::<u64>().ok()?.saturating_mul(100),
        2 => fraction.parse::<u64>().ok()?.saturating_mul(10),
        3 => fraction.parse::<u64>().ok()?,
        _ => return None,
    };
    Some(
        hours
            .saturating_mul(3_600_000)
            .saturating_add(minutes.saturating_mul(60_000))
            .saturating_add(seconds.saturating_mul(1_000))
            .saturating_add(fraction),
    )
}

fn parse_ass_timestamp(value: &str) -> Option<u64> {
    parse_timestamp(value, '.')
}

#[cfg(test)]
mod tests {
    use std::fs::File;

    use super::*;

    #[test]
    fn parses_multiline_srt_and_preserves_identifier() {
        let track = SubtitleTrack::parse(
            SubtitleFormat::Srt,
            "1\r\n00:00:01,250 --> 00:00:03,500\r\nPrimera\r\nsegunda\r\n\r\n2\r\n00:00:04,000 --> 00:00:05,000\r\nFin\r\n",
        )
        .unwrap();

        assert_eq!(track.cues().len(), 2);
        assert_eq!(track.cues()[0].identifier.as_deref(), Some("1"));
        assert_eq!(track.cues()[0].text, "Primera\nsegunda");
        assert_eq!(
            (track.cues()[0].start_ms, track.cues()[0].end_ms),
            (1_250, 3_500)
        );
    }

    #[test]
    fn parses_webvtt_identifiers_settings_and_minute_timestamps() {
        let track = SubtitleTrack::parse(
            SubtitleFormat::WebVtt,
            "\u{feff}WEBVTT\nKind: captions\n\nintro\n01:02.300 --> 01:04.000 align:start\nHello\n",
        )
        .unwrap();

        assert_eq!(
            track.metadata.get("Kind").map(String::as_str),
            Some("captions")
        );
        assert_eq!(track.cues()[0].identifier.as_deref(), Some("intro"));
        assert_eq!(track.cues()[0].settings.as_deref(), Some("align:start"));
        assert_eq!(track.cues()[0].start_ms, 62_300);
    }

    #[test]
    fn parses_ass_format_order_commas_and_known_metadata() {
        let track = SubtitleTrack::parse(
            SubtitleFormat::Ass,
            "[Script Info]\nTitle: Demo\nLanguage: es\n[Events]\nFormat: Layer, Start, End, Style, Name, Text\nDialogue: 0,0:00:01.20,0:00:03.40,Default,Ana,Hola, mundo\\Nsegunda línea\n",
        )
        .unwrap();

        assert_eq!(track.title.as_deref(), Some("Demo"));
        assert_eq!(track.language.as_deref(), Some("es"));
        assert_eq!(track.cues()[0].text, "Hola, mundo\nsegunda línea");
        assert_eq!(track.cues()[0].speaker.as_deref(), Some("Ana"));
        assert_eq!(
            (track.cues()[0].start_ms, track.cues()[0].end_ms),
            (1_200, 3_400)
        );
    }

    #[test]
    fn reports_invalid_durations_overlaps_empty_text_and_outside_media() {
        let mut track = SubtitleTrack::new(SubtitleFormat::Srt);
        let first = track.add_cue(1_000, 3_000, "One");
        let second = track.add_cue(2_500, 2_000, " ");

        assert_eq!(
            track.diagnostics(2_700),
            vec![
                SubtitleDiagnostic::OutsideMedia {
                    cue_id: first,
                    media_duration_ms: 2_700,
                },
                SubtitleDiagnostic::InvalidDuration { cue_id: second },
                SubtitleDiagnostic::EmptyText { cue_id: second },
                SubtitleDiagnostic::Overlap {
                    first_cue_id: first,
                    second_cue_id: second,
                },
            ]
        );
    }

    #[test]
    fn reports_every_overlap_with_a_long_running_cue() {
        let mut track = SubtitleTrack::new(SubtitleFormat::Srt);
        let long = track.add_cue(0, 10_000, "Long");
        let first = track.add_cue(1_000, 2_000, "First");
        let second = track.add_cue(3_000, 4_000, "Second");

        let overlaps = track
            .diagnostics(10_000)
            .into_iter()
            .filter(|diagnostic| matches!(diagnostic, SubtitleDiagnostic::Overlap { .. }))
            .collect::<Vec<_>>();
        assert_eq!(
            overlaps,
            vec![
                SubtitleDiagnostic::Overlap {
                    first_cue_id: long,
                    second_cue_id: first,
                },
                SubtitleDiagnostic::Overlap {
                    first_cue_id: long,
                    second_cue_id: second,
                },
            ]
        );
    }

    #[test]
    fn editing_resorts_cues_and_keeps_ids_stable() {
        let mut track = SubtitleTrack::new(SubtitleFormat::Srt);
        let late = track.add_cue(5_000, 6_000, "Late");
        let early = track.add_cue(1_000, 2_000, "Early");

        assert_eq!(track.cues()[0].id, early);
        assert!(track.update_cue(late, 500, 900, "Now first"));
        assert_eq!(track.cues()[0].id, late);
        assert!(track.remove_cue(early));
        assert!(!track.remove_cue(999));
    }

    #[test]
    fn retimes_and_clips_subtitles_across_joined_ranges() {
        let mut track = SubtitleTrack::new(SubtitleFormat::Srt);
        track.add_cue(500, 1_500, "A");
        track.add_cue(4_500, 5_500, "B");
        track.add_cue(8_000, 9_000, "Excluded");

        let output =
            track.retimed_for_ranges(&[ClipRange::new(1_000, 5_000), ClipRange::new(5_000, 6_000)]);

        assert_eq!(output.cues().len(), 3);
        assert_eq!(
            (output.cues()[0].start_ms, output.cues()[0].end_ms),
            (0, 500)
        );
        assert_eq!(
            (output.cues()[1].start_ms, output.cues()[1].end_ms),
            (3_500, 4_000)
        );
        assert_eq!(
            (output.cues()[2].start_ms, output.cues()[2].end_ms),
            (4_000, 4_500)
        );
    }

    #[test]
    fn discovers_only_compatible_sidecar_names() {
        let directory = std::env::temp_dir().join(format!(
            "clips-subtitles-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        fs::create_dir_all(&directory).unwrap();
        let video = directory.join("movie.final.mp4");
        for name in [
            "movie.final.srt",
            "movie.final.es.vtt",
            "movie.final-forced.ass",
            "movie.srt",
            "movie.final.txt",
        ] {
            File::create(directory.join(name)).unwrap();
        }

        let matches = discover_sidecar_subtitles(&video).unwrap();
        let names = matches
            .iter()
            .filter_map(|path| path.file_name()?.to_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec![
                "movie.final-forced.ass",
                "movie.final.es.vtt",
                "movie.final.srt"
            ]
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
