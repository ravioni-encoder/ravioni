use std::fs;
use std::fs::File;
use std::io;
use std::io::BufRead;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Result};
use isolang::Language;
use log::debug;

use whatlang::detect;

use crate::strings::split_by_non_letters;

/// File extensions that are recognized as subtitle files.
static SUBTITLE_EXTENSIONS: [&str; 4] = ["srt", "ass", "ssa", "idx"];

#[derive(Debug, Clone, PartialEq)]
pub enum SubTitleType {
    Normal,
    SDH,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubtitleFile {
    pub path: PathBuf,
    // Option because only for some formats such as SRT we need to recognize the language. For
    // others, such as VobSub, ffmpeg will recognize it automatically and no stream mapping is
    // necessary.
    pub language: Option<Language>,
    pub kind: SubTitleType,
}

/// Gathers subtitle file paths recursively in the given directory.
///
/// Recursion depths values:
/// * 0: no subtitle paths will be gathered
/// * 1: only the given directory will be searched
/// * 2: the given and all immediate subdirectories will be searched
/// * n: function will descend n-1 times
pub fn gather_subtitle_file_paths_in(
    directory: &Path,
    recursion_depth: u32,
) -> Result<Vec<PathBuf>, io::Error> {
    let mut subtitles = Vec::new();

    if recursion_depth <= 0 {
        return Ok(subtitles);
    }

    for entry in fs::read_dir(directory)? {
        let path = entry?.path();

        if path.is_dir() {
            subtitles.append(&mut gather_subtitle_file_paths_in(
                &path,
                recursion_depth - 1,
            )?)
        } else if let Some(extension) = path.extension().and_then(|ext| ext.to_str()) {
            if SUBTITLE_EXTENSIONS.contains(&extension) {
                subtitles.push(path);
            }
        }
    }

    Ok(subtitles)
}

/// Detect whether a subtitle file are SDH (subtitles for the Deaf and hard of hearing) or a
/// normal subtitle file.
pub fn detect_type_of_subtitle_file_type(path: &Path) -> SubTitleType {
    if let Some(file_stem) = path.file_stem().and_then(|s| s.to_str()) {
        let parts: Vec<String> = split_by_non_letters(file_stem)
            .iter()
            .map(|s| s.to_lowercase())
            .collect();

        // Subtitle type is SDH if (sdh OR cc) OR (hearing AND impaired)
        if parts
            .iter()
            .any(|part| matches!(part.as_str(), "sdh" | "cc"))
            || (parts.contains(&"hearing".to_string()) && parts.contains(&"impaired".to_string()))
        {
            return SubTitleType::SDH;
        }
    }
    SubTitleType::Normal
}

/// Detect the language of the given subtitle file by analyzing its dialogue.
pub fn detect_language_of_subtitle_file(path: impl AsRef<Path>) -> Result<Language> {
    let path: &Path = path.as_ref();
    let text = match path.extension().and_then(|extension| extension.to_str()) {
        Some("srt") => extract_dialogue_from_srt_file(path)?,
        Some(other) => return Err(anyhow!("Unsupported subtitle file extension: {}.", other)),
        _ => return Err(anyhow!("No (recognizable) subtitle file extension.")),
    };

    return detect_language_from_string(&text);
}

fn detect_language_from_string(text: &str) -> Result<Language> {
    let info =
        detect(text).ok_or_else(|| anyhow!("Subtitle dialogue yielded no language info."))?;
    debug!(
        "Detected language as {} with confidence {}.",
        info.lang(),
        info.confidence()
    );
    // TODO: implement behavior for when confidence is too low.
    Language::from_639_3(info.lang().code()).ok_or_else(|| {
        anyhow!(
            "Whatlang returned an invalid ISO 639-3 language code. This is likely a bug. \
        Please report it in the Ravioni repo.",
        )
    })
}

// Extract only the dialogue from the given srt file without any metadata.
fn extract_dialogue_from_srt_file(file_path: impl AsRef<Path>) -> Result<String> {
    let file_path = file_path.as_ref();

    if let Ok(lines) = read_lines(file_path) {
        let mut text = String::new();

        for line in lines.flatten() {
            if !line.trim().is_empty()
                && !line.chars().all(char::is_numeric)
                && !line.contains("-->")
            {
                text.push_str(&line);
                text.push(' ');
            }
        }
        Ok(text.trim_end().to_string())
    } else {
        bail!(
            "Could not read SRT subtitle file from {}.",
            file_path.to_string_lossy()
        )
    }
}

fn read_lines(file_path: impl AsRef<Path>) -> io::Result<io::Lines<io::BufReader<File>>> {
    let file = File::open(file_path)?;
    Ok(io::BufReader::new(file).lines())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    use std::io::Write;

    #[test]
    fn no_subtitles_are_found_in_empty_directory() {
        let dir = tempdir().unwrap();
        let results = gather_subtitle_file_paths_in(dir.path(), 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn single_subtitle_is_found_in_directory() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("example.srt");
        File::create(&file_path).unwrap();

        let results = gather_subtitle_file_paths_in(dir.path(), 5).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], file_path);
    }

    #[test]
    fn multiple_subtitles_are_found_in_directory() {
        let dir = tempdir().unwrap();
        let first_srt_file_path = dir.path().join("de.srt");
        let second_srt_file_path = dir.path().join("en.srt");
        let first_idx_file_path = dir.path().join("subs.idx");
        File::create(&first_srt_file_path).unwrap();
        File::create(&second_srt_file_path).unwrap();
        File::create(&first_idx_file_path).unwrap();

        let subtitle_file_paths = gather_subtitle_file_paths_in(dir.path(), 5).unwrap();
        assert_eq!(subtitle_file_paths.len(), 3);
        assert!(subtitle_file_paths.contains(&first_srt_file_path));
        assert!(subtitle_file_paths.contains(&second_srt_file_path));
        assert!(subtitle_file_paths.contains(&first_idx_file_path));
    }

    #[test]
    fn non_subtitle_file_is_ignored() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("example.txt");
        File::create(&file_path).unwrap();

        let results = gather_subtitle_file_paths_in(dir.path(), 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn subtitle_in_subdirectory_is_found() {
        let dir = tempdir().unwrap();
        let subdir = dir.path().join("subtitles");
        fs::create_dir(&subdir).unwrap();
        let file_path = subdir.join("example.ass");
        File::create(&file_path).unwrap();

        let results = gather_subtitle_file_paths_in(dir.path(), 5).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results.contains(&file_path));
    }

    #[test]
    fn recursion_depth_is_respected() {
        let dir = tempdir().unwrap();
        let subdir = dir.path().join("subs");
        fs::create_dir(&subdir).unwrap();
        let file_path = subdir.join("example.ssa");
        File::create(&file_path).unwrap();

        let results = gather_subtitle_file_paths_in(dir.path(), 1).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn str_dialogue_extraction_works_from_valid_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("example.srt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "1\n00:00:01,000 --> 00:00:02,000\nHello World!").unwrap();
        let dialogue = extract_dialogue_from_srt_file(&file_path).unwrap();
        assert_eq!(dialogue, "Hello World!");
    }

    #[test]
    fn str_dialogue_extraction_ignores_metadata() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("example.srt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(file, "1\n00:00:01,000 --> 00:00:02,000\nHello World!\n\n2\n00:00:03,000 --> 00:00:04,000\nGoodbye!").unwrap();
        let dialogue = extract_dialogue_from_srt_file(&file_path).unwrap();
        assert_eq!(dialogue, "Hello World! Goodbye!");
    }

    #[test]
    fn str_dialogue_extraction_ignores_numeric_lines() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("example.srt");
        let mut file = File::create(&file_path).unwrap();
        writeln!(
            file,
            "1\n00:00:01,000 --> 00:00:02,000\n12345\nHello World!"
        )
        .unwrap();
        let dialogue = extract_dialogue_from_srt_file(&file_path).unwrap();
        assert_eq!(dialogue, "Hello World!");
    }
}
