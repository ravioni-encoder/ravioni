use std::default::Default;
use std::fmt;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::ops::Div;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use log::{debug, error, info};

// Values for opus bitrates are somewhat arbitrarily chosen to be around 90% of Xiph's recommended
// transparency settings[1]. This is a compromise between the fear of missing out on perfect quality
// and "It's just a movie, bro!". 🤷‍♂️ [1]
// https://wiki.xiph.org/Opus_Recommended_Settings#Recommended_Bitrates
const DEFAULT_OPUS_BITRATE_2_0: u32 = 112;
const DEFAULT_OPUS_BITRATE_5_1: u32 = 224;
const DEFAULT_OPUS_BITRATE_7_1: u32 = 394;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathSettings {
    pub input_file: PathBuf,
    pub scan_for_subtitles: bool,
    pub output_file: PathBuf,
    pub sync_output_filename: bool,
}

impl PathSettings {
    /// Asserts that all settings are ready to be encoded.
    pub fn assert_that_is_ready_for_encoding(&self) -> Result<()> {
        self.assert_that_input_file_is_ready_for_encoding()?;
        self.assert_that_output_file_is_ready_for_encoding()?;
        Ok(())
    }

    pub fn assert_that_input_file_is_ready_for_encoding(&self) -> Result<()> {
        self.assert_input_file_is_a_file_and_exists()?;
        Ok(())
    }

    pub fn assert_that_output_file_is_ready_for_encoding(&self) -> Result<()> {
        self.assert_output_file_path_does_not_end_with_a_path_separator()?;
        self.assert_output_file_is_writable()?;
        Ok(())
    }

    fn assert_input_file_is_a_file_and_exists(&self) -> Result<()> {
        if !self.input_file.is_file() {
            bail!("Input file does not exist or is not readable.");
        }
        Ok(())
    }

    ///  Returns whether the output directory can be written into.
    ///
    /// This can not always be determined by permission flags. The most reliable way is to simply
    /// attempt to write a file, what we do here.
    fn assert_output_file_is_writable(&self) -> Result<()> {
        if let Some(test_file_parent) = self.output_file.parent() {
            let test_file_path = test_file_parent.join(".ravioni-write-test-22604.tmp");
            if let Ok(mut file) = File::create(&test_file_path) {
                if file.write_all(b"ravioni-write-test").is_ok() {
                    let _ = fs::remove_file(test_file_path); // Clean up the test file
                    return Ok(());
                }
            }
        }
        bail!("Output directory can not be written into.");
    }

    /// Assert that output file path does end with a path separator such as / on Linux or \ on
    /// Windows.
    fn assert_output_file_path_does_not_end_with_a_path_separator(&self) -> Result<()> {
        if let Some(last_char) = self.output_file.to_string_lossy().chars().last() {
            if std::path::is_separator(last_char) {
                bail!("Path points to a directory and not a file.");
            }
        }
        Ok(())
    }

    /// Swap filename in output file path with the input filename.
    ///
    /// Used when opening a new input file to automatically update the output file path without
    /// changing the output directory.
    pub fn output_file_path_with_inputs_filename(&self) -> anyhow::Result<PathBuf> {
        let input_filename = self
            .input_file
            .file_name()
            .context("Input file path has no filename.")?;
        let parent = self
            .output_file
            .parent()
            .context("Output file path has no directory (parent).")?;

        Ok(parent.join(input_filename))
    }
}

impl Default for PathSettings {
    fn default() -> Self {
        Self {
            input_file: PathBuf::new(),
            scan_for_subtitles: true,
            output_file: PathBuf::new(),
            sync_output_filename: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Av1Crf(u32);

impl Av1Crf {
    const MIN: u32 = 0;
    const MAX: u32 = 63;

    pub fn new(value: u32) -> Result<Self> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(anyhow!(
                "AV1 crf must be between {} and {}.",
                Self::MIN,
                Self::MAX
            ))
        }
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Av1Preset(u32);

impl Av1Preset {
    const MIN: u32 = 0;
    const MAX: u32 = 8;

    pub fn new(value: u32) -> Result<Self> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(anyhow!(
                "AV1 preset must be between {} and {}.",
                Self::MIN,
                Self::MAX
            ))
        }
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoSettings {
    pub crf: Av1Crf,
    pub preset: Av1Preset,
    pub crop: bool,
    pub crop_width: Option<u32>,
    pub crop_height: Option<u32>,
    pub crop_x_offset: Option<u32>,
    pub crop_y_offset: Option<u32>,
    pub scale: bool,
    pub scale_width: Option<u32>,
    pub scale_height: Option<u32>,
    pub grain: u32,
    pub denoise: bool,
}

impl VideoSettings {
    pub fn filter_args(&self) -> Vec<String> {
        let mut filters: Vec<String> = Vec::new();

        if self.crop {
            let width = self
                .crop_width
                .map_or("iw".to_string(), |number| number.to_string());
            let height = self
                .crop_height
                .map_or("ih".to_string(), |number| number.to_string());
            let x_offset = self
                .crop_x_offset
                .map_or("".to_string(), |number| number.to_string());
            let y_offset = self
                .crop_y_offset
                .map_or("".to_string(), |number| number.to_string());
            filters.push(format!(
                "crop={}:{}:{}:{}",
                width, height, x_offset, y_offset
            ));
        }

        if self.scale {
            let width = self
                .scale_width
                .map_or("iw".to_string(), |number| number.to_string());
            let height = self
                .scale_height
                .map_or("ih".to_string(), |number| number.to_string());
            filters.push(format!(
                "scale={}:{}:flags=bicubic:param0=0:param1=1/2",
                width, height
            ));
        }

        // Returning a Vec<String> here instead of an already assembled string, because `args()` in
        // [`crate::ffmpeg::encode()`] can skip an empty iterable gracefully. An empty string, on
        // the other hand, still "activates" `arg()` which then passes an empty argument to ffmpeg
        // which causes errors. ffmpeg interprets an argument without `-` as the output file and
        // complains that we gave it an empty output file.
        if filters.is_empty() {
            Vec::<String>::new()
        } else {
            vec!["-vf".to_string(), filters.join(",")]
        }
    }

    pub fn av1_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let denoise = (self.denoise as u32).to_string();
        args.push("-svtav1-params".to_string());
        // TODO: change to `tune=3` after custom SVT-AV1-PSY is included !!!
        args.push(format!(
            "tune=0:sharpness=1:input-depth=10:enable-qm=1:qm-min=0:keyint=300:aq-mode=2:sharpness=1:irefresh-type=2:film-grain={}:film-grain-denoise={}",
            self.grain, denoise
        ));
        args
    }
}

impl Default for VideoSettings {
    fn default() -> Self {
        VideoSettings {
            crf: Av1Crf::new(22).unwrap(),
            preset: Av1Preset::new(4).unwrap(),
            crop: false,
            crop_width: None,
            crop_height: None,
            crop_x_offset: None,
            crop_y_offset: None,
            scale: false,
            scale_width: None,
            scale_height: None,
            grain: 0,
            denoise: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AudioChannelLayout {
    Mono,
    Stereo20,
    Surround51,
    Surround51Side,
    Surround71,
}

impl fmt::Display for AudioChannelLayout {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Copy)]
pub struct OpusBitrate(u32);

impl OpusBitrate {
    const MIN: u32 = 6;
    const MAX: u32 = 510;

    pub fn new(value: u32) -> Result<Self> {
        if (Self::MIN..=Self::MAX).contains(&value) {
            Ok(Self(value))
        } else {
            Err(anyhow!(
                "Opus bitrate must be between {} and {}.",
                Self::MIN,
                Self::MAX
            ))
        }
    }

    pub fn value(&self) -> u32 {
        self.0
    }
}

impl Div<u32> for OpusBitrate {
    type Output = Self;

    fn div(self, right: u32) -> Self::Output {
        assert!(right != 0, "division by zero");
        // Force new bitrate to be within bounds. This operator is used for calculating the Mono
        // bitrate. Results don't have to be precise and fault-tolerance is more important.
        let new_value = (self.0 / right).clamp(OpusBitrate::MIN, OpusBitrate::MAX);
        OpusBitrate(new_value)
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioSettings {
    pub bitrate_2_0: OpusBitrate,
    pub bitrate_5_1: OpusBitrate,
    pub bitrate_7_1: OpusBitrate,
    pub downmix_5_1_to_stereo: bool,
    pub downmix_7_1_to_stereo: bool,
    pub volume_adjustment: u32,
    pub re_encode_good_audio_codecs: bool,
    pub copy: bool,
}

impl AudioSettings {
    pub fn bitrate_for_layout(&self, channel_layout: &AudioChannelLayout) -> OpusBitrate {
        match channel_layout {
            AudioChannelLayout::Mono => self.bitrate_2_0 / 2,
            AudioChannelLayout::Stereo20 => self.bitrate_2_0,
            AudioChannelLayout::Surround51 | AudioChannelLayout::Surround51Side => {
                if self.downmix_5_1_to_stereo {
                    self.bitrate_2_0
                } else {
                    self.bitrate_5_1
                }
            }
            AudioChannelLayout::Surround71 => {
                if self.downmix_7_1_to_stereo {
                    self.bitrate_2_0
                } else {
                    self.bitrate_7_1
                }
            }
        }
    }

    /// Return whether the given codec should be re-encoded according to the current AudioSettings.
    pub fn should_re_encode(&self, codec: &str) -> bool {
        self.re_encode_good_audio_codecs || !matches!(codec, "opus" | "vorbis" | "mp3")
    }
}

impl Default for AudioSettings {
    fn default() -> Self {
        AudioSettings {
            bitrate_2_0: OpusBitrate::new(DEFAULT_OPUS_BITRATE_2_0).unwrap(),
            bitrate_5_1: OpusBitrate::new(DEFAULT_OPUS_BITRATE_5_1).unwrap(),
            bitrate_7_1: OpusBitrate::new(DEFAULT_OPUS_BITRATE_7_1).unwrap(),
            downmix_5_1_to_stereo: false,
            downmix_7_1_to_stereo: false,
            volume_adjustment: 100,
            re_encode_good_audio_codecs: false,
            copy: false,
        }
    }
}

//

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct EncodingSettings {
    pub video: VideoSettings,
    pub audio: AudioSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApplicationSettings {
    pub last_input_directory: Option<PathBuf>,
    pub last_output_directory: Option<PathBuf>,
    pub dark_mode: bool,
    pub advanced_settings: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct MatroskaSettings {
    pub file_title: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSettings {
    pub only_encode_segment: bool,
    only_encode_from: Option<String>,
    pub only_encode_for: Option<Duration>,
}

impl RunSettings {
    fn is_valid_time_format(time: &str) -> bool {
        let parts: Vec<&str> = time.split(':').collect();
        if parts.len() != 3 {
            return false;
        }

        if let (Ok(_), Ok(minutes), Ok(seconds)) = (
            parts[0].parse::<u32>(),
            parts[1].parse::<u32>(),
            parts[2].parse::<u32>(),
        ) {
            return minutes < 60 && seconds < 60;
        }
        false
    }

    pub fn try_new(
        only_encode_segment: bool,
        only_encode_from: Option<String>,
        only_encode_for: Option<Duration>,
    ) -> Result<Self, &'static str> {
        if let Some(ref encode_from) = only_encode_from {
            if !Self::is_valid_time_format(encode_from) {
                return Err("Invalid time format for only_encode_from");
            }
        }
        Ok(Self {
            only_encode_segment,
            only_encode_from,
            only_encode_for,
        })
    }

    pub fn set_only_encode_from(&mut self, duration: String) -> Result<(), &'static str> {
        if !Self::is_valid_time_format(&duration) {
            return Err("Invalid time format given.");
        }
        self.only_encode_from = Some(duration);
        Ok(())
    }

    pub fn only_encode_from(&self) -> Option<&String> {
        self.only_encode_from.as_ref()
    }
}

impl Default for RunSettings {
    fn default() -> Self {
        Self::try_new(false, None, None).unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub version: i32,
    pub paths: PathSettings,
    pub encoding: EncodingSettings,
    pub matroska: MatroskaSettings,
    pub run: RunSettings,
    pub application: ApplicationSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            paths: PathSettings::default(),
            encoding: EncodingSettings::default(),
            matroska: MatroskaSettings::default(),
            run: RunSettings::default(),
            application: ApplicationSettings::default(),
        }
    }
}

impl Settings {
    pub fn new() -> Self {
        let path = Settings::config_file_path();
        debug!("Reading settings from {:?}", path);

        let Ok(string) = fs::read_to_string(&path) else {
            info!(
                "Could not read file at {:?}. Will use default settings.",
                path
            );
            return Settings::default();
        };
        let Ok(settings) = toml::from_str::<Settings>(&string) else {
            error!(
            "Failed to parse encoding settings. Will use default settings. The content of the file was: {:?}",
            string);
            return Settings::default();
        };

        debug!("Read settings: {:?}", settings);
        settings
    }

    fn config_file_path() -> PathBuf {
        dirs::config_dir()
            .expect("Could not determine config directory.")
            .join("ravioni/ravioni.toml")
    }

    pub fn write_to_file(&self) {
        let toml = toml::to_string(&self).expect("Failed to serialize settings to TOML.");
        let file_path = Settings::config_file_path();
        let parent_directory = file_path
            .parent()
            .unwrap_or_else(|| panic!("Failed to get parent of {:?}.", file_path));
        if !parent_directory.exists() {
            fs::create_dir_all(parent_directory).unwrap_or_else(|_| {
                panic!(
                    "Failed to create config directory {:?} and it does not exist.",
                    parent_directory
                )
            });
        }
        fs::write(&file_path, toml.clone()).expect("Unable to write config file.");
        debug!("Wrote settings to file {:?}. Content: {toml}", file_path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn opus_bitrate_can_only_be_created_within_bounds() {
        assert!(OpusBitrate::new(6).is_ok());
        assert!(OpusBitrate::new(510).is_ok());
        assert!(OpusBitrate::new(5).is_err());
        assert!(OpusBitrate::new(511).is_err());
    }

    #[test]
    fn av1_crf_can_only_be_created_within_bounds() {
        assert!(Av1Crf::new(0).is_ok());
        assert!(Av1Crf::new(63).is_ok());
        assert!(Av1Crf::new(64).is_err());
        assert!(Av1Crf::new(u32::MAX).is_err());
    }

    #[test]
    fn av1_preset_can_only_be_created_within_bounds() {
        assert!(Av1Preset::new(0).is_ok());
        assert!(Av1Preset::new(8).is_ok());
        assert!(Av1Preset::new(9).is_err());
        assert!(Av1Preset::new(100).is_err());
    }

    #[test]
    fn downmixed_surround_5_1_audio_uses_stereo_bitrate() {
        let audio_settings = AudioSettings {
            bitrate_2_0: OpusBitrate::new(96).unwrap(),
            bitrate_5_1: OpusBitrate::new(200).unwrap(),
            bitrate_7_1: OpusBitrate::new(300).unwrap(),
            downmix_5_1_to_stereo: true,
            downmix_7_1_to_stereo: false,
            volume_adjustment: 100,
            re_encode_good_audio_codecs: false,
            copy: false,
        };
        assert_eq!(
            audio_settings
                .bitrate_for_layout(&AudioChannelLayout::Surround51)
                .value(),
            96
        );
    }

    #[test]
    fn downmixed_surround_7_1_audio_uses_stereo_bitrate() {
        let audio_settings = AudioSettings {
            bitrate_2_0: OpusBitrate::new(96).unwrap(),
            bitrate_5_1: OpusBitrate::new(200).unwrap(),
            bitrate_7_1: OpusBitrate::new(300).unwrap(),
            downmix_5_1_to_stereo: false,
            downmix_7_1_to_stereo: true,
            volume_adjustment: 100,
            re_encode_good_audio_codecs: false,
            copy: false,
        };
        assert_eq!(
            audio_settings
                .bitrate_for_layout(&AudioChannelLayout::Surround71)
                .value(),
            96
        );
    }

    #[test]
    fn mono_audio_channel_bitrate_is_half_of_stereo_bitrate() {
        let audio_settings = AudioSettings {
            bitrate_2_0: OpusBitrate::new(256).unwrap(),
            bitrate_5_1: OpusBitrate::new(400).unwrap(),
            bitrate_7_1: OpusBitrate::new(500).unwrap(),
            downmix_5_1_to_stereo: false,
            downmix_7_1_to_stereo: false,
            volume_adjustment: 100,
            re_encode_good_audio_codecs: false,
            copy: false,
        };

        let mono_bitrate = audio_settings
            .bitrate_for_layout(&AudioChannelLayout::Mono)
            .value();

        assert_eq!(mono_bitrate, 128);
    }

    #[test]
    fn video_settings_with_valid_values_are_allowed() {
        let _ = VideoSettings {
            crf: Av1Crf::new(22).unwrap(),
            preset: Av1Preset::new(6).unwrap(),
            crop: true,
            crop_width: Some(3840),
            crop_height: Some(2160),
            crop_x_offset: None,
            crop_y_offset: None,
            scale: true,
            scale_width: Some(1920),
            scale_height: Some(1080),
            grain: 5,
            denoise: true,
        };
    }

    #[test]
    fn path_settings_fail_when_input_file_does_not_exist() {
        let path_settings = PathSettings {
            input_file: PathBuf::from("nonexistent.mkv"),
            scan_for_subtitles: false,
            output_file: PathBuf::from("output.mkv"),
            sync_output_filename: true,
        };
        let result = path_settings.assert_that_is_ready_for_encoding();
        assert!(result.is_err());
    }

    #[test]
    fn path_settings_fail_when_output_directory_does_not_exist() {
        let temp_dir = tempdir().unwrap();
        let input_file_path = temp_dir.path().join("input.mkv");
        File::create(&input_file_path).unwrap();
        let path_settings = PathSettings {
            input_file: input_file_path,
            scan_for_subtitles: true,
            output_file: PathBuf::from("nonexistent_directory").join("output.mkv"),
            sync_output_filename: true,
        };
        let result = path_settings.assert_that_is_ready_for_encoding();
        assert!(result.is_err());
    }

    #[test]
    fn path_settings_fail_when_output_directory_is_not_writable() {
        let temp_dir = tempdir().unwrap();
        let input_file_path = temp_dir.path().join("input.mkv");
        File::create(&input_file_path).unwrap();
        let path_settings = PathSettings {
            input_file: input_file_path,
            scan_for_subtitles: true,
            output_file: PathBuf::from("/root/output.mkv"),
            sync_output_filename: true,
        };
        let result = path_settings.assert_that_is_ready_for_encoding();
        assert!(result.is_err());
    }

    #[test]
    fn path_settings_are_valid_when_input_and_output_are_valid() {
        let temp_dir = tempdir().unwrap();
        let input_file_path = temp_dir.path().join("input.mkv");
        let output_file_path = temp_dir.path().join("output.mkv");
        File::create(&input_file_path).unwrap();
        std::fs::create_dir(&output_file_path).unwrap();
        let path_settings = PathSettings {
            input_file: input_file_path,
            scan_for_subtitles: true,
            output_file: output_file_path,
            sync_output_filename: true,
        };
        let result = path_settings.assert_that_is_ready_for_encoding();
        assert!(result.is_ok());
    }
}
