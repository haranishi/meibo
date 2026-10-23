//! 合成の断片語から候補を絞り、公開検索口の実件数で代表語を決める。
use super::conditions::{Operation, Selection, add, search_options};
use meibo_core::{Engine, normalize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
type Result<T> = std::result::Result<T, String>;

pub(super) struct Names {
    short: BTreeMap<String, u64>,
    long: HashMap<String, u64>,
    max_width: usize,
}
impl Names {
    pub(super) fn new() -> Self {
        let fragments = fragments();
        let mut long = HashMap::new();
        for a in &fragments {
            let mut terms = vec![(*a).to_owned()];
            terms.extend(fragments.iter().map(|b| format!("{a}{b}")));
            for term in terms {
                if term.chars().count() >= 3 {
                    long.insert(term.clone(), 0);
                }
                // 形式語との境界も候補にし、中間の選択率を得る。
                for candidate in [format!("{term}株式会社"), format!("株式会社{term}")] {
                    long.insert(candidate, 0);
                }
            }
        }
        let max_width = long.keys().map(|s| s.chars().count()).max().unwrap_or(3);
        Self {
            short: BTreeMap::new(),
            long,
            max_width,
        }
    }
    pub(super) fn observe(&mut self, name: &str) {
        let name = normalize::name_key(name);
        let mut boundaries: Vec<_> = name.char_indices().map(|(i, _)| i).collect();
        boundaries.push(name.len());
        let count = boundaries.len() - 1;
        let mut short = BTreeSet::new();
        let mut long = BTreeSet::new();
        for width in 1..=self.max_width.min(count) {
            for start in 0..=count - width {
                let term = &name[boundaries[start]..boundaries[start + width]];
                if width <= 2 {
                    short.insert(term);
                } else if self.long.contains_key(term) {
                    long.insert(term);
                }
            }
        }
        for term in short {
            *self.short.entry(term.to_owned()).or_default() += 1;
        }
        for term in long {
            *self.long.get_mut(term).expect("known candidate") += 1;
        }
    }
}

fn fragments() -> Vec<&'static str> {
    // 非公開の断片表と選定候補がずれないよう、同じソースから読む。
    include_str!("../../../core/src/synth.rs")
        .split("const FRAGMENTS: &[&str] = &[")
        .nth(1)
        .expect("synth fragment table")
        .split("];")
        .next()
        .expect("fragment end")
        .lines()
        .filter_map(|line| line.trim().strip_prefix('"')?.split('"').next())
        .collect()
}
fn search_count(engine: &Engine, term: &str) -> Result<u64> {
    engine
        .search_name(term, &search_options(true))
        .map_err(|e| e.to_string())?
        .n
        .ok_or_else(|| "検索件数なし".into())
}
fn search_case(
    selection: &mut Selection,
    engine: &Engine,
    term: &str,
    name: &str,
    long: bool,
    rows: u64,
) -> Result<()> {
    let n = search_count(engine, term)?;
    selection.extra.insert(
        format!("search/{name}"),
        format!("term={term}; count={n}; fraction={}", ratio(n, rows)),
    );

    for count in [false, true] {
        add(
            selection,
            search_target(long, count, n, rows),
            name,
            Operation::Search(term.into(), count),
        );
    }
    Ok(())
}
fn search_target(long: bool, count: bool, n: u64, rows: u64) -> &'static str {
    match (long, count) {
        (true, true) if u128::from(n) * 20 > u128::from(rows) => "search-common-count",
        (true, true) => "search-long-count",
        (true, false) => "search-long-first",
        (false, true) => "search-short-count",
        (false, false) => "search-short-first",
    }
}
#[allow(clippy::cast_precision_loss)]
fn ratio(n: u64, rows: u64) -> f64 {
    n as f64 / rows as f64
}

pub(super) fn expand(
    selection: &mut Selection,
    engine: &Engine,
    names: &Names,
    rows: u64,
) -> Result<()> {
    let mut used = BTreeSet::new();
    for (name, fraction) in [
        ("five-percent", 0.05),
        ("half-percent", 0.005),
        ("hundredth-percent", 0.0001),
    ] {
        let mut shortlist: Vec<_> = names
            .long
            .iter()
            .filter(|(term, n)| **n > 0 && !used.contains(*term))
            .collect();
        shortlist.sort_by(|a, b| {
            (ratio(*a.1, rows) - fraction)
                .abs()
                .total_cmp(&(ratio(*b.1, rows) - fraction).abs())
                .then(a.0.cmp(b.0))
        });
        let mut ranked = Vec::new();
        for (term, _) in shortlist.into_iter().take(3) {
            ranked.push((search_count(engine, term)?, term));
        }
        let (_, term) = ranked
            .iter()
            .filter(|(n, _)| *n > 0)
            .min_by(|a, b| {
                (ratio(a.0, rows) - fraction)
                    .abs()
                    .total_cmp(&(ratio(b.0, rows) - fraction).abs())
                    .then(a.1.cmp(b.1))
            })
            .ok_or("長い検索語の代表値不足")?;
        search_case(selection, engine, term, name, true, rows)?;
        used.insert((*term).clone());
    }
    search_case(selection, engine, "株式会社", "common-term", true, rows)?;
    search_case(selection, engine, "有限会社", "limited-company", true, rows)?;
    let high = names
        .short
        .iter()
        .filter(|(s, _)| s.chars().count() == 1)
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))
        .ok_or("1文字の代表語なし")?
        .0;
    let low = names
        .short
        .iter()
        .filter(|(s, n)| s.chars().count() == 2 && ratio(**n, rows) < 0.001)
        .min_by(|a, b| a.1.cmp(b.1).then(a.0.cmp(b.0)))
        .ok_or("0.1%未満の2文字語なし（rowsを増やしてください）")?
        .0;
    search_case(selection, engine, high, "one-char-high", false, rows)?;
    search_case(selection, engine, low, "two-char-low", false, rows)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    // REQ 4.4
    fn count_target_uses_strict_five_percent_boundary() {
        for (n, expected, target) in [
            (0, "search-long-count", 100.0_f64),
            (4, "search-long-count", 100.0),
            (5, "search-long-count", 100.0),
            (6, "search-common-count", 300.0),
        ] {
            let id = search_target(true, true, n, 100);
            assert_eq!(id, expected);
            let case = super::super::targets()
                .into_iter()
                .find(|c| c.id == id)
                .unwrap();
            assert_eq!(case.target_ms.to_bits(), target.to_bits());
            assert_eq!(search_target(true, false, n, 100), "search-long-first");
        }
    }

    #[test]
    // REQ 4.4
    fn common_term_with_zero_matches_is_a_regular_case() -> crate::Result<()> {
        let dir = tempfile::tempdir()?;
        let engine = Engine::create(
            &dir.path().join("engine.db"),
            meibo_core::types::DataOrigin::Synthetic,
            &meibo_core::OpenOptions::default(),
        )?;
        let mut selection = Selection {
            prepared: Vec::new(),
            extra: BTreeMap::new(),
        };
        search_case(
            &mut selection,
            &engine,
            "有限会社",
            "limited-company",
            true,
            1,
        )?;
        assert_eq!(
            selection.extra["search/limited-company"],
            "term=有限会社; count=0; fraction=0"
        );
        assert_eq!(
            selection.prepared[1].case.id,
            "search-long-count/limited-company"
        );
        selection.prepared.clear();
        search_case(&mut selection, &engine, "株式会社", "common-term", true, 1)?;
        assert_eq!(
            selection.extra["search/common-term"],
            "term=株式会社; count=0; fraction=0"
        );
        assert_eq!(selection.prepared.len(), 2);
        assert_eq!(
            selection.prepared[0].case.id,
            "search-long-first/common-term"
        );
        assert_eq!(
            selection.prepared[1].case.id,
            "search-long-count/common-term"
        );
        Ok(())
    }
}
