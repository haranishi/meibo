use super::*;
use crate::{
    OpenOptions,
    types::{CompanyInput, DataOrigin, Fields, Status},
};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

fn company(name: &str, sole: bool) -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: name.into(),
        prefecture: 13,
        municipality: None,
        industry: None,
        phone: None,
        website: None,
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: sole,
        source_id: None,
        source_as_of: None,
    }
}

fn fixture() -> (tempfile::TempDir, Engine, CompanyId, CompanyId) {
    let dir = tempfile::tempdir().unwrap();
    let mut engine = Engine::create(
        &dir.path().join("fields.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let a = engine
        .register_company(&company("合成取引先A", false))
        .unwrap();
    let b = engine
        .register_company(&company("合成取引先B", true))
        .unwrap();
    (dir, engine, a, b)
}

fn field(name: &str, kind: CustomFieldKind, indexed: bool) -> CustomFieldInput {
    CustomFieldInput {
        name: name.into(),
        kind,
        indexed,
    }
}

fn search(field_id: i64, value: CustomFieldValue) -> CustomFieldSearch {
    CustomFieldSearch {
        field_id,
        value,
        after: None,
        limit: 200,
    }
}

#[test]
fn definitions_and_typed_values_survive_reopen_without_changing_default_details() {
    let (dir, mut engine, a, _) = fixture();
    let before = engine.company_detail(a, &Fields::default()).unwrap();
    let text = engine
        .create_custom_field(&field("  自社メモ  ", CustomFieldKind::Text, true))
        .unwrap();
    let number = engine
        .create_custom_field(&field("点数", CustomFieldKind::Number, false))
        .unwrap();
    let boolean = engine
        .create_custom_field(&field("対象", CustomFieldKind::Boolean, true))
        .unwrap();
    for (id, value) in [
        (text, CustomFieldValue::Text("合成の内部情報".into())),
        (number, CustomFieldValue::Number(12.5)),
        (boolean, CustomFieldValue::Boolean(false)),
    ] {
        engine.set_custom_value(a, id, &value).unwrap();
    }
    let expected = engine.custom_values(a).unwrap();
    assert_eq!(expected.len(), 3);
    let after = engine.company_detail(a, &Fields::default()).unwrap();
    assert_eq!(after, before);
    drop(engine);
    let engine = Engine::open(&dir.path().join("fields.db"), &OpenOptions::default()).unwrap();
    assert_eq!(engine.custom_fields().unwrap()[0].name, "自社メモ");
    assert_eq!(engine.custom_values(a).unwrap(), expected);
}

#[test]
fn invalid_names_types_limits_and_missing_targets_leave_values_unchanged() {
    let (_dir, mut engine, a, _) = fixture();
    for name in ["", "  ", "a\0b", &"あ".repeat(101)] {
        assert!(
            engine
                .create_custom_field(&field(name, CustomFieldKind::Text, false))
                .is_err()
        );
    }
    let text = engine
        .create_custom_field(&field(
            "SQL'; DROP TABLE companies;--",
            CustomFieldKind::Text,
            true,
        ))
        .unwrap();
    assert!(
        engine
            .create_custom_field(&field(
                "SQL'; DROP TABLE companies;--",
                CustomFieldKind::Text,
                true
            ))
            .is_err()
    );
    let number = engine
        .create_custom_field(&field("数値", CustomFieldKind::Number, true))
        .unwrap();
    let value = CustomFieldValue::Text("あ".repeat(4000));
    engine.set_custom_value(a, text, &value).unwrap();
    let before = engine.custom_values(a).unwrap();
    for (company, id, bad) in [
        (a, text, CustomFieldValue::Text("あ".repeat(4001))),
        (a, text, CustomFieldValue::Number(1.0)),
        (a, number, CustomFieldValue::Number(f64::NAN)),
        (a, number, CustomFieldValue::Number(f64::INFINITY)),
        (CompanyId(999), text, value.clone()),
        (a, 999, value.clone()),
        (CompanyId(0), text, value.clone()),
    ] {
        assert!(engine.set_custom_value(company, id, &bad).is_err());
    }
    assert_eq!(engine.custom_values(a).unwrap(), before);
    assert!(engine.custom_values(CompanyId(999)).is_err());
    for n in 2..100 {
        engine
            .create_custom_field(&field(&format!("項目{n}"), CustomFieldKind::Text, false))
            .unwrap();
    }
    assert_eq!(engine.custom_fields().unwrap().len(), 100);
    assert!(
        engine
            .create_custom_field(&field("101件目", CustomFieldKind::Text, false))
            .is_err()
    );
    assert_eq!(engine.custom_fields().unwrap().len(), 100);
}

#[test]
fn indexed_and_nonindexed_searches_page_stably_and_validate_values() {
    let (_dir, mut engine, a, b) = fixture();
    let c = engine
        .register_company(&company("合成取引先C", false))
        .unwrap();
    for (name, kind, value, indexed) in [
        (
            "文",
            CustomFieldKind::Text,
            CustomFieldValue::Text("a' OR 1=1--".into()),
            true,
        ),
        (
            "数",
            CustomFieldKind::Number,
            CustomFieldValue::Number(-0.5),
            true,
        ),
        (
            "真偽",
            CustomFieldKind::Boolean,
            CustomFieldValue::Boolean(false),
            true,
        ),
        (
            "索引なし",
            CustomFieldKind::Text,
            CustomFieldValue::Text("同値".into()),
            false,
        ),
    ] {
        let id = engine
            .create_custom_field(&field(name, kind, indexed))
            .unwrap();
        for company in [a, b, c] {
            engine.set_custom_value(company, id, &value).unwrap();
        }
        let mut q = search(id, value.clone());
        q.limit = 2;
        let first = engine.search_custom_field(&q).unwrap();
        assert_eq!(first.company_ids, [a, b]);
        assert_eq!(first.next, Some(b));
        assert_eq!(first.indexed, indexed);
        q.after = first.next;
        let last = engine.search_custom_field(&q).unwrap();
        assert_eq!(last.company_ids, [c]);
        assert_eq!(last.next, None);
        q.after = None;
        q.limit = 0;
        assert!(engine.search_custom_field(&q).is_err());
        q.limit = 201;
        assert!(engine.search_custom_field(&q).is_err());
        q.limit = 200;
        q.after = Some(CompanyId(-1));
        assert!(engine.search_custom_field(&q).is_err());
        q.after = None;
        q.value = match kind {
            CustomFieldKind::Text => CustomFieldValue::Number(1.0),
            _ => CustomFieldValue::Text("x".into()),
        };
        assert!(engine.search_custom_field(&q).is_err());
    }
}

#[test]
fn clearing_and_deleting_only_remove_the_requested_values() {
    let (_dir, mut engine, a, b) = fixture();
    let one = engine
        .create_custom_field(&field("一", CustomFieldKind::Text, true))
        .unwrap();
    let two = engine
        .create_custom_field(&field("二", CustomFieldKind::Text, false))
        .unwrap();
    let value = CustomFieldValue::Text("保持".into());
    for company in [a, b] {
        for id in [one, two] {
            engine.set_custom_value(company, id, &value).unwrap();
        }
    }
    engine.clear_custom_value(a, one).unwrap();
    assert_eq!(engine.custom_values(a).unwrap().len(), 1);
    assert_eq!(engine.custom_values(b).unwrap().len(), 2);
    engine.clear_custom_value(a, one).unwrap();
    engine.delete_custom_field(two).unwrap();
    assert_eq!(engine.custom_values(a).unwrap().len(), 0);
    assert_eq!(engine.custom_values(b).unwrap().len(), 1);
    assert_eq!(engine.custom_fields().unwrap().len(), 1);
    assert!(engine.delete_custom_field(two).is_err());
    let named = vec![crate::types::Field::Name];
    assert_eq!(
        engine
            .company_detail(a, &named)
            .unwrap()
            .company
            .name
            .as_deref(),
        Some("合成取引先A")
    );
}

#[test]
fn erasing_company_values_removes_search_matches_and_rejects_reinsertion() {
    let (_dir, mut engine, a, b) = fixture();
    let id = engine
        .create_custom_field(&field("個別メモ", CustomFieldKind::Text, true))
        .unwrap();
    let value = CustomFieldValue::Text("合成個人メモ".into());
    engine.set_custom_value(a, id, &value).unwrap();
    engine.set_custom_value(b, id, &value).unwrap();
    engine.erase_company(b).unwrap();
    assert!(engine.custom_values(b).is_err());
    assert!(engine.set_custom_value(b, id, &value).is_err());
    assert_eq!(
        engine
            .search_custom_field(&search(id, value))
            .unwrap()
            .company_ids,
        [a]
    );
    assert_eq!(
        engine
            .db
            .conn()
            .query_row(
                "SELECT count(*) FROM custom_field_values WHERE company_id=?1",
                [b.0],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn injected_storage_failure_rolls_back_updates_and_field_deletion() {
    let (_dir, mut engine, a, _) = fixture();
    let id = engine
        .create_custom_field(&field("保持", CustomFieldKind::Text, true))
        .unwrap();
    engine
        .set_custom_value(a, id, &CustomFieldValue::Text("元".into()))
        .unwrap();
    engine.db.conn().execute_batch("CREATE TEMP TRIGGER fail_value_update AFTER UPDATE ON custom_field_values BEGIN SELECT RAISE(ABORT,'injected'); END; CREATE TEMP TRIGGER fail_definition_delete AFTER DELETE ON custom_field_definitions BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert!(
        engine
            .set_custom_value(a, id, &CustomFieldValue::Text("変更".into()))
            .is_err()
    );
    assert!(engine.delete_custom_field(id).is_err());
    assert_eq!(
        engine.custom_values(a).unwrap()[0].value,
        CustomFieldValue::Text("元".into())
    );
    assert_eq!(engine.custom_fields().unwrap().len(), 1);
}

struct MutableSpace(AtomicU64);
impl crate::SpaceProbe for MutableSpace {
    fn available(&self, _path: &Path) -> io::Result<u64> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

#[test]
fn insufficient_space_rejects_changes_before_commit() {
    let dir = tempfile::tempdir().unwrap();
    let space = Arc::new(MutableSpace(AtomicU64::new(u64::MAX)));
    let options = OpenOptions {
        space_probe: Some(space.clone()),
        ..OpenOptions::default()
    };
    let mut engine = Engine::create(
        &dir.path().join("space.db"),
        DataOrigin::Synthetic,
        &options,
    )
    .unwrap();
    let a = engine.register_company(&company("合成", false)).unwrap();
    let id = engine
        .create_custom_field(&field("容量", CustomFieldKind::Text, true))
        .unwrap();
    space.0.store(0, Ordering::Relaxed);
    assert!(matches!(
        engine.set_custom_value(a, id, &CustomFieldValue::Text("未保存".into())),
        Err(EngineError::InsufficientSpace)
    ));
    assert_eq!(engine.custom_values(a).unwrap().len(), 0);
}

#[test]
fn declared_indexes_are_used_for_each_typed_equality_search() {
    let (_dir, mut engine, a, _) = fixture();
    for (kind, value, index) in [
        (
            CustomFieldKind::Text,
            CustomFieldValue::Text("合成検索".into()),
            "custom_field_text_lookup",
        ),
        (
            CustomFieldKind::Number,
            CustomFieldValue::Number(3.5),
            "custom_field_number_lookup",
        ),
        (
            CustomFieldKind::Boolean,
            CustomFieldValue::Boolean(true),
            "custom_field_boolean_lookup",
        ),
    ] {
        let id = engine
            .create_custom_field(&field(index, kind, true))
            .unwrap();
        engine.set_custom_value(a, id, &value).unwrap();
        let mut statement = engine
            .db
            .conn()
            .prepare(&format!("EXPLAIN QUERY PLAN {}", search_sql(kind, true)))
            .unwrap();
        let plan = statement
            .query_map(params![id, 0, value.sql_value(), 201], |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .join(" ");
        assert!(plan.contains(index), "{plan}");
        assert_eq!(
            engine
                .search_custom_field(&search(id, value))
                .unwrap()
                .company_ids,
            [a]
        );
    }
}

#[test]
fn company_erasure_failure_restores_custom_values_in_the_same_transaction() {
    let (_dir, mut engine, _, b) = fixture();
    let id = engine
        .create_custom_field(&field("消去対象", CustomFieldKind::Text, true))
        .unwrap();
    let value = CustomFieldValue::Text("合成の消去前メモ".into());
    engine.set_custom_value(b, id, &value).unwrap();
    engine.db.conn().execute_batch("CREATE TEMP TRIGGER fail_company_erasure BEFORE UPDATE OF name ON companies BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
    assert!(engine.erase_company(b).is_err());
    assert_eq!(engine.custom_values(b).unwrap()[0].value, value);
    assert_eq!(
        engine
            .search_custom_field(&search(id, value))
            .unwrap()
            .company_ids,
        [b]
    );
}

#[test]
fn newer_extension_version_is_rejected_without_modifying_stored_values() {
    let (dir, mut engine, a, _) = fixture();
    let id = engine
        .create_custom_field(&field("版", CustomFieldKind::Boolean, false))
        .unwrap();
    engine
        .set_custom_value(a, id, &CustomFieldValue::Boolean(true))
        .unwrap();
    engine
        .db
        .conn()
        .execute(
            "UPDATE meta SET value='2' WHERE key='custom_fields_schema_version'",
            [],
        )
        .unwrap();
    drop(engine);
    assert!(matches!(
        Engine::open(&dir.path().join("fields.db"), &OpenOptions::default()),
        Err(EngineError::UnsupportedVersion {
            found: 2,
            supported: 1
        })
    ));
    let conn = Connection::open(dir.path().join("fields.db")).unwrap();
    assert!(
        conn.query_row(
            "SELECT boolean_value FROM custom_field_values WHERE field_id=?1 AND company_id=?2",
            (id, a.0),
            |row| row.get::<_, bool>(0)
        )
        .unwrap()
    );
}

#[test]
fn deleted_definition_ids_are_not_reassigned_to_new_fields() {
    let (_dir, mut engine, a, _) = fixture();
    let old = engine
        .create_custom_field(&field("旧項目", CustomFieldKind::Text, false))
        .unwrap();
    engine.delete_custom_field(old).unwrap();
    let new = engine
        .create_custom_field(&field("新項目", CustomFieldKind::Text, false))
        .unwrap();
    assert_ne!(old, new);
    assert!(matches!(
        engine.set_custom_value(a, old, &CustomFieldValue::Text("古い画面の入力".into())),
        Err(EngineError::NotFound)
    ));
    assert_eq!(engine.custom_values(a).unwrap().len(), 0);
}
