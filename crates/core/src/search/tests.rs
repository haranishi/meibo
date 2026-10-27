use super::*;
use crate::query::tests::{fixture, random};
use crate::types::{CompanyId, PERSONAL_FIELDS};

// REQ 4.1
// REQ 4.2
// REQ 4.3
// REQ 4.4
// REQ 4.5
// REQ 4.6
// REQ 6.4
// REQ 7.1
#[test]
#[allow(clippy::too_many_lines)]
fn randomized_search_matches_normalized_substrings() {
    let (_dir, db) = fixture(1200);
    db.conn()
        .execute(
            "UPDATE companies SET status=CASE WHEN id%10<3 THEN 5 ELSE 0 END",
            [],
        )
        .unwrap();
    let companies: Vec<_> = db
        .conn()
        .prepare("SELECT * FROM companies ORDER BY id")
        .unwrap()
        .query_map([], |r| company_row(r, db.origin(), &vec![]))
        .unwrap()
        .collect::<crate::sql::Result<_>>()
        .unwrap();
    let source: Vec<(i64, String, i64)> = db
        .conn()
        .prepare("SELECT id,name_key,status FROM companies ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<crate::sql::Result<_>>()
        .unwrap();
    let mut terms: Vec<String> = [
        "建設",
        "商事",
        "北",
        "ＡＢＣ",
        "abc",
        "ａｂｃ１２３",
        "とうきょう",
        "トウキョウ",
        "あい",
        "ｱｲ",
        "合成",
        "不存在xyz",
        "a\"b",
        "a b",
        "a\"bc",
        "ヌル\0",
        "ル\0",
        "1",
        "１",
        " ",
        "---",
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let mut rng = 719;
    for _ in 0..80 {
        let (_, key, _) = &source[usize::try_from(random(&mut rng) % 1200).unwrap()];
        let chars: Vec<_> = key.chars().collect();
        let start = usize::try_from(random(&mut rng)).unwrap() % chars.len();
        let length = usize::try_from(random(&mut rng) % 6 + 1).unwrap();
        terms.push(chars.iter().skip(start).take(length).collect());
    }
    for term in terms {
        for (include_excluded, with_count) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let limit = u32::try_from(random(&mut rng) % 200 + 1).unwrap();
            let mut opt = SearchOptions {
                after: None,
                limit,
                fields: vec![],
                include_excluded,
                with_count,
            };
            let Ok((key, _)) = normalize::search_term(&term) else {
                assert!(matches!(
                    search_name(&db, &term, &opt),
                    Err(EngineError::InvalidSearchTerm)
                ));
                continue;
            };
            let want: Vec<_> = source
                .iter()
                .filter(|(_, name, status)| {
                    name.contains(&key) && (include_excluded || *status != 5)
                })
                .map(|(id, _, _)| *id)
                .collect();
            let mut got = Vec::new();
            loop {
                let page = search_name(&db, &term, &opt).unwrap();
                assert_eq!(
                    page.n,
                    if opt.after.is_none() && with_count {
                        Some(want.len() as u64)
                    } else {
                        None
                    },
                    "{term}"
                );
                assert!(
                    page.rows
                        .iter()
                        .filter(|c| c.is_sole_proprietor)
                        .all(|c| c.name.is_none() && c.phone.is_none() && c.website.is_none())
                );
                let end = (got.len() + limit as usize).min(want.len());
                let expected: Vec<_> = want[got.len()..end]
                    .iter()
                    .map(|id| companies[usize::try_from(id - 1).unwrap()].clone())
                    .collect();
                assert_eq!(page.rows, expected, "{term:?}");
                assert_eq!(
                    page.next,
                    (end < want.len()).then(|| Cursor {
                        token: format!("v1:search:{}", want[end - 1]),
                    }),
                    "{term:?}"
                );
                got.extend(page.rows.iter().map(|c| c.id.0));
                if page.next.is_none() {
                    break;
                }
                assert_eq!(page.rows.len(), limit as usize);
                opt.after = page.next;
                assert!(got.len() <= source.len());
            }
            assert_eq!(got, want, "{term:?}, include_excluded={include_excluded}");
        }
    }
}

// REQ 4.1
// REQ 4.6
// REQ 4.7
#[test]
fn invalid_search_input_and_excluded_candidate_batches() {
    let (_dir, db) = fixture(30);
    let mut o = SearchOptions {
        after: None,
        limit: 1,
        fields: PERSONAL_FIELDS.to_vec(),
        include_excluded: false,
        with_count: true,
    };
    for term in ["", " ", "！？", "---"] {
        assert!(matches!(
            search_name(&db, term, &o),
            Err(EngineError::InvalidSearchTerm)
        ));
    }
    for limit in [0, 201] {
        o.limit = limit;
        assert!(matches!(
            search_name(&db, "合成", &o),
            Err(EngineError::InvalidInput { .. })
        ));
    }
    o.limit = 1;
    for token in ["x", "v1:22:1", "v1:search:x", "v1:search:1,2"] {
        o.after = Some(Cursor {
            token: token.into(),
        });
        assert!(
            matches!(search_name(&db,"合成",&o),Err(EngineError::InvalidInput{field,..}) if field=="after")
        );
    }
    o.after = None;
    db.conn()
        .execute("UPDATE companies SET status=5 WHERE id<29", [])
        .unwrap();
    let first = search_name(&db, "合成", &o).unwrap();
    assert_eq!(first.n, Some(2));
    assert_eq!(first.rows[0].id, CompanyId(29));
    assert!(first.next.is_some());
    o.after = first.next;
    let last = search_name(&db, "合成", &o).unwrap();
    assert_eq!(last.rows[0].id, CompanyId(30));
    assert!(last.next.is_none());
    assert_eq!(last.n, None);
    o.after = None;
    let none = search_name(&db, "合成不存在", &o).unwrap();
    assert_eq!(none.rows, vec![]);
    assert_eq!(none.n, Some(0));
    assert!(none.next.is_none());
    o.include_excluded = true;
    assert_eq!(search_name(&db, "合成", &o).unwrap().n, Some(30));
}

// REQ 4.1
// REQ 4.4
// REQ 4.5
#[test]
fn first_page_without_count_matches_counted_page() {
    let (_dir, db) = fixture(200);
    db.conn()
        .execute("UPDATE companies SET status=5 WHERE id%7=0", [])
        .unwrap();
    // 全文索引（3文字以上）と部分一致（2文字以下）の両方。
    for term in ["合成", "建設", "合成会社", "不存在xyz"] {
        let counted = SearchOptions {
            after: None,
            limit: 20,
            fields: vec![],
            include_excluded: false,
            with_count: true,
        };
        let plain = SearchOptions {
            with_count: false,
            ..counted.clone()
        };
        let with = search_name(&db, term, &counted).unwrap();
        let without = search_name(&db, term, &plain).unwrap();
        assert!(without.n.is_none(), "{term}");
        assert_eq!(with.rows, without.rows, "{term}");
        assert_eq!(with.next, without.next, "{term}");
        assert!(with.n.is_some(), "{term}");
    }
}

// REQ 4.1
// REQ 4.4
// REQ 4.5
// REQ 4.6
#[test]
fn exhausted_candidates_and_limit_boundaries_match_counts() {
    let (_dir, db) = fixture(30);
    // 除外が複数バッチにまたがっても、最後まで読む必要がある。
    db.conn()
        .execute(
            "UPDATE companies SET status=CASE WHEN id<=24 THEN 5 ELSE 0 END",
            [],
        )
        .unwrap();
    for term in ["境界企業", "短語", "単"] {
        let key = normalize::name_key(term);
        db.conn()
            .execute("UPDATE companies SET name=?1,name_key=?1", [&key])
            .unwrap();
        crate::schema::fts::rebuild(db.conn()).unwrap();
        for include_excluded in [false, true] {
            let total = if include_excluded { 30 } else { 6 };
            for limit in [total - 1, total, total + 1] {
                let opt = SearchOptions {
                    limit,
                    include_excluded,
                    with_count: true,
                    after: None,
                    fields: vec![],
                };
                let counted = search_name(&db, term, &opt).unwrap();
                let plain = search_name(
                    &db,
                    term,
                    &SearchOptions {
                        with_count: false,
                        ..opt
                    },
                )
                .unwrap();
                assert_eq!(counted.n, Some(u64::from(total)));
                assert_eq!(plain.n, None);
                assert_eq!(counted.rows, plain.rows);
                assert_eq!(counted.next, plain.next);
                assert_eq!(counted.next.is_some(), limit < total);
                if term.chars().count() >= 3 && !term.contains('\0') {
                    let phrase = format!("\"{key}\"");
                    assert_eq!(
                        counted.n,
                        Some(fts_count(&db, &phrase, &key, include_excluded).unwrap())
                    );
                } else {
                    let n: u64 = db.conn().query_row(
                        "SELECT count(*) FROM companies WHERE instr(name_key,?)>0 AND (? OR status<>5)",
                        params![key, include_excluded], unsigned).unwrap();
                    assert_eq!(counted.n, Some(n));
                }
            }
        }
    }
}

// REQ 4.1
// REQ 4.4
// REQ 6.4
// REQ 7.5
#[test]
fn erased_company_is_subtracted_from_fts_count() {
    let (_dir, mut db) = fixture(30);
    crate::erase::erase_company(&mut db, CompanyId(7)).unwrap();
    for include_excluded in [false, true] {
        for term in ["消去済み", "消去", "済"] {
            let opt = SearchOptions {
                limit: 1,
                include_excluded,
                with_count: true,
                after: None,
                fields: PERSONAL_FIELDS.to_vec(),
            };
            let page = search_name(&db, term, &opt).unwrap();
            assert_eq!(page.n, Some(u64::from(include_excluded)));
            assert_eq!(page.rows.len(), usize::from(include_excluded));
            assert!(page.next.is_none());
            if term.chars().count() >= 3 {
                let key = normalize::name_key(term);
                let phrase = format!("\"{key}\"");
                assert_eq!(
                    fts_count(&db, &phrase, &key, include_excluded).unwrap(),
                    u64::from(include_excluded)
                );
            }
        }
    }
}

// REQ 4.4
// REQ 4.5
#[test]
#[ignore = "合成100万社の検索時間を測る"]
fn dense_million_search_timing() {
    use crate::{
        bulk::{self, BulkOptions},
        schema::OpenOptions,
        synth::{Profile, Synth, SynthConfig},
        types::DataOrigin,
    };
    use std::time::Instant;

    let dir = tempfile::tempdir().unwrap();
    let mut db = Db::create(
        &dir.path().join("search-bench.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    let config = SynthConfig {
        rows: 1_000_000,
        seed: 20_261_022,
        profile: Profile::Dense,
    };
    bulk::load(
        &mut db,
        Synth::new(config).map(Into::into),
        Some(config.rows),
        &BulkOptions::default(),
    )
    .unwrap();
    db.conn()
        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")
        .unwrap();
    let excluded: u64 = db
        .conn()
        .query_row(
            "SELECT count(*) FROM companies WHERE status=5",
            [],
            unsigned,
        )
        .unwrap();
    eprintln!(
        "rows={} profile=Dense seed={} excluded={} trials=31",
        config.rows, config.seed, excluded
    );
    let opt = SearchOptions {
        after: None,
        limit: 50,
        fields: vec![],
        include_excluded: false,
        with_count: true,
    };
    for term in ["株式会社", "システム", "不存在xyz", "建設", "商事"] {
        search_name(&db, term, &opt).unwrap();
        let mut elapsed = Vec::new();
        let mut count = None;
        for _ in 0..31 {
            let start = Instant::now();
            let page = search_name(&db, term, &opt).unwrap();
            elapsed.push(start.elapsed());
            count = page.n;
        }
        elapsed.sort_unstable();
        eprintln!(
            "term={term:?} n={count:?} p50_ms={:.3}",
            elapsed[15].as_secs_f64() * 1000.0
        );
    }
}

// REQ 4.6, 10.5
#[test]
fn nul_search_terms_return_empty_without_error() {
    let (_dir, db) = fixture(30);
    for term in ["合成\0", "a\0bc", "ル\0"] {
        for include_excluded in [false, true] {
            let page = search_name(
                &db,
                term,
                &SearchOptions {
                    after: None,
                    limit: 1,
                    fields: vec![],
                    include_excluded,
                    with_count: true,
                },
            )
            .unwrap();
            assert_eq!(page.rows, vec![]);
            assert_eq!(page.n, Some(0));
            assert!(page.next.is_none());
        }
    }
}

// REQ 4.1, 4.3, 4.4
#[test]
fn unicode_fts_matches_independent_substrings_on_all_pages() {
    let (_dir, mut db) = fixture(0);
    let names = [
        "aσb",
        "aςb",
        "aΣb",
        "aİb",
        "aßb",
        "aǅb",
        "aﬁb",
        "ＡＢＣ１２３",
        "ｱｲｳ",
        "a𠮷b",
    ];
    db.write(0, |tx| {
        for id in 1..=1000 {
            let name = names[usize::try_from(id % 10).unwrap()];
            tx.execute("INSERT INTO companies(id,name,name_key,prefecture_code,status,created_at,updated_at) VALUES(?,?,?,13,?,1,1)", params![id, name, normalize::name_key(name), if id % 100 < 30 {5} else {0}]).unwrap();
        }
        Ok(())
    }).unwrap();
    crate::schema::fts::rebuild(db.conn()).unwrap();
    for term in names
        .into_iter()
        .chain(["ai\u{307}b", "aǆb", "afib", "abc", "アイウ", "不存在xyz"])
    {
        assert!(normalize::search_term(term).unwrap().1 >= 3);
        assert_search_reference(&db, term, 17);
    }
}

fn assert_search_reference(db: &Db, term: &str, limit: u32) {
    let source: Vec<(i64, String, i64)> = db
        .conn()
        .prepare("SELECT id,name_key,status FROM companies ORDER BY id")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<crate::sql::Result<_>>()
        .unwrap();
    let key = normalize::name_key(term);
    for include_excluded in [false, true] {
        let want: Vec<_> = source
            .iter()
            .filter(|(_, name, status)| name.contains(&key) && (include_excluded || *status != 5))
            .map(|(id, _, _)| *id)
            .collect();
        for with_count in [false, true] {
            let mut o = SearchOptions {
                after: None,
                limit,
                fields: vec![],
                include_excluded,
                with_count,
            };
            let mut got = Vec::new();
            loop {
                let page = search_name(db, term, &o).unwrap();
                assert_eq!(
                    page.n,
                    (with_count && o.after.is_none()).then_some(want.len() as u64),
                    "{term}"
                );
                let end = (got.len() + limit as usize).min(want.len());
                assert_eq!(
                    page.rows.iter().map(|c| c.id.0).collect::<Vec<_>>(),
                    want[got.len()..end],
                    "{term}"
                );
                assert_eq!(
                    page.next,
                    (end < want.len()).then(|| Cursor {
                        token: format!("v1:search:{}", want[end - 1])
                    })
                );
                got.extend(page.rows.into_iter().map(|c| c.id.0));
                o.after = page.next;
                if o.after.is_none() {
                    break;
                }
                assert!(got.len() <= want.len());
            }
            assert_eq!(got, want, "{term}");
        }
    }
}

// REQ 4.4
#[test]
fn excluded_heavy_fts_uses_primary_key_join_and_matches_reference() {
    let (_dir, db) = fixture(20_000);
    db.conn().execute("UPDATE companies SET name='検索対象',name_key='検索対象',status=CASE WHEN id%20=0 THEN 0 ELSE 5 END", []).unwrap();
    crate::schema::fts::rebuild(db.conn()).unwrap();
    let plan: Vec<String> = db
        .conn()
        .prepare(&format!("EXPLAIN QUERY PLAN {FTS_PAGE_SQL}"))
        .unwrap()
        .query_map(params!["\"検索対象\"", 0, false, 51], |r| r.get(3))
        .unwrap()
        .collect::<crate::sql::Result<_>>()
        .unwrap();
    assert!(
        plan[0].contains("SCAN name_fts VIRTUAL TABLE INDEX"),
        "{plan:?}"
    );
    assert!(
        plan[1].contains("SEARCH c USING INTEGER PRIMARY KEY (rowid=?)"),
        "{plan:?}"
    );
    assert!(!plan.iter().any(|p| p.contains("TEMP B-TREE")), "{plan:?}");
    let o = SearchOptions {
        after: None,
        limit: 50,
        fields: vec![],
        include_excluded: false,
        with_count: true,
    };
    let start = std::time::Instant::now();
    let page = search_name(&db, "検索対象", &o).unwrap();
    eprintln!(
        "FTS rows=20000 excluded=95% limit=50 with_count=true elapsed_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(page.n, Some(1000));
    let start = std::time::Instant::now();
    let old = legacy_fts_page(&db, &o);
    eprintln!(
        "FTS legacy rows=20000 excluded=95% limit=50 with_count=true elapsed_ms={:.3}",
        start.elapsed().as_secs_f64() * 1000.0
    );
    assert_eq!(old.rows, page.rows);
    assert_eq!(old.next, page.next);
    assert_eq!(old.n, page.n);
    assert_search_reference(&db, "検索対象", 200);
}

// 変更前の候補バッチと個別取得を、時間の比較に残す。
fn legacy_fts_page(db: &Db, o: &SearchOptions) -> SearchPage {
    use crate::sql::OptionalExtension;
    let _snapshot = db.conn().unchecked_transaction().unwrap();
    let phrase = "\"検索対象\"";
    let mut candidates = db
        .conn()
        .prepare(
            "SELECT rowid FROM name_fts WHERE name_fts MATCH ? AND rowid>? ORDER BY rowid LIMIT ?",
        )
        .unwrap();
    let mut company = db
        .conn()
        .prepare("SELECT * FROM companies WHERE id=? AND (? OR status<>5)")
        .unwrap();
    let mut position = i64::MIN;
    let mut rows = Vec::new();
    while rows.len() <= o.limit as usize {
        let ids: Vec<i64> = candidates
            .query_map(params![phrase, position, o.limit + 1], |r| r.get(0))
            .unwrap()
            .collect::<crate::sql::Result<_>>()
            .unwrap();
        if ids.is_empty() {
            break;
        }
        for id in ids {
            position = id;
            if let Some(row) = company
                .query_row(params![id, o.include_excluded], |r| {
                    company_row(r, db.origin(), &o.fields)
                })
                .optional()
                .unwrap()
            {
                rows.push(row);
            }
            if rows.len() > o.limit as usize {
                break;
            }
        }
    }
    let n = Some(fts_count(db, phrase, "検索対象", o.include_excluded).unwrap());
    let next = if rows.len() > o.limit as usize {
        rows.pop();
        rows.last().map(|c| Cursor {
            token: format!("v1:search:{}", c.id.0),
        })
    } else {
        None
    };
    SearchPage { rows, next, n }
}
