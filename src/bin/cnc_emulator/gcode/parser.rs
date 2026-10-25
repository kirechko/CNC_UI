//! Parses letter-number words from a G-code line without changing machine state.
pub fn parse_words(line: &str) -> Vec<(char, f64)> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        if !chars[i].is_ascii_alphabetic() {
            i += 1;
            continue;
        }

        let letter = chars[i].to_ascii_uppercase();
        i += 1;
        let start = i;

        if i < chars.len() && matches!(chars[i], '+' | '-') {
            i += 1;
        }

        let mut dot_seen = false;
        while i < chars.len() {
            if chars[i].is_ascii_digit() {
                i += 1;
            } else if chars[i] == '.' && !dot_seen {
                dot_seen = true;
                i += 1;
            } else {
                break;
            }
        }

        if i > start {
            let value: String = chars[start..i].iter().collect();
            if let Ok(value) = value.parse::<f64>() {
                words.push((letter, value));
            }
        }
    }

    words
}

#[cfg(test)]
mod tests {
    use super::parse_words;

    #[test]
    fn parses_signed_decimal_values_and_decimal_g_codes() {
        assert_eq!(
            parse_words("G1 X10.5 Y-2 Z+0.25 F1200"),
            vec![
                ('G', 1.0),
                ('X', 10.5),
                ('Y', -2.0),
                ('Z', 0.25),
                ('F', 1200.0),
            ]
        );
        assert_eq!(parse_words("g59.3 x-1.25"), vec![('G', 59.3), ('X', -1.25)]);
    }

    #[test]
    fn handles_spaces_case_and_concatenated_words() {
        assert_eq!(
            parse_words("G1 X10 Y20"),
            vec![('G', 1.0), ('X', 10.0), ('Y', 20.0)]
        );
        assert_eq!(
            parse_words("g1x10y20"),
            vec![('G', 1.0), ('X', 10.0), ('Y', 20.0)]
        );
        assert_eq!(
            parse_words("  g1   X10  y20  "),
            vec![('G', 1.0), ('X', 10.0), ('Y', 20.0)]
        );
    }

    #[test]
    fn preserves_repeated_words_and_order() {
        assert_eq!(
            parse_words("X1 Y2 X3 X-4"),
            vec![('X', 1.0), ('Y', 2.0), ('X', 3.0), ('X', -4.0)]
        );
    }

    #[test]
    fn empty_or_non_word_input_produces_no_words() {
        assert!(parse_words("").is_empty());
        assert!(parse_words("   \t\n ").is_empty());
        assert!(parse_words("@#$%^&*").is_empty());
        assert!(parse_words("G X Y").is_empty());
    }

    #[test]
    fn ignores_unparseable_values_and_continues_scanning() {
        assert_eq!(parse_words("G X Y7"), vec![('Y', 7.0)]);
        assert!(parse_words("X. Y.").is_empty());
        assert_eq!(parse_words("X1.2.3 Y4"), vec![('X', 1.2), ('Y', 4.0)]);
    }

    #[test]
    fn currently_accepts_overflow_as_infinity() {
        let input = format!("X{}", "9".repeat(400));
        let words = parse_words(&input);

        assert_eq!(words.len(), 1);
        assert_eq!(words[0].0, 'X');
        assert!(words[0].1.is_infinite());
    }

    #[test]
    fn extracts_words_from_parentheses_and_semicolon_text() {
        assert_eq!(
            parse_words("G1 X10 (G99 Y42) ; M8 X20"),
            vec![
                ('G', 1.0),
                ('X', 10.0),
                ('G', 99.0),
                ('Y', 42.0),
                ('M', 8.0),
                ('X', 20.0),
            ]
        );
    }
}
