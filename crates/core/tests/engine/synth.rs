use super::support::*;
use meibo_core::{
    houjin,
    synth::{Profile, Synth},
    types::*,
};

// REQ 8.1
// REQ 8.2
#[test]
fn both_profiles_reproduce_all_company_content() {
    for profile in [Profile::Sparse, Profile::Dense] {
        let (_a, mut a) = database();
        let (_b, mut b) = database();
        let cfg = config(1000, profile);
        let report = a.load_synthetic(cfg).unwrap();
        assert_eq!(report.companies, cfg.rows);
        assert_eq!(report, b.load_synthetic(cfg).unwrap());
        let q = query(
            Filters {
                include_excluded: true,
                ..Filters::default()
            },
            Sort::ShapeDefault,
        );
        let left = all(&a, q.clone());
        let mut right = all(&b, q);
        assert_eq!(left.len(), 1000);
        for (l, r) in left.iter().zip(&mut right) {
            // 登録時刻だけは各保存先の実行時刻になる。
            assert!(l.created_at > 0 && r.created_at > 0);
            r.created_at = l.created_at;
            assert_eq!(l, r);
            if let Some(number) = &l.corporate_number {
                houjin::validate(number).unwrap();
                assert!(b"689".contains(&number.as_bytes()[1]));
            }
            let ld = a.company_detail(l.id, &fields()).unwrap();
            let rd = b.company_detail(l.id, &fields()).unwrap();
            assert_eq!(ld.contacts, rd.contacts);
            assert_eq!(ld.activities, rd.activities);
        }
        let reference: Vec<_> = Synth::new(cfg).collect();
        assert_eq!(reference.len(), 1000);
        for (actual, expected) in left.iter().zip(reference) {
            assert_eq!(actual.name, Some(expected.company.name));
            assert_eq!(actual.corporate_number, expected.company.corporate_number);
        }
        assert_eq!(
            a.count(&Filters {
                include_excluded: true,
                ..Filters::default()
            })
            .unwrap()
            .n,
            cfg.rows
        );
        verify(&a);
        verify(&b);
    }
}
