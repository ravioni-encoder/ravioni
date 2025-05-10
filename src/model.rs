use slint::SharedString;

use crate::job::{Job, JobStatus};
use crate::metadata::{duration_to_shared_string, InputFileMetadata};
use crate::settings::{AudioSettings, Settings, VideoSettings};
use crate::MainWindow;
use crate::{ffmpeg, job};
use log::{debug, error};
use rfd::FileDialog;
use slint::Weak;

use anyhow::Result;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{
    mpsc,
    mpsc::{Receiver, Sender},
};
use std::thread;

pub struct Model {
    ui_weak: Weak<MainWindow>,
    pub settings: Settings,
    pub current_job: Job,
    // Channel used by the ffmpeg thread to send progress information back to the main thread.
    progress_rx: Option<Receiver<(i32, f32)>>,
    // Channel used by the ffmpeg thread to send information back to the main thread with
    // information on why the ffmpeg thread stopped. Fail or Success.
    stopped_rx: Option<Receiver<(job::JobStatus, String)>>,
    // Channel used by the main thread to stop the ffmpeg thread when user cancels encoding.
    ffmpeg_quit_tx: Option<Sender<()>>,
    // Channel used by the ffmpeg thread to send input file metadata back to the main thread.
    input_file_metadata_rx: Option<Receiver<Result<InputFileMetadata>>>,
    // Channel used by the ffmpeg thread to send cropdetect data back to the main thread.
    cropdetect_rx: Option<Receiver<Result<ffmpeg::CropRegion>>>,
}

impl Model {
    pub fn new(settings: Settings, ui_weak: Weak<MainWindow>) -> Model {
        let mut model = Model {
            ui_weak,
            settings: settings.clone(),
            current_job: Job::new(settings.paths, settings.encoding, settings.run),
            progress_rx: None,
            stopped_rx: None,
            ffmpeg_quit_tx: None,
            input_file_metadata_rx: None,
            cropdetect_rx: None,
        };

        // Apply configuration from settings file to self.
        model.set_input_file(model.current_job.paths.input_file.clone());
        model.set_output_file(model.current_job.paths.output_file.clone());
        model.write_path_settings_to_ui();
        model.write_encoding_settings_to_ui();
        model.write_run_settings_to_ui();
        model.write_job_status_to_ui();
        model.write_application_settings_to_ui();

        model
    }

    /// Opens the file picker dialog for the input file.
    ///
    /// Delegates the model change logic to set_input_file().
    pub fn pick_input_file(&mut self) {
        // Try to determine the most likely input directory. Try to use the previously selected
        // directory first, then the user video directory, then the home directory and lastly an
        // empty path.
        let input_directory = self
            .settings
            .application
            .last_input_directory
            .clone()
            .or_else(dirs::video_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_default();

        if let Some(input_file_path) = FileDialog::new()
            .set_directory(&input_directory)
            .pick_file()
        {
            debug!("File picker for input file picked {:?}", input_file_path);
            // Store chosen input directory for next time.
            self.settings.application.last_input_directory =
                input_file_path.parent().map(PathBuf::from);

            self.set_input_file(input_file_path.clone());
        }
    }

    /// Sets the given path as the input file. Updates the model's state and the UI and performs
    /// error checks.
    pub fn set_input_file(&mut self, file_path: PathBuf) {
        // Set in model
        self.current_job.paths.input_file = file_path.clone();
        debug!("Setting input file in model to {:?}", file_path);

        if let Some(ui) = self.ui_weak.clone().upgrade() {
            // Must wait for metadata before encoding can start. Will be set to true there.
            ui.set_canStartEncoding(false);
            // Remove old errors
            ui.set_inputFileError(SharedString::new());

            let Some(file_path_str) = file_path.to_str() else {
                ui.set_inputFileError(
                    "Could not convert path into valid string. Unicode problem?".into(),
                );
                return;
            };

            ui.set_inputFilePath(file_path_str.into());

            // Assert that input file path is valid.
            if !file_path_str.is_empty() {
                if let Err(err) = self
                    .current_job
                    .paths
                    .assert_that_input_file_is_ready_for_encoding()
                {
                    ui.set_inputFileError(err.to_string().into());
                } else {
                    // Spawn metadata thread.
                    self.retrieve_input_file_metadata();

                    // Write to file last, so that potential errors that crash the UI are
                    // not persisted.
                    self.write_settings_to_file();

                    // Update output filename if an output file path was already set and filename
                    // sync is on.
                    if !self.current_job.paths.output_file.as_os_str().is_empty()
                        && self.current_job.paths.sync_output_filename
                    {
                        self.set_output_file(
                            self.current_job
                                .paths
                                .output_file_path_with_inputs_filename()
                                .unwrap_or_default(),
                        );
                    }
                }
            }
        }
    }

    /// Opens the file picker dialog for the output file.
    ///
    /// Delegates the model change logic to set_output_file().
    pub fn pick_output_file(&mut self) {
        // Try to determine the most likely output directory. Try to use the previously selected directory
        // first, then the input file directory, then the user video directory, then the home directory and
        // lastly an empty path.
        let output_directory = self
            .settings
            .application
            .last_output_directory
            .clone()
            .or_else(|| self.settings.application.last_input_directory.clone())
            .or_else(dirs::video_dir)
            .or_else(dirs::home_dir)
            .unwrap_or_default();

        // Prepare input filename as suggestion for the output filename.
        let input_file = self
            .current_job
            .paths
            .input_file
            .file_name()
            .unwrap_or(OsStr::new(""))
            .to_string_lossy();

        // set_directory() does not work with rfd 0.15.1 and KDE Plasma 5/6.
        if let Some(output_file_path) = FileDialog::new()
            .set_directory(&output_directory)
            .set_file_name(input_file)
            .save_file()
        {
            debug!("File picker for output file picked {:?}", output_file_path);
            // Store chosen output directory for next time.
            self.settings.application.last_output_directory =
                output_file_path.parent().map(PathBuf::from);
            self.set_output_file(output_file_path.clone());
        }
    }

    /// Sets the given path as the output file. Updates the model's state and the UI and performs
    /// error checks.
    pub fn set_output_file(&mut self, directory_path: PathBuf) {
        // Set in model
        self.current_job.paths.output_file = directory_path.clone();
        debug!("Set output directory in model to {:?}", directory_path);

        if let Some(ui) = self.ui_weak.clone().upgrade() {
            // Wait for checks below to allow encoding.
            ui.set_canStartEncoding(false);
            // Remove old errors
            ui.set_outputFileError(SharedString::new());

            // Set output file in UI
            let Some(directory_path_str) = directory_path.to_str() else {
                ui.set_outputFileError(
                    "Could not convert path into valid string. Unicode problem?".into(),
                );
                return;
            };
            ui.set_outputFilePath(directory_path_str.into());
            debug!("Set output directory in UI to {:?}", directory_path_str);

            // Assert that output file path is valid
            if !directory_path_str.is_empty() {
                if let Err(err) = self
                    .current_job
                    .paths
                    .assert_that_output_file_is_ready_for_encoding()
                {
                    ui.set_outputFileError(err.to_string().into());
                    return;
                }
            }
            // There is no metadata extraction for the directory, so we can
            // determine in this  if its ready.
            ui.set_canStartEncoding(self.is_encode_startable());
        }

        // Write to file last, so that potential errors that crash the UI are
        // not persisted.
        self.write_settings_to_file();
    }

    /// Writes the model's path settings to the UI.
    ///
    /// The input and output file are handled by different methods.
    fn write_path_settings_to_ui(&self) {
        let paths = self.current_job.paths.clone();

        // Hint: Input and output file paths are also set in Model constructor.
        if let Some(ui) = self.ui_weak.clone().upgrade() {
            ui.set_scanForSubtitles(paths.scan_for_subtitles);
            ui.set_syncOutputFilename(paths.sync_output_filename);
            ui.set_outputFilePath(paths.output_file.to_string_lossy().as_ref().into());
        }
    }

    /// Writes the model's encoding settings to the UI.
    fn write_encoding_settings_to_ui(&self) {
        let settings = self.current_job.encoding.clone();
        debug!(
            "Will write current jobs encoding settings to UI: {:?}",
            settings
        );

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                // Video
                ui.set_crf(settings.video.crf.value() as f32);
                ui.set_preset(settings.video.preset.value() as f32);

                ui.set_cropVideo(settings.video.crop);
                ui.set_cropVideoWidth(
                    settings
                        .video
                        .crop_width
                        .map_or_else(SharedString::new, |w| w.to_string().into()),
                );
                ui.set_cropVideoHeight(
                    settings
                        .video
                        .crop_height
                        .map_or_else(SharedString::new, |h| h.to_string().into()),
                );
                ui.set_cropVideoXOffset(
                    settings
                        .video
                        .crop_x_offset
                        .map_or_else(SharedString::new, |x| x.to_string().into()),
                );
                ui.set_cropVideoYOffset(
                    settings
                        .video
                        .crop_y_offset
                        .map_or_else(SharedString::new, |y| y.to_string().into()),
                );

                ui.set_scaleVideo(settings.video.scale);
                ui.set_scaleVideoWidth(
                    settings
                        .video
                        .scale_width
                        .map_or_else(SharedString::new, |w| w.to_string().into()),
                );
                ui.set_scaleVideoHeight(
                    settings
                        .video
                        .scale_height
                        .map_or_else(SharedString::new, |h| h.to_string().into()),
                );

                ui.set_do_not_denoise_video(!settings.video.denoise);
                ui.set_grain(settings.video.grain as f32);

                // Audio
                ui.set_bitrate_2_0(settings.audio.bitrate_2_0.value() as i32);
                ui.set_bitrate_5_1(settings.audio.bitrate_5_1.value() as i32);
                ui.set_bitrate_7_1(settings.audio.bitrate_7_1.value() as i32);
                ui.set_downmix_5_1_to_stereo(settings.audio.downmix_5_1_to_stereo);
                ui.set_downmix_7_1_to_stereo(settings.audio.downmix_7_1_to_stereo);
                ui.set_volume_adjustment(settings.audio.volume_adjustment as i32);
                ui.set_re_encode_good_audio_codecs(settings.audio.re_encode_good_audio_codecs);
                ui.set_copyAudio(settings.audio.copy);
            })
            .unwrap();
    }

    /// Writes the model's run settings to the UI.
    fn write_run_settings_to_ui(&self) {
        let run = self.current_job.run.clone();

        if let Some(ui) = self.ui_weak.clone().upgrade() {
            ui.set_only_encode_segment(run.only_encode_segment);
            ui.set_only_encode_from(
                run.only_encode_from()
                    .map_or_else(SharedString::new, |from| from.to_string().into()),
            );
            ui.set_only_encode_for(
                run.only_encode_for
                    .map_or_else(SharedString::new, |duration| {
                        duration.as_secs().to_string().into()
                    }),
            );
        }
    }

    /// Writes the model's application settings to the UI.
    pub fn write_application_settings_to_ui(&self) {
        if let Some(ui) = self.ui_weak.clone().upgrade() {
            ui.invoke_setDarkMode(self.settings.application.dark_mode);
            ui.set_showAdvancedSettings(self.settings.application.advanced_settings);
            ui.set_version(env!("CARGO_PKG_VERSION").into());
        }
    }

    /// Writes the job status to the UI.
    ///
    /// Job status is the progress and FPS which are updated during the encoding every second.
    pub fn write_job_status_to_ui(&self) {
        let status_string = self.current_job.status.clone().to_string();
        if let Some(ui) = self.ui_weak.clone().upgrade() {
            ui.set_status(status_string.into());
        }
    }

    /// Spawns a worker thread to retrieve input file metadata.
    ///
    /// The spawned thread uses ffmpeg to read metadata such as streams and duration of input video
    /// file. The channel created here is persisted in the model for
    /// [`Model::collect_input_file_metadata`] to use later.
    pub fn retrieve_input_file_metadata(&mut self) {
        // Create a channel and a thread to retrieve the file's metadata.
        let (tx, rx) = mpsc::channel();
        self.input_file_metadata_rx = Some(rx);
        let file_path_string = self.current_job.paths.input_file.display().to_string();
        let ui_weak_clone = self.ui_weak.clone();

        thread::spawn(move || {
            let maybe_metadata = ffmpeg::extract_metadata(&file_path_string);
            tx.send(maybe_metadata).unwrap();
            ui_weak_clone
                .upgrade_in_event_loop(move |ui| {
                    ui.invoke_collect_input_file_metadata();
                })
                .unwrap();
        });
    }

    /// Spawn a worker thread to retrieve crop dimensions.

    /// The spawned thread uses ffmpeg to autodetect crop dimensions from the video stream. The
    /// channel created here is persisted in the model for [`Model::collect_crop_data()] to use
    /// later.
    pub fn autodetect_input_file_crop_dimensions(&mut self) {
        // Create a channel and a thread to retrieve the autodetected crop data.
        let (tx, rx) = mpsc::channel();
        self.cropdetect_rx = Some(rx);
        let file_path_string = self.current_job.paths.input_file.display().to_string();

        let ui_weak_clone = self.ui_weak.clone();
        thread::spawn(move || {
            let maybe_crop_data = ffmpeg::autodetect_crop_parameters(&file_path_string);
            tx.send(maybe_crop_data).unwrap();
            ui_weak_clone
                .upgrade_in_event_loop(move |ui| {
                    ui.invoke_collect_crop_data();
                })
                .unwrap();
        });
    }

    /// Returns whether all preconditions are met for the encode to start. This method is used by
    /// the UI to enable the "Encode" button.
    pub fn is_encode_startable(&self) -> bool {
        self.current_job
            .paths
            .assert_that_is_ready_for_encoding()
            .is_ok()
            && self.current_job.input_file_metadata.is_some()
    }

    /// Puts the model into the state of encoding.
    pub fn start_encoding(&mut self) {
        debug!(
            "Model starting encoding with settings: {:?}",
            self.current_job.encoding
        );

        if let Err(e) = self.current_job.paths.assert_that_is_ready_for_encoding() {
            error!("Cannot start encoding: {e}");
            self.fail_encoding(e.to_string());
        }
        self.current_job.start();
        let status_string = self.current_job.status.clone().to_string();

        self.ui_weak
            .upgrade_in_event_loop(move |ui: MainWindow| {
                ui.set_status(status_string.into());
                ui.set_encoding(true);
                ui.set_ffmpegError("".into());
            })
            .unwrap();

        // Setup channels to communicate with ffmpeg process and create it.
        // Model <--- progress + fps ---< ffmpeg
        // Model <--- stopped ----------< ffmpeg
        // Model >--- quit -------------> ffmpeg
        let (progress_tx, progress_rx) = mpsc::channel::<(i32, f32)>();
        let (stopped_tx, stopped_rx) = mpsc::channel::<(job::JobStatus, String)>();
        let (quit_tx, quit_rx) = mpsc::channel::<()>();
        self.progress_rx = Some(progress_rx);
        self.stopped_rx = Some(stopped_rx);
        self.ffmpeg_quit_tx = Some(quit_tx);
        ffmpeg::encode(
            self.ui_weak.clone(),
            &self.current_job,
            progress_tx,
            stopped_tx,
            quit_rx,
        );
    }

    pub fn cancel_encoding(&mut self) {
        self.ffmpeg_quit_tx
            .take() // can take() because for the next encoding a new channel will be created
            .expect("No channel for ffmpeg quit found but cancel event received. This is a bug.")
            .send(())
            .unwrap();

        self.current_job.cancel();
        self.update_progress_of_current_job(0, 0.0);
        let status_string = self.current_job.status.to_string();

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.set_status(SharedString::from(status_string));
                ui.set_encoding(false);
            })
            .unwrap();
    }

    pub fn fail_encoding(&mut self, error_message: String) {
        self.current_job.fail(error_message.clone());
        let status_string = self.current_job.status.to_string();

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.set_status(SharedString::from(status_string));
                ui.set_encoding(false);
                ui.set_ffmpegError(error_message.into());
            })
            .unwrap();
    }

    pub fn finish_encoding(&mut self) {
        self.current_job.finish();
        // Reuse current frame to keep progress at 100% but reset fps.
        self.update_progress_of_current_job(self.current_job.current_frame, 0.0);
        self.update_job_times();
        let status_string = self.current_job.status.to_string();

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.set_status(SharedString::from(status_string));
                ui.set_encoding(false);
            })
            .unwrap();
    }

    /// Resets the audio and video settings on the "Settings" tab.
    pub fn reset_video_audio_settings(&mut self) {
        self.current_job.encoding.video = VideoSettings::default();
        self.current_job.encoding.audio = AudioSettings::default();

        self.write_encoding_settings_to_ui();
        self.write_settings_to_file();
    }

    /// Resets all settings, including paths and application settings.
    pub fn reset_all_settings(&mut self) {
        let settings = Settings::default();
        self.current_job = Job::new(
            settings.paths.clone(),
            settings.encoding.clone(),
            settings.run.clone(),
        );
        self.settings = settings;

        self.set_input_file(PathBuf::new());
        self.set_output_file(PathBuf::new());
        self.write_path_settings_to_ui();
        self.write_encoding_settings_to_ui();
        self.write_run_settings_to_ui();
        self.write_application_settings_to_ui();
        self.update_job_times();
        self.write_job_status_to_ui();
        self.write_settings_to_file();
    }

    /// Writes all current settings into a the settings file.
    ///
    /// Serializes the entire settings object. Therefore, no need to add new parameters manually.
    pub fn write_settings_to_file(&self) {
        // Combine the global non-encoding settings with the current job's
        // encoding settings.
        let mut settings = self.settings.clone();
        settings.paths = self.current_job.paths.clone();
        settings.encoding = self.current_job.encoding.clone();
        settings.run = self.current_job.run.clone();
        settings.write_to_file();
    }

    // update_job_times() and update_progress_of_current_job() are called independently.
    pub fn update_progress_of_current_job(&mut self, current_frame: i32, current_fps: f32) {
        debug!(
            "update_progress(): Called with current_frame {current_frame} and fps {current_fps}."
        );
        self.current_job.current_fps = current_fps;
        self.current_job.current_frame = current_frame;
        let progress = self.current_job.progress();

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.set_encodingFps(current_fps.clone());
                ui.set_progress(progress);
            })
            .unwrap();
    }

    /// Updates times in the UI. Is called once a second - more often than the progress indicator.
    pub fn update_job_times(&mut self) {
        let elapsed_time = duration_to_shared_string(&self.current_job.elapsed_time());
        let total_time = duration_to_shared_string(&self.current_job.total_time());

        self.ui_weak
            .upgrade_in_event_loop(move |ui| {
                ui.set_elapsedTime(elapsed_time);
                ui.set_totalTime(total_time);
            })
            .unwrap();
    }

    /// Retrieves input file metadata that was previously sent by a worker thread.
    ///
    /// This function is called on demand, after the thread has notified the main event loop that it
    /// is has finished. Therefore, this method can be sure that there is metadata to fetch.
    pub fn collect_input_file_metadata(&mut self) {
        if let Some(rx) = self.input_file_metadata_rx.take() {
            if let Ok(Ok(metadata)) = rx.recv() {
                debug!(
                    "collect_input_file_metadata(): Received valid metadata over channel: {:?}",
                    metadata.clone()
                );
                self.current_job.input_file_metadata = Some(metadata.clone());

                if let Some(ui) = self.ui_weak.clone().upgrade() {
                    debug!("collect_input_file_metadata(): Updating UI");
                    ui.set_inputFps(metadata.fps);
                    ui.set_inputDurationAsString(duration_to_shared_string(&metadata.duration));
                    ui.set_canStartEncoding(self.is_encode_startable());
                }
            } else {
                error!(
                    "collect_input_file_metadata() was called but no valid metadata was received
                    over the channel. This is likely a bug."
                )
            }
        }
    }

    /// Retrieves crop data that was previously sent by a worker thread.
    ///
    /// This function is called on demand, after the thread has notified the main event loop that it
    /// is has finished. Therefore, this method can be sure that there is crop data to fetch.
    pub fn collect_crop_data(&mut self) {
        if let Some(rx) = self.cropdetect_rx.take() {
            if let Ok(Ok((width, height, x_offset, y_offset))) = rx.recv() {
                debug!(
                    "collect_crop_data(): Received valid crop data over channel: width {} height {} \
                     x_offset {} y_offset {}",
                    width, height, x_offset, y_offset
                );
                self.current_job.encoding.video.crop_width = Some(width);
                self.current_job.encoding.video.crop_height = Some(height);
                self.current_job.encoding.video.crop_x_offset = Some(x_offset);
                self.current_job.encoding.video.crop_y_offset = Some(y_offset);

                if let Some(ui) = self.ui_weak.clone().upgrade() {
                    ui.set_cropVideoWidth(width.to_string().into());
                    ui.set_cropVideoHeight(height.to_string().into());
                    ui.set_cropVideoXOffset(x_offset.to_string().into());
                    ui.set_cropVideoYOffset(y_offset.to_string().into());
                    ui.set_spinAutodetectThrobber(false);
                    self.write_settings_to_file();
                }
            } else {
                error!(
                    "collect_crop_data() was called but no valid crop data was received
                    over the channel. This is likely a bug."
                )
            }
        }
    }

    pub fn collect_progress_from_ffmpeg(&mut self) {
        if let Some(ref rx) = self.progress_rx {
            if let Ok((current_frame, fps)) = rx.try_recv() {
                debug!(
                    "collect_progress(): Collected current frame {current_frame} and fps {fps} over channel."
                );
                self.update_progress_of_current_job(current_frame, fps);
            }
        }
    }

    pub fn collect_finished(&mut self) {
        match self.stopped_rx.take().unwrap().try_recv().unwrap() {
            (JobStatus::Finished, _) => {
                debug!("collect_finished(): Received finished message.");
                self.finish_encoding();
            }
            (JobStatus::Failed, message) => {
                debug!("collect_finished(): Received failed message.");
                self.fail_encoding(message);
            }
            _ => {
                panic!(
                    "Received an unexpected job status from ffmpeg thread. This is a bug. \
                    Please report it."
                );
            }
        }
    }
}
