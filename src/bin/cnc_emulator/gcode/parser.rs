pub(super) fn parse_words(line: &str) -> Vec<(char, f64)> {
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
