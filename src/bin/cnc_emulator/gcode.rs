mod parser;

pub(super) fn parse_words(line: &str) -> Vec<(char, f64)> {
    parser::parse_words(line)
}

use super::dynamics::ActiveMove;
use super::protocol::{api_error, WireError};
use super::state::Emulator;
use crate::cnc_api::*;
use std::f64::consts::PI;

impl Emulator {
    pub(super) fn execute_gcode(&mut self, line: &str) -> Result<(), WireError> {
        let upper = line.to_ascii_uppercase();
        if upper.contains("G59.1") {
            self.select_coordinate_system(CoordinateSystem::G59_1);
        } else if upper.contains("G59.2") {
            self.select_coordinate_system(CoordinateSystem::G59_2);
        } else if upper.contains("G59.3") {
            self.select_coordinate_system(CoordinateSystem::G59_3);
        }

        let words = parse_words(line);
        let mut motion_code: Option<i32> = None;
        let mut dwell = false;
        let mut p_word: Option<f64> = None;
        let mut x_word = None;
        let mut y_word = None;
        let mut z_word = None;
        let mut i_word = None;
        let mut j_word = None;
        let mut l_word = None;

        for &(letter, value) in &words {
            match letter {
                'G' => {
                    // Десятичные варианты нужно распознать до преобразования в int:
                    // иначе G59.1 превратится в G59.
                    if (value - 59.1).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_1);
                    } else if (value - 59.2).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_2);
                    } else if (value - 59.3).abs() < 1e-6 {
                        self.select_coordinate_system(CoordinateSystem::G59_3);
                    } else {
                        match value as i32 {
                            0 | 1 | 2 | 3 => motion_code = Some(value as i32),
                            4 => dwell = true,
                            17 => {} // Поддерживаемая дуговая плоскость XY.
                            54 => self.select_coordinate_system(CoordinateSystem::G54),
                            55 => self.select_coordinate_system(CoordinateSystem::G55),
                            56 => self.select_coordinate_system(CoordinateSystem::G56),
                            57 => self.select_coordinate_system(CoordinateSystem::G57),
                            58 => self.select_coordinate_system(CoordinateSystem::G58),
                            59 => self.select_coordinate_system(CoordinateSystem::G59),
                            90 => self.absolute_mode = true,
                            91 => self.absolute_mode = false,
                            20 => return Err(api_error("unsupported_gcode", "G20 inch units are not supported; use G21")),
                            21 => {}
                            other => return Err(api_error("unsupported_gcode", format!("unsupported G-code G{other}"))),
                        }
                    }
                },
                'X' => x_word = Some(value),
                'Y' => y_word = Some(value),
                'Z' => z_word = Some(value),
                'I' => i_word = Some(value),
                'J' => j_word = Some(value),
                'P' => p_word = Some(value),
                'L' => l_word = Some(value),
                'F' if value.is_finite() && value > 0.0 => self.status.feed_rate = value,
                'F' => return Err(api_error("bad_feed", "F must be positive and finite")),
                'S' if value.is_finite() && value >= 0.0 => {
                    if value > self.config.spindle.max_rpm {
                        return Err(api_error("spindle_limit", format!(
                            "S{value:.0} exceeds maximum {:.0} RPM",
                            self.config.spindle.max_rpm
                        )));
                    }
                    self.status.spindle.commanded_rpm = value;
                }
                'M' => match value as i32 {
                    3 => self.set_spindle(SpindleDirection::Clockwise, None),
                    4 => self.set_spindle(SpindleDirection::CounterClockwise, None),
                    5 => self.set_spindle(SpindleDirection::Stopped, Some(0.0)),
                    7 => {
                        self.status.mist_coolant_on = true;
                        self.status.message = Some("Mist coolant ON (simulated)".into());
                    }
                    8 => {
                        self.status.flood_coolant_on = true;
                        self.status.message = Some("Flood coolant ON (simulated)".into());
                    }
                    9 => {
                        self.status.mist_coolant_on = false;
                        self.status.flood_coolant_on = false;
                        self.status.message = Some("Coolant OFF".into());
                    }
                    30 => {
                        self.status.message = Some("Program end M30".into());
                    }
                    2 => {
                        self.status.message = Some("Program end M2".into());
                    }
                    other => return Err(api_error("unsupported_gcode", format!("unsupported M-code M{other}"))),
                },
                // Стандартные слова, не используемые обработчиком траектории.
                'N' | 'T' | 'H' => {}
                _ => {}
            }
        }

        if words.iter().any(|(letter, value)| *letter == 'G' && (*value as i32) == 10) {
            if l_word.map(|v| v as i32) != Some(2) {
                return Err(api_error("unsupported_gcode", "only G10 L2 is supported"));
            }
            let p = p_word.ok_or_else(|| api_error("bad_g10", "G10 L2 requires P1..P9"))? as i32;
            let system = match p {
                1 => CoordinateSystem::G54,
                2 => CoordinateSystem::G55,
                3 => CoordinateSystem::G56,
                4 => CoordinateSystem::G57,
                5 => CoordinateSystem::G58,
                6 => CoordinateSystem::G59,
                7 => CoordinateSystem::G59_1,
                8 => CoordinateSystem::G59_2,
                9 => CoordinateSystem::G59_3,
                _ => return Err(api_error("bad_g10", "G10 L2 P must be in range 1..9")),
            };

            let mut offset = self.status.coordinate_systems.iter()
                .find(|entry| entry.system == system)
                .map(|entry| entry.offset)
                .unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 });

            if let Some(x) = x_word { offset.x = x; }
            if let Some(y) = y_word { offset.y = y; }
            if let Some(z) = z_word { offset.z = z; }

            if let Some(entry) = self.status.coordinate_systems.iter_mut()
                .find(|entry| entry.system == system)
            {
                entry.offset = offset;
            }
            self.persist_coordinate_offsets()?;
            self.status.message = Some(format!("G10 L2 {} offset saved", system.label()));
            return Ok(());
        }

        if dwell {
            let seconds = p_word
                .filter(|value| value.is_finite() && *value >= 0.0)
                .ok_or_else(|| api_error("bad_dwell", "G4 requires a finite, non-negative P value in seconds"))?;

            self.dwell_remaining = seconds;
            self.status.message = Some(format!("Dwell: {:.3} s", seconds));
            return Ok(());
        }

        if let Some(code) = motion_code {
            let current = self.position_array();
            let offset = [
                self.status.work_offset.x,
                self.status.work_offset.y,
                self.status.work_offset.z,
            ];
            let words_axis = [x_word, y_word, z_word];
            let mut target = current;

            for index in 0..3 {
                if let Some(value) = words_axis[index] {
                    target[index] = if self.absolute_mode {
                        value + offset[index]
                    } else {
                        current[index] + value
                    };
                }
            }

            self.check_target(target)?;

            if code == 2 || code == 3 {
                if i_word.is_none() && j_word.is_none() {
                    return Err(api_error("bad_arc", "G2/G3 requires I or J center offsets"));
                }

                let center_x = current[0] + i_word.unwrap_or(0.0);
                let center_y = current[1] + j_word.unwrap_or(0.0);
                let radius_start = ((current[0] - center_x).powi(2)
                    + (current[1] - center_y).powi(2)).sqrt();
                let radius_end = ((target[0] - center_x).powi(2)
                    + (target[1] - center_y).powi(2)).sqrt();

                if radius_start <= 1e-9
                    || (radius_start - radius_end).abs() > (0.02_f64).max(radius_start * 0.002)
                {
                    return Err(api_error("bad_arc", format!(
                        "arc radii mismatch: start={radius_start:.4}, end={radius_end:.4}"
                    )));
                }

                let start_angle = (current[1] - center_y).atan2(current[0] - center_x);
                let end_angle = (target[1] - center_y).atan2(target[0] - center_x);
                let same_xy = (target[0] - current[0]).abs() < 1e-8
                    && (target[1] - current[1]).abs() < 1e-8;

                let sweep = if same_xy {
                    if code == 2 { -2.0 * PI } else { 2.0 * PI }
                } else if code == 2 {
                    let mut a = end_angle - start_angle;
                    while a >= 0.0 { a -= 2.0 * PI; }
                    a
                } else {
                    let mut a = end_angle - start_angle;
                    while a <= 0.0 { a += 2.0 * PI; }
                    a
                };

                // Проверяем мягкие пределы по дуге, а не только в конечной точке.
                let samples = ((radius_start * sweep.abs() / 1.0).ceil() as usize).max(16);
                for n in 0..=samples {
                    let t = n as f64 / samples as f64;
                    let angle = start_angle + sweep * t;
                    let p = [
                        center_x + radius_start * angle.cos(),
                        center_y + radius_start * angle.sin(),
                        current[2] + (target[2] - current[2]) * t,
                    ];
                    self.check_target(p)?;
                }

                if self.active_move.is_some() {
                    return Err(api_error("motion_busy", "another motion is active"));
                }

                let arc = ActiveMove::from_arc(
                    current,
                    target,
                    center_x,
                    center_y,
                    radius_start,
                    start_angle,
                    sweep,
                    self.status.feed_rate,
                    &self.config,
                )?;
                self.active_move = Some(arc);
            } else if target != current {
                if code == 0 && z_word.is_some() {
                    self.last_rapid_z = Some(target[2]);
                }
                if self.active_move.is_some() {
                    return Err(api_error("motion_busy", "another motion is active"));
                }
                let rapid = code == 0;
                let feed = if rapid {
                    self.config.axes.x.max_velocity_mm_min
                        .min(self.config.axes.y.max_velocity_mm_min)
                        .min(self.config.axes.z.max_velocity_mm_min)
                } else {
                    self.status.feed_rate
                };
                self.queue_move(target, feed, rapid)?;
            }
        } else if x_word.is_some() || y_word.is_some() || z_word.is_some() {
            return Err(api_error("unsupported_gcode", "axis words require an explicit supported motion G0/G1/G2/G3"));
        }

        self.status.message = Some(format!("Executing: {line}"));
        Ok(())
    }

    pub(super) fn set_spindle(&mut self, direction: SpindleDirection, rpm: Option<f64>) {
        let requested_rpm = rpm.unwrap_or(self.status.spindle.commanded_rpm)
            .clamp(0.0, self.config.spindle.max_rpm);

        if direction == SpindleDirection::Stopped || requested_rpm <= 0.0 {
            // STOP отменяет ожидающее реверсирование и начинает торможение.
            self.pending_spindle = None;
            self.status.spindle.commanded_rpm = 0.0;

            if self.status.spindle.actual_rpm <= 0.01 {
                self.status.spindle.actual_rpm = 0.0;
                self.status.spindle.direction = SpindleDirection::Stopped;
                self.status.spindle.enabled = false;
            } else {
                self.status.spindle.enabled = true;
            }
            return;
        }

        let current_direction = self.status.spindle.direction;
        let is_running = self.status.spindle.actual_rpm > 0.01;

        if is_running && current_direction != direction {
            // Сначала тормозим до нуля. Tick запустит новый разгон после остановки.
            self.pending_spindle = Some((direction, requested_rpm));
            self.status.spindle.commanded_rpm = 0.0;
            self.status.spindle.enabled = true;
        } else {
            self.pending_spindle = None;
            self.status.spindle.direction = direction;
            self.status.spindle.commanded_rpm = requested_rpm;
            self.status.spindle.enabled = true;
        }
    }
}



#[cfg(test)]
mod tests {
    use super::{parse_words, Emulator};
    use super::super::config::{
        AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig,
    };
    use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position};
    use std::path::PathBuf;

    fn test_emulator() -> Emulator {
        let config = EmulatorConfig {
            axes: AxesConfig {
                x: AxisLimits {
                    min_mm: 0.0, max_mm: 300.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                y: AxisLimits {
                    min_mm: 0.0, max_mm: 400.0,
                    max_velocity_mm_min: 6000.0, acceleration_mm_s2: 100.0,
                },
                z: AxisLimits {
                    min_mm: 0.0, max_mm: 100.0,
                    max_velocity_mm_min: 3000.0, acceleration_mm_s2: 50.0,
                },
            },
            spindle: SpindleConfig {
                max_rpm: 24_000.0,
                acceleration_rpm_per_sec: 1000.0,
            },
        };
        let offsets = vec![CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: Position { x: 0.0, y: 0.0, z: 0.0 },
        }];
        Emulator::new(config, PathBuf::from("unused-test-offsets.yaml"), offsets)
    }

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

#[cfg(test)]
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

#[cfg(test)]
mod execute_gcode_linear_motion_tests {
    use super::Emulator;
    use super::super::config::{
        AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig,
    };
    use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position};
    use std::path::PathBuf;

    fn test_emulator() -> Emulator {
        let config = EmulatorConfig {
            axes: AxesConfig {
                x: AxisLimits {
                    min_mm: 0.0,
                    max_mm: 300.0,
                    max_velocity_mm_min: 6000.0,
                    acceleration_mm_s2: 100.0,
                },
                y: AxisLimits {
                    min_mm: 0.0,
                    max_mm: 400.0,
                    max_velocity_mm_min: 6000.0,
                    acceleration_mm_s2: 100.0,
                },
                z: AxisLimits {
                    min_mm: 0.0,
                    max_mm: 100.0,
                    max_velocity_mm_min: 3000.0,
                    acceleration_mm_s2: 50.0,
                },
            },
            spindle: SpindleConfig {
                max_rpm: 24_000.0,
                acceleration_rpm_per_sec: 1000.0,
            },
        };

        let offsets = vec![CoordinateSystemOffset {
            system: CoordinateSystem::G54,
            offset: Position { x: 0.0, y: 0.0, z: 0.0 },
        }];

        Emulator::new(
            config,
            PathBuf::from("unused-test-offsets.yaml"),
            offsets,
        )
    }

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

