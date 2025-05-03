pub fn split_by_non_letters(input: &str) -> Vec<&str> {
    input
        .split(|c: char| !c.is_alphabetic())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_by_whitespace_and_special_characters() {
        let text = "Hello, World! 123";
        let words = split_by_non_letters(text);
        assert_eq!(words, vec!["Hello", "World"]);
    }

    #[test]
    fn splits_by_special_characters() {
        let text = "Rust-lang:2023-eng";
        let words = split_by_non_letters(text);
        assert_eq!(words, vec!["Rust", "lang", "eng"]);
    }

    #[test]
    fn splits_by_periods() {
        let text = "A.Movie.I.Made.2004.French";
        let words = split_by_non_letters(text);
        assert_eq!(words, vec!["A", "Movie", "I", "Made", "French"]);
    }
}
