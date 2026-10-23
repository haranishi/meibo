//! 合成の値だけを使う純粋論理の検証。
use meibo_core::{
    houjin, normalize,
    shapes::{self, Shape},
    types::{
        ActivityKind, EngineError, Field, Fields, Filters, Industry, PERSONAL_FIELDS, Region, Sort,
        Status, UserId,
    },
};
use proptest::prelude::*;

// REQ 1.7
#[test]
fn fixed_codes() {
    let statuses = [
        Status::Untouched,
        Status::InProgress,
        Status::Negotiating,
        Status::Won,
        Status::Lost,
        Status::Excluded,
    ];
    for code in 0..=255 {
        assert_eq!(
            Status::from_code(code),
            statuses.get(usize::from(code)).copied()
        );
        if let Some(status) = Status::from_code(code) {
            assert_eq!(status.code(), code);
        }
    }
    for code in 0..=255 {
        let expected = match code {
            1 => Some(ActivityKind::Call),
            2 => Some(ActivityKind::Visit),
            3 => Some(ActivityKind::Email),
            4 => Some(ActivityKind::Memo),
            5 => Some(ActivityKind::StatusChange),
            9 => Some(ActivityKind::Cancel),
            _ => None,
        };
        assert_eq!(ActivityKind::from_code(code), expected);
        if let Some(kind) = expected {
            assert_eq!(kind.code(), code);
        }
        assert_eq!(
            Industry::from_code(code).map(Industry::code),
            (1..=20).contains(&code).then_some(code)
        );
    }
}

// REQ 7.1
// REQ 7.2
#[test]
fn personal_field_inventory() {
    let expected = [
        Field::Name,
        Field::Phone,
        Field::Website,
        Field::ContactName,
        Field::ContactPhone,
        Field::ContactEmail,
    ];
    assert_eq!(PERSONAL_FIELDS, expected);
    for field in expected {
        assert!(field.is_personal());
        assert!(field.is_personal_for(true));
    }
    for field in [Field::Name, Field::Phone, Field::Website] {
        assert!(!field.is_personal_for(false));
    }
    for field in [Field::ContactName, Field::ContactPhone, Field::ContactEmail] {
        assert!(field.is_personal_for(false));
    }
    for field in [
        Field::Id,
        Field::CorporateNumber,
        Field::Prefecture,
        Field::Municipality,
        Field::Industry,
        Field::Owner,
        Field::Status,
        Field::IsSoleProprietor,
        Field::SourceId,
        Field::SourceAsOf,
        Field::CreatedAt,
        Field::ErasedAt,
        Field::LastActivityAt,
        Field::NextActionAt,
    ] {
        assert!(!field.is_personal());
        assert!(!field.is_personal_for(true));
    }
    assert_eq!(Fields::default(), Vec::<Field>::new());
}

fn reference_digit(base: &str) -> u32 {
    let digits: Vec<_> = base.chars().map(|c| c.to_digit(10).unwrap()).collect();
    let mut total = 0;
    for position in 1..=12 {
        total += digits[12 - position] * if position % 2 == 1 { 1 } else { 2 };
    }
    9 - total % 9
}

// REQ 1.1
// REQ 8.2
#[test]
fn corporate_examples_and_boundaries() {
    for value in ["6600000000000", "2800000000000", "9900000000000"] {
        assert!(houjin::validate(value).is_ok());
    }
    for value in [
        "",
        "660000000000",
        "66000000000000",
        "0600000000000",
        "6600000000001",
        "a600000000000",
        "６600000000000",
        "660000000000é",
        "660000000000\n",
    ] {
        match houjin::validate(value) {
            Err(EngineError::InvalidCorporateNumber { reason }) => assert_ne!(reason, ""),
            _ => panic!("accepted {value:?}"),
        }
    }
    for (n, head) in [
        (0, '6'),
        (99_999_999_999, '6'),
        (100_000_000_000, '8'),
        (199_999_999_999, '8'),
        (200_000_000_000, '9'),
        (299_999_999_999, '9'),
    ] {
        let value = houjin::generate(n).unwrap();
        assert_eq!(value.chars().nth(1).unwrap(), head);
        assert_eq!(value.len(), 13);
        assert_eq!(value[2..].parse::<u64>().unwrap(), n % 100_000_000_000);
        assert!(houjin::validate(&value).is_ok());
    }
    assert!(houjin::generate(300_000_000_000).is_err());
    assert!(houjin::generate(u64::MAX).is_err());
}

proptest! {
    // REQ 1.1
    #[test]
    fn corporate_matches_independent_formula(base in "[0-9]{12}") {
        let digit = reference_digit(&base);
        for candidate in 0..=9 {
            prop_assert_eq!(houjin::validate(&format!("{candidate}{base}")).is_ok(), candidate == digit);
        }
    }
    // REQ 8.2
    #[test]
    fn generated_numbers_match_reference(n in 0..300_000_000_000u64) {
        let value = houjin::generate(n).unwrap();
        prop_assert_eq!(value.as_bytes()[0] - b'0', u8::try_from(reference_digit(&value[1..])).unwrap());
        prop_assert!(houjin::validate(&value).is_ok());
        prop_assert_eq!(value.clone(), houjin::generate(n).unwrap());
        prop_assert_ne!(value, houjin::generate((n + 1) % 300_000_000_000).unwrap());
    }
    // REQ 4.3
    #[test]
    fn name_key_is_idempotent(value in any::<String>()) {
        let key = normalize::name_key(&value);
        prop_assert_eq!(normalize::name_key(&key), key);
    }
}

// REQ 1.5
// REQ 1.8
// REQ 4.2
// REQ 4.3
// REQ 4.7
#[test]
fn normalization_table_and_search_validation() {
    for (input, expected) in [
        ("ＡbＣ１２３", "abc123"),
        ("かぶしきがいしゃ", "カブシキガイシャ"),
        ("ｶﾞｷﾞｸﾞ", "ガギグ"),
        ("ゝゞ", "ヽヾ"),
        ("ぁゖ", "ァヶ"),
        ("゙゚゛゜", "\u{3099}\u{309a} \u{3099} \u{309a}"),
        ("㍿", "株式会社"),
        ("İ", "i\u{307}"),
        ("あ\u{3099}", "ア\u{3099}"),
    ] {
        assert_eq!(normalize::name_key(input), expected, "{input}");
    }
    let original = String::from("０３－００００－０００１");
    assert_eq!(normalize::phone(&original), "03-0000-0001");
    assert_eq!(original, "０３－００００－０００１");
    assert_eq!(normalize::phone("①–　＋Ａ"), "①– +A");
    for term in ["", " ", "！？", "---", "\u{3099}", "😀"] {
        assert!(matches!(
            normalize::search_term(term),
            Err(EngineError::InvalidSearchTerm)
        ));
    }
    for (term, length) in [
        ("建設", 2),
        ("Ａ", 1),
        ("㍿", 4),
        ("ｶﾞ", 1),
        ("İ", 2),
        ("a b", 3),
        ("１２３", 3),
    ] {
        let (key, n) = normalize::search_term(term).unwrap();
        assert_eq!(n, length);
        assert_eq!(key, normalize::name_key(term));
    }
}

fn filters(signature: &str) -> Filters {
    Filters {
        region: if signature.contains('P') {
            vec![Region::Prefecture(13)]
        } else if signature.contains('M') {
            vec![Region::Municipality("13101".into())]
        } else {
            vec![]
        },
        industry: if signature.contains('I') {
            vec![Industry::A]
        } else {
            vec![]
        },
        status: if signature.contains('S') {
            vec![Status::Untouched]
        } else {
            vec![]
        },
        has_phone: signature.contains('T').then_some(true),
        owner: if signature.contains('O') {
            vec![UserId(1)]
        } else {
            vec![]
        },
        next_action_before: signature.contains('N').then_some(500),
        include_excluded: false,
    }
}

// REQ 3.1
// REQ 3.5
// REQ 3.6
#[test]
fn all_declared_shapes_and_metadata() {
    let cases = [
        ("P", "K2", "P, S, T, id", "SEARCH K2 (P)"),
        ("PS", "K2", "P, S, T, id", "SEARCH K2 (P, S)"),
        ("PST", "K2", "P, S, T, id", "SEARCH K2 (P, S, T)"),
        ("PI", "K1", "P, I, S, T, id", "SEARCH K1 (P, I)"),
        ("PIS", "K1", "P, I, S, T, id", "SEARCH K1 (P, I, S)"),
        ("PIST", "K1", "P, I, S, T, id", "SEARCH K1 (P, I, S, T)"),
        ("M", "K4", "M, S, T, id", "SEARCH K4 (M)"),
        ("MS", "K4", "M, S, T, id", "SEARCH K4 (M, S)"),
        ("MST", "K4", "M, S, T, id", "SEARCH K4 (M, S, T)"),
        ("MI", "K3", "M, I, S, T, id", "SEARCH K3 (M, I)"),
        ("MIS", "K3", "M, I, S, T, id", "SEARCH K3 (M, I, S)"),
        ("MIST", "K3", "M, I, S, T, id", "SEARCH K3 (M, I, S, T)"),
        ("I", "K5", "I, S, T, id", "SEARCH K5 (I)"),
        ("IS", "K5", "I, S, T, id", "SEARCH K5 (I, S)"),
        ("IST", "K5", "I, S, T, id", "SEARCH K5 (I, S, T)"),
        ("S", "K6", "S, T, id", "SEARCH K6 (S)"),
        ("ST", "K6", "S, T, id", "SEARCH K6 (S, T)"),
        ("T", "K7", "T, S, id", "SEARCH K7 (T)"),
        (
            "ON",
            "K8",
            "O, 次回対応日, S, id",
            "SEARCH K8 (O, 次回対応日<)",
        ),
        ("N", "K9", "次回対応日, S, id", "SEARCH K9 (次回対応日<)"),
        (
            "",
            "K10",
            "最終活動日 DESC, S DESC, id DESC",
            "SCAN K10（LIMITで早期終了）",
        ),
        ("", "主キー", "id", "SCAN（LIMITで早期終了）"),
    ];
    assert_eq!(shapes::SHAPES.len(), cases.len());
    for (i, (signature, index, order, plan)) in cases.into_iter().enumerate() {
        let number = u8::try_from(i + 1).unwrap();
        let sort = match number {
            19 | 20 => Sort::NextAction,
            21 => Sort::LastActivityDesc,
            _ => Sort::ShapeDefault,
        };
        let mut f = filters(signature);
        assert_eq!(shapes::classify(&f, &sort), Shape::Declared(number));
        f.include_excluded = true;
        assert_eq!(shapes::classify(&f, &sort), Shape::Declared(number));
        if f.has_phone.is_some() {
            f.has_phone = Some(false);
        }
        assert_eq!(shapes::classify(&f, &sort), Shape::Declared(number));
        let actual = &shapes::SHAPES[i];
        assert_eq!(
            (
                actual.number,
                actual.filters,
                actual.index,
                actual.order_by,
                actual.expected_plan
            ),
            (number, signature, index, order, plan)
        );
    }
    for signature in [
        "PT", "IT", "PIT", "MT", "MIT", "O", "PO", "IN", "SN", "OT", "PON",
    ] {
        for sort in [Sort::ShapeDefault, Sort::NextAction, Sort::LastActivityDesc] {
            assert_eq!(shapes::classify(&filters(signature), &sort), Shape::Slow);
        }
    }
    for (signature, sort) in [
        ("P", Sort::NextAction),
        ("", Sort::NextAction),
        ("N", Sort::ShapeDefault),
        ("ON", Sort::LastActivityDesc),
        ("P", Sort::LastActivityDesc),
    ] {
        assert_eq!(shapes::classify(&filters(signature), &sort), Shape::Slow);
    }
}

// REQ 3.1
// REQ 3.5
#[test]
fn multiple_value_rules() {
    for (signature, dimension, sort, number) in [
        ("PIS", 'P', Sort::ShapeDefault, 5),
        ("MIS", 'M', Sort::ShapeDefault, 11),
        ("IS", 'I', Sort::ShapeDefault, 14),
        ("ST", 'S', Sort::ShapeDefault, 17),
        ("ON", 'O', Sort::NextAction, 19),
        ("PIS", 'S', Sort::ShapeDefault, 5),
    ] {
        for n in [1, 2, 10, 11] {
            let mut f = filters(signature);
            match dimension {
                'P' => f.region = vec![Region::Prefecture(1); n],
                'M' => f.region = vec![Region::Municipality("01101".into()); n],
                'I' => f.industry = vec![Industry::A; n],
                'S' => f.status = vec![Status::Untouched; n],
                'O' => f.owner = vec![UserId(1); n],
                _ => unreachable!(),
            }
            assert_eq!(
                shapes::classify(&f, &sort),
                if n > 10 {
                    Shape::Slow
                } else {
                    Shape::Declared(number)
                }
            );
        }
    }
    let mut f = filters("PIS");
    f.industry.push(Industry::B);
    assert_eq!(shapes::classify(&f, &Sort::ShapeDefault), Shape::Slow);
    f.industry.pop();
    f.region.push(Region::Prefecture(14));
    f.status.push(Status::Won);
    assert_eq!(shapes::classify(&f, &Sort::ShapeDefault), Shape::Slow);
    f.status.pop();
    f.region.push(Region::Municipality("13101".into()));
    assert_eq!(shapes::classify(&f, &Sort::ShapeDefault), Shape::Slow);
    f.region.reverse();
    assert_eq!(shapes::classify(&f, &Sort::ShapeDefault), Shape::Slow);
}

use meibo_core::types::{FilterKey, check_filter};

// REQ 3.6
#[test]
fn field_names_round_trip_and_inventory() {
    assert_eq!(Field::ALL.len(), 20);
    let unique: std::collections::HashSet<_> = Field::ALL.into_iter().collect();
    assert_eq!(unique.len(), 20);
    let names: std::collections::HashSet<_> = Field::ALL.into_iter().map(Field::name).collect();
    assert_eq!(names.len(), 20);
    for value in Field::ALL {
        let expected = match value {
            Field::Id => "id",
            Field::CorporateNumber => "corporate_number",
            Field::Name => "name",
            Field::Prefecture => "prefecture",
            Field::Municipality => "municipality",
            Field::Industry => "industry",
            Field::Phone => "phone",
            Field::Website => "website",
            Field::Owner => "owner",
            Field::Status => "status",
            Field::IsSoleProprietor => "is_sole_proprietor",
            Field::SourceId => "source_id",
            Field::SourceAsOf => "source_as_of",
            Field::CreatedAt => "created_at",
            Field::ErasedAt => "erased_at",
            Field::LastActivityAt => "last_activity_at",
            Field::NextActionAt => "next_action_at",
            Field::ContactName => "contact_name",
            Field::ContactPhone => "contact_phone",
            Field::ContactEmail => "contact_email",
        };
        assert_eq!(value.name(), expected);
        assert!(matches!(Field::from_name(value.name()), Ok(parsed) if parsed == value));
    }
}

// REQ 3.6
#[test]
fn sort_names_round_trip_and_inventory() {
    assert_eq!(Sort::ALL.len(), 3);
    let unique: std::collections::HashSet<_> = Sort::ALL.into_iter().collect();
    assert_eq!(unique.len(), 3);
    let names: std::collections::HashSet<_> = Sort::ALL.into_iter().map(Sort::name).collect();
    assert_eq!(names.len(), 3);
    for value in Sort::ALL {
        let expected = match value {
            Sort::ShapeDefault => "shape_default",
            Sort::NextAction => "next_action",
            Sort::LastActivityDesc => "last_activity_desc",
        };
        assert_eq!(value.name(), expected);
        assert!(matches!(Sort::from_name(value.name()), Ok(parsed) if parsed == value));
    }
}

// REQ 3.6
#[test]
fn filter_key_names_round_trip_and_inventory() {
    assert_eq!(FilterKey::ALL.len(), 6);
    let unique: std::collections::HashSet<_> = FilterKey::ALL.into_iter().collect();
    assert_eq!(unique.len(), 6);
    let names: std::collections::HashSet<_> =
        FilterKey::ALL.into_iter().map(FilterKey::name).collect();
    assert_eq!(names.len(), 6);
    for value in FilterKey::ALL {
        let expected = match value {
            FilterKey::Region => "region",
            FilterKey::Industry => "industry",
            FilterKey::Status => "status",
            FilterKey::HasPhone => "has_phone",
            FilterKey::Owner => "owner",
            FilterKey::NextActionBefore => "next_action_before",
        };
        assert_eq!(value.name(), expected);
        assert!(matches!(FilterKey::from_name(value.name()), Ok(parsed) if parsed == value));
    }
}

fn assert_field_not_allowed<T: std::fmt::Debug>(
    result: &Result<T, EngineError>,
    expected: &[String],
) {
    assert!(
        matches!(
            result,
            Err(EngineError::FieldNotAllowed { allowed }) if allowed == expected
        ),
        "{result:?}"
    );
}

// REQ 3.6
#[test]
fn unknown_names_return_complete_inventory() {
    let field_names: Vec<_> = Field::ALL
        .into_iter()
        .map(|v| v.name().to_owned())
        .collect();
    let sort_names: Vec<_> = Sort::ALL.into_iter().map(|v| v.name().to_owned()).collect();
    let filter_names: Vec<_> = FilterKey::ALL
        .into_iter()
        .map(|v| v.name().to_owned())
        .collect();
    let long = "x".repeat(100_000);
    for name in [
        "",
        "NAME",
        " name",
        "name ",
        "nam",
        "SELECT",
        "id; DROP TABLE companies",
        "\0",
        &long,
        "ShapeDefault",
        "shape-default",
        " shape_default",
        "shape_default ",
        "REGION",
        " region",
        "region ",
        "include_excluded",
        "名前",
    ] {
        assert_field_not_allowed(&Field::from_name(name), &field_names);
        assert_field_not_allowed(&Sort::from_name(name), &sort_names);
        assert_field_not_allowed(&FilterKey::from_name(name), &filter_names);
    }
}

// REQ 3.6
#[test]
fn filter_operations_match_validation() {
    for key in FilterKey::ALL {
        let expected: &[&str] = match key {
            FilterKey::Region | FilterKey::Industry | FilterKey::Status | FilterKey::Owner => {
                &["eq", "in"]
            }
            FilterKey::HasPhone => &["eq"],
            FilterKey::NextActionBefore => &["lte"],
        };
        assert_eq!(key.operations(), expected);
        let allowed: Vec<_> = expected
            .iter()
            .map(|op| format!("{}:{op}", key.name()))
            .collect();
        let long = "x".repeat(100_000);
        for op in [
            "eq",
            "in",
            "lte",
            "like",
            "",
            "EQ",
            " eq",
            "eq ",
            "\0",
            "eq; DROP TABLE companies",
            &long,
        ] {
            if expected.contains(&op) {
                assert!(
                    matches!(check_filter(key.name(), op), Ok((parsed, operation)) if parsed == key && operation == op)
                );
            } else {
                assert_field_not_allowed(&check_filter(key.name(), op), &allowed);
            }
        }
    }
}

// REQ 3.6
#[test]
fn unknown_filter_key_returns_all_operations() {
    let expected = [
        "region:eq",
        "region:in",
        "industry:eq",
        "industry:in",
        "status:eq",
        "status:in",
        "has_phone:eq",
        "owner:eq",
        "owner:in",
        "next_action_before:lte",
    ]
    .map(str::to_owned);
    let long = "x".repeat(100_000);
    for key in [
        "",
        "NAME",
        " name",
        "name ",
        "nam",
        "SELECT",
        "id; DROP TABLE companies",
        "\0",
        &long,
        "REGION",
        " region",
        "region ",
        "include_excluded",
    ] {
        for op in ["eq", "like", ""] {
            assert_field_not_allowed(&check_filter(key, op), &expected);
        }
    }
}
