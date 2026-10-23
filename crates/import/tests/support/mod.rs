use meibo_core::{Engine, OpenOptions, types::*};

pub fn database() -> (tempfile::TempDir, Engine) {
    let directory = tempfile::tempdir().unwrap();
    let engine = Engine::create(
        &directory.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    (directory, engine)
}

#[allow(dead_code)] // Shared by the separate export integration-test binary.
pub fn company(name: &str) -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: name.into(),
        prefecture: 13,
        municipality: Some("13101".into()),
        industry: Some(Industry::A),
        phone: Some("0123-4567".into()),
        website: Some("https://synthetic.invalid".into()),
        owner: Some(UserId(1)),
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: Some("synthetic-source".into()),
        source_as_of: Some("2026-10-23".into()),
    }
}

pub fn companies(engine: &Engine) -> Vec<Company> {
    let mut companies = Vec::new();
    engine
        .visit_companies(
            &Filters {
                include_excluded: true,
                ..Filters::default()
            },
            &vec![Field::Name, Field::Phone, Field::Website],
            |company| {
                companies.push(company);
                Ok(())
            },
        )
        .unwrap();
    companies
}

pub fn verify(engine: &Engine) {
    let report = engine.verify().unwrap();
    assert!(report.integrity_ok && report.counts_match && report.folds_match);
}
