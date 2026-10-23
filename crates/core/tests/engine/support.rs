use meibo_core::{
    Engine, OpenOptions,
    synth::{Profile, Synth, SynthConfig},
    types::*,
};

pub fn database() -> (tempfile::TempDir, Engine) {
    let dir = tempfile::tempdir().unwrap();
    let db = Engine::create(
        &dir.path().join("synthetic.db"),
        DataOrigin::Synthetic,
        &OpenOptions::default(),
    )
    .unwrap();
    (dir, db)
}
pub fn company() -> CompanyInput {
    CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: None,
        name: "合成会社".into(),
        prefecture: 13,
        municipality: Some("13101".into()),
        industry: Some(Industry::A),
        phone: Some("0123".into()),
        website: Some("https://synthetic.invalid".into()),
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: Some("synthetic-source".into()),
        source_as_of: Some("2026-10-22".into()),
    }
}
pub fn activity(id: CompanyId) -> ActivityInput {
    ActivityInput {
        company: id,
        contact: None,
        kind: ActivityKind::Memo,
        at: 50,
        note: Some("synthetic note".into()),
        status_after: None,
        next_action: NextAction::Keep,
        reverses: None,
    }
}
pub fn fields() -> Fields {
    vec![
        Field::Name,
        Field::Phone,
        Field::Website,
        Field::ContactName,
        Field::ContactPhone,
        Field::ContactEmail,
    ]
}
pub fn query(filters: Filters, sort: Sort) -> ListQuery {
    ListQuery {
        filters,
        sort,
        after: None,
        limit: 50,
        fields: fields(),
    }
}
pub fn search() -> SearchOptions {
    SearchOptions {
        after: None,
        limit: 50,
        fields: fields(),
        include_excluded: true,
        with_count: true,
    }
}
pub fn config(rows: u64, profile: Profile) -> SynthConfig {
    SynthConfig {
        rows,
        seed: 42,
        profile,
    }
}
pub fn verify(db: &Engine) {
    let report = db.verify().unwrap();
    assert!(
        report.integrity_ok && report.counts_match && report.folds_match,
        "{report:?}"
    );
}
pub fn all(db: &Engine, mut q: ListQuery) -> Vec<Company> {
    q.after = None;
    let mut rows = Vec::new();
    loop {
        let page = db.list(&q).unwrap();
        rows.extend(page.rows);
        q.after = page.next;
        if q.after.is_none() {
            return rows;
        }
    }
}
pub fn shapes() -> Vec<ListQuery> {
    let mut groups = std::collections::BTreeMap::new();
    for mut r in Synth::new(config(20_000, Profile::Dense)) {
        for activity in &r.activities {
            if let Some(status) = activity.status_after {
                r.company.status = status;
            }
        }
        let c = &r.company;
        if c.phone.is_some()
            && c.municipality.is_some()
            && c.status != Status::Excluded
            && let Some(industry) = c.industry
        {
            let key = (
                c.prefecture,
                c.municipality.clone(),
                industry.code(),
                c.status.code(),
            );
            let entry = groups.entry(key).or_insert((0, r.company));
            entry.0 += 1;
        }
    }
    let (_, c) = groups.into_values().max_by_key(|(n, _)| *n).unwrap();
    let mut cases = Vec::new();
    for signature in [
        "P", "PS", "PST", "PI", "PIS", "PIST", "M", "MS", "MST", "MI", "MIS", "MIST", "I", "IS",
        "IST", "S", "ST", "T", "ON", "N", "A", "",
    ] {
        let f = Filters {
            region: if signature.contains('P') {
                vec![Region::Prefecture(c.prefecture)]
            } else if signature.contains('M') {
                vec![Region::Municipality(c.municipality.clone().unwrap())]
            } else {
                vec![]
            },
            industry: if signature.contains('I') {
                vec![c.industry.unwrap()]
            } else {
                vec![]
            },
            status: if signature.contains('S') {
                vec![c.status]
            } else {
                vec![]
            },
            has_phone: signature.contains('T').then_some(true),
            owner: if signature.contains('O') {
                vec![UserId(1)]
            } else {
                vec![]
            },
            next_action_before: signature.contains('N').then_some(i64::MAX),
            include_excluded: false,
        };
        cases.push(query(
            f,
            if signature.contains('N') {
                Sort::NextAction
            } else if signature == "A" {
                Sort::LastActivityDesc
            } else {
                Sort::ShapeDefault
            },
        ));
    }
    cases
}
