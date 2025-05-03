//! This module contains code that represents metadata of video files such as audio streams or
//! frames per second.

use std::convert::From;
use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use ffmpeg_sidecar::event::Stream;
use slint::SharedString;

use crate::settings::AudioChannelLayout;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStream {
    pub index: u32,
    pub channel_layout: AudioChannelLayout,
    pub codec: String,
}

impl TryFrom<Stream> for AudioStream {
    type Error = anyhow::Error;

    fn try_from(stream: Stream) -> Result<Self> {
        let audio_data = stream
            .audio_data()
            .ok_or_else(|| anyhow!("Stream does not contain audio data"))?;

        let channel_layout = match audio_data.channels.as_str() {
            "mono" => AudioChannelLayout::Mono,
            "stereo" => AudioChannelLayout::Stereo20,
            "5.1" => AudioChannelLayout::Surround51,
            "5.1(side)" => AudioChannelLayout::Surround51Side,
            "7.1" => AudioChannelLayout::Surround71,
            channels => bail!("Unsupported channel layout detected: {}", channels),
        };

        Ok(AudioStream {
            index: stream.stream_index,
            channel_layout,
            codec: stream.format,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleStream {
    pub index: u32,
    pub format: String,
}

impl From<Stream> for SubtitleStream {
    fn from(stream: Stream) -> Self {
        if stream.is_subtitle() {
            SubtitleStream {
                index: stream.stream_index,
                format: stream.format,
            }
        } else {
            panic!("Stream does not contain subtitle data");
        }
    }
}

/// Stores metadata of a input file. Extracted mostly by `[ffmpeg::detect_metadata]`
#[derive(Debug, Clone, PartialEq)]
pub struct InputFileMetadata {
    pub fps: f32,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub audio_streams: Vec<AudioStream>,
    pub subtitle_streams: Vec<SubtitleStream>,
    pub highest_stream_index: u32,
}

/// Returns a Duration formatted as a hh:mm:ss string.
pub fn duration_to_string(duration: &Duration) -> String {
    let total_seconds = duration.as_secs();
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
}

/// Returns a Duration formatted as a hh:mm:ss SharedString as required by Slint.
pub fn duration_to_shared_string(duration: &Duration) -> SharedString {
    SharedString::from(duration_to_string(duration))
}

#[cfg(test)]
pub mod tests {

    use super::*;
    use std::time::Duration;

    #[test]
    fn format_duration_returns_hh_mm_ss_format() {
        //                                          4 hours,   12 min,   30 s
        let duration = Duration::new(4 * 3600 + 12 * 60 + 30, 0);
        assert_eq!("04:12:30", duration_to_string(&duration).as_str());
    }
}
