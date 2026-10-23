use super::support::*;
use meibo_core::{synth::Profile, types::*};
use std::collections::BTreeSet;

// REQ 3.2
// REQ 3.3
#[test]
fn unchanged_companies_appear_once_during_writes() {
    for shape in [1, 2, 6, 8, 9, 21, 22] {
        let (_dir, mut db) = database();
        db.load_synthetic(config(20_000, Profile::Dense)).unwrap();
        let mut q = shapes().remove(shape - 1);
        // 狭い形も50件の境界を複数回通るよう、合成200社の条件をそろえる。
        let mut owner_input = None;
        for (i, r) in meibo_core::synth::Synth::new(config(200, Profile::Dense)).enumerate() {
            let id = CompanyId(i64::try_from(i).unwrap() + 1);
            let mut c = r.company;
            c.status = db.company_detail(id, &fields()).unwrap().company.status;
            for region in &q.filters.region {
                match region {
                    Region::Prefecture(p) => {
                        c.prefecture = *p;
                        c.municipality = None;
                    }
                    Region::Municipality(m) => {
                        c.prefecture = m[..2].parse().unwrap();
                        c.municipality = Some(m.clone());
                    }
                }
            }
            if let Some(industry) = q.filters.industry.first() {
                c.industry = Some(*industry);
            }
            if q.filters.has_phone == Some(true) {
                c.phone = Some("0123".into());
            }
            db.update_company(id, &c).unwrap();
            let target = q
                .filters
                .status
                .first()
                .copied()
                .unwrap_or(Status::Untouched);
            if c.status == Status::Excluded {
                db.release_exclusion(id, target).unwrap();
            } else {
                db.record_activity(&ActivityInput {
                    kind: ActivityKind::StatusChange,
                    status_after: Some(target),
                    ..activity(id)
                })
                .unwrap();
            }
            c.status = target;
            if id == CompanyId(2) {
                owner_input = Some(c);
            }
        }
        let expected: BTreeSet<_> = all(&db, q.clone())
            .iter()
            .filter(|c| c.id != CompanyId(1))
            .map(|c| c.id.0)
            .collect();
        assert!(expected.len() > 100, "shape {shape}");
        let mut seen = BTreeSet::new();
        let mut pages = 0;
        loop {
            let page = db.list(&q).unwrap();
            for c in page.rows {
                if c.id.0 <= 20_000 && c.id != CompanyId(1) {
                    assert!(seen.insert(c.id.0), "duplicate shape {shape}: {:?}", c.id);
                }
            }
            pages += 1;
            db.register_company(&company()).unwrap();
            // 担当の変更は選んだ形の並びと適合を変えない。
            let c = owner_input.as_mut().unwrap();
            c.owner = Some(UserId(pages));
            db.update_company(CompanyId(2), c).unwrap();
            db.record_activity(&ActivityInput {
                kind: ActivityKind::StatusChange,
                status_after: Some(if pages % 2 == 0 {
                    Status::Untouched
                } else {
                    Status::Won
                }),
                ..activity(CompanyId(1))
            })
            .unwrap();
            q.after = page.next;
            if q.after.is_none() {
                break;
            }
            assert!(pages < 1000);
        }
        assert!(pages >= 3, "shape {shape}");
        assert_eq!(seen, expected, "shape {shape}");
        verify(&db);
    }
}

// REQ 3.2
// REQ 3.3
#[test]
fn offset_cursors_match_sequential_pages_including_slow_null_keys() {
    let (_dir, mut db) = database();
    db.load_synthetic(config(20_000, Profile::Dense)).unwrap();
    let mut cases = shapes();
    cases.push(query(
        Filters {
            owner: vec![UserId(1)],
            ..Filters::default()
        },
        Sort::NextAction,
    ));
    cases.push(query(
        Filters {
            has_phone: Some(true),
            region: vec![Region::Prefecture(13)],
            ..Filters::default()
        },
        Sort::LastActivityDesc,
    ));
    cases.push(query(
        Filters {
            region: vec![Region::Prefecture(13), Region::Prefecture(27)],
            ..Filters::default()
        },
        Sort::ShapeDefault,
    ));
    cases.push(query(
        Filters {
            region: vec![Region::Prefecture(13)],
            status: vec![Status::Untouched, Status::Won],
            ..Filters::default()
        },
        Sort::ShapeDefault,
    ));
    for mut q in cases {
        let expected = all(&db, q.clone());
        assert!(db.cursor_after_offset(&q, 0).unwrap().is_none());
        assert!(
            db.cursor_after_offset(&q, expected.len() as u64)
                .unwrap()
                .is_none()
        );
        assert!(db.cursor_after_offset(&q, u64::MAX).unwrap().is_none());
        for offset in [1, 50, expected.len() / 2, expected.len() * 95 / 100] {
            if offset == 0 || offset >= expected.len() {
                continue;
            }
            q.after = Some(Cursor {
                token: "ignored invalid cursor".into(),
            });
            q.after = db.cursor_after_offset(&q, offset as u64).unwrap();
            assert!(q.after.is_some());
            let page = db.list(&q).unwrap();
            assert_eq!(
                page.rows,
                expected[offset..(offset + 50).min(expected.len())]
            );
            if offset == 50 {
                assert_eq!(
                    q.after,
                    db.list(&ListQuery {
                        after: None,
                        ..q.clone()
                    })
                    .unwrap()
                    .next
                );
            }
        }
    }
}
