//! Source isolation, idempotence and transaction boundaries for company contact imports.
use std::fmt::Write as _;

use meibo_core::{
    Engine, OpenOptions, houjin,
    types::{DataOrigin, Field, Status},
};
use meibo_import::{Encoding, supplement_contacts};

mod support;
use support::{companies, company, database, verify};

const HEADER: &str = "source,corporate_number,phone,website,industry\n";

// REQ 19.3
#[test]
fn fills_missing_fields_only_and_the_same_file_is_idempotent() {
    let (_dir, mut db) = database();
    let number = houjin::generate(5).unwrap();
    let mut row = company("Synthetic account");
    row.corporate_number = Some(number.clone());
    row.phone = None;
    row.industry = None;
    db.register_company(&row).unwrap();
    let text =
        format!("{HEADER}synthetic,{number},０３-００００-００００,https://changed.invalid,F\n");
    let first = supplement_contacts(&mut db, text.as_bytes(), Encoding::Utf8).unwrap();
    assert_eq!((first.updated, first.skipped), (1, 0));
    let second = supplement_contacts(&mut db, text.as_bytes(), Encoding::Utf8).unwrap();
    assert_eq!((second.updated, second.skipped), (0, 1));
    let result = companies(&db).remove(0);
    assert_eq!(result.phone.as_deref(), Some("03-0000-0000"));
    assert_eq!(result.website, row.website);
    assert_eq!(
        db.company_detail(result.id, &vec![Field::Industry])
            .unwrap()
            .company
            .industry
            .unwrap()
            .code(),
        6
    );
    verify(&db);
}

#[test]
fn foreign_origins_unknown_numbers_and_suppressed_companies_are_never_filled() {
    let (_dir, mut db) = database();
    let mut rows = String::from(HEADER);
    for i in 0..5 {
        let number = houjin::generate(i).unwrap();
        if i < 4 {
            let mut c = company("Synthetic account");
            c.corporate_number = Some(number.clone());
            c.is_sole_proprietor = i == 1 || i == 3;
            c.status = if i == 2 {
                Status::Excluded
            } else {
                Status::Untouched
            };
            c.phone = None;
            let id = db.register_company(&c).unwrap();
            if i == 3 {
                db.erase_company(id).unwrap();
            }
        }
        writeln!(rows, "synthetic,{number},03-0000-0000,,").unwrap();
    }
    for wrong in ["first_party", "shokulabo"] {
        assert!(
            supplement_contacts(
                &mut db,
                rows.replacen("synthetic", wrong, 1).as_bytes(),
                Encoding::Utf8
            )
            .is_err()
        );
    }
    let outcome = supplement_contacts(&mut db, rows.as_bytes(), Encoding::Utf8).unwrap();
    assert_eq!((outcome.updated, outcome.skipped), (1, 4));
    assert_eq!(companies(&db).len(), 3);
    verify(&db);
}

#[test]
fn partial_batches_report_committed_rows_and_preserve_the_next_batch() {
    let (_dir, mut db) = database();
    let number = houjin::generate(11).unwrap();
    let mut row = company("Synthetic company");
    row.corporate_number = Some(number.clone());
    row.phone = None;
    row.industry = None;
    let id = db.register_company(&row).unwrap();
    let mut csv = String::from(HEADER);
    for _ in 0..500 {
        writeln!(csv, "synthetic,{number},03-0000-0000,,").unwrap();
    }
    writeln!(csv, "synthetic,{number},,,F").unwrap();
    writeln!(csv, "synthetic,{number},,file:///invalid,").unwrap();
    let error = supplement_contacts(&mut db, csv.as_bytes(), Encoding::Utf8).unwrap_err();
    assert_eq!(
        (error.row, error.report.updated, error.report.skipped),
        (503, 1, 499)
    );
    let detail = db
        .company_detail(id, &vec![Field::Industry, Field::Phone])
        .unwrap();
    assert_eq!(detail.company.industry, None);
    assert_eq!(detail.company.phone.as_deref(), Some("03-0000-0000"));
    verify(&db);
}

#[test]
fn first_party_records_preserve_public_attribution_owner_and_sales_status() {
    let dir = tempfile::tempdir().unwrap();
    let mut db = Engine::create(
        &dir.path().join("origin-test.db"),
        DataOrigin::Real,
        &OpenOptions::default(),
    )
    .unwrap();
    let number = houjin::generate(20).unwrap();
    let mut row = company("Synthetic fixture");
    row.origin = DataOrigin::Real;
    row.corporate_number = Some(number.clone());
    row.phone = None;
    row.source_id = Some("nta".into());
    row.status = Status::Negotiating;
    let id = db.register_company(&row).unwrap();
    let csv = format!("{HEADER}first_party,{number},03-0000-0000,,\n");
    assert_eq!(
        supplement_contacts(&mut db, csv.as_bytes(), Encoding::Utf8)
            .unwrap()
            .updated,
        1
    );
    let detail = db
        .company_detail(id, &vec![Field::Owner, Field::SourceId, Field::SourceAsOf])
        .unwrap();
    assert_eq!(detail.company.owner, row.owner);
    assert_eq!(detail.company.status, row.status);
    assert_eq!(detail.company.source_id, row.source_id);
    assert_eq!(detail.company.source_as_of, row.source_as_of);
    verify(&db);
}
