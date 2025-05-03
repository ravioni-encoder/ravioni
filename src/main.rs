// Prevent console window in addition to Slint window in Windows release builds when, e.g., starting
// the app via file manager. Ignored on other platforms.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ffmpeg;
mod job;
mod metadata;
mod model;
mod settings;
mod strings;
mod subtitles;

use std::{cell::RefCell, path::PathBuf, rc::Rc, time::Duration};

use anyhow::Result;
use env_logger;
use log::debug;
use settings::{Av1Crf, Av1Preset, OpusBitrate, Settings};

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    env_logger::init();
    let ui = MainWindow::new()?;
    let settings = Settings::new();

    // Instantiate model that will be used throughout the module.
    let ui_weak: slint::Weak<MainWindow> = ui.as_weak();
    let model = Rc::new(RefCell::new(model::Model::new(settings, ui_weak.clone())));

    ////////////////////
    // Tab: Paths
    ////////////////////

    let model_clone = Rc::clone(&model);
    ui.on_select_input_file(move || {
        model_clone.borrow_mut().pick_input_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_scan_for_subtitles_toggled(move |scan| {
        debug!(
            "Received scan for subtitles toggled event. Value now is {}.",
            scan
        );
        model_clone
            .borrow_mut()
            .current_job
            .paths
            .scan_for_subtitles = scan;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_select_output_file(move || {
        model_clone.borrow_mut().pick_output_file();
    });

    let model_clone: Rc<RefCell<model::Model>> = Rc::clone(&model);
    ui.on_output_file_edited(move |path| {
        debug!("Received output file edited event. Value now is {}.", path);
        model_clone
            .borrow_mut()
            .set_output_file(PathBuf::from(path.to_string()));
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_sync_output_filename_toggled(move |sync| {
        debug!(
            "Received sync output filename toggled event. Value now is {}.",
            sync
        );
        model_clone
            .borrow_mut()
            .current_job
            .paths
            .sync_output_filename = sync;
        model_clone.borrow().write_settings_to_file();
    });

    ////////////////////
    // Tab: Settings
    ////////////////////

    //// Video

    let invalid_number_error = "Received invalid (probably negative i32) number from UI.";
    let model_clone = Rc::clone(&model);
    ui.on_crf_changed(move |crf| {
        debug!("Received crf changed event with value {}.", crf);
        model_clone.borrow_mut().current_job.encoding.video.crf =
            Av1Crf::new(crf.try_into().expect(invalid_number_error)).unwrap();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_preset_changed(move |preset| {
        debug!("Received preset changed event with value {}.", preset);
        model_clone.borrow_mut().current_job.encoding.video.preset =
            Av1Preset::new(preset.try_into().expect(invalid_number_error)).unwrap();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_toggled(move |crop| {
        debug!("Received crop video toggled event. Value now is {}.", crop);
        model_clone.borrow_mut().current_job.encoding.video.crop = crop;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_width_edited(move |value| {
        debug!(
            "Received crop video width edited event. Value now is {}.",
            value
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .crop_width = value.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_height_edited(move |value| {
        debug!(
            "Received crop video height edited event. Value now is {}.",
            value
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .crop_height = value.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_x_offset_edited(move |x_offset| {
        debug!(
            "Received crop x offset edited event. Value now is {}.",
            x_offset
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .crop_x_offset = x_offset.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_y_offset_edited(move |y_offset| {
        debug!(
            "Received crop y offset edited event. Value now is {}.",
            y_offset
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .crop_y_offset = y_offset.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_crop_autodetect_clicked(move || {
        debug!("Received crop autodetect clicked event");
        model_clone
            .borrow_mut()
            .autodetect_input_file_crop_dimensions();
    });

    let model_clone = Rc::clone(&model);
    ui.on_scale_toggled(move |scale| {
        debug!(
            "Received scale video toggled event. Value now is {}.",
            scale
        );
        model_clone.borrow_mut().current_job.encoding.video.scale = scale;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_scale_width_edited(move |value| {
        debug!(
            "Received scale video width edited event. Value now is {}.",
            value
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .scale_width = value.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_scale_height_edited(move |value| {
        debug!(
            "Received scale video height edited event. Value now is {}.",
            value
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .video
            .scale_height = value.parse().ok();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone: Rc<RefCell<model::Model>> = Rc::clone(&model);
    ui.on_denoise_toggled(move |denoise| {
        debug!(
            "Received denoise video toggled event. Value now is {}.",
            denoise
        );
        model_clone.borrow_mut().current_job.encoding.video.denoise = denoise;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_grain_changed(move |level| {
        debug!("Received grain changed event. Value now is {}.", level);
        model_clone.borrow_mut().current_job.encoding.video.grain =
            level.try_into().expect(invalid_number_error);
        model_clone.borrow().write_settings_to_file();
    });

    //// Audio

    let model_clone = Rc::clone(&model);
    ui.on_bitrate_2_0_edited(move |bitrate| {
        debug!(
            "Received 2.0 audio bitrate edited event. Value now is {}.",
            bitrate
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .bitrate_2_0 =
            OpusBitrate::new(bitrate.try_into().expect(invalid_number_error)).unwrap();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_bitrate_5_1_edited(move |bitrate| {
        debug!(
            "Received 5.1 audio bitrate edited event. Value now is {}.",
            bitrate
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .bitrate_5_1 =
            OpusBitrate::new(bitrate.try_into().expect(invalid_number_error)).unwrap();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_bitrate_7_1_edited(move |bitrate| {
        debug!(
            "Received 7.1 audio bitrate edited event. Value now is {}.",
            bitrate
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .bitrate_7_1 =
            OpusBitrate::new(bitrate.try_into().expect(invalid_number_error)).unwrap();
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_re_encode_good_audio_codecs_toggled(move |re_encode| {
        debug!(
            "Received re-encode good audio codecs toggled event. Value now is {}.",
            re_encode
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .re_encode_good_audio_codecs = re_encode;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_copy_toggled(move |copy| {
        debug!("Received copy audio toggled event. Value now is {}.", copy);
        model_clone.borrow_mut().current_job.encoding.audio.copy = copy;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_downmix_5_1_to_stereo_toggled(move |downmix| {
        debug!(
            "Received downmix 5.1 audio to stereo toggled event. Value now is {}.",
            downmix
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .downmix_5_1_to_stereo = downmix;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_downmix_7_1_to_stereo_toggled(move |downmix| {
        debug!(
            "Received downmix 7.1 audio to stereo toggled event. Value now is {}.",
            downmix
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .downmix_7_1_to_stereo = downmix;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_volume_adjustment_edited(move |value| {
        debug!(
            "Received volume adjustment edited event. Value now is {}.",
            value
        );
        model_clone
            .borrow_mut()
            .current_job
            .encoding
            .audio
            .volume_adjustment = value.try_into().expect(invalid_number_error);
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_reset_video_audio_settings(move || {
        debug!("Received reset encoding settings event.");
        model_clone.borrow_mut().reset_video_audio_settings();
    });

    ////////////////////
    // Tab: Run
    ////////////////////

    let model_clone = Rc::clone(&model);
    ui.on_only_encode_segment_toggled(move |only_part| {
        debug!(
            "Received only encode part toggled event. Value now is {}.",
            only_part
        );
        model_clone.borrow_mut().current_job.run.only_encode_segment = only_part;
        model_clone.borrow().write_settings_to_file();
    });

    let ui_weak_clone = ui_weak.clone();
    let model_clone = Rc::clone(&model);
    ui.on_only_encode_from_edited(move |time_string| {
        debug!(
            "Received only encode from edited event. Value now is {}.",
            time_string
        );
        let time_string_is_valid = model_clone
            .borrow_mut()
            .current_job
            .run
            .set_only_encode_from(time_string.into())
            .is_ok();

        ui_weak_clone
            .upgrade_in_event_loop(move |ui| {
                ui.set_encode_from_string_is_valid(time_string_is_valid)
            })
            .unwrap();

        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_only_encode_for_edited(move |seconds| {
        debug!(
            "Received only encode for edited event. Value now is {}.",
            seconds
        );
        let seconds: u64 = seconds.parse::<u64>().unwrap_or(0);
        let duration: Duration = Duration::from_secs(seconds);
        model_clone.borrow_mut().current_job.run.only_encode_for = Some(duration);
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_encode_clicked(move || {
        debug!("Encode button clicked.");
        model_clone.borrow_mut().start_encoding();
    });

    let model_clone = Rc::clone(&model);
    ui.on_cancel_clicked(move || {
        debug!("Cancel button clicked.");
        model_clone.borrow_mut().cancel_encoding();
    });

    let model_clone = Rc::clone(&model);
    ui.on_collect_input_file_metadata(move || {
        model_clone.borrow_mut().collect_input_file_metadata();
    });

    let model_clone = Rc::clone(&model);
    ui.on_collect_crop_data(move || {
        model_clone.borrow_mut().collect_crop_data();
    });

    let model_clone = Rc::clone(&model);
    ui.on_collect_progress(move || {
        model_clone.borrow_mut().collect_progress_from_ffmpeg();
    });

    let model_clone = Rc::clone(&model);
    ui.on_collect_finished(move || {
        model_clone.borrow_mut().collect_finished();
    });

    ////////////////////
    // Settings (Gear)
    ////////////////////

    let model_clone = Rc::clone(&model);
    ui.on_reset_all_settings_clicked(move || {
        debug!("Received reset all settings event");
        model_clone.borrow_mut().reset_all_settings();
    });

    let model_clone = Rc::clone(&model);
    ui.on_dark_mode_toggled(move |dark| {
        debug!("Received dark mode toggled event. Value now is {}.", dark);
        model_clone.borrow_mut().settings.application.dark_mode = dark;
        model_clone.borrow().write_settings_to_file();
    });

    let model_clone = Rc::clone(&model);
    ui.on_advanced_settings_toggled(move |show| {
        debug!("Received dark mode toggled event. Value now is {}.", show);
        model_clone
            .borrow_mut()
            .settings
            .application
            .advanced_settings = show;
        model_clone.borrow().write_settings_to_file();
    });

    ////////////////////
    // Global
    ////////////////////
    let model_clone = Rc::clone(&model);
    ui.on_update_job_times(move || {
        debug!("Received update job times event.");
        model_clone.borrow_mut().update_job_times();
    });

    ui.run()
}
