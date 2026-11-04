//! Focused persistence, compatibility, and validation tests for machine profiles.

use super::*;

/// Builds an isolated in-memory database using the production profile schema migration.
fn database() -> Connection {
    let connection = Connection::open_in_memory().expect("open test database");
    ensure_schema(&connection).expect("apply profile schema");
    connection
}

/// Creates a complete but intentionally non-binding sample profile for persistence tests.
fn sample_profile(id: &str, name: &str) -> MachineProfile {
    MachineProfile {
        id: id.into(),
        name: name.into(),
        machine_type: MachineType::Hybrid,
        units: Units::Millimeters,
        travel: AxisTravel {
            x_min: Some(0.0),
            x_max: Some(300.0),
            ..AxisTravel::default()
        },
        speeds: AxisSpeeds::default(),
        power: Some(PowerProfile {
            kind: PowerKind::Laser,
            min: Some(0.0),
            max: Some(1000.0),
        }),
        source_priorities: SourcePriorities::default(),
        tool_reference: None,
        extensions: serde_json::json!({"note":"operator supplied"}),
        created_at_ms: 10,
        updated_at_ms: 10,
    }
}

/// Verifies schema setup is idempotent and can upgrade an existing legacy application database.
#[test]
fn schema_migration_is_idempotent_and_preserves_legacy_tables() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute(
            "CREATE TABLE messages(id INTEGER PRIMARY KEY, message TEXT)",
            [],
        )
        .unwrap();
    ensure_schema(&connection).unwrap();
    ensure_schema(&connection).unwrap();
    let legacy: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(legacy, 1);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM machine_profiles", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

/// Confirms old profile JSON remains compatible when new optional metadata fields are absent.
#[test]
fn legacy_profile_json_defaults_new_metadata_fields() {
    let mut value = serde_json::to_value(sample_profile("legacy", "Legacy")).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("speeds");
    object.remove("toolReference");
    object.remove("units");
    object.remove("sourcePriorities");
    let decoded: MachineProfile = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.units, Units::Millimeters);
    assert_eq!(decoded.speeds, AxisSpeeds::default());
    assert_eq!(decoded.source_priorities, SourcePriorities::default());
    assert_eq!(decoded.tool_reference, None);
}

/// Verifies active profile selection persists across connection reopening and deletion clears it.
#[test]
fn active_selection_is_durable_and_deleted_with_profile() {
    let connection = database();
    let profile = sample_profile("durable", "Durable");
    let mut writer = connection;
    upsert(&mut writer, &profile).unwrap();
    select_profile(&writer, Some(&profile.id)).unwrap();
    assert_eq!(
        active_profile_id(&writer).unwrap(),
        Some(profile.id.clone())
    );
    delete(&writer, &profile.id).unwrap();
    assert_eq!(active_profile_id(&writer).unwrap(), None);
}

/// Verifies CRUD upsert and export/import preserve explicit known and unknown values.
#[test]
fn profile_crud_and_json_round_trip_preserve_unknowns() {
    let mut connection = database();
    let profile = sample_profile("local-1", "Hybrid One");
    upsert(&mut connection, &profile).unwrap();
    let stored = list(&connection).unwrap();
    assert_eq!(stored, vec![profile.clone()]);
    let document = export_document(stored);
    let serialized = serde_json::to_string(&document).unwrap();
    let parsed: MachineProfileDocument = serde_json::from_str(&serialized).unwrap();
    let mut imported = database();
    import_document(&mut imported, &parsed).unwrap();
    assert_eq!(list(&imported).unwrap(), vec![profile]);
    assert!(delete(&imported, "local-1").unwrap());
    assert!(!delete(&imported, "local-1").unwrap());
}

/// Verifies invalid bounds and unsupported document versions fail before persistence.
#[test]
fn invalid_profile_and_document_are_rejected() {
    let mut invalid = sample_profile("bad", "Bad");
    invalid.travel.x_min = Some(20.0);
    invalid.travel.x_max = Some(10.0);
    assert!(validate_profile(&invalid).unwrap_err().contains("X travel"));
    let mut connection = database();
    let document = MachineProfileDocument {
        schema_version: 99,
        profiles: vec![sample_profile("ok", "OK")],
    };
    assert!(import_document(&mut connection, &document)
        .unwrap_err()
        .contains("unsupported"));
    assert!(list(&connection).unwrap().is_empty());
}
