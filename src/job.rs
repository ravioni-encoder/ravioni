//! This module contains structs and functions that represent a job which encompasses a specific
//! input video file all encoding and transformation settings and also the progress data about the
//! completion of the job.
//!
//! A job is mostly self-sustained and will be the unit of a future job queue functionality.

use std::collections::VecDeque;
use std::fmt;
use std::time::{Duration, Instant};

use log::debug;

use crate::metadata::InputFileMetadata;
use crate::settings::{EncodingSettings, MatroskaSettings, PathSettings, RunSettings};
use crate::subtitles::{
    detect_language_of_subtitle_file, detect_type_of_subtitle_file_type,
    gather_subtitle_file_paths_in, SubtitleFile,
};

/// The number of the ffmpeg progress events to remember and calculate the average prediction over.
const PROGRESS_HISTORY_LENGTH: usize = 20;
/// Skip this many ffmpeg progress events before storing any in the history. Ffmpeg starts "slowly"
/// and the first few  events would lead to overestimation of the remaining time.
const SKIP_N_FIRST_PROGRESS_EVENTS: u32 = 4;
/// Gather this many history data points (after skipping) before calculating the average and finally
/// showing a total time estimation to the user.
const MIN_PROGRESS_HISTORY_TO_USE: usize = 3;

/// How deep to descend into directories when searching for subtitle files in
/// [`Job::scan_for_subtitle_files`].
static SUBTITLE_SEARCH_DEPTH: u32 = 10;

// The UI only allows one file input. Therefore, it will always be the first and have index 0.
pub static INPUT_INDEX: u32 = 0;

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Editing,
    Encoding,
    Cancelled,
    Failed,
    Finished,
}

impl fmt::Display for JobStatus {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub status: JobStatus,
    pub started_at: Option<Instant>,
    pub stopped_at: Option<Instant>,
    pub paths: PathSettings,
    pub matroska: MatroskaSettings,
    pub encoding: EncodingSettings,
    pub run: RunSettings,
    pub input_file_metadata: Option<InputFileMetadata>,
    pub subtitle_files: Vec<SubtitleFile>,
    pub ffmpeg_error: Option<String>,
    pub current_frame: i32,
    pub current_fps: f32,
    progress_history: VecDeque<(f32, Instant)>,
    progress_history_skips: u32,
}

impl Job {
    pub fn new(
        paths: PathSettings,
        encoding: EncodingSettings,
        matroska: MatroskaSettings,
        run: RunSettings,
    ) -> Self {
        Self {
            status: JobStatus::Editing,
            started_at: None,
            stopped_at: None,
            paths,
            matroska,
            encoding,
            run,
            input_file_metadata: None,
            subtitle_files: Vec::new(),
            ffmpeg_error: None,
            current_frame: 0,
            current_fps: 0.0,
            progress_history: VecDeque::with_capacity(PROGRESS_HISTORY_LENGTH),
            progress_history_skips: SKIP_N_FIRST_PROGRESS_EVENTS,
        }
    }

    pub fn start(&mut self) {
        self.status = JobStatus::Encoding;
        self.started_at = Some(Instant::now());
        if self.paths.scan_for_subtitles {
            self.scan_for_subtitle_files();
        }
    }

    pub fn cancel(&mut self) {
        self.status = JobStatus::Cancelled;
    }

    pub fn finish(&mut self) {
        self.status = JobStatus::Finished;
        self.stopped_at = Some(Instant::now());
    }

    pub fn fail(&mut self, error_message: String) {
        self.status = JobStatus::Failed;
        self.ffmpeg_error = Some(error_message);
        self.stopped_at = Some(Instant::now());
    }

    pub fn seconds_to_encode(&self) -> u64 {
        let Some(metadata) = self.input_file_metadata.as_ref() else {
            return 0u64;
        };
        // Subtract one second from duration, because the actual duration
        // can be slightly less and then with short input files the progress
        // might never reach 100%.
        let duration_of_entire_file = metadata.duration.as_secs().saturating_sub(1);
        if self.run.only_encode_segment {
            // Take min, in case user-given duration is longer than entire file duration.
            u64::min(
                self.run.only_encode_for.unwrap().as_secs(),
                duration_of_entire_file,
            )
        } else {
            duration_of_entire_file
        }
    }

    /// Returns the completion fraction of the current job, where 1 is 100%.
    ///
    /// Uses the currently encoded frame, as reported by ffmpeg, as a measure of completion. This
    /// value can be slightly off, because the total number of frames is not precise and frame
    /// encoding times vary. FFmpeg stalls just before it finishes encoding.
    pub fn progress(&self) -> f32 {
        if let Some(metadata) = self.input_file_metadata.as_ref() {
            let progress =
                self.current_frame as f32 / (metadata.fps * (self.seconds_to_encode() as f32));
            debug!(
                "progress(): Calculated non-zero progress {:?} using current frame {:?}, fps
             {:?} and duration {:?}.",
                progress,
                self.current_frame,
                metadata.fps,
                metadata.duration.as_secs()
            );
            // Limit returned progress to 1.0. Above calculation is not exact
            // and can return values that are slightly above 1.0.
            progress.min(1.0)
        } else {
            0.0
        }
    }

    fn update_progress_history(&mut self) {
        // Skip the first progress data points to not pollute the history with
        // the first slow progress data points.
        if self.progress_history_skips > 0 {
            self.progress_history_skips -= 1;
            debug!(
                "Skipping progress history. {} skips left.",
                self.progress_history_skips
            );
            return;
        }
        if self.progress_history.len() >= PROGRESS_HISTORY_LENGTH {
            self.progress_history.pop_front();
        }
        self.progress_history
            .push_back((self.progress(), Instant::now()));
    }

    /// Returns the duration for which the job has run so far.
    pub fn elapsed_time(&self) -> Duration {
        self.started_at
            .map_or(Duration::ZERO, |start| Instant::now() - start)
    }

    /// Returns the estimated total time the job will run. Uses a recent history
    /// of progress to remove the influence of a slow startup (exhibited by ffmpeg).
    pub fn total_time(&mut self) -> Duration {
        self.update_progress_history();

        if self.progress_history.len() < MIN_PROGRESS_HISTORY_TO_USE {
            return Duration::ZERO;
        }

        let (first_progress, first_time) = self.progress_history.front().unwrap();
        let (last_progress, last_time) = self.progress_history.back().unwrap();

        let progress_delta = last_progress - first_progress;
        let time_delta = last_time.duration_since(*first_time);

        if progress_delta <= 0.0 {
            return self.total_time_without_history();
        }

        let progress_rate = progress_delta / time_delta.as_secs_f32();
        let remaining_progress = 1.0 - last_progress;
        let estimated_remaining_time = remaining_progress / progress_rate;
        Duration::from_secs_f32(estimated_remaining_time) + self.elapsed_time()
    }

    /// Returns the estimated total time the job will run without using the
    /// progress history.
    fn total_time_without_history(&self) -> Duration {
        match self.progress() {
            0.0 => Duration::ZERO,
            1.0 => self.elapsed_time(),
            _ => Duration::from_secs_f32(self.elapsed_time().as_secs_f32() / self.progress()),
        }
    }

    /// Scans for subtitle files and writes them into instance state. Scan is performed when job is
    /// started (see [`Job::start`]), not when the input file (and thus its directory) is selected.
    pub fn scan_for_subtitle_files(&mut self) {
        if let Some(parent) = self.paths.input_file.parent() {
            if let Ok(subtitle_paths) = gather_subtitle_file_paths_in(parent, SUBTITLE_SEARCH_DEPTH)
            {
                self.subtitle_files = subtitle_paths
                    .into_iter()
                    .map(move |p| SubtitleFile {
                        language: detect_language_of_subtitle_file(&p).ok(),
                        kind: detect_type_of_subtitle_file_type(&p),
                        path: p,
                    })
                    .collect();
                debug!(
                    "Scanned and found {} subtitle files.",
                    &self.subtitle_files.len()
                )
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use crate::{
        metadata::{AudioStream, InputFileMetadata, SubtitleStream},
        settings::AudioChannelLayout,
    };

    use super::*;
    use std::time::{Duration, Instant};

    pub fn empty_job(input_file_metadata: InputFileMetadata) -> Job {
        Job {
            status: JobStatus::Editing,
            started_at: None,
            stopped_at: None,
            paths: PathSettings::default(),
            matroska: MatroskaSettings::default(),
            encoding: EncodingSettings::default(),
            run: RunSettings::default(),
            input_file_metadata: Some(input_file_metadata),
            subtitle_files: Vec::new(),
            ffmpeg_error: None,
            current_frame: 0,
            current_fps: 0.0,
            progress_history: VecDeque::new(),
            progress_history_skips: 0,
        }
    }

    /// Creates an input file metadata instance with three audio streams that each have a different
    /// channel layout.
    pub fn input_file_metadata_with_three_audio_streams() -> InputFileMetadata {
        // Video index is always 0. Therefore audio indices start at 1.
        let stereo_stream = AudioStream {
            index: 1,
            channel_layout: AudioChannelLayout::Stereo20,
            codec: "aac".to_string(),
        };

        let surround51_stream = AudioStream {
            index: 2,
            channel_layout: AudioChannelLayout::Surround51,
            codec: "ac3".to_string(),
        };

        let surround71_stream = AudioStream {
            index: 3,
            channel_layout: AudioChannelLayout::Surround71,
            codec: "dts".to_string(),
        };

        InputFileMetadata {
            fps: 24.0,
            duration: Duration::new(3600, 0),
            width: 1920,
            height: 1080,
            audio_streams: vec![stereo_stream, surround51_stream, surround71_stream],
            subtitle_streams: vec![],
            highest_stream_index: 2,
        }
    }

    /// Creates an input file metadata instance with one stereo audio stream and two subtitle
    /// streams.
    pub fn input_file_metadata_with_one_audio_stream_and_two_subtitle_streams() -> InputFileMetadata
    {
        // Video index is always 0. Therefore audio indices start at 1.
        let stereo_stream = AudioStream {
            index: 1,
            channel_layout: AudioChannelLayout::Stereo20,
            codec: "aac".to_string(),
        };

        let first_subtitle_stream = SubtitleStream {
            index: 2,
            format: "srt".to_string(),
        };

        let second_subtitle_stream = SubtitleStream {
            index: 3,
            format: "srt".to_string(),
        };

        InputFileMetadata {
            fps: 24.0,
            duration: Duration::new(3600, 0),
            width: 1920,
            height: 1080,
            audio_streams: vec![stereo_stream],
            subtitle_streams: vec![first_subtitle_stream, second_subtitle_stream],
            highest_stream_index: 3,
        }
    }

    #[test]
    fn new_jobs_fields_are_identical_to_the_ones_passed_to_the_constructor() {
        let paths = PathSettings::default();
        let encoding = EncodingSettings::default();
        let matroska = MatroskaSettings::default();
        let run = RunSettings::default();

        let job = Job::new(
            paths.clone(),
            encoding.clone(),
            matroska.clone(),
            run.clone(),
        );

        assert_eq!(JobStatus::Editing, job.status);
        assert!(job.started_at.is_none());
        assert!(job.stopped_at.is_none());
        assert_eq!(paths, job.paths);
        assert_eq!(encoding, job.encoding);
        assert_eq!(0, job.current_frame);
        assert_eq!(0.0, job.current_fps);
    }

    #[test]
    fn new_job_has_status_editing() {
        let job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        assert_eq!(job.status, JobStatus::Editing);
    }

    #[test]
    fn started_job_has_status_encoding_and_a_started_at_time() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.start();

        assert_eq!(job.status, JobStatus::Encoding);
        assert!(job.started_at.is_some());
    }

    #[test]
    fn cancelled_job_has_status_cancelled() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.cancel();

        assert_eq!(JobStatus::Cancelled, job.status);
    }

    #[test]
    fn finished_job_has_status_finished_and_a_stopped_at_time() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.finish();

        assert_eq!(job.status, JobStatus::Finished);
        assert!(job.stopped_at.is_some());
    }

    #[test]
    fn progress_of_half_finished_job_is_ca_0_5() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.input_file_metadata = Some(InputFileMetadata {
            fps: 30.0,
            duration: Duration::from_secs(100),
            width: 1920,
            height: 1080,
            audio_streams: Vec::new(),
            subtitle_streams: Vec::new(),
            highest_stream_index: 14,
        });
        job.current_frame = 1500;

        let progress = job.progress();
        // Due to some workarounds, the progress value is slightly off, so we test imprecisely.
        assert!(progress >= 0.49 && progress <= 0.51);
    }

    #[test]
    fn progress_history_skips_initial_events() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.update_progress_history();
        assert_eq!(0, job.progress_history.len());
        assert_eq!(SKIP_N_FIRST_PROGRESS_EVENTS - 1, job.progress_history_skips);
    }

    #[test]
    fn progress_history_maintains_correct_length() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.progress_history_skips = 0;
        for _ in 0..(PROGRESS_HISTORY_LENGTH + 5) {
            job.update_progress_history();
        }
        assert_eq!(PROGRESS_HISTORY_LENGTH, job.progress_history.len());
    }

    #[test]
    fn elapsed_time_increases_after_job_start() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.start();
        std::thread::sleep(Duration::from_millis(10));
        assert!(job.elapsed_time() > Duration::ZERO);
    }

    #[test]
    fn if_forty_percent_of_job_was_completed_in_4s_then_full_job_will_take_10s() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.progress_history_skips = 0;
        job.input_file_metadata = Some(InputFileMetadata {
            fps: 30.0,
            duration: Duration::from_secs(101),
            width: 1920,
            height: 1080,
            audio_streams: Vec::new(),
            subtitle_streams: Vec::new(),
            highest_stream_index: 14,
        });
        job.current_frame = 1200;

        let now = Instant::now();
        job.progress_history
            .push_back((0.0, now - Duration::from_secs(4)));
        job.progress_history
            .push_back((0.1, now - Duration::from_secs(3)));
        job.progress_history
            .push_back((0.2, now - Duration::from_secs(2)));
        job.progress_history
            .push_back((0.3, now - Duration::from_secs(1)));

        // The current_frame (1200) is chosen such that progress is 0.4 now. Previous entries went
        // from 0 to 0.3 each second - 4 seconds in total. Thus 0.6 is remaining and should be done
        // in 6 s.
        let total_time = job.total_time();
        assert!(
            total_time >= Duration::from_secs_f32(5.9)
                && total_time <= Duration::from_secs_f32(6.1)
        );
    }

    #[test]
    fn total_time_returns_zero_duration_with_insufficient_history() {
        let mut job = Job::new(
            PathSettings::default(),
            EncodingSettings::default(),
            MatroskaSettings::default(),
            RunSettings::default(),
        );
        job.progress_history_skips = 0;
        job.progress_history.push_back((0.0, Instant::now()));
        let total_time = job.total_time();
        assert_eq!(Duration::ZERO, total_time);
    }

    #[test]
    fn job_status_enum_prints_its_variant_names_correctly() {
        assert_eq!("Editing", format!("{}", JobStatus::Editing));
        assert_eq!("Encoding", format!("{}", JobStatus::Encoding));
        assert_eq!("Cancelled", format!("{}", JobStatus::Cancelled));
        assert_eq!("Failed", format!("{}", JobStatus::Failed));
        assert_eq!("Finished", format!("{}", JobStatus::Finished));
    }
}
