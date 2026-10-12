from pathlib import Path
import shutil
import sys

ROOT = Path(__file__).resolve().parent.parent
FILES = {
    "gcode": ROOT / "src/bin/cnc_emulator/gcode.rs",
    "state": ROOT / "src/bin/cnc_emulator/state.rs",
    "config": ROOT / "src/bin/cnc_emulator/config.rs",
}


def find_module_start(text):
    """Находит начало #[cfg(test)] mod tests { без регулярных выражений."""
    lines = text.splitlines(keepends=True)
    offset = 0

    for index, line in enumerate(lines):
        if line.strip() != "#[cfg(test)]":
            offset += len(line)
            continue

        next_index = index + 1
        while next_index < len(lines) and not lines[next_index].strip():
            next_index += 1

        if next_index < len(lines) and lines[next_index].strip().startswith("mod tests"):
            return offset, offset + sum(len(item) for item in lines[index:next_index + 1])

        offset += len(line)

    return None


def find_closing_brace(text, opening):
    depth = 0
    i = opening
    mode = "code"

    while i < len(text):
        char = text[i]
        next_char = text[i + 1] if i + 1 < len(text) else ""

        if mode == "code":
            if char == "/" and next_char == "/":
                mode = "line_comment"
                i += 2
                continue
            if char == "/" and next_char == "*":
                mode = "block_comment"
                i += 2
                continue
            if char == '"':
                mode = "string"
            elif char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    return i

        elif mode == "line_comment":
            if char == "\n":
                mode = "code"

        elif mode == "block_comment":
            if char == "*" and next_char == "/":
                mode = "code"
                i += 2
                continue

        elif mode == "string":
            if char == "\\":
                i += 2
                continue
            if char == '"':
                mode = "code"

        i += 1

    raise RuntimeError("Не найдена закрывающая скобка модуля tests")


def remove_test_modules(text):
    while True:
        found = find_module_start(text)
        if found is None:
            return text.rstrip() + "\n"

        start, declaration_end = found
        opening = text.find("{", start, declaration_end)
        if opening < 0:
            raise RuntimeError("Не найдена открывающая скобка модуля tests")

        closing = find_closing_brace(text, opening)
        text = text[:start] + text[closing + 1:]


GCODE_TESTS = r'''

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
'''

STATE_TESTS = r'''

#[cfg(test)]
mod tests {
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
    fn target_inside_limits_is_accepted() {
        assert!(test_emulator().check_target([150.0, 200.0, 50.0]).is_ok());
    }

    #[test]
    fn exact_axis_boundaries_are_accepted() {
        let emulator = test_emulator();
        assert!(emulator.check_target([0.0, 0.0, 0.0]).is_ok());
        assert!(emulator.check_target([300.0, 400.0, 100.0]).is_ok());
    }

    #[test]
    fn targets_below_axis_minima_are_rejected() {
        let emulator = test_emulator();
        assert!(emulator.check_target([-0.001, 100.0, 50.0]).is_err());
        assert!(emulator.check_target([100.0, -0.001, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 100.0, -0.001]).is_err());
    }

    #[test]
    fn targets_above_axis_maxima_are_rejected() {
        let emulator = test_emulator();
        assert!(emulator.check_target([300.001, 100.0, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 400.001, 50.0]).is_err());
        assert!(emulator.check_target([100.0, 100.0, 100.001]).is_err());
    }

    #[test]
    fn small_floating_point_tolerance_is_allowed() {
        let emulator = test_emulator();
        assert!(emulator.check_target([-5e-9, 100.0, 50.0]).is_ok());
        assert!(emulator.check_target([300.0 + 5e-9, 100.0, 50.0]).is_ok());
    }
}
'''


def main():
    for path in FILES.values():
        if not path.is_file():
            print(f"Файл не найден: {path}", file=sys.stderr)
            return 1

    for name, path in FILES.items():
        backup = path.with_suffix(path.suffix + ".bak")
        shutil.copy2(path, backup)

        content = remove_test_modules(path.read_text(encoding="utf-8"))
        if name == "gcode":
            content += GCODE_TESTS
        elif name == "state":
            content += STATE_TESTS

        path.write_text(content, encoding="utf-8")
        print(f"Обновлён {path.relative_to(ROOT)}; резервная копия: {backup.name}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
