use std::cmp::max;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::LazyLock;
use std::thread;

use std::time::Duration;

use anyhow::{bail, Context};
use ffmpeg_sidecar::{
    command::FfmpegCommand,
    event::{FfmpegEvent, LogLevel},
};
use log::{debug, error, warn};
use rand::Rng;
use regex::Regex;
use slint::Weak;

use crate::job::{self, Job, INPUT_INDEX};
use crate::metadata::{duration_to_string, AudioStream, InputFileMetadata, SubtitleStream};
use crate::settings::AudioChannelLayout;
use crate::subtitles::SubTitleType;
use crate::MainWindow;

static CROP_DETECT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"crop=(\d+):(\d+):(\d+):(\d+)").unwrap());

// Number of time points which are used for crop detection.
static CROP_DETECT_NUMBER_OF_TIME_POINTS: u32 = 12;
// For how long (in seconds) to analyze frames for crop detection at each time point.
static CROP_DETECT_DURATION_AT_TIME_POINT: &str = "0.2";

/// Encodes given job and sends progress and final status back through the given channels.
pub fn encode(
    ui_weak: Weak<MainWindow>,
    job: &Job,
    progress_tx: Sender<(i32, f32)>,
    stopped_tx: Sender<(job::JobStatus, String)>,
    quit_rx: Receiver<()>,
) {
    let output_file_path = &job.paths.output_file;
    let mut encoder = FfmpegCommand::new()
        .hide_banner()
        .args(args_for_input_seek(job))
        .input(
            job.paths
                .input_file
                .to_str()
                .expect("can not unwrap input file in ffmpeg"),
        )
        .args(args_for_input_encoding_duration_limit(job))
        .args(args_for_subtitle_file_inputs(job))
        .args(["-pix_fmt", "yuv420p10le"])
        .codec_video("libsvtav1")
        .crf(job.encoding.video.crf.value())
        .preset(job.encoding.video.preset.value().to_string())
        .args(job.encoding.video.filter_args())
        .args(job.encoding.video.av1_args())
        .args(args_for_audio_codecs(job))
        .args(args_for_all_maps(job))
        .args(args_for_audio_bitrate(job))
        .args(args_for_audio_downmix(job))
        .args(args_for_audio_5_1_side_workaround(job))
        // Subtitle streams are always copied.
        .codec_subtitle("copy")
        // The mov_text workaround must come after the subtitle copy argument to override it.
        .args(args_for_mov_text_subtitle_workaround(job))
        .format("matroska")
        .overwrite()
        .output(
            output_file_path
                .to_str()
                .expect("can not unwrap output file path in ffmpeg"),
        )
        .print_command()
        .spawn()
        .unwrap();

    let events = encoder.iter().unwrap();

    debug!("encode(): About to spawn thread for events.");
    thread::spawn(move || {
        debug!("encode(): Inside thread for events.");
        for event in events {
            if quit_rx.try_recv().is_ok() {
                debug!("encode(): Received quit signal. Quitting.");
                encoder.quit().expect("Could not quit ffmpeg command.");
                // prevent defunct (zombie) process by waiting
                encoder.wait().unwrap();
                return;
            }
            match event {
                FfmpegEvent::Log(LogLevel::Error, e) => {
                    error!("{}", e);
                    // FLAW: Only returns the first error from ffmpeg. For multiline errors, the
                    // actually returned line might not be informative to the user. Ffmpeg errors
                    // should never occur, so this is tolerable. Also, the order of consecutive
                    // error events is not deterministic, i.e. differs from run to run for the same
                    // error.
                    stopped_tx.send((job::JobStatus::Failed, e)).unwrap();
                }
                FfmpegEvent::Progress(p) => {
                    progress_tx.send((p.frame as i32, p.fps)).unwrap();
                    let ui_weak_clone = ui_weak.clone();
                    ui_weak_clone
                        .upgrade_in_event_loop(move |ui| {
                            ui.invoke_collect_progress();
                            // TODO2: I could probably just send the progress
                            // here directly without a channel.
                        })
                        .unwrap();
                }
                // Ffmpeg sidecar yields errors that are more like warnings for us. But still print
                // them to watch for potential bugs.
                FfmpegEvent::Error(e) => warn!(
                    "(likely benign) FfmpegEvent::Error event from sidecar: {}",
                    e
                ),
                _ => {}
            }
        }
        // prevent defunct (zombie) process by waiting
        encoder.wait().unwrap();

        stopped_tx
            .send((job::JobStatus::Finished, String::new()))
            .unwrap();
        ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.invoke_collect_finished();
            })
            .unwrap();
    });
}

/// Extracts metadata such as duration, fps, streams and more of the file in the given path.
pub fn extract_metadata(file_path_str: &str) -> anyhow::Result<InputFileMetadata> {
    debug!("About to extract metadata from {}.", file_path_str);
    let mut ffmpeg = FfmpegCommand::new()
        .input(file_path_str)
        .spawn()
        .context("Failed to spawn ffmpeg process")?;

    let events = ffmpeg.iter().context("Failed to iterate ffmpeg events")?;

    let mut fps: Option<f32> = None;
    let mut duration: Option<Duration> = None;
    let mut width: Option<u32> = None;
    let mut height: Option<u32> = None;
    let mut audio_streams: Vec<AudioStream> = Vec::new();
    let mut subtitle_streams: Vec<SubtitleStream> = Vec::new();
    let mut highest_stream_index: u32 = 0;

    for event in events {
        match event {
            FfmpegEvent::Log(LogLevel::Error, e) => error!("Ffmpeg error: {}", e),
            FfmpegEvent::ParsedDuration(e) => {
                debug!("ParsedDuration: {:?}", e);
                duration = Some(Duration::new(e.duration as u64, 0));
            }
            FfmpegEvent::ParsedInputStream(stream) => {
                debug!("ParsedInputStream: {:?}", stream);

                if let Some(data) = stream.video_data() {
                    fps = Some(data.fps);
                    width = Some(data.width);
                    height = Some(data.height);
                    highest_stream_index = max(highest_stream_index, stream.stream_index);
                } else if stream.is_audio() {
                    highest_stream_index = max(highest_stream_index, stream.stream_index);
                    if let Ok(audio_stream) = AudioStream::try_from(stream) {
                        audio_streams.push(audio_stream);
                    }
                } else if stream.is_subtitle() {
                    highest_stream_index = max(highest_stream_index, stream.stream_index);
                    subtitle_streams.push(stream.into());
                }
            }
            _ => continue,
        }
    }
    debug!("Found {} audio streams.", audio_streams.len());
    debug!("Found {} subtitle streams.", subtitle_streams.len());

    // prevent defunct (zombie) process by waiting
    ffmpeg.wait().unwrap();

    if let (Some(fps), Some(duration), Some(width), Some(height)) = (fps, duration, width, height) {
        debug!(
            "extract_metadata(): Detected fps {}, duration {:?}, width {} and height {} from file {}.",
            fps, duration, width, height, file_path_str
        );
        Ok(InputFileMetadata {
            fps,
            duration,
            width,
            height,
            audio_streams,
            subtitle_streams,
            highest_stream_index,
        })
    } else {
        bail!(
            "extract_metadata(): Failed to extract metadata from {}.",
            file_path_str
        )
    }
}

/// Autodetects crop parameters at random time positions of the input video.
///
/// Use the least crop (biggest dimensions) found. The number of time positions is defined by
/// NUMBER_OF_TIME_POINTS_TO_CHECK_FOR_CROP_DETECTION.
pub fn autodetect_crop_parameters(file_path_str: &str) -> anyhow::Result<(u32, u32, u32, u32)> {
    // Get metadata of input file to know the total duration.
    let input_file_metadata = extract_metadata(file_path_str)?;
    let time_codes = random_time_codes_within_range(
        input_file_metadata.duration,
        CROP_DETECT_NUMBER_OF_TIME_POINTS,
    );
    debug!(
        "Will autodetect crop at time codes: {}.",
        time_codes
            .iter()
            .map(duration_to_string)
            .collect::<Vec<String>>()
            .join(", ")
    );

    let mut crop_parameters: Vec<(u32, u32, u32, u32)> = Vec::new();
    for time_code in time_codes {
        debug!(
            "Now autodetecting at time code {}",
            duration_to_string(&time_code)
        );
        let mut ffmpeg = FfmpegCommand::new()
            // Hint: -ss must come before -i for it to take effect!
            .args([
                "-ss",
                duration_to_string(&time_code).as_str(),
                "-t",
                CROP_DETECT_DURATION_AT_TIME_POINT,
            ])
            .input(file_path_str)
            .args(["-vf", "cropdetect=round=2:limit=0.1,metadata=mode=print"])
            .args(["-f", "null", "-"])
            .spawn()
            .context("Failed to spawn ffmpeg process")?;

        let events = ffmpeg.iter().context("Failed to iterate ffmpeg events")?;

        for event in events {
            match event {
                FfmpegEvent::Log(LogLevel::Error, e) => error!("Ffmpeg error: {}", e),
                FfmpegEvent::Log(LogLevel::Info, log_event) => {
                    if let Some((width, height, x_offset, y_offset)) =
                        extract_crop_parameters(&log_event)
                    {
                        crop_parameters.push((width, height, x_offset, y_offset));
                        debug!("Extracted crop parameters: width:{width} height:{height} x_offset:{x_offset} y_offset:{y_offset}");
                    }
                }
                _ => (),
            }
        }
        // prevent defunct (zombie) process by waiting
        ffmpeg.wait().unwrap();
    }

    let least_cropped_parameters = select_least_cropped_parameters(crop_parameters)
        .context("Could not select least cropped parameters.")?;

    debug!(
        "Final least cropped_parameters: {:?}",
        least_cropped_parameters
    );

    Ok(least_cropped_parameters)
}

fn extract_crop_parameters(string: &str) -> Option<(u32, u32, u32, u32)> {
    CROP_DETECT_REGEX.captures(string).map(|caps| {
        (
            // width
            caps[1].parse().unwrap(),
            // height
            caps[2].parse().unwrap(),
            // x-offset
            caps[3].parse().unwrap(),
            // y-offset
            caps[4].parse().unwrap(),
        )
    })
}

/// From a collection of cropped parameters selects the ones that crop the video the least, i.e.
/// leave the biggest image.
///
/// Some movies have segments with open matte which would otherwise be cut off.
fn select_least_cropped_parameters(
    crop_parameters: Vec<(u32, u32, u32, u32)>,
) -> Option<(u32, u32, u32, u32)> {
    if crop_parameters.len() == 0 {
        return None;
    }
    let (mut max_width, mut max_height) = (0, 0);
    let (mut x_offset, mut y_offset) = (0, 0);

    for (width, height, x_off, y_off) in crop_parameters {
        if width > max_width {
            max_width = width;
            x_offset = x_off;
        }
        if height > max_height {
            max_height = height;
            y_offset = y_off;
        }
    }
    Some((max_width, max_height, x_offset, y_offset))
}

/// Returns random time codes within the given duration (interpreted as time code). Does not return
/// more time codes than there are seconds in the given duration.
///
/// For example, when given time_code 00:10:00 (ten minutes) and n=4 could return 00:01:20,
/// 00:01:59, 00:06:11 and 00:08:02. When given time_code 00:00:04 and n=6, only 4 time codes would
/// be returned.
fn random_time_codes_within_range(time_code: Duration, n: u32) -> Vec<Duration> {
    let mut rng = rand::rng();

    // Don't return more time codes than there are seconds in the video.
    let max_number_of_time_codes = time_code.as_secs() as u32;
    let time_code_count = n.min(max_number_of_time_codes);

    (0..time_code_count)
        .map(|_| {
            let random_duration = rng.random_range(Duration::ZERO..time_code);
            Duration::new(random_duration.as_secs(), 0)
        })
        .collect()
}

/// Creates arguments for ffmpeg that explicitly map (forward) the first video stream.
///
/// Mappings must be given to ffmpeg explicitly because by default ffmpeg only maps (forwards)
/// one stream of each type (video, audio, subtitle) from the input file to the output file.
pub fn args_for_video_maps() -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    // Assume that there is exactly one video stream and that it is the first stream.
    args.push("-map".to_string());
    args.push("v:0".to_string());
    args
}

/// Creates arguments for ffmpeg that explicitly map (forward) all audio streams.
///
/// Mappings must be given to ffmpeg explicitly because by default ffmpeg only maps (forwards)
/// one stream of each type (video, audio, subtitle) from the input file to the output file.
pub fn args_for_audio_maps(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for audio_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for stream maps, if input file metadata
                     is missing. This is a bug. Please report it.",
        )
        .audio_streams
        .iter()
    {
        //Create strings like "-map 0:3"
        args.push("-map".to_string());
        args.push(format!("{INPUT_INDEX}:{}", audio_stream.index));
    }

    args
}

/// Creates arguments for ffmpeg that select the codec for audio streams.
///
/// The stream is either encoded to opus or left unchanged (copied).
pub fn args_for_audio_codecs(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for audio_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for audio stream codecs, if input file metadata
                     is missing. This is a bug. Please report it.",
        )
        .audio_streams
        .iter()
    {
        // "-c:5 libopus" or "-c:5 copy". The 5 here is the global stream index, not just for audio.
        args.push(format!("-c:{}", audio_stream.index));
        if job
            .encoding
            .audio
            .should_re_encode(audio_stream.codec.as_str())
        {
            args.push(format!("libopus"));
        } else {
            args.push(format!("copy"));
        }
    }

    args
}

/// Creates arguments for ffmpeg that explicitly map (forward) all subtitle streams of the input
/// file.
///
/// Mappings must be given to ffmpeg explicitly because by default ffmpeg only maps (forwards)
/// one stream of each type (video, audio, subtitle) from the input file to the output file.
pub fn args_for_internal_subtitles_maps(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for subtitle_stream in job
        .input_file_metadata
        .as_ref()
        .unwrap()
        .subtitle_streams
        .iter()
    {
        // -map 0:3
        args.push("-map".to_string());
        args.push(format!("{INPUT_INDEX}:{}", subtitle_stream.index));
    }

    args
}

/// Creates arguments for ffmpeg that explicitly map (forward) all subtitle streams from
/// external subtitle files.
///
/// Mappings must be given to ffmpeg explicitly because by default ffmpeg only maps (forwards)
/// one stream of each type (video, audio, subtitle) from the input file to the output file.
pub fn args_for_external_subtitles_maps(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    let mut next_free_stream_index = job
        .input_file_metadata
        .as_ref()
        .unwrap()
        .highest_stream_index
        + 1;

    // Always start with 1 for the first external subtitle file, because user is only
    // allowed to select one input file in UI which is index 0.
    let mut next_input_index = 1;

    for subtitle_file in &job.subtitle_files {
        // For external subtitles, in addition to the explicit mapping, metadata about the
        // language has to be provided as well. For each external subtitle file, the arguments
        // look like "-map 3 -metadata:s:3 language=eng".

        // -map 3
        args.push("-map".to_string());
        args.push(next_input_index.to_string());

        // Language will have been only set for subtitle files where ffmpeg can not recognize the
        // language automatically, such as SRT.
        if let Some(lang) = &subtitle_file.language {
            // -metadata:s:3 language=eng
            args.push(format!("-metadata:s:{}", next_free_stream_index));
            args.push(format!("language={}", lang.to_639_3()));
        }

        // -metadata:s:3 title="SDH"
        if subtitle_file.kind == SubTitleType::SDH {
            args.push(format!("-metadata:s:{}", next_free_stream_index));
            args.push("title=SDH".to_string());
        }
        next_free_stream_index += 1;
        next_input_index += 1;
    }

    args
}

/// Creates arguments for ffmpeg that create a workaround for subtitles in mov_text format. These
/// can not be stored in a matroska container and thus need to be converted to srt.
pub fn args_for_mov_text_subtitle_workaround(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for subtitle_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for mov_text subtitle workaround, if input file
                    metadata is missing. This is a bug. Please report it.",
        )
        .subtitle_streams
        .iter()
    {
        // -c:3 srt
        if subtitle_stream.format == "mov_text" {
            debug!("stream format is {}", subtitle_stream.format);
            args.push(format!("-c:{}", subtitle_stream.index));
            args.push("srt".to_string());
        }
    }
    args
}

/// Creates map arguments for ffmpeg that explicitly map (forward) all streams of all types.
///
/// Mappings must be given to ffmpeg explicitly because by default ffmpeg only maps (forwards)
/// one stream of each type (video, audio, subtitle) from the input file to the output file.
pub fn args_for_all_maps(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    args.extend(args_for_video_maps());
    args.extend(args_for_audio_maps(job));
    args.extend(args_for_internal_subtitles_maps(job));
    args.extend(args_for_external_subtitles_maps(job));

    args
}

/// Creates arguments for ffmpeg that set the bitrates for (opus) audio streams depending on
/// their channel layout.
pub fn args_for_audio_bitrate(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for audio_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for audio stream bitrate, if input file
                    metadata is missing. This is a bug. Please report it.",
        )
        .audio_streams
        .iter()
    {
        // -b:3 112k
        args.push(format!("-b:{}", audio_stream.index));
        args.push(format!(
            "{}K",
            job.encoding
                .audio
                .bitrate_for_layout(&audio_stream.channel_layout)
                .value(),
        ));
    }
    args
}

/// Creates arguments for ffmpeg that downmix (opus) audio streams depending on audio settings
/// of this job.
pub fn args_for_audio_downmix(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for audio_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for audio downmixing, if input file
                    metadata is missing. This is a bug. Please report it.",
        )
        .audio_streams
        .iter()
    {
        // -ac:3 2
        if job.encoding.audio.downmix_5_1_to_stereo
            && audio_stream.channel_layout == AudioChannelLayout::Surround51
        {
            args.push(format!("-ac:{}", audio_stream.index));
            args.push("2".to_string());
        }
        if job.encoding.audio.downmix_7_1_to_stereo
            && audio_stream.channel_layout == AudioChannelLayout::Surround71
        {
            args.push(format!("-ac:{}", audio_stream.index));
            args.push("2".to_string());
        }
    }
    args
}

/// Creates arguments for ffmpeg that create a workaround for 5.1 side channels which are not
/// properly supported by libopus. See https://trac.ffmpeg.org/ticket/5718
pub fn args_for_audio_5_1_side_workaround(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for audio_stream in job
        .input_file_metadata
        .as_ref()
        .expect(
            "Can not assemble ffmpeg arguments for audio 5.1 side workaround, if input file
                    metadata is missing. This is a bug. Please report it.",
        )
        .audio_streams
        .iter()
    {
        // -filter:<stream_index> aformat=channel_layouts="7.1|5.1|stereo"
        if audio_stream.channel_layout == AudioChannelLayout::Surround51Side {
            args.push(format!("-filter:{}", audio_stream.index));
            args.push("aformat=channel_layouts='7.1|5.1|stereo'".to_string());
        }
    }
    args
}

/// Creates additional inputs for subtitle files.
///
/// E.g. `-i Movie.eng.srt -i Movie.ger.srt`
pub fn args_for_subtitle_file_inputs(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    for subtitle_file in &job.subtitle_files {
        // Seek argument `-ss` needs to be added before each input, even for subtitles.
        args.extend(args_for_input_seek(job));

        args.push("-i".to_string());
        args.push(subtitle_file.path.to_string_lossy().into_owned());

        // Duration argument `-t` needs to be added after each input, even for subtitles.
        args.extend(args_for_input_encoding_duration_limit(job));
    }
    args
}

/// Creates string arguments to seek in input file for partial encoding.
///
/// E.g. `-ss 01:10:00` to start encoding at 70 minutes.
pub fn args_for_input_seek(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if job.run.only_encode_segment {
        if let Some(only_encode_from) = job.run.only_encode_from() {
            args.push("-ss".to_string());
            args.push(only_encode_from.clone());
        }
    }
    args
}

/// Creates string arguments to limit duration of encoding.
///
/// E.g. `-t 100` to stop encoding after 100 encoded seconds.
pub fn args_for_input_encoding_duration_limit(job: &Job) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    if job.run.only_encode_segment {
        if let Some(only_encode_for) = job.run.only_encode_for.as_ref() {
            args.push("-t".to_string());
            args.push(only_encode_for.as_secs().to_string());
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    use isolang::Language;
    use std::path::PathBuf;

    use crate::job::tests::{
        empty_job, input_file_metadata_with_one_audio_stream_and_two_subtitle_streams,
        input_file_metadata_with_three_audio_streams,
    };
    use crate::settings::EncodingSettings;
    use crate::subtitles::SubtitleFile;

    #[test]
    fn creates_correct_ffmpeg_map_arguments_for_video_stream() {
        let input_metadata = input_file_metadata_with_three_audio_streams();

        let mut job = empty_job(input_metadata);
        job.scan_for_subtitle_files();
        // input_file_metadata_with_three_audio_streams() creates mock file metadata with one video
        // stream.
        let expected_string = "-map v:0";
        let assembled_string = args_for_video_maps().join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_map_arguments_for_audio_streams() {
        let input_file_metadata = input_file_metadata_with_three_audio_streams();

        let job = empty_job(input_file_metadata);

        // input_file_metadata_with_three_audio_streams() creates mock file metadata with three
        // audio streams. Therefore, the audio streams have the indices 1, 2 and 3.
        let expected_string = "-map 0:1 -map 0:2 -map 0:3";
        let assembled_string = args_for_audio_maps(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_map_arguments_for_internal_subtitle_streams() {
        let input_file_metadata =
            input_file_metadata_with_one_audio_stream_and_two_subtitle_streams();

        let job = empty_job(input_file_metadata);

        // input_file_metadata_with_three_audio_streams() creates mock file metadata with one video
        // stream, one audio stream and two subtitle streams. Therefore, the subtitle streams are
        // third and fourth, having indices 2 and 3.
        let expected_string = "-map 0:2 -map 0:3";
        let assembled_string = args_for_internal_subtitles_maps(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_map_arguments_and_language_metadata_for_external_subtitle_streams() {
        let input_file_metadata =
            input_file_metadata_with_one_audio_stream_and_two_subtitle_streams();
        // Paths for subtitle files are not tested. Therefore, empty PathBuf is sufficient.
        let first_subtitle_file = SubtitleFile {
            path: PathBuf::new(),
            language: Some(Language::from_639_1("en").unwrap()),
            kind: SubTitleType::Normal,
        };

        let second_subtitle_file = SubtitleFile {
            path: PathBuf::new(),
            language: Some(Language::from_639_1("de").unwrap()),
            kind: SubTitleType::Normal,
        };

        let mut job = empty_job(input_file_metadata);
        job.subtitle_files = vec![first_subtitle_file, second_subtitle_file];

        // input_file_metadata_with_one_audio_stream_and_two_subtitle_streams() creates mock file
        // metadata with one video stream, one audio stream and two internal subtitle streams.
        // Therefore, the additional external subtitle streams are fifth and sixth, having indices 4
        // and 5. There inputs start at 0 (input file), so the additional external subtitles are
        // inputs 1 and 2 (`-map`).
        let expected_string = "-map 1 -metadata:s:4 language=eng -map 2 -metadata:s:5 language=deu";
        let assembled_string = args_for_external_subtitles_maps(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_audio_bitrate_arguments() {
        let input_file_metadata = input_file_metadata_with_three_audio_streams();

        let job = empty_job(input_file_metadata);

        // First stream is video and has index 0. First audio stream index is 1.
        let expected_string = "-b:1 112K -b:2 224K -b:3 394K";
        let assembled_string = args_for_audio_bitrate(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_audio_downmixing_arguments() {
        let input_file_metadata = input_file_metadata_with_three_audio_streams();

        let mut job = empty_job(input_file_metadata);
        job.encoding = EncodingSettings::default();
        job.encoding.audio.downmix_5_1_to_stereo = true;
        job.encoding.audio.downmix_7_1_to_stereo = true;

        // First stream is video and has index 0. First audio stream index is 1. First surround
        // audio stream has index 2. Therefore, streams 2 and 3 are 5.1 and 7.1 respectively.
        let expected_string = "-ac:2 2 -ac:3 2";
        let assembled_string = args_for_audio_downmix(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_arguments_for_ffmpeg_audio_5_1_side_workaround() {
        let input_file_metadata = input_file_metadata_with_three_audio_streams();
        let mut job = empty_job(input_file_metadata);

        // Set one of the audio streams to have a 5.1 side channel layout
        job.input_file_metadata.as_mut().unwrap().audio_streams[1].channel_layout =
            AudioChannelLayout::Surround51Side;

        let expected_string = "-filter:2 aformat=channel_layouts='7.1|5.1|stereo'";
        let assembled_string = args_for_audio_5_1_side_workaround(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_ffmpeg_input_arguments_for_external_subtitle_streams() {
        let input_file_metadata =
            input_file_metadata_with_one_audio_stream_and_two_subtitle_streams();
        // Paths for subtitle files are not tested. Therefore, empty PathBuf is sufficient.
        let first_subtitle_file = SubtitleFile {
            path: PathBuf::from("/dummy/movie.en.srt"),
            language: Some(Language::from_639_1("en").unwrap()),
            kind: SubTitleType::Normal,
        };

        let second_subtitle_file = SubtitleFile {
            path: PathBuf::from("/dummy/Film - Der Öß.de.srt"),
            language: Some(Language::from_639_1("de").unwrap()),
            kind: SubTitleType::Normal,
        };

        let mut job = empty_job(input_file_metadata);
        job.subtitle_files = vec![first_subtitle_file, second_subtitle_file];

        let expected_string = "-i /dummy/movie.en.srt -i /dummy/Film - Der Öß.de.srt";
        let assembled_string = args_for_subtitle_file_inputs(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }

    #[test]
    fn creates_correct_arguments_for_ffmpeg_mov_text_subtitle_workaround() {
        let input_file_metadata =
            input_file_metadata_with_one_audio_stream_and_two_subtitle_streams();
        let mut job = empty_job(input_file_metadata);

        // Set one of the subtitle streams to have a mov_text format
        job.input_file_metadata.as_mut().unwrap().subtitle_streams[0].format =
            "mov_text".to_string();

        let expected_string = "-c:2 srt";
        let assembled_string = args_for_mov_text_subtitle_workaround(&job).join(" ");
        assert_eq!(expected_string, assembled_string);
    }
}
