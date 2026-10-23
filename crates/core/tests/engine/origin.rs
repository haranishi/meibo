use super::support::*;
use meibo_core::{Engine, OpenOptions, synth::Profile, types::*};

// REQ 8.3
#[test]
fn mismatched_claims_leave_both_origins_unchanged() {
    for origin in [DataOrigin::Synthetic, DataOrigin::Real] {
        let dir = tempfile::tempdir().unwrap();
        let mut db = Engine::create(
            &dir.path().join("origin.db"),
            origin,
            &OpenOptions::default(),
        )
        .unwrap();
        assert_eq!(db.origin(), origin);
        let c = CompanyInput {
            origin: if origin == DataOrigin::Real {
                DataOrigin::Synthetic
            } else {
                DataOrigin::Real
            },
            ..company()
        };
        assert!(matches!(
            db.register_company(&c),
            Err(EngineError::OriginMismatch)
        ));
        if origin == DataOrigin::Real {
            for rows in [0, 100] {
                assert!(matches!(
                    db.load_synthetic(config(rows, Profile::Dense)),
                    Err(EngineError::OriginMismatch)
                ));
            }
        }
        assert_eq!(db.count(&Filters::default()).unwrap().n, 0);
        verify(&db);
    }
}
