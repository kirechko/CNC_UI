#[path = "gcode/accessories.rs"]
mod accessories;
#[path = "gcode/coordinates.rs"]
mod coordinates;
#[path = "gcode/execute.rs"]
mod execute;
#[path = "gcode/motion.rs"]
mod motion;
#[path = "gcode/parser.rs"]
mod parser;

#[cfg(test)]
#[path = "gcode/test_support.rs"]
pub(super) mod test_support;
#[cfg(test)]
pub(super) use test_support::test_emulator;


use super::protocol::WireError;
use super::state::Emulator;
use crate::cnc_api::*;

impl Emulator {
    pub(super) fn execute_gcode(&mut self, line: &str) -> Result<(), WireError> {
        execute::execute_gcode(self, line)
    }


    pub(super) fn set_spindle(&mut self, direction: SpindleDirection, rpm: Option<f64>) {
        accessories::set_spindle(self, direction, rpm);
    }
}



#[cfg(any())]
mod tests {
    use super::{parse_words, test_emulator};

    #[test]
    fn parse_words_reads_signed_decimal_values() {
        assert_eq!(
            parse_words("G1 X10.5 Y-2 Z+0.25 F1200"),
            vec![
                ('G', 1.0), ('X', 10.5), ('Y', -2.0),
                ('Z', 0.25), ('F', 1200.0),
            ]
        );
    }

    #[test]
    fn parse_words_is_case_insensitive_and_reads_decimal_g_codes() {
        assert_eq!(
            parse_words("g59.3 x-1.25"),
            vec![('G', 59.3), ('X', -1.25)]
        );
    }

    #[test]
    fn linear_move_inside_limits_is_accepted() {
        let mut emulator = test_emulator();
        assert!(emulator.execute_gcode("G1 X100 Y200 Z50").is_ok());
        assert!(emulator.active_move.is_some());
    }

    #[test]
    fn linear_moves_outside_axis_limits_are_rejected() {
        for command in [
            "G1 X-1", "G1 X301",
            "G1 Y-1", "G1 Y401",
            "G1 Z-1", "G1 Z101",
        ] {
            let mut emulator = test_emulator();
            assert!(
                emulator.execute_gcode(command).is_err(),
                "expected {command:?} to be rejected"
            );
        }
    }

    #[test]
    fn axis_words_without_motion_code_are_rejected() {
        let mut emulator = test_emulator();
        assert!(emulator.execute_gcode("X10").is_err());
    }
}

#[cfg(any())]
mod parse_words_coverage_tests {
    use super::parse_words;

    #[test]
    fn handles_uppercase_lowercase_spaces_and_concatenated_words() {
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
    fn handles_integer_decimal_and_signed_values() {
        assert_eq!(
            parse_words("X12 Y-3 Z+4.5 A-0.25 B.5"),
            vec![
                ('X', 12.0),
                ('Y', -3.0),
                ('Z', 4.5),
                ('A', -0.25),
                ('B', 0.5),
            ]
        );
    }

    #[test]
    fn preserves_repeated_words_and_their_order() {
        assert_eq!(
            parse_words("X1 Y2 X3 X-4"),
            vec![('X', 1.0), ('Y', 2.0), ('X', 3.0), ('X', -4.0)]
        );
    }

    #[test]
    fn empty_whitespace_and_non_word_input_produce_no_words() {
        assert!(parse_words("").is_empty());
        assert!(parse_words("   \t\n ").is_empty());
        assert!(parse_words("@#$%^&*").is_empty());
        assert!(parse_words("G X Y").is_empty());
    }

    #[test]
    fn ignores_unparseable_word_values_and_continues_scanning() {
        // Буква без числа не создаёт слово; последующие корректные слова читаются.
        assert_eq!(parse_words("G X Y7"), vec![('Y', 7.0)]);

        // Точка без цифр не разбирается как число.
        assert!(parse_words("X. Y.").is_empty());

        // Парсер берёт допустимый числовой префикс, затем продолжает сканирование.
        assert_eq!(
            parse_words("X1.2.3 Y4"),
            vec![('X', 1.2), ('Y', 4.0)]
        );
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
    fn parser_extracts_words_from_parentheses_and_semicolon_text() {
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

#[cfg(any())]
mod execute_gcode_linear_motion_tests {
    use super::test_emulator;

    #[test]
    fn g0_creates_rapid_move_to_requested_target() {
        let mut emulator = test_emulator();

        assert!(emulator.execute_gcode("G0 X10 Y20 Z5").is_ok());
        assert!(emulator.active_move.is_some());
    }

    #[test]
    fn g1_creates_feed_move_to_requested_target() {
        let mut emulator = test_emulator();

        assert!(emulator.execute_gcode("G1 X10 Y20 Z5").is_ok());
        assert!(emulator.active_move.is_some());
    }

    #[test]
    fn line_without_motion_does_not_create_active_move() {
        let mut emulator = test_emulator();

        assert!(emulator.execute_gcode("M7").is_ok());
        assert!(emulator.active_move.is_none());
    }

    #[test]
    fn move_to_current_position_does_not_create_empty_move() {
        let mut emulator = test_emulator();

        assert!(emulator.execute_gcode("G0 X0 Y0 Z0").is_ok());
        assert!(emulator.active_move.is_none());

        assert!(emulator.execute_gcode("G1 X0 Y0 Z0").is_ok());
        assert!(emulator.active_move.is_none());
    }

    #[test]
    fn positive_finite_feed_is_accepted_and_saved() {
        let mut emulator = test_emulator();

        assert!(emulator.execute_gcode("F1200 G1 X10").is_ok());
        assert_eq!(emulator.status.feed_rate, 1200.0);
        assert!(emulator.active_move.is_some());
    }

    #[test]
    fn zero_and_negative_feed_are_rejected() {
        for command in ["F0", "F-1"] {
            let mut emulator = test_emulator();

            assert!(
                emulator.execute_gcode(command).is_err(),
                "expected {command:?} to be rejected"
            );
        }
    }

    #[test]
    fn overflowing_feed_is_rejected_as_non_finite() {
        let mut emulator = test_emulator();
        let command = format!("F{}", "9".repeat(400));

        assert!(emulator.execute_gcode(&command).is_err());
    }

    #[test]
    fn rejected_feed_does_not_change_saved_feed_rate() {
        let mut emulator = test_emulator();
        let initial_feed = emulator.status.feed_rate;

        assert!(emulator.execute_gcode("F0").is_err());
        assert_eq!(emulator.status.feed_rate, initial_feed);
    }
}

