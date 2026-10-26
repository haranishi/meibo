//! 絞り込み結果の固定リストと、活動を伴う一括変更。

use crate::sql::{OptionalExtension, Transaction, params_from_iter, types::Value};

use crate::{
    schema::{Db, WRITE_BASE},
    types::{BulkChange, EngineError, ListId, OwnerChange, Status},
    write::{now_secs, reject_nul},
};

fn storage(error: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}

const INSERT_LIST: &str = "INSERT INTO lists(name,created_at) VALUES(?1,?2)";
const INSERT_MEMBERS: &str =
    "INSERT INTO list_members(list_id,company_id) SELECT ?,id FROM ({select_sql})";
const DELETE_LIST: &str = "DELETE FROM lists WHERE id=?1";
const FIND_LIST: &str = "SELECT id FROM lists WHERE id=?1";
const COUNT_TARGETS: &str = "SELECT count(*) FROM companies WHERE {TARGET}";
const HAS_EXCLUDED_TARGET: &str =
    "SELECT EXISTS(SELECT 1 FROM companies WHERE {TARGET} AND status=5)";
const UPDATE_OWNER: &str = "UPDATE companies SET owner_id=?3,updated_at=?4 WHERE {TARGET}";
const COUNT_CHANGED_GROUPS: &str =
    "SELECT status,has_phone,count(*) FROM companies WHERE {condition} GROUP BY status,has_phone";
const DECREMENT_COUNT: &str = "UPDATE counts SET n=n-?3 WHERE status=?1 AND has_phone=?2 AND n>=?3";
const INCREMENT_COUNT: &str = "UPDATE counts SET n=n+?3 WHERE status=?1 AND has_phone=?2";
const INSERT_STATUS_ACTIVITIES: &str = "INSERT INTO activities(company_id,kind,at,status_after,next_action_kind) SELECT id,5,?4,?3,0 FROM companies WHERE {condition}";
const UPDATE_OWNER_SAME_STATUS: &str =
    "UPDATE companies SET owner_id=?5,updated_at=?4 WHERE {TARGET} AND status=?3";
const UPDATE_OWNER_AND_STATUS: &str =
    "UPDATE companies SET owner_id=?5,status=?3,updated_at=?4 WHERE {condition}";
const UPDATE_STATUS: &str = "UPDATE companies SET status=?3,updated_at=?4 WHERE {condition}";

pub(crate) fn create_list_estimate(name: &str, limit: u32) -> u64 {
    WRITE_BASE
        .saturating_add(u64::from(limit) * 64)
        .saturating_add((name.len() as u64).saturating_mul(4))
}

pub(crate) fn delete_list_estimate(members: u64) -> u64 {
    WRITE_BASE.saturating_add(members.saturating_mul(32))
}

pub(crate) fn bulk_update_estimate(members: u64, average_row_bytes: u64) -> u64 {
    // 445万社の密な分布ではWALが1社約2〜2.7KB。余裕を見て1ページ分。
    WRITE_BASE.saturating_add(members.saturating_mul(4096.max(average_row_bytes.saturating_mul(3))))
}

pub(crate) fn average_company_row_bytes(db: &Db) -> Result<u64, EngineError> {
    let pages: u32 = db
        .conn()
        .pragma_query_value(None, "page_count", |r| r.get(0))
        .map_err(storage)?;
    let size: u32 = db
        .conn()
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(storage)?;
    // 索引・活動も含め、安全側に見積る。企業表の全走査を避ける。
    let companies: i64 = db
        .conn()
        .query_row("SELECT coalesce(sum(n),0) FROM counts", [], |r| r.get(0))
        .map_err(storage)?;
    let companies = u64::try_from(companies).map_err(|_| EngineError::Corrupt)?;
    Ok(u64::from(pages).saturating_mul(u64::from(size)) / companies.max(1))
}

pub(crate) fn create_list_from_select(
    db: &mut Db,
    name: &str,
    select_sql: &str,
    params: &[Value],
    limit: u32,
) -> Result<ListId, EngineError> {
    reject_nul("name", name)?;
    for (field, invalid, reason) in [
        (
            "limit",
            !(1..=100_000).contains(&limit),
            "must be 1..=100000",
        ),
        ("name", name.trim().is_empty(), "must not be empty"),
    ] {
        if invalid {
            return Err(EngineError::InvalidInput {
                field: field.into(),
                reason: reason.into(),
            });
        }
    }
    // SQL は query::select_ids_sql の内部出力だけを受け取る。
    // リストの構成員1社につき64Bを見込む。
    db.write(create_list_estimate(name, limit), |tx| {
        tx.execute(INSERT_LIST, (name, now_secs()))
            .map_err(storage)?;
        let id = ListId(tx.last_insert_rowid());
        let sql = INSERT_MEMBERS.replace("{select_sql}", select_sql);
        let values = std::iter::once(Value::Integer(id.0)).chain(params.iter().cloned());
        tx.execute(&sql, params_from_iter(values))
            .map_err(storage)?;
        Ok(id)
    })
}

pub(crate) fn delete_list(db: &mut Db, id: ListId) -> Result<(), EngineError> {
    let members: i64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM list_members WHERE list_id=?1",
            [id.0],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let members = u64::try_from(members).map_err(|_| EngineError::Corrupt)?;
    db.write(delete_list_estimate(members), |tx| {
        if tx.execute(DELETE_LIST, [id.0]).map_err(storage)? == 0 {
            return Err(EngineError::NotFound);
        }
        Ok(())
    })
}

// list_id、除外の指定の順。消去済みは明示指定でも対象にしない。
const TARGET: &str = "id IN (SELECT company_id FROM list_members WHERE list_id=?1) AND erased_at IS NULL AND (?2 OR status<>5)";

fn owner_value(owner: OwnerChange) -> Option<i64> {
    match owner {
        OwnerChange::Set(id) => Some(id.0),
        _ => None,
    }
}

/// 状態の変更。件数表の差分、状態変更の活動の追記、企業の更新（担当営業の変更も同時に）。
fn change_status(
    tx: &Transaction<'_>,
    list: ListId,
    change: &BulkChange,
    status: Status,
    now: i64,
) -> Result<(), EngineError> {
    let condition = "{TARGET} AND status<>?3".replace("{TARGET}", TARGET);
    let args = (list.0, change.include_excluded, status.code());
    let mut stmt = tx
        .prepare(&COUNT_CHANGED_GROUPS.replace("{condition}", &condition))
        .map_err(storage)?;
    let groups = stmt
        .query_map(args, |r| {
            Ok((
                r.get::<_, u8>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })
        .map_err(storage)?
        .collect::<crate::sql::Result<Vec<_>>>()
        .map_err(storage)?;
    // 最大12組の差分だけを反映し、企業単位の往復を避ける。
    for (old, phone, count) in groups {
        if tx
            .execute(DECREMENT_COUNT, (old, phone, count))
            .map_err(storage)?
            != 1
        {
            return Err(EngineError::Corrupt);
        }
        if tx
            .execute(INCREMENT_COUNT, (status.code(), phone, count))
            .map_err(storage)?
            != 1
        {
            return Err(EngineError::Corrupt);
        }
    }
    tx.execute(
        &INSERT_STATUS_ACTIVITIES.replace("{condition}", &condition),
        (list.0, change.include_excluded, status.code(), now),
    )
    .map_err(storage)?;
    if change.owner == OwnerChange::Keep {
        tx.execute(
            &UPDATE_STATUS.replace("{condition}", &condition),
            (list.0, change.include_excluded, status.code(), now),
        )
        .map_err(storage)?;
        return Ok(());
    }
    // 1社を1回の更新にする。同じ状態の企業は担当営業だけ、状態が変わる企業は両方を同時に。
    let owner = owner_value(change.owner);
    tx.execute(
        &UPDATE_OWNER_SAME_STATUS.replace("{TARGET}", TARGET),
        (list.0, change.include_excluded, status.code(), now, owner),
    )
    .map_err(storage)?;
    tx.execute(
        &UPDATE_OWNER_AND_STATUS.replace("{condition}", &condition),
        (list.0, change.include_excluded, status.code(), now, owner),
    )
    .map_err(storage)?;
    Ok(())
}

pub(crate) fn bulk_update(
    db: &mut Db,
    list: ListId,
    change: &BulkChange,
) -> Result<u64, EngineError> {
    let members: i64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM list_members WHERE list_id=?1",
            [list.0],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let members = u64::try_from(members).map_err(|_| EngineError::Corrupt)?;
    let estimate = bulk_update_estimate(members, average_company_row_bytes(db)?);
    db.write(estimate, |tx| {
        tx.query_row(FIND_LIST, [list.0], |r| r.get::<_, i64>(0))
            .optional()
            .map_err(storage)?
            .ok_or(EngineError::NotFound)?;
        let target = (list.0, change.include_excluded);
        let n: i64 = tx
            .query_row(&COUNT_TARGETS.replace("{TARGET}", TARGET), target, |r| {
                r.get(0)
            })
            .map_err(storage)?;
        if change.include_excluded && change.status.is_some_and(|s| s != Status::Excluded) {
            let excluded: bool = tx
                .query_row(
                    &HAS_EXCLUDED_TARGET.replace("{TARGET}", TARGET),
                    target,
                    |r| r.get(0),
                )
                .map_err(storage)?;
            if excluded {
                return Err(EngineError::ExcludedCompany);
            }
        }
        let now = now_secs();
        match change.status {
            Some(status) => change_status(tx, list, change, status, now)?,
            None if change.owner != OwnerChange::Keep => {
                tx.execute(
                    &UPDATE_OWNER.replace("{TARGET}", TARGET),
                    (
                        list.0,
                        change.include_excluded,
                        owner_value(change.owner),
                        now,
                    ),
                )
                .map_err(storage)?;
            }
            None => {}
        }
        u64::try_from(n).map_err(|_| EngineError::Corrupt)
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::sql::Connection;
    use crate::{
        query::select_ids_sql,
        schema::{OpenOptions, counts, fts},
        types::{
            ActivityInput, ActivityKind, CompanyId, DataOrigin, Filters, NextAction, Region, UserId,
        },
        write,
    };
    use tempfile::TempDir;

    pub(crate) fn database() -> (TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::create(
            &dir.path().join("synthetic.db"),
            DataOrigin::Synthetic,
            &OpenOptions::default(),
        )
        .unwrap();
        (dir, db)
    }

    pub(crate) fn seed(db: &mut Db, n: u32) {
        db.write(WRITE_BASE + u64::from(n) * 512, |tx| {
            tx.execute("WITH RECURSIVE seq(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM seq WHERE i<?1) INSERT INTO companies(id,name,name_key,prefecture_code,phone,owner_id,is_sole_proprietor,created_at,updated_at) SELECT i,'synthetic'||i,'synthetic'||i,13,CASE WHEN i%2=0 THEN '0123' END,7,1,10,10 FROM seq", [n]).map_err(storage)?;
            counts::recount(tx)?;
            fts::rebuild(tx)
        }).unwrap();
    }

    pub(crate) fn scalar(db: &Db, sql: &str) -> i64 {
        db.conn().query_row(sql, [], |r| r.get(0)).unwrap()
    }

    pub(crate) fn activity(db: &mut Db, id: i64, status: Option<Status>) {
        write::record_activity(
            db,
            &ActivityInput {
                company: CompanyId(id),
                contact: None,
                kind: ActivityKind::Call,
                at: 100,
                note: Some("synthetic note".into()),
                status_after: status,
                next_action: NextAction::Set(500),
                reverses: None,
            },
        )
        .unwrap();
    }

    fn list(db: &mut Db, include_excluded: bool) -> ListId {
        let (sql, args) = select_ids_sql(
            &Filters {
                include_excluded,
                ..Filters::default()
            },
            100_000,
        )
        .unwrap();
        create_list_from_select(db, "synthetic list", &sql, &args, 100_000).unwrap()
    }

    pub(crate) fn consistent(db: &Db) {
        assert!(counts::matches_recount(db.conn()));
        assert!(write::verify_folds(db.conn()).unwrap());
    }

    pub(crate) fn snapshot(conn: &Connection) -> Vec<Vec<Vec<Value>>> {
        [
            "companies",
            "contacts",
            "activities",
            "counts",
            "lists",
            "list_members",
            "erase_gate",
        ]
        .iter()
        .map(|table| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM {table} ORDER BY 1"))
                .unwrap();
            let columns = stmt.column_count();
            stmt.query_map([], |r| {
                (0..columns)
                    .map(|i| r.get(i))
                    .collect::<crate::sql::Result<Vec<Value>>>()
            })
            .unwrap()
            .collect::<crate::sql::Result<Vec<_>>>()
            .unwrap()
        })
        .collect()
    }

    // REQ 6.1
    // REQ 6.3
    // REQ 6.4
    #[test]
    fn lists_freeze_declared_and_fallback_selections_and_delete_only_members() {
        let (_dir, mut db) = database();
        seed(&mut db, 4);
        activity(&mut db, 4, Some(Status::Excluded));
        for filters in [
            Filters {
                region: vec![Region::Prefecture(13)],
                ..Filters::default()
            },
            Filters {
                region: vec![Region::Prefecture(13)],
                owner: vec![UserId(7)],
                ..Filters::default()
            },
        ] {
            let (sql, args) = select_ids_sql(&filters, 2).unwrap();
            let id = create_list_from_select(&mut db, "list ' ?", &sql, &args, 2).unwrap();
            assert_eq!(scalar(&db, "SELECT count(*) FROM list_members"), 2);
            assert_eq!(
                scalar(&db, "SELECT count(*) FROM list_members WHERE company_id=4"),
                0
            );
            delete_list(&mut db, id).unwrap();
        }
        let a = list(&mut db, false);
        let b = list(&mut db, true);
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM list_members WHERE company_id=1"),
            2
        );
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM list_members WHERE company_id=4"),
            1
        );
        activity(&mut db, 1, Some(Status::Excluded));
        assert_eq!(scalar(&db, "SELECT count(*) FROM list_members"), 7);
        delete_list(&mut db, a).unwrap();
        delete_list(&mut db, b).unwrap();
        assert_eq!(scalar(&db, "SELECT count(*) FROM list_members"), 0);
        assert_eq!(scalar(&db, "SELECT count(*) FROM companies"), 4);
        assert_eq!(scalar(&db, "SELECT count(*) FROM activities"), 2);
        assert!(matches!(
            delete_list(&mut db, a),
            Err(EngineError::NotFound)
        ));
        consistent(&db);
    }

    // REQ 6.1
    #[test]
    fn invalid_list_input_and_failed_selection_leave_no_list() {
        let (_dir, mut db) = database();
        for limit in [0, 100_001, u32::MAX] {
            assert!(
                matches!(create_list_from_select(&mut db,"list","SELECT id FROM companies",&[],limit),Err(EngineError::InvalidInput{field,..}) if field=="limit")
            );
        }
        assert!(
            matches!(create_list_from_select(&mut db," \t","SELECT id FROM companies",&[],1),Err(EngineError::InvalidInput{field,..}) if field=="name")
        );
        assert!(
            create_list_from_select(&mut db, "list", "SELECT id FROM missing", &[], 1).is_err()
        );
        assert_eq!(scalar(&db, "SELECT count(*) FROM lists"), 0);
    }

    // REQ 6.2
    // REQ 6.4
    // REQ 6.5
    #[test]
    fn bulk_owners_statuses_exclusion_and_same_status() {
        let (_dir, mut db) = database();
        seed(&mut db, 4);
        activity(&mut db, 1, None);
        activity(&mut db, 3, Some(Status::Excluded));
        crate::erase::erase_company(&mut db, CompanyId(4)).unwrap();
        let id = list(&mut db, true);
        let mut change = BulkChange {
            owner: OwnerChange::Keep,
            status: Some(Status::Won),
            include_excluded: false,
        };
        assert_eq!(bulk_update(&mut db, id, &change).unwrap(), 2);
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM companies WHERE owner_id=7 AND erased_at IS NULL"
            ),
            3
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE kind=5 AND status_after=3"
            ),
            2
        );
        assert_eq!(
            scalar(&db, "SELECT last_activity_at FROM companies WHERE id=1"),
            100
        );
        assert_eq!(
            scalar(&db, "SELECT next_action_at FROM companies WHERE id=1"),
            500
        );
        change.owner = OwnerChange::Clear;
        assert_eq!(bulk_update(&mut db, id, &change).unwrap(), 2);
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE kind=5 AND status_after=3"
            ),
            2
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM companies WHERE id IN(1,2) AND owner_id IS NULL"
            ),
            2
        );
        change.owner = OwnerChange::Set(UserId(9));
        change.include_excluded = true;
        let before = snapshot(db.conn());
        assert!(matches!(
            bulk_update(&mut db, id, &change),
            Err(EngineError::ExcludedCompany)
        ));
        assert_eq!(snapshot(db.conn()), before);
        change.status = None;
        assert_eq!(bulk_update(&mut db, id, &change).unwrap(), 3);
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM companies WHERE owner_id=9"),
            3
        );
        change.status = Some(Status::Excluded);
        assert_eq!(bulk_update(&mut db, id, &change).unwrap(), 3);
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE kind=5 AND status_after=5"
            ),
            3
        );
        assert!(matches!(
            bulk_update(&mut db, ListId(999), &change),
            Err(EngineError::NotFound)
        ));
        consistent(&db);
    }

    // REQ 6.2
    #[test]
    fn combined_owner_status_updates_each_company_once() {
        let (_dir, mut db) = database();
        seed(&mut db, 4);
        activity(&mut db, 1, Some(Status::Won));
        let id = list(&mut db, false);
        db.conn().execute_batch("CREATE TEMP TABLE updated(company_id INTEGER PRIMARY KEY); CREATE TEMP TRIGGER once_per_company AFTER UPDATE ON companies BEGIN INSERT INTO updated VALUES(NEW.id); END;").unwrap();
        assert_eq!(
            bulk_update(
                &mut db,
                id,
                &BulkChange {
                    owner: OwnerChange::Set(UserId(99)),
                    status: Some(Status::Won),
                    include_excluded: false
                }
            )
            .unwrap(),
            4
        );
        assert_eq!(scalar(&db, "SELECT count(*) FROM updated"), 4);
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM companies WHERE owner_id=99 AND status=3"
            ),
            4
        );
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM activities WHERE kind=5"),
            3
        );
        assert_eq!(
            scalar(
                &db,
                "SELECT count(*) FROM activities WHERE kind=5 AND company_id=1"
            ),
            0
        );
        consistent(&db);
    }

    // REQ 6.2
    #[test]
    fn bulk_request_without_any_change_keeps_every_company_as_is() {
        let (_dir, mut db) = database();
        seed(&mut db, 4);
        activity(&mut db, 2, Some(Status::Won));
        let id = list(&mut db, false);
        let before = snapshot(db.conn());
        assert_eq!(
            bulk_update(
                &mut db,
                id,
                &BulkChange {
                    owner: OwnerChange::Keep,
                    status: None,
                    include_excluded: false
                }
            )
            .unwrap(),
            4
        );
        // 担当営業・状態・更新日時・活動・件数表のどれも変わらない。
        assert_eq!(snapshot(db.conn()), before);
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM companies WHERE owner_id=7"),
            4
        );
        consistent(&db);
    }

    // REQ 6.2
    #[test]
    fn bulk_failure_rolls_back_owners_activities_companies_and_counts() {
        let (_dir, mut db) = database();
        seed(&mut db, 3);
        let id = list(&mut db, false);
        db.conn().execute_batch("CREATE TEMP TRIGGER fail_bulk BEFORE UPDATE OF status ON companies WHEN NEW.id=2 BEGIN SELECT RAISE(ABORT,'injected'); END").unwrap();
        let before = snapshot(db.conn());
        let change = BulkChange {
            owner: OwnerChange::Set(UserId(99)),
            status: Some(Status::Won),
            include_excluded: false,
        };
        assert!(bulk_update(&mut db, id, &change).is_err());
        assert_eq!(snapshot(db.conn()), before);
        consistent(&db);
    }

    // REQ 6.1
    // REQ 6.2
    #[test]
    fn hundred_thousand_list_and_status_update_timing() {
        let (_dir, mut db) = database();
        seed(&mut db, 100_000);
        let (sql, args) = select_ids_sql(&Filters::default(), 100_000).unwrap();
        let start = std::time::Instant::now();
        let id = create_list_from_select(&mut db, "100k", &sql, &args, 100_000).unwrap();
        eprintln!("Stage 3D list 100000: {:?} (guide <1s)", start.elapsed());
        assert_eq!(scalar(&db, "SELECT count(*) FROM list_members"), 100_000);
        let start = std::time::Instant::now();
        assert_eq!(
            bulk_update(
                &mut db,
                id,
                &BulkChange {
                    owner: OwnerChange::Set(UserId(8)),
                    status: Some(Status::Won),
                    include_excluded: false
                }
            )
            .unwrap(),
            100_000
        );
        eprintln!(
            "Stage 3D bulk status 100000: {:?} (guide <2s)",
            start.elapsed()
        );
        assert_eq!(
            scalar(&db, "SELECT count(*) FROM activities WHERE kind=5"),
            100_000
        );
        consistent(&db);
    }
    #[test]
    #[ignore = "Stage 4E-1 合成10万社の担当営業＋状態変更の計測"]
    fn bulk_owner_status_100k_benchmark() {
        hundred_thousand_list_and_status_update_timing();
    }
}
