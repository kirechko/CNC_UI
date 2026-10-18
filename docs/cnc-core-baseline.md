# Снимок состояния перед переносом в `cnc_core`

## Область проверки

Этот документ фиксирует архитектуру и внешние контракты, видимые в предоставленных исходниках. Это базовый снимок для последующей миграции; он не меняет API и не предписывает переносить UI-код в `cnc_core`.

## Текущее состояние ядра и адаптеров

- Корневой `Cargo.toml` подключает `cnc_core` как зависимость по пути `cnc_core`.
- `cnc_core/Cargo.toml` содержит зависимость `serde` с feature `derive`.
- `cnc_core/src/lib.rs` уже содержит общие CNC-типы, `ControllerError`, `CncResult` и trait `LinuxCncApi`.
- Приложение импортирует типы ядра через `use cnc_core as cnc_api`.
- Реализация клиента протокола эмулятора находится в `src/linuxcnc_api.rs` и экспортирует `EmAdapter`.
- GRBL Serial-адаптер находится в `src/grbl_adapter.rs`.
- Временные LinuxCNC и FluidNC debug-адаптеры находятся в `src/adapters.rs`.
- `CNCController` и его очередь команд находятся в `src/cnc_controller.rs`.
- В `src/bin/cnc_emulator` размещены отдельный сервер эмулятора, его протокол и реализация G-code. Это не клиентские адаптеры `LinuxCncApi`.
- Tauri-команды размещены в `src/tauri_app.rs`; HTTP API и Axum-маршруты — в `src/web_server.rs`.
- HTML/JS интерфейс находится в `web/index.html` и обращается к backend через Tauri `invoke`, а при запуске в браузере — через HTTP.

## Общий API `cnc_core`

Публичные типы ядра описывают:

- оси `X`, `Y`, `Z`, `A`, `B`, единицы измерения и конфигурацию осей;
- режим, состояние и состояние интерпретатора станка;
- системы координат G54–G59.3, offsets и позицию;
- статус и команду шпинделя;
- движение, Jog-запросы, ошибки и результат вызова.

Trait `LinuxCncApi` задаёт операции получения статуса и конфигурации, питания и E-Stop, homing, систем координат, Jog, MDI, загрузки и выполнения программы, управления шпинделем и сброса ошибки. `jog_continuous_xy`, `cancel_jog`, `machine_configuration` имеют реализации по умолчанию, которые сообщают о неподдерживаемой возможности.

## Контракты Tauri Commands

Имена ниже — имена команд, зарегистрированных в `tauri::generate_handler!` и используемых интерфейсом. Параметры перечислены в терминах Rust; интерфейс передаёт аргументы команд в camelCase там, где имя составное.

| Команда | Аргументы интерфейса |
| --- | --- |
| `get_status` | нет |
| `get_program` | нет |
| `load_program` | `fileName`, `content` |
| `start_program` | нет |
| `pause_program` | нет |
| `resume_program` | нет |
| `abort_program` | нет |
| `jog` | `axis`, `direction`, `distance` |
| `jog_xy` | `xDirection`, `yDirection`, `distance` |
| `jog_continuous` | `axis`, `direction` |
| `jog_continuous_xy` | `xDirection`, `yDirection` |
| `cancel_jog` | нет |
| `set_spindle` | `rpm`, `enabled`, `direction` |
| `select_coordinate_system` | `system` |
| `zero_all` | `system` |
| `zero_axis` | `axis` |
| `home_axis` | `axis` |
| `home_all` | нет |
| `set_axis_work_value` | `axis`, `system`, `value` |
| `go_to_axis_work_value` | `axis`, `system`, `value` |

В предоставленном `src/tauri_app.rs` команда `jog` указана в списке обработчиков дважды. Это наблюдение фиксируется как предмет проверки при валидации сборки, а не как изменение внешнего контракта.

## Контракты Axum Routes

| Маршрут | Метод | Назначение |
| --- | --- | --- |
| `/api/status` | GET | Получение статуса |
| `/api/program` | GET, POST | Получение и загрузка программы |
| `/api/program/run` | POST | Запуск программы |
| `/api/program/pause` | POST | Пауза |
| `/api/program/resume` | POST | Продолжение |
| `/api/program/abort` | POST | Остановка |
| `/api/jog` | POST | Инкрементальный Jog одной оси |
| `/api/jog/xy` | POST | Инкрементальный согласованный Jog XY |
| `/api/jog/continuous` | POST | Непрерывный Jog одной оси |
| `/api/jog/continuous-xy` | POST | Непрерывный согласованный Jog XY |
| `/api/jog/cancel` | POST | Отмена Jog |
| `/api/spindle` | POST | Команда шпинделя |
| `/api/position/select` | POST | Выбор системы координат |
| `/api/position/zero` | POST | Установка общего рабочего нуля |
| `/api/position/zero-axis` | POST | Установка нуля оси |
| `/api/position/home` | POST | Homing одной оси |
| `/api/position/home-all` | POST | Homing всех осей |
| `/api/position/set` | POST | Задание рабочего значения оси |
| `/api/position/go` | POST | Перемещение к рабочему значению оси |

### Форматы запросов, подтверждённые исходниками

- Загрузка программы: `{ "fileName": string, "content": string }`.
- Jog одной оси: `{ "axis": Axis, "direction": number, "distance": number }`.
- XY Jog: `{ "xDirection": number, "yDirection": number, "distance": number }`.
- Непрерывный Jog одной оси: `{ "axis": Axis, "direction": number }`.
- Непрерывный XY Jog: `{ "xDirection": number, "yDirection": number }`.
- Шпиндель: `{ "rpm": number, "enabled": boolean, "direction": "CW" | "CCW" }`.
- Выбор системы координат и общий ноль: `{ "system": string }`.
- Ноль оси: `{ "axis": Axis }`.
- Задание или перемещение к координате: `{ "axis": Axis, "system": string, "value": number }`.

Для Tauri аргументы передаются объектом команды; для Axum тело POST-запроса JSON. Имена camelCase в веб-контракте нужно сохранить.

## Контракт статуса для HTML/JS

`StatusResponse`, используемый как ответ `/api/status` и результат `get_status`, сериализует поля:

- соединение и состояние: `connected`, `state`;
- рабочие и машинные координаты: `x`, `y`, `z`, `a`, `b`, `machine_x`, `machine_y`, `machine_z`, `machine_a`, `machine_b`;
- координаты и конфигурация: `coordinate_system`, `configuration`;
- движение: `movement_speed`, `movement_mode`;
- шпиндель: `spindle_rpm`, `spindle_commanded_rpm`, `spindle_enabled`, `spindle_direction`, `spindle_min_rpm`, `spindle_max_rpm`;
- программа и сообщения: `current_line`, `total_lines`, `message`, `error`.

Интерфейс также ожидает `get_program`/`/api/program` в форме `{ "name": string, "content": string }` либо `null`, а `load_program`/POST `/api/program` — объект с загруженными `name` и `content`.

## Проверки и открытые вопросы для последующих этапов

- Сравнить фактический набор команд Tauri и маршрутов Axum со всеми вызовами из `web/index.html`; сохранить имена, аргументы, JSON-формы и семантику ошибок при внутреннем переносе.
- Проверить сборку с зарегистрированным дважды обработчиком `jog`; устранить возможную проблему отдельно, если она подтверждается компилятором.
- Проверить тестами, что все адаптеры реализуют один trait из `cnc_core` и что внешние контракты не зависят от местоположения Rust-модулей.
- Решить на следующем этапе, какие адаптеры перемещать в `cnc_core`: перенос GRBL потребует учесть `serialport`, а перенос `EmAdapter` — Unix-сокеты и JSON. Это потребует отдельно определить зависимости и поддерживаемые платформы.
