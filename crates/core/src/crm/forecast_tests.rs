use super::*;

fn assigned(owner: i64, probability: Option<u8>) -> OpportunityForecastPatch {
    OpportunityForecastPatch {
        owner: OpportunityOwnerChange::Assign(UserId(owner)),
        probability: Some(probability),
    }
}

// Requirement 20.5: deal ownership, decision month and one-yen rounding are independent.
#[test]
fn weights_round_each_deal_and_targets_include_owners_without_open_pipeline() {
    let (_dir, mut engine, a, _) = fixture();
    let mut fields = company("合成取引先A", false);
    fields.owner = Some(UserId(17));
    engine.update_company(a, &fields).unwrap();
    let mut input = opportunity(a);
    input.amount_jpy = 1;
    for _ in 0..2 {
        engine
            .crm_create_opportunity_forecast(&input, None, assigned(2, Some(50)))
            .unwrap();
    }
    input.amount_jpy = MAX_AMOUNT_JPY;
    for _ in 0..2 {
        engine
            .crm_create_opportunity_forecast(&input, None, assigned(2, Some(100)))
            .unwrap();
    }
    engine
        .crm_create_opportunity_forecast(&input, None, assigned(2, None))
        .unwrap();
    engine
        .crm_create_opportunity_forecast(&input, None, assigned(3, Some(0)))
        .unwrap();
    input.stage = 4;
    input.close_date = "2027-02-01".into();
    input.amount_jpy = 70;
    engine
        .crm_create_opportunity_forecast(&input, Some("2026-12-31"), assigned(4, None))
        .unwrap();
    engine
        .crm_set_sales_target("2026-12", UserId(4), Some(50))
        .unwrap();
    engine
        .crm_set_sales_target("2026-12", UserId(5), Some(100))
        .unwrap();
    engine
        .crm_set_sales_target("2026-12", UserId(3), Some(0))
        .unwrap();
    let report = engine.crm_sales_report("2026-12").unwrap();
    let max = u128::try_from(MAX_AMOUNT_JPY).unwrap();
    assert_eq!(
        (
            report.forecasts[&fields.owner].count,
            report.forecasts[&fields.owner].amount_jpy
        ),
        (6, 4 * max + 2)
    );
    let heavy = &report.projections[&Some(UserId(2))];
    assert_eq!(
        (
            heavy.weighted_amount_jpy,
            heavy.open_amount_jpy,
            heavy.unknown_probability_count
        ),
        (2 * max + 2, 3 * max + 2, 1)
    );
    assert_eq!(heavy.target_difference_jpy(), None);
    let zero = &report.projections[&Some(UserId(3))];
    assert_eq!(
        (
            zero.unknown_probability_count,
            zero.target_difference_jpy().as_deref()
        ),
        (0, Some("0"))
    );
    let won = &report.projections[&Some(UserId(4))];
    assert_eq!(
        (
            won.won_amount_jpy,
            won.projected_amount_jpy(),
            won.target_difference_jpy().as_deref()
        ),
        (70, 70, Some("20"))
    );
    assert_eq!(
        report.projections[&Some(UserId(5))]
            .target_difference_jpy()
            .as_deref(),
        Some("-100")
    );
    engine
        .crm_set_sales_target("2026-12", UserId(5), None)
        .unwrap();
    assert!(
        !engine
            .crm_sales_report("2026-12")
            .unwrap()
            .projections
            .contains_key(&Some(UserId(5)))
    );
    assert!(
        engine
            .crm_sales_report("2027-02")
            .unwrap()
            .projections
            .is_empty()
    );
}

// Requirement 20.8: a physical v2 database has no forecast tables; old callers can still edit.
#[test]
fn v2_reads_inherit_and_forecast_updates_preserve_omitted_fields_and_erase() {
    let (dir, mut engine, _, _) = fixture();
    let mut company_fields = company("合成個人事業", true);
    company_fields.owner = Some(UserId(17));
    let a = engine.register_company(&company_fields).unwrap();
    let input = opportunity(a);
    let id = engine.crm_create_opportunity(&input).unwrap();
    engine
        .db
        .conn()
        .execute_batch(
            "DROP TRIGGER crm_opportunity_forecast_erase; DROP TABLE crm_opportunity_ownership;
        DROP TABLE crm_sales_targets; UPDATE meta SET value='2' WHERE key='crm_schema_version';",
        )
        .unwrap();
    let get = |engine: &Engine| {
        engine
            .crm_opportunities(&opportunity_query(Some(a)))
            .unwrap()
            .items
            .remove(0)
    };
    let old = get(&engine);
    assert_eq!(
        (old.owner, old.owner_inherited, old.probability),
        (company_fields.owner, true, None)
    );
    assert_eq!(
        engine.crm_sales_report("2026-12").unwrap().projections[&company_fields.owner]
            .unknown_probability_count,
        1
    );
    assert_eq!(version(engine.db.conn()).unwrap(), 2);
    engine
        .crm_update_opportunity_forecast(id, &input, None, assigned(19, Some(35)))
        .unwrap();
    assert_eq!(version(engine.db.conn()).unwrap(), 3);
    engine.crm_update_opportunity(id, &input).unwrap();
    engine.crm_set_stage(id, 2).unwrap();
    let updated = get(&engine);
    assert_eq!(
        (updated.owner, updated.owner_inherited, updated.probability),
        (Some(UserId(19)), false, Some(35))
    );
    let change = OpportunityForecastPatch {
        owner: OpportunityOwnerChange::Unassigned,
        probability: None,
    };
    engine
        .crm_update_opportunity_forecast(id, &input, None, change)
        .unwrap();
    let unassigned = get(&engine);
    assert_eq!(
        (
            unassigned.owner,
            unassigned.owner_inherited,
            unassigned.probability
        ),
        (None, false, Some(35))
    );
    let change = OpportunityForecastPatch {
        owner: OpportunityOwnerChange::Inherit,
        probability: Some(None),
    };
    engine
        .crm_update_opportunity_forecast(id, &input, None, change)
        .unwrap();
    company_fields.owner = Some(UserId(22));
    engine.update_company(a, &company_fields).unwrap();
    let inherited = get(&engine);
    assert_eq!(
        (
            inherited.owner,
            inherited.owner_inherited,
            inherited.probability
        ),
        (Some(UserId(22)), true, None)
    );
    engine
        .crm_set_sales_target("2026-12", UserId(22), Some(500))
        .unwrap();
    let before = engine.crm_sales_report("2026-12").unwrap();
    let backup = dir.path().join("forecast-backup.db");
    engine.backup_to(&backup).unwrap();
    let copy = Engine::open(&backup, &OpenOptions::default()).unwrap();
    assert_eq!(copy.crm_sales_report("2026-12").unwrap(), before);
    engine.erase_company(a).unwrap();
    let rows: i64 = engine
        .db
        .conn()
        .query_row("SELECT count(*) FROM crm_opportunity_ownership", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(rows, 0);
}

#[test]
fn invalid_forecast_values_roll_back_the_entire_deal_or_target() {
    let (_dir, mut engine, a, _) = fixture();
    let input = opportunity(a);
    for invalid in [assigned(19, Some(101)), assigned(0, Some(90))] {
        assert!(
            engine
                .crm_create_opportunity_forecast(&input, None, invalid)
                .is_err()
        );
    }
    assert_eq!(
        engine
            .crm_opportunities(&opportunity_query(Some(a)))
            .unwrap()
            .total,
        0
    );
    assert!(
        engine
            .crm_set_sales_target("2026-13", UserId(19), Some(10))
            .is_err()
    );
    assert!(
        engine
            .crm_set_sales_target("2026-12", UserId(19), Some(MAX_AMOUNT_JPY + 1))
            .is_err()
    );
}
