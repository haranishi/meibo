use super::*;
use crate::schema::{OpenOptions, counts, fts};
use crate::types::PERSONAL_FIELDS;
use std::{collections::HashSet, time::Instant};

pub(crate) fn fixture(size: i64) -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::create(
        &dir.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let names = [
        "合成東京建設",
        "合成とうきょう商事",
        "合成トウキョウ工業",
        "合成ＡＢＣ１２３",
        "合成abc123",
        "合成あい",
        "合成アイ",
        "合成北",
        "合成a\"bc商事",
        "合成a b商事",
    ];
    let mut rng = 37;
    db.write(0, |tx| {
        let mut insert = tx.prepare("INSERT INTO companies(id,name,name_key,prefecture_code,municipality_code,industry_code,status,phone,website,owner_id,last_activity_at,next_action_at,is_sole_proprietor,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)").unwrap();
        for id in 1..=size {
            let prefecture = if random(&mut rng).is_multiple_of(3) { i64::try_from(random(&mut rng)%47+1).unwrap() } else { 13 };
            let municipality = format!("{prefecture:02}{:03}", random(&mut rng)%4+100);
            let industry = (random(&mut rng).is_multiple_of(4)).then(|| i64::try_from(random(&mut rng)%20+1).unwrap());
            let status = if random(&mut rng).is_multiple_of(4) { i64::try_from(random(&mut rng)%6).unwrap() } else { 0 };
            let phone = (random(&mut rng).is_multiple_of(3)).then_some("010-0000");
            let owner = (random(&mut rng).is_multiple_of(4)).then(|| i64::try_from(random(&mut rng)%3+1).unwrap());
            let activity = if random(&mut rng).is_multiple_of(4) { i64::try_from(random(&mut rng)%20+1).unwrap() } else { 0 };
            let next = (random(&mut rng).is_multiple_of(4)).then(|| i64::try_from(random(&mut rng)%20).unwrap()-5);
            let name = format!("{} {id}", names[usize::try_from(random(&mut rng)%10).unwrap()]);
            insert.execute(rusqlite::params![id,name,crate::normalize::name_key(&name),prefecture,municipality,industry,status,phone,"synthetic.example",owner,activity,next,id%7==0,1,1]).unwrap();
        }
        Ok(())
    }).unwrap();
    counts::recount(db.conn()).unwrap();
    fts::rebuild(db.conn()).unwrap();
    db.conn().execute_batch("ANALYZE").unwrap();
    (dir, db)
}

pub(crate) fn random(seed: &mut u64) -> u64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *seed ^ (*seed >> 29)
}
fn query(f: Filters, sort: Sort, limit: u32) -> ListQuery {
    ListQuery {
        filters: f,
        sort,
        after: None,
        limit,
        fields: vec![Field::Name, Field::Phone, Field::Website],
    }
}
fn cases() -> Vec<ListQuery> {
    shapes::SHAPES
        .iter()
        .map(|s| {
            let f = Filters {
                region: if s.filters.contains('P') {
                    vec![Region::Prefecture(13)]
                } else if s.filters.contains('M') {
                    vec![Region::Municipality("13100".into())]
                } else {
                    vec![]
                },
                industry: if s.filters.contains('I') {
                    vec![Industry::A]
                } else {
                    vec![]
                },
                status: if s.filters.contains('S') {
                    vec![Status::Untouched]
                } else {
                    vec![]
                },
                has_phone: s.filters.contains('T').then_some(true),
                owner: if s.filters.contains('O') {
                    vec![UserId(1)]
                } else {
                    vec![]
                },
                next_action_before: s.filters.contains('N').then_some(12),
                ..Filters::default()
            };
            query(
                f,
                if s.number == 21 {
                    Sort::LastActivityDesc
                } else if s.number >= 19 && s.number <= 20 {
                    Sort::NextAction
                } else {
                    Sort::ShapeDefault
                },
                50,
            )
        })
        .collect()
}
fn all(db: &Db) -> Vec<Company> {
    db.conn()
        .prepare("SELECT * FROM companies")
        .unwrap()
        .query_map([], |r| {
            company_row(
                r,
                db.origin(),
                &vec![Field::Name, Field::Phone, Field::Website],
            )
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}
fn matches(c: &Company, f: &Filters) -> bool {
    (f.region.is_empty()
        || f.region.iter().any(|r| match r {
            Region::Prefecture(p) => c.prefecture == *p,
            Region::Municipality(m) => c.municipality.as_ref() == Some(m),
        }))
        && (f.industry.is_empty() || c.industry.is_some_and(|i| f.industry.contains(&i)))
        && (f.status.is_empty() || f.status.contains(&c.status))
        && f.has_phone.is_none_or(|t| t == c.phone.is_some())
        && (f.owner.is_empty() || c.owner.is_some_and(|o| f.owner.contains(&o)))
        && f.next_action_before
            .is_none_or(|n| c.next_action_at.is_some_and(|v| v <= n))
        && (!f.status.is_empty() || f.include_excluded || c.status != Status::Excluded)
}
fn key(c: &Company, col: &str) -> Value {
    match col {
        P => Value::Integer(i64::from(c.prefecture)),
        M => c.municipality.clone().map_or(Value::Null, Value::Text),
        I => c
            .industry
            .map_or(Value::Null, |i| Value::Integer(i64::from(i.code()))),
        S => Value::Integer(i64::from(c.status.code())),
        T => Value::Integer(i64::from(c.phone.is_some())),
        O => c.owner.map_or(Value::Null, |o| Value::Integer(o.0)),
        N => c.next_action_at.map_or(Value::Null, Value::Integer),
        A => Value::Integer(c.last_activity_at),
        ID => Value::Integer(c.id.0),
        _ => unreachable!(),
    }
}
fn reference<'a>(rows: &'a [Company], q: &ListQuery) -> Vec<&'a Company> {
    let cols: &[&str] = match shapes::classify(&q.filters, &q.sort) {
        Shape::Declared(1..=3) => &[P, S, T, ID],
        Shape::Declared(4..=6) => &[P, I, S, T, ID],
        Shape::Declared(7..=9) => &[M, S, T, ID],
        Shape::Declared(10..=12) => &[M, I, S, T, ID],
        Shape::Declared(13..=15) => &[I, S, T, ID],
        Shape::Declared(16..=17) => &[S, T, ID],
        Shape::Declared(18) => &[T, S, ID],
        Shape::Declared(19) => &[O, N, S, ID],
        Shape::Declared(20) => &[N, S, ID],
        Shape::Declared(21) => &[A, S, ID],
        _ => match q.sort {
            Sort::NextAction => &[N, ID],
            Sort::LastActivityDesc => &[A, ID],
            Sort::ShapeDefault => &[ID],
        },
    };
    let mut result: Vec<_> = rows.iter().filter(|c| matches(c, &q.filters)).collect();
    result.sort_by(|a, b| {
        let order = cols
            .iter()
            .map(|col| value_cmp(&key(a, col), &key(b, col)))
            .find(|o| !o.is_eq())
            .unwrap_or(std::cmp::Ordering::Equal);
        if q.sort == Sort::LastActivityDesc {
            order.reverse()
        } else {
            order
        }
    });
    result
}
fn walk(db: &Db, mut q: ListQuery) -> Vec<i64> {
    let mut ids = Vec::new();
    loop {
        let page = list(db, &q).unwrap();
        assert_eq!(
            page.slow,
            shapes::classify(&q.filters, &q.sort) == Shape::Slow
        );
        assert!(page.rows.len() <= q.limit as usize);
        if page.next.is_some() {
            assert_eq!(page.rows.len(), q.limit as usize);
        }
        ids.extend(page.rows.iter().map(|c| c.id.0));
        q.after = page.next;
        if q.after.is_none() {
            break;
        }
        assert!(ids.len() <= 100_000, "cursor did not advance");
    }
    assert_eq!(ids.len(), ids.iter().collect::<HashSet<_>>().len());
    ids
}

// REQ 3.1
// REQ 3.2
// REQ 3.4
#[test]
fn plans_all_22_shapes_at_head_half_and_95_percent() {
    let (_dir, db) = fixture(100_000);
    let rows = all(&db);
    let mut examples = cases();
    let mut p = examples[0].clone();
    p.filters.region = vec![
        Region::Prefecture(14),
        Region::Prefecture(13),
        Region::Prefecture(1),
    ];
    examples.push(p);
    let mut s = examples[4].clone();
    s.filters.status = vec![Status::Won, Status::Untouched, Status::Excluded];
    examples.push(s);
    let mut o = examples[18].clone();
    o.filters.owner = vec![UserId(3), UserId(1)];
    examples.push(o);
    for mut q in examples {
        let Shape::Declared(number) = shapes::classify(&q.filters, &q.sort) else {
            panic!()
        };
        let plan = Plan::new(&q.filters, q.sort).unwrap();
        let expected = reference(&rows, &q);
        assert!(!expected.is_empty(), "shape {number} lacks data");
        for pct in [0, 50, 95] {
            q.after = if pct == 0 {
                None
            } else {
                Some(
                    plan.encode(
                        &plan
                            .columns
                            .iter()
                            .map(|col| key(expected[(expected.len() - 1) * pct / 100], col))
                            .collect::<Vec<_>>(),
                    ),
                )
            };
            let output = explain(&db, &q).unwrap();
            assert_ne!(output, Vec::<String>::new());
            for line in &output {
                assert!(!line.contains("TEMP B-TREE"), "shape {number}: {line}");
                if number <= 20 {
                    assert!(line.starts_with("SEARCH "), "shape {number}: {line}");
                    assert!(
                        line.contains(&format!(
                            "INDEX {} (",
                            shapes::SHAPES[usize::from(number - 1)]
                                .index
                                .to_ascii_lowercase()
                        )),
                        "{line}"
                    );
                    let equal_prefix = shapes::SHAPES[usize::from(number - 1)]
                        .filters
                        .chars()
                        .take_while(|c| *c != 'N')
                        .map(|c| match c {
                            'P' => P,
                            'M' => M,
                            'I' => I,
                            'S' => S,
                            'T' => T,
                            'O' => O,
                            _ => unreachable!(),
                        })
                        .map(|c| format!("{c}=?"))
                        .collect::<Vec<_>>()
                        .join(" AND ");
                    assert!(
                        line.contains(&format!("({equal_prefix}")),
                        "shape {number}: {line}"
                    );
                }
            }
            eprintln!("shape {number:02} depth {pct:02}%: {}", output.join(" | "));
            let got = list(&db, &q).unwrap();
            let start = if pct == 0 {
                0
            } else {
                (expected.len() - 1) * pct / 100 + 1
            };
            assert_eq!(
                got.rows.iter().map(|c| c.id.0).collect::<Vec<_>>(),
                expected
                    .iter()
                    .skip(start)
                    .take(q.limit as usize)
                    .map(|c| c.id.0)
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(count(&db, &q.filters).unwrap().n, expected.len() as u64);
    }
}

// REQ 3.1
// REQ 3.2
// REQ 3.4
// REQ 3.5
// REQ 6.4
#[test]
fn randomized_pages_counts_and_selection_match_memory() {
    let (_dir, db) = fixture(1600);
    let rows = all(&db);
    let mut rng = 173;
    let mut examples = cases();
    for _ in 0..120 {
        let mut q = cases().remove(usize::try_from(random(&mut rng) % 22).unwrap());
        q.limit = u32::try_from(random(&mut rng) % 200 + 1).unwrap();
        q.filters.include_excluded = random(&mut rng).is_multiple_of(2);
        if random(&mut rng).is_multiple_of(2) {
            q.filters.status = vec![
                Status::Excluded,
                Status::Won,
                Status::Untouched,
                Status::Untouched,
            ];
        }
        if random(&mut rng).is_multiple_of(3) {
            q.filters.region = vec![Region::Prefecture(14), Region::Prefecture(13)];
        }
        if random(&mut rng).is_multiple_of(7) {
            q.filters.region.push(Region::Municipality("13100".into()));
        }
        if random(&mut rng).is_multiple_of(3) {
            q.filters.industry = vec![Industry::B, Industry::A];
        }
        if random(&mut rng).is_multiple_of(3) {
            q.filters.owner = vec![UserId(2), UserId(1)];
        }
        if random(&mut rng).is_multiple_of(5) {
            q.sort = Sort::NextAction;
        }
        examples.push(q);
    }
    for q in examples {
        let want = reference(&rows, &q)
            .iter()
            .map(|c| c.id.0)
            .collect::<Vec<_>>();
        assert_eq!(walk(&db, q.clone()), want, "{q:?}");
        let mut cq = q.clone();
        cq.sort = filter_sort(&cq.filters);
        let counted = count(&db, &q.filters).unwrap();
        assert_eq!(counted.n, want.len() as u64);
        assert_eq!(
            counted.slow,
            shapes::classify(&cq.filters, &cq.sort) == Shape::Slow
        );
        let (sql, args) = select_ids_sql(&q.filters, 100_000).unwrap();
        let got = db
            .conn()
            .prepare(&sql)
            .unwrap()
            .query_map(params_from_iter(args), |r| r.get::<_, i64>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(
            got,
            reference(&rows, &cq)
                .iter()
                .map(|c| c.id.0)
                .collect::<Vec<_>>()
        );
    }
    for f in [
        Filters::default(),
        Filters {
            include_excluded: true,
            ..Filters::default()
        },
        Filters {
            has_phone: Some(false),
            include_excluded: true,
            ..Filters::default()
        },
        Filters {
            status: vec![Status::Untouched, Status::Excluded],
            has_phone: Some(false),
            ..Filters::default()
        },
    ] {
        assert_eq!(
            count(&db, &f).unwrap().n,
            rows.iter().filter(|c| matches(c, &f)).count() as u64
        );
    }
}

// REQ 3.3
#[test]
fn updates_preserve_every_unchanged_sort_key_once() {
    let (_dir, db) = fixture(800);
    let mut q = cases().remove(0);
    q.limit = 13;
    let original = all(&db);
    let unchanged = reference(&original, &q)
        .iter()
        .filter(|c| c.id.0 % 11 != 0)
        .map(|c| c.id.0)
        .collect::<Vec<_>>();
    let first = list(&db, &q).unwrap();
    let mut ids = first.rows.iter().map(|c| c.id.0).collect::<Vec<_>>();
    db.conn()
        .execute("UPDATE companies SET website='synthetic.changed'", [])
        .unwrap();
    db.conn()
        .execute("UPDATE companies SET status=(status+1)%5 WHERE id%11=0", [])
        .unwrap();
    q.after = first.next;
    ids.extend(walk(&db, q));
    assert_eq!(
        ids.into_iter()
            .filter(|id| id % 11 != 0)
            .collect::<Vec<_>>(),
        unchanged
    );
}

// REQ 3.6
#[test]
fn invalid_inputs_and_cursors_are_rejected() {
    let (_dir, db) = fixture(30);
    let base = cases().remove(0);
    for region in [
        Region::Prefecture(0),
        Region::Prefecture(48),
        Region::Municipality("1310".into()),
        Region::Municipality("１２３４５".into()),
        Region::Municipality("13A01".into()),
    ] {
        let mut q = base.clone();
        q.filters.region = vec![region];
        assert!(matches!(
            list(&db, &q),
            Err(EngineError::InvalidInput { .. })
        ));
        assert!(matches!(
            count(&db, &q.filters),
            Err(EngineError::InvalidInput { .. })
        ));
        assert!(select_ids_sql(&q.filters, 10).is_err());
    }
    for limit in [0, 100_001, u32::MAX] {
        assert!(select_ids_sql(&base.filters, limit).is_err(), "{limit}");
    }
    for limit in [1, 100_000] {
        assert!(select_ids_sql(&base.filters, limit).is_ok(), "{limit}");
    }
    for limit in [0, 201] {
        let mut q = base.clone();
        q.limit = limit;
        assert!(matches!(
            list(&db, &q),
            Err(EngineError::InvalidInput { .. })
        ));
    }
    for token in [
        "broken",
        "v1:2:13,0,0,1",
        "v1:1:14,0,0,1",
        "v1:1:13,9,0,1",
        "v1:1:13,0,2,1",
        "v1:1:13,5,0,1",
        "v1:1:13,0,0",
        "v1:1:13,0,0,overflow",
    ] {
        let mut q = base.clone();
        q.after = Some(Cursor {
            token: token.into(),
        });
        assert!(
            matches!(list(&db,&q),Err(EngineError::InvalidInput{field,..}) if field=="after"),
            "{token}"
        );
    }
}

// REQ 2.2
// REQ 7.1
#[test]
fn detail_order_privacy_and_not_found() {
    let (_dir, db) = fixture(14);
    db.conn().execute_batch("INSERT INTO contacts(id,company_id,name,phone,email,created_at) VALUES(1,7,'synthetic name','010','synthetic@example.invalid',1),(2,7,'synthetic second','011','second@example.invalid',1); INSERT INTO activities(id,company_id,kind,at,next_action_kind,next_action_at) VALUES(1,7,4,10,0,NULL),(2,7,1,20,1,NULL),(3,7,2,20,2,42);").unwrap();
    let d = company_detail(&db, CompanyId(7), &vec![]).unwrap();
    assert_eq!(
        (d.company.name, d.company.phone, d.company.website),
        (None, None, None)
    );
    assert!(
        d.contacts
            .iter()
            .all(|c| c.name.is_none() && c.phone.is_none() && c.email.is_none())
    );
    assert_eq!(
        d.contacts.iter().map(|c| c.id.0).collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(
        d.activities.iter().map(|a| a.id.0).collect::<Vec<_>>(),
        [3, 2, 1]
    );
    assert_eq!(d.activities[0].next_action, NextAction::Set(42));
    let fields = PERSONAL_FIELDS.to_vec();
    let d = company_detail(&db, CompanyId(7), &fields).unwrap();
    assert!(d.company.name.is_some() && d.company.website.is_some());
    assert!(
        d.contacts
            .iter()
            .all(|c| c.name.is_some() && c.phone.is_some() && c.email.is_some())
    );
    db.conn()
        .execute("UPDATE contacts SET erased_at=2 WHERE id=2", [])
        .unwrap();
    assert!(
        company_detail(&db, CompanyId(7), &fields).unwrap().contacts[0]
            .name
            .is_none()
    );
    let regular = company_detail(&db, CompanyId(1), &vec![]).unwrap().company;
    assert!(regular.name.is_some() && regular.website.is_some());
    let mut q = query(
        Filters {
            include_excluded: true,
            ..Filters::default()
        },
        Sort::ShapeDefault,
        20,
    );
    q.fields.clear();
    assert!(
        list(&db, &q)
            .unwrap()
            .rows
            .iter()
            .filter(|c| c.is_sole_proprietor)
            .all(|c| c.name.is_none() && c.phone.is_none() && c.website.is_none())
    );
    assert!(matches!(
        company_detail(&db, CompanyId(99), &fields),
        Err(EngineError::NotFound)
    ));
}

// REQ 3.1
#[test]
fn timing_first_pages_200k() {
    let (_dir, db) = fixture(200_000);
    for q in cases() {
        let mut times = Vec::new();
        for _ in 0..30 {
            let start = Instant::now();
            std::hint::black_box(list(&db, &q).unwrap());
            times.push(start.elapsed());
        }
        times.sort_unstable();
        eprintln!(
            "TIMING {:?} 200000 rows, 30 runs: p50={:.3}ms p95={:.3}ms max={:.3}ms",
            shapes::classify(&q.filters, &q.sort),
            times[14].as_secs_f64() * 1000.,
            times[28].as_secs_f64() * 1000.,
            times[29].as_secs_f64() * 1000.
        );
    }
}

// REQ 2.2
#[test]
fn contacts_follow_created_time_then_record_order() {
    let (_dir, db) = fixture(1);
    db.conn().execute_batch("INSERT INTO contacts(id,company_id,name,created_at) VALUES(1,1,'first',30),(2,1,'second',20),(3,1,'third',10),(4,1,'tie first',20),(5,1,'tie second',20)").unwrap();
    let detail = company_detail(&db, CompanyId(1), &vec![]).unwrap();
    assert_eq!(
        detail.contacts.iter().map(|c| c.id.0).collect::<Vec<_>>(),
        [1, 5, 4, 2, 3]
    );
}

// REQ 3.3, 4.1, 2.2
#[test]
fn reads_remain_consistent_during_commits() {
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    let (dir, db) = fixture(100);
    db.conn().execute_batch("UPDATE companies SET status=0,prefecture_code=13,owner_id=0,name='検索対象',name_key='検索対象'; INSERT INTO contacts(company_id,created_at) VALUES(1,0)").unwrap();
    fts::rebuild(db.conn()).unwrap();
    let path = dir.path().join("synthetic.db");
    let barrier = Arc::new(Barrier::new(2));
    let stop = Arc::new(AtomicBool::new(false));
    let writer_stop = Arc::clone(&stop);
    let start = Arc::clone(&barrier);
    let writer = std::thread::spawn(move || {
        let mut conn = rusqlite::Connection::open(path).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        start.wait();
        for epoch in 1..=10_000 {
            if writer_stop.load(Ordering::Relaxed) {
                break;
            }
            let tx = conn.transaction().unwrap();
            tx.execute("UPDATE companies SET status=CASE WHEN id>20 AND (id+?)%3=0 THEN 5 ELSE 0 END,name=CASE WHEN (id+?)%2=0 THEN '検索対象' ELSE '改名対象' END", rusqlite::params![epoch,epoch]).unwrap();
            tx.execute("UPDATE companies SET name_key=name,owner_id=?", [epoch])
                .unwrap();
            tx.execute(
                "UPDATE contacts SET created_at=? WHERE company_id=1",
                [epoch],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO activities(company_id,kind,at) VALUES(1,4,?)",
                [epoch],
            )
            .unwrap();
            tx.execute("INSERT OR IGNORE INTO companies(id,name,name_key,prefecture_code,status,created_at,updated_at) VALUES(?,'検索対象','検索対象',13,0,1,1)", [101+epoch%50]).unwrap();
            fts::rebuild(&tx).unwrap();
            tx.commit().unwrap();
        }
    });
    barrier.wait();
    for _ in 0..500 {
        for term in ["検索対象", "対象"] {
            let mut opt = crate::types::SearchOptions {
                after: None,
                limit: 200,
                fields: vec![],
                include_excluded: false,
                with_count: true,
            };
            let page = crate::search::search_name(&db, term, &opt).unwrap();
            assert_eq!(page.n, Some(page.rows.len() as u64));
            assert!(page.next.is_none());
            // 複数文で件数を読む経路も競合させる。
            opt.limit = 1;
            let page = crate::search::search_name(&db, term, &opt).unwrap();
            assert!(page.n.unwrap() >= page.rows.len() as u64);
        }
        let detail = company_detail(&db, CompanyId(1), &vec![]).unwrap();
        let epoch = detail.company.owner.unwrap().0;
        assert_eq!(detail.contacts[0].created_at, epoch);
        assert_eq!(detail.activities.first().map_or(0, |a| a.at), epoch);
        assert_stable_list_pages(&db);
    }
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    assert!(
        company_detail(&db, CompanyId(1), &vec![])
            .unwrap()
            .company
            .owner
            .unwrap()
            .0
            > 0
    );
}

fn assert_stable_list_pages(db: &Db) {
    let mut q = query(
        Filters {
            region: vec![Region::Prefecture(13)],
            ..Filters::default()
        },
        Sort::ShapeDefault,
        7,
    );
    let mut seen = HashSet::new();
    loop {
        let page = list(db, &q).unwrap();
        for row in page.rows {
            if row.id.0 <= 20 {
                assert!(seen.insert(row.id.0));
            }
        }
        q.after = page.next;
        if q.after.is_none() {
            break;
        }
    }
    assert_eq!(seen, (1..=20).collect());
}
