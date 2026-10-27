use super::support::*;
use meibo_core::types::*;

// REQ 7.2
// REQ 7.3
// REQ 7.5
// REQ 5.3
#[test]
fn erasure_repeat_wipes_notes_and_compaction_removes_bytes() {
    let (_dir, mut db) = database();
    let token = "zqxjvk7";
    let id = db
        .register_company(&CompanyInput {
            name: format!("sole{token}"),
            phone: Some(format!("phone{token}")),
            website: Some(format!("site{token}")),
            is_sole_proprietor: true,
            ..company()
        })
        .unwrap();
    let person = db
        .add_contact(&ContactInput {
            company: id,
            name: format!("person{token}"),
            phone: Some(format!("contact{token}")),
            email: Some(format!("mail{token}")),
        })
        .unwrap();
    db.record_activity(&ActivityInput {
        contact: Some(person),
        note: Some(format!("note{token}")),
        ..activity(id)
    })
    .unwrap();
    let conn = crate::sql::Connection::open(db.path().unwrap()).unwrap();
    assert!(
        conn.execute("UPDATE activities SET note='changed'", [])
            .is_err()
    );
    assert!(conn.execute("DELETE FROM activities", []).is_err());
    db.erase_contact(person, false).unwrap();
    let before = db.company_detail(id, &fields()).unwrap();
    assert!(before.contacts[0].name.is_none());
    assert!(before.activities[0].note.is_some());
    db.erase_contact(person, true).unwrap();
    let erased = db.company_detail(id, &fields()).unwrap();
    assert_eq!(erased.contacts[0].erased_at, before.contacts[0].erased_at);
    assert!(erased.activities[0].note.is_none());
    assert!(erased.activities[0].note_erased_at.is_some());
    db.erase_contact(person, true).unwrap();
    assert_eq!(db.company_detail(id, &fields()).unwrap(), erased);
    db.erase_company(id).unwrap();
    let d = db.company_detail(id, &fields()).unwrap();
    assert!(d.company.erased_at.is_some());
    assert!(d.company.phone.is_none() && d.company.website.is_none());
    assert!(d.activities.iter().all(|a| a.note.is_none()));
    assert_eq!(db.search_name(token, &search()).unwrap().n, Some(0));
    for include_excluded in [false, true] {
        let page = db
            .search_name(
                "消去済み",
                &SearchOptions {
                    include_excluded,
                    ..search()
                },
            )
            .unwrap();
        assert_eq!(page.n, Some(u64::from(include_excluded)));
        assert_eq!(
            page.rows.iter().map(|c| c.id).collect::<Vec<_>>(),
            if include_excluded { vec![id] } else { vec![] }
        );
    }
    db.compact().unwrap();
    let mut wal = db.path().unwrap().as_os_str().to_owned();
    wal.push("-wal");
    for path in [db.path().unwrap().to_path_buf(), wal.into()] {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => panic!("{e}"),
        };
        for fragment in token.as_bytes().windows(3) {
            assert!(!bytes.windows(3).any(|w| w == fragment));
        }
    }
    verify(&db);
}
