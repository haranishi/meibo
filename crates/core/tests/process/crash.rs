use super::support::*;
use meibo_core::{Durability, Engine, OpenOptions, types::*};
use std::{path::Path, thread, time::Duration};

fn activity(index: usize, target: Option<ActivityId>) -> ActivityInput {
    let mut input = memo(
        CompanyId(i64::try_from(index / 5 % 2_000 + 1).unwrap()),
        i64::try_from(index + 1).unwrap(),
    );
    match index % 5 {
        0 => {
            input.kind = ActivityKind::Call;
            input.status_after = Some(Status::InProgress);
            input.next_action = NextAction::Set(10_000);
        }
        1 => {}
        2 => {
            input.kind = ActivityKind::StatusChange;
            input.status_after = Some(Status::Negotiating);
            input.next_action = NextAction::Clear;
        }
        3 => {
            input.next_action = NextAction::Set(20_000);
        }
        _ => {
            input.kind = ActivityKind::Cancel;
            input.reverses = target;
        }
    }
    input
}

// REQ 5.7
// REQ 5.2
#[test]
fn activity_child() {
    let Some(path) = std::env::var_os(DB_ENV) else {
        return;
    };
    let options = OpenOptions {
        durability: if std::env::var_os("MEIBO_PROCESS_FULL").is_some() {
            Durability::Full
        } else {
            Durability::Normal
        },
        ..OpenOptions::default()
    };
    let mut db = Engine::open(Path::new(&path), &options).unwrap();
    let mut target = None;
    let mut index = 0;
    loop {
        let id = db.record_activity(&activity(index, target)).unwrap();
        if index % 5 == 0 {
            target = Some(id);
        }
        emit(&format!("ACK {}", id.0));
        index += 1;
    }
}

// REQ 5.7
// REQ 5.2
#[test]
fn acknowledged_activities_survive_kill() {
    for round in 0..12 {
        let n = [1, 10, 100, 1_000][round % 4];
        let full = round >= 8;
        let delay = [0, 1, 5, 20][round % 4];
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crash.db");
        seed(&path, 2_000, true);
        let extra = if full {
            vec![("MEIBO_PROCESS_FULL", "1")]
        } else {
            vec![]
        };
        let mut child = ChildGuard::spawn("crash::activity_child", &path, &extra);
        let mut ack = Vec::new();
        for _ in 0..n {
            ack.push(child.receive("ACK ").parse::<i64>().unwrap());
        }
        thread::sleep(Duration::from_millis(delay));
        child.kill();
        for line in child.drain() {
            if let Some((_, value)) = line.split_once("ACK ") {
                ack.push(value.parse().unwrap());
            }
        }
        let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
        let actual = activities(&db, 2_000);
        eprintln!(
            "CRASH round={} full={full} threshold={n} delay_ms={delay} ack={} stored={}",
            round + 1,
            ack.len(),
            actual.len()
        );
        assert!((ack.len()..=ack.len() + 1).contains(&actual.len()));
        for (index, saved) in actual.iter().enumerate() {
            if index < ack.len() {
                assert_eq!(saved.id.0, ack[index]);
            }
            let target = if index % 5 == 4 {
                Some(actual[index - 4].id)
            } else {
                None
            };
            let expected = activity(index, target);
            assert_eq!(
                (saved.company, saved.contact, saved.kind, saved.at),
                (
                    expected.company,
                    expected.contact,
                    expected.kind,
                    expected.at
                )
            );
            assert_eq!(
                (
                    &saved.note,
                    saved.status_after,
                    saved.next_action,
                    saved.reverses,
                    saved.note_erased_at
                ),
                (
                    &expected.note,
                    expected.status_after,
                    expected.next_action,
                    expected.reverses,
                    None
                )
            );
        }
        verify(&db);
        db.record_activity(&memo(CompanyId(2_000), 99_999)).unwrap();
        assert_eq!(activities(&db, 2_000).len(), actual.len() + 1);
        verify(&db);
    }
}

// REQ 6.2
// REQ 5.7
#[test]
fn bulk_child() {
    let Some(path) = std::env::var_os(DB_ENV) else {
        return;
    };
    let list = ListId(
        std::env::var("MEIBO_PROCESS_LIST")
            .unwrap()
            .parse()
            .unwrap(),
    );
    let mut db = Engine::open(Path::new(&path), &OpenOptions::default()).unwrap();
    emit("STARTED");
    assert_eq!(
        db.bulk_update(
            list,
            &BulkChange {
                owner: OwnerChange::Keep,
                status: Some(Status::Excluded),
                include_excluded: true
            }
        )
        .unwrap(),
        std::env::var("MEIBO_PROCESS_ROWS").map_or(100_000, |n| n.parse::<u64>().unwrap())
    );
    emit("COMMITTED");
    // 完了後も親の強制終了を待つ。
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();
}

// REQ 6.2
// REQ 5.7
#[test]
fn bulk_kill_is_all_or_nothing() {
    for (round, delay) in [0, 30, 150, 300].into_iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bulk.db");
        seed(&path, 100_000, false);
        let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
        let list = db
            .create_list("synthetic", &query().filters, 100_000)
            .unwrap();
        let before = statuses(&db);
        drop(db);
        let list_text = list.0.to_string();
        let mut child = ChildGuard::spawn(
            "crash::bulk_child",
            &path,
            &[("MEIBO_PROCESS_LIST", &list_text)],
        );
        child.receive("STARTED");
        thread::sleep(Duration::from_millis(delay));
        child.kill();
        child.drain();
        let db = Engine::open(&path, &OpenOptions::default()).unwrap();
        let after = statuses(&db);
        assert_eq!(after.len(), 100_000);
        assert!(after == before || after.iter().all(|(_, s)| *s == Status::Excluded));
        verify(&db);
        for status in [
            Status::Untouched,
            Status::InProgress,
            Status::Negotiating,
            Status::Won,
            Status::Lost,
            Status::Excluded,
        ] {
            let counted = db
                .count(&Filters {
                    status: vec![status],
                    include_excluded: true,
                    ..Filters::default()
                })
                .unwrap()
                .n;
            assert_eq!(
                counted,
                u64::try_from(after.iter().filter(|(_, s)| *s == status).count()).unwrap()
            );
        }
        eprintln!(
            "BULK round={} delay_ms={delay} all_excluded={}",
            round + 1,
            after.iter().all(|(_, s)| *s == Status::Excluded)
        );
    }
}

fn statuses(db: &Engine) -> Vec<(CompanyId, Status)> {
    let mut q = query();
    let mut rows = Vec::new();
    loop {
        let page = db.list(&q).unwrap();
        rows.extend(page.rows.into_iter().map(|c| (c.id, c.status)));
        q.after = page.next;
        if q.after.is_none() {
            return rows;
        }
    }
}

// REQ 5.7, 6.2
#[test]
fn acknowledged_bulk_survives_kill() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("committed-bulk.db");
    seed(&path, 20_000, false);
    let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
    let list = db
        .create_list("synthetic", &query().filters, 20_000)
        .unwrap();
    drop(db);
    let list_text = list.0.to_string();
    let mut child = ChildGuard::spawn(
        "crash::bulk_child",
        &path,
        &[
            ("MEIBO_PROCESS_LIST", &list_text),
            ("MEIBO_PROCESS_ROWS", "20000"),
        ],
    );
    child.receive("COMMITTED");
    child.kill();
    child.drain();
    let db = Engine::open(&path, &OpenOptions::default()).unwrap();
    let after = statuses(&db);
    assert_eq!(after.len(), 20_000);
    assert!(after.iter().all(|(_, status)| *status == Status::Excluded));
    verify(&db);
}
