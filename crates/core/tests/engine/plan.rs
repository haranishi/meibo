use super::support::*;
use meibo_core::{shapes::SHAPES, synth::Profile, types::*};

// REQ 3.1
// REQ 3.2
// REQ 3.3
// REQ 3.4
#[test]
fn all_declared_plans_at_head_half_and_ninety_five_percent() {
    let (_dir, mut db) = database();
    db.load_synthetic(config(100_000, Profile::Dense)).unwrap();
    let mut cases = shapes();
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
    for (i, mut q) in cases.into_iter().enumerate() {
        let shape = match i {
            0..=21 => &SHAPES[i],
            22 => &SHAPES[0],
            _ => &SHAPES[1],
        };
        let count = db.count(&q.filters).unwrap();
        assert!(!count.slow);
        assert!(count.n > 1, "shape {} needs deep rows", shape.number);
        for offset in [0, count.n / 2, count.n * 95 / 100] {
            q.after = db.cursor_after_offset(&q, offset).unwrap();
            if offset > 0 {
                assert!(q.after.is_some());
            }
            let plan = db.explain(&q).unwrap();
            assert_ne!(plan.len(), 0, "shape {} offset {offset}", shape.number);
            for step in plan {
                assert!(
                    !step.contains("TEMP B-TREE"),
                    "shape {} offset {offset}: {step}",
                    shape.number
                );
                if shape.number <= 20 {
                    assert!(step.contains("SEARCH"), "{step}");
                }
                if shape.number < 22 {
                    assert!(
                        step.contains(&format!("INDEX {}", shape.index.to_ascii_lowercase())),
                        "{step}"
                    );
                }
            }
            assert!(!db.list(&q).unwrap().slow);
        }
    }
}
