//! 公開口だけで代表値を選び、計画を検査して条件を展開する。
use super::search_cases::{self, Names};
use super::{Case, targets};
use meibo_core::{
    Engine,
    shapes::{SHAPES, Shape, classify},
    types::{Company, Field, Filters, ListQuery, Region, SearchOptions, Sort, Status, UserId},
};
use std::collections::BTreeMap;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone)]
pub(super) enum Operation {
    List(ListQuery),
    Count(Filters),
    Search(String, bool),
    Activity(bool),
    CreateList,
    BulkUpdate,
}
pub(super) struct Prepared {
    pub case: Case,
    pub operation: Operation,
}
pub(super) struct Selection {
    pub prepared: Vec<Prepared>,
    pub extra: BTreeMap<String, String>,
}

pub(super) fn query(filters: Filters) -> ListQuery {
    let sort = if filters.next_action_before.is_some() {
        Sort::NextAction
    } else {
        Sort::ShapeDefault
    };
    ListQuery {
        filters,
        sort,
        after: None,
        limit: 50,
        fields: vec![],
    }
}
pub(super) fn search_options(count: bool) -> SearchOptions {
    SearchOptions {
        after: None,
        limit: if count { 1 } else { 50 },
        fields: vec![],
        include_excluded: false,
        with_count: count,
    }
}

fn projected(c: &Company, signature: &str) -> Option<Filters> {
    let mut f = Filters::default();
    for ch in signature.chars() {
        match ch {
            'P' => f.region = vec![Region::Prefecture(c.prefecture)],
            'M' => f.region = vec![Region::Municipality(c.municipality.clone()?)],
            'I' => f.industry = vec![c.industry?],
            'S' => f.status = vec![c.status],
            'T' => f.has_phone = Some(c.phone.is_some()),
            'O' => f.owner = vec![c.owner?],
            // 合成の時刻を日単位へ区切り、候補数を行数に依存させない。
            'N' => f.next_action_before = Some(c.next_action_at?.div_euclid(86400) * 86400 + 86399),
            _ => unreachable!(),
        }
    }
    if !signature.contains('S') && c.status == Status::Excluded {
        return None;
    }
    Some(f)
}

struct Catalog {
    candidates: Vec<BTreeMap<String, Filters>>,
    names: Names,
}
fn catalog(engine: &Engine) -> Result<Catalog> {
    let mut candidates = vec![BTreeMap::new(); 22];
    let mut names = Names::new();
    let mut q = query(Filters {
        include_excluded: true,
        ..Filters::default()
    });
    q.limit = 200;
    q.fields = vec![Field::Phone, Field::Name];
    loop {
        let page = engine.list(&q).map_err(|e| e.to_string())?;
        for c in page.rows {
            for (i, spec) in SHAPES.iter().enumerate() {
                if let Some(f) = projected(&c, spec.filters) {
                    candidates[i].entry(format!("{f:?}")).or_insert(f);
                }
            }
            if c.status != Status::Excluded {
                names.observe(c.name.as_deref().unwrap_or(""));
            }
        }
        let Some(next) = page.next else {
            break;
        };
        q.after = Some(next);
    }
    Ok(Catalog { candidates, names })
}

pub(super) fn add(selection: &mut Selection, target: &str, suffix: &str, operation: Operation) {
    let mut case = targets()
        .into_iter()
        .find(|c| c.id == target)
        .expect("static target");
    case.id = format!("{target}/{suffix}");
    case.trials = if matches!(target, "slow" | "create-list" | "bulk-update") {
        30
    } else {
        100
    };
    selection.prepared.push(Prepared { case, operation });
}

pub(super) fn check_plan(engine: &Engine, q: &ListQuery, number: u8) -> Result<()> {
    if classify(&q.filters, &q.sort) != Shape::Declared(number) {
        return Err(format!("形{number}の分類不一致: {q:?}"));
    }
    let plan = engine.explain(q).map_err(|e| format!("形{number}: {e}"))?;
    if plan.is_empty() {
        return Err(format!("形{number}: 計画が空です"));
    }
    let spec = &SHAPES[usize::from(number) - 1];
    for step in plan {
        let upper = step.to_ascii_uppercase();
        if upper.contains("TEMP B-TREE")
            || (number <= 20 && !upper.contains("SEARCH"))
            || (number < 22 && !upper.contains(&format!("INDEX {}", spec.index)))
            || (number >= 21 && !upper.contains("SEARCH") && !upper.contains("SCAN"))
        {
            return Err(format!("形{number}の計画違反: {step}"));
        }
    }
    Ok(())
}

fn declared(
    selection: &mut Selection,
    engine: &Engine,
    suffix: &str,
    mut q: ListQuery,
    number: u8,
) -> Result<()> {
    check_plan(engine, &q, number)?;
    let n = engine.count(&q.filters).map_err(|e| e.to_string())?.n;
    selection.extra.insert(
        format!("representative/{suffix}"),
        format!("count={n}; filters={:?}; sort={:?}", q.filters, q.sort),
    );
    add(selection, "list-first", suffix, Operation::List(q.clone()));
    add(
        selection,
        "count",
        suffix,
        Operation::Count(q.filters.clone()),
    );
    if n >= 200 {
        for (name, percent) in [("p50", 50), ("p95", 95)] {
            q.after = engine
                .cursor_after_offset(&q, n * percent / 100)
                .map_err(|e| e.to_string())?;
            if q.after.is_none() {
                return Err(format!("{suffix}: 深いページのカーソルなし"));
            }
            check_plan(engine, &q, number)?;
            add(
                selection,
                "list-deep",
                &format!("{suffix}/{name}"),
                Operation::List(q.clone()),
            );
        }
    }
    Ok(())
}

fn representatives(selection: &mut Selection, engine: &Engine, catalog: &Catalog) -> Result<()> {
    for (i, spec) in SHAPES.iter().enumerate() {
        let mut ranked = Vec::new();
        for (key, filters) in &catalog.candidates[i] {
            let n = engine.count(filters).map_err(|e| e.to_string())?.n;
            ranked.push((n, key, filters));
        }
        ranked.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(b.1)));
        if ranked.is_empty() {
            return Err(format!(
                "形{}: 実在する代表値がありません（rowsを増やしてください）",
                spec.number
            ));
        }
        for (name, index) in [
            ("large", ranked.len() - 1),
            ("median", (ranked.len() - 1) / 2),
        ] {
            let mut q = query(ranked[index].2.clone());
            if spec.number == 21 {
                q.sort = Sort::LastActivityDesc;
            }
            declared(
                selection,
                engine,
                &format!("s{:02}/{name}", spec.number),
                q,
                spec.number,
            )?;
        }
    }
    Ok(())
}

fn multi(selection: &mut Selection, engine: &Engine, catalog: &Catalog) -> Result<()> {
    for (name, index, number) in [
        ("prefecture", 0, 1),
        ("municipality", 6, 7),
        ("industry", 12, 13),
        ("status", 15, 16),
        ("owner", 18, 19),
    ] {
        let mut f = Filters::default();
        for candidate in catalog.candidates[index].values().take(10) {
            match name {
                "prefecture" | "municipality" => f.region.extend(candidate.region.clone()),
                "industry" => f.industry.extend(candidate.industry.clone()),
                "status" => f.status.extend(candidate.status.clone()),
                "owner" => {
                    f.owner.extend(candidate.owner.clone());
                    f.next_action_before = f.next_action_before.max(candidate.next_action_before);
                }
                _ => unreachable!(),
            }
        }
        // 状態は6種類だけ。10指定のうち重複分は公開口が集合に直す。
        if name == "status" {
            let distinct = f.status.clone();
            if distinct.is_empty() {
                return Err("複数状態の候補なし".into());
            }
            while f.status.len() < 10 {
                f.status.push(distinct[f.status.len() % distinct.len()]);
            }
        }
        if name == "owner" {
            f.owner.sort_by_key(|v| v.0);
            f.owner.dedup();
            for id in 1..=10 {
                if f.owner.len() < 10 && !f.owner.contains(&UserId(id)) {
                    f.owner.push(UserId(id));
                }
            }
        }
        // 複数値の先頭候補は同じ値の異なるNを含むことがある。
        if name == "owner" {
            f.owner.sort_by_key(|v| v.0);
            f.owner.dedup();
        }
        declared(
            selection,
            engine,
            &format!("multi/{name}"),
            query(f),
            number,
        )?;
    }
    Ok(())
}

fn slow(selection: &mut Selection, engine: &Engine, catalog: &Catalog) -> Result<()> {
    let selected = |id: &str| -> Result<Filters> {
        selection
            .prepared
            .iter()
            .find_map(|p| match &p.operation {
                Operation::List(q) if p.case.id == id => Some(q.filters.clone()),
                _ => None,
            })
            .ok_or_else(|| format!("代表値なし: {id}"))
    };
    let region = selected("list-first/s01/large")?.region;
    let industry = selected("list-first/s13/large")?.industry;
    let phone = selected("list-first/s18/large")?.has_phone;
    let owner = selected("list-first/s19/large")?.owner;
    let cases = [
        (
            "pt",
            Filters {
                region: region.clone(),
                has_phone: phone,
                ..Filters::default()
            },
        ),
        (
            "it",
            Filters {
                industry: industry.clone(),
                has_phone: phone,
                ..Filters::default()
            },
        ),
        (
            "pit",
            Filters {
                region: region.clone(),
                industry: industry.clone(),
                has_phone: phone,
                ..Filters::default()
            },
        ),
        (
            "two-multi",
            Filters {
                region: catalog.candidates[0]
                    .values()
                    .take(2)
                    .flat_map(|f| f.region.clone())
                    .collect(),
                industry: catalog.candidates[12]
                    .values()
                    .take(2)
                    .flat_map(|f| f.industry.clone())
                    .collect(),
                ..Filters::default()
            },
        ),
        (
            "owner",
            Filters {
                owner,
                ..Filters::default()
            },
        ),
    ];
    for (name, f) in cases {
        let q = query(f.clone());
        if classify(&f, &q.sort) != Shape::Slow {
            return Err(format!("{name}: 遅い条件の分類不一致"));
        }
        let n = engine.count(&f).map_err(|e| e.to_string())?.n;
        selection.extra.insert(
            format!("representative/slow/{name}"),
            format!("count={n}; {f:?}"),
        );
        add(
            selection,
            "slow",
            &format!("{name}/first"),
            Operation::List(q),
        );
        add(
            selection,
            "slow",
            &format!("{name}/count"),
            Operation::Count(f),
        );
    }
    Ok(())
}

pub(super) fn expand(engine: &Engine, rows: u64) -> Result<Selection> {
    let catalog = catalog(engine)?;
    eprintln!("企業と検索語の候補を収集しました");
    let mut selection = Selection {
        prepared: Vec::new(),
        extra: BTreeMap::new(),
    };
    representatives(&mut selection, engine, &catalog)?;
    eprintln!("22形の代表値と計画を検査しました");
    multi(&mut selection, engine, &catalog)?;
    slow(&mut selection, engine, &catalog)?;
    search_cases::expand(&mut selection, engine, &catalog.names, rows)?;
    for (target, operation) in [
        ("activity", Operation::Activity(false)),
        ("activity-reader", Operation::Activity(true)),
        ("create-list", Operation::CreateList),
        ("bulk-update", Operation::BulkUpdate),
    ] {
        add(&mut selection, target, "default", operation);
    }
    Ok(selection)
}
