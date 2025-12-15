use pest::Parser;
use pest_derive::Parser;

#[derive(Parser)]
#[grammar_inline = r#"
WHITESPACE = _{ " " | "\t" }

movie = { SOI ~ title ~ year ~ edition? ~ rest* ~ EOI }

// Greedy title up to the first year-like token (bare 4 digits or (4 digits))
title = @{ (!year_start ~ ANY)+ }

year = @{ year_paren | year_bare }
year_paren = { "(" ~ ASCII_DIGIT{4} ~ ")" }
year_bare  = { ASCII_DIGIT{4} }

// Used only as a lookahead sentinel in `title`
year_start = _{ year }

// Optional "edition" like "Director's Cut" or "Remastered"
// allowing preceding separators such as spaces/dots/dashes/underscores
edition = @{
    space_or_dot*
    ~ (
        "Unrated Director's Cut"
      | "Director's Cut"
      | "REMASTERED"
      | "Remastered"
      | "Special Edition"
      | "International Cut"
      | "Unrated"
      | "Uncut"
    )
}

space_or_dot = _{ " " | "." | "-" | "_" }

// Everything after edition is irrelevant for the title
rest = _{ ANY }
"#]
struct MatroskaParser;

pub fn extract_matroska_title_from_filename(filename: &str) -> String {
    // Strip extension (last dot segment)
    let base = filename
        .rsplit_once('.')
        .map(|(b, _)| b)
        .unwrap_or(filename);

    // Episode-style filenames: Some.Show.Name.S07E11.Parallels.mkv
    // Look for a token SxxEyy and format as "Some Show Name - S07E11 - Parallels"
    let segments: Vec<&str> = base.split('.').collect();
    if let Some((idx, se_token)) = segments
        .iter()
        .enumerate()
        .find(|(_, s)| is_season_episode_token(s))
    {
        let show_parts = &segments[..idx];
        let episode_parts = &segments[idx + 1..];

        if !show_parts.is_empty() {
            let show_name = show_parts.join(" ");
            let season_episode = se_token.to_ascii_uppercase();
            let episode_title = episode_parts.join(" ");

            return if episode_title.is_empty() {
                format!("{show_name} - {season_episode}")
            } else {
                format!("{show_name} - {season_episode} - {episode_title}")
            };
        }
    }

    // If there's no 4-digit year at all, fall back to a simple
    // "title only" heuristic without metadata like BluRay, 1080p, etc.
    if !has_four_digit_year(base) {
        return infer_title_without_year(base);
    }

    // Movie-style filenames (default)
    let parse_result = MatroskaParser::parse(Rule::movie, base);
    if parse_result.is_err() {
        return base.to_string();
    }

    let mut pairs = parse_result.unwrap();
    let movie_pair = match pairs.next() {
        Some(p) => p,
        None => return base.to_string(),
    };

    let mut raw_title: Option<&str> = None;
    let mut raw_year: Option<&str> = None;
    let mut raw_edition: Option<&str> = None;

    for p in movie_pair.into_inner() {
        match p.as_rule() {
            Rule::title => raw_title = Some(p.as_str()),
            Rule::year => raw_year = Some(p.as_str()),
            Rule::edition => raw_edition = Some(p.as_str()),
            _ => {}
        }
    }

    let (raw_title, raw_year) = match (raw_title, raw_year) {
        (Some(t), Some(y)) => (t, y),
        _ => return base.to_string(),
    };

    // Clean title: remove leading/trailing separators, replace dots with spaces
    let title = raw_title
        .trim_matches(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .replace('.', " ");

    // Normalize year: strip parentheses if present
    let year_clean = raw_year
        .trim_matches(|c: char| c == '(' || c == ')')
        .to_string();

    // Optional edition
    let edition_clean = raw_edition.map(|e| {
        let e = e.trim_matches(|c: char| c.is_whitespace() || c == '.' || c == '-' || c == '_');
        let lower = e.to_ascii_lowercase();

        if lower.contains("unrated") && lower.contains("director") {
            "Unrated Director's Cut".to_string()
        } else if lower.contains("director") {
            "Director's Cut".to_string()
        } else if lower.contains("special edition") {
            "Special Edition".to_string()
        } else if lower.contains("international cut") {
            "International Cut".to_string()
        } else if lower.contains("remaster") {
            "Remastered".to_string()
        } else if lower.contains("unrated") {
            "Unrated".to_string()
        } else if lower.contains("uncut") {
            "Uncut".to_string()
        } else {
            e.to_string()
        }
    });

    match edition_clean {
        Some(ed) if !ed.is_empty() => format!("{title} ({year_clean}) {ed}"),
        _ => format!("{title} ({year_clean})"),
    }
}

// Helper: detect SxxEyy pattern (case-insensitive)
fn is_season_episode_token(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 6 {
        return false;
    }

    let s_char = bytes[0];
    let e_char = bytes[3];

    if !((s_char == b'S') || (s_char == b's')) {
        return false;
    }
    if !((e_char == b'E') || (e_char == b'e')) {
        return false;
    }

    bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_digit()
        && bytes[4].is_ascii_digit()
        && bytes[5].is_ascii_digit()
}

// Helper: check if the string contains any 4-digit sequence (year-like)
fn has_four_digit_year(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() < 4 {
        return false;
    }

    for i in 0..=bytes.len() - 4 {
        if bytes[i].is_ascii_digit()
            && bytes[i + 1].is_ascii_digit()
            && bytes[i + 2].is_ascii_digit()
            && bytes[i + 3].is_ascii_digit()
        {
            return true;
        }
    }
    false
}

// Helper: infer a title when there is no 4-digit year.
// Drops common metadata tokens like BluRay, 1080p, Extended, Edition, etc.
fn infer_title_without_year(base: &str) -> String {
    let segments: Vec<&str> = base.split('.').collect();

    if segments.len() == 1 {
        return base
            .trim_matches(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
            .replace('.', " ");
    }

    const METADATA_TOKENS: &[&str] = &[
        "+ extras",
        "1080p",
        "2160p",
        "480p",
        "720p",
        "ac3",
        "atmos",
        "av1",
        "bdrip",
        "bluray",
        "dts",
        "dvdrip",
        "edition",
        "extended",
        "h264",
        "h265",
        "hdr",
        "hdrip",
        "hevc",
        "proper",
        "remastered",
        "remux",
        "repack",
        "rm4k",
        "truehd",
        "uhd",
        "web-dl",
        "webdl",
        "webrip",
        "x264",
        "x265",
    ];

    let mut cutoff = segments.len();

    'outer: for (i, seg) in segments.iter().enumerate() {
        let lower = seg.to_ascii_lowercase();

        // Direct metadata tokens
        for token in METADATA_TOKENS {
            if lower == *token {
                cutoff = i;
                break 'outer;
            }
        }

        // Resolution-like tokens, e.g. "1080p"
        if lower.ends_with('p') {
            let digits = lower.chars().take_while(|c| c.is_ascii_digit()).count();
            if digits >= 3 {
                cutoff = i;
                break 'outer;
            }
        }
    }

    if cutoff == 0 {
        cutoff = segments.len();
    }

    let title_tokens = &segments[..cutoff];
    let raw_title = title_tokens.join(" ");

    raw_title
        .trim_matches(|c: char| c.is_whitespace() || c == '.' || c == '_' || c == '-')
        .replace('.', " ")
}

#[cfg(test)]
mod tests {
    use super::extract_matroska_title_from_filename;

    #[test]
    fn simple_dotted_title_with_year_before_metadata() {
        let input = "Sample.Movie.2010.BluRay.1080p.Audio.Format.Codec.REMUX-Winter.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Sample Movie (2010)");
    }

    #[test]
    fn dotted_title_with_year_and_short_metadata() {
        let input = "Another.Film.Title.1986.1080p.BDRip.AV1-Encoder.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Another Film Title (1986)");
    }

    #[test]
    fn dotted_title_with_uhd_and_long_metadata() {
        let input =
            "Example.Feature.1993.UHD.BluRay.2160p.Audio.Track.DV.HEVC.HYBRID.REMUX-Release.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Example Feature (1993)");
    }

    #[test]
    fn dotted_title_with_remastered_flag_before_metadata() {
        let input = "Long.Dotted.Movie.Name.1967.Remastered.1080p.BluRay.H264.AAC-Group[Tag].mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Long Dotted Movie Name (1967) Remastered");
    }

    #[test]
    fn spaced_title_with_year_in_parentheses_before_tech_info() {
        let input = "Generic Epic Title (2024) (2160p BluRay x265 10bit DV HDR Group).mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Generic Epic Title (2024)");
    }

    #[test]
    fn spaced_title_with_extra_words_and_year_late_in_string() {
        let input = "Franchise Part IV - The Return 1986 Eng Multi-Subs 1080p [Enc].mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Franchise Part IV - The Return (1986)");
    }

    #[test]
    fn spaced_title_directors_cut() {
        let input =
            "Placeholder Title (1995) Director's Cut 1080p BluRay AV1 Audio 5.1 [Group].mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Placeholder Title (1995) Director's Cut");
    }

    #[test]
    fn remastered_uppercase_normalized() {
        let input = "Classic.Movie.1977.REMASTERED.1080p.BluRay.DTS.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Classic Movie (1977) Remastered");
    }

    #[test]
    fn unrated_directors_cut_normalized() {
        let input = "Gritty.Movie.2003.Unrated Director's Cut.1080p.BluRay.AC3.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Gritty Movie (2003) Unrated Director's Cut");
    }

    #[test]
    fn edition_with_dots_and_dashes_around_it() {
        let input = "Action.Movie.2012.-.Special Edition.-.1080p.BluRay.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Action Movie (2012) Special Edition");
    }

    #[test]
    fn no_year() {
        let input = "A.Pretty.Movie.BluRay.Extended.Edition.mkv";
        let output = extract_matroska_title_from_filename(input);
        // No 4-digit year -> just the movie title without year
        assert_eq!(output, "A Pretty Movie");
    }

    #[test]
    fn episode_with_show_title_and_episode_title() {
        let input = "Some.Show.Name.S07E11.Summer.in.Paris.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Some Show Name - S07E11 - Summer in Paris");
    }

    #[test]
    fn episode_with_show_title_but_no_episode_title() {
        let input = "Some.Show.Name.S02E03.mkv";
        let output = extract_matroska_title_from_filename(input);
        assert_eq!(output, "Some Show Name - S02E03");
    }
}
