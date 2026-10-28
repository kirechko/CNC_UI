 //! Parses letter-number words from a G-code line without changing emulator state.
pub(super) fn parse_words(input: &str) -> Vec<(char, f64)> {
    let bytes = input.as_bytes();
    let mut words = Vec::new();
    let mut index = 0;

    while index < bytes.len() {
        if !bytes[index].is_ascii_alphabetic() {
            index += 1;
            continue;
        }

        let letter = bytes[index].to_ascii_uppercase() as char;
        let number_start = index + 1;
        let mut end = number_start;

        if end < bytes.len() && matches!(bytes[end], b'+' | b'-') {
            end += 1;
        }

        let mut digits = 0;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            digits += 1;
        }

        if end < bytes.len() && bytes[end] == b'.' {
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
                digits += 1;
            }
        }

        if digits == 0 {
            index += 1;
            continue;
        }

        // Support exponent notation when digits follow the E.
        let mantissa_end = end;
        if end < bytes.len() && matches!(bytes[end], b'e' | b'E') {
            let mut exponent_end = end + 1;
            if exponent_end < bytes.len()
                && matches!(bytes[exponent_end], b'+' | b'-')
            {
                exponent_end += 1;
            }

            let exponent_start = exponent_end;
            while exponent_end < bytes.len() && bytes[exponent_end].is_ascii_digit() {
                exponent_end += 1;
            }

            if exponent_end > exponent_start {
                end = exponent_end;
            }
        }

        let number_end = if end == mantissa_end { mantissa_end } else { end };
        let value = input[number_start..number_end]
            .parse::<f64>()
            .expect("scanned G-code number must be a valid f64");

        words.push((letter, value));
        index = number_end;
    }

    words
}
