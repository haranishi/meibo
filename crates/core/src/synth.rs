//! 決定的な合成企業と活動。外部の名簿や通信を使わない。
use crate::{
    houjin,
    types::{ActivityKind, CompanyInput, DataOrigin, Industry, NextAction, Status, UserId},
};

/// 合成する列の密度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// 公的名簿に近い疎な列。
    Sparse,
    /// 購入リストに近い密な列。
    Dense,
}
/// 合成データの再現条件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthConfig {
    /// 作る企業数。
    pub rows: u64,
    /// 乱数の種。
    pub seed: u64,
    /// 列の密度。
    pub profile: Profile,
}
/// 合成した活動。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthActivity {
    /// 活動の種類。
    pub kind: ActivityKind,
    /// UNIX秒。
    pub at: i64,
    /// 活動後の状態。
    pub status_after: Option<Status>,
    /// 次回対応日の変更。
    pub next_action: NextAction,
}
/// 企業と記録順の活動。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthRecord {
    /// 登録入力。
    pub company: CompanyInput,
    /// 追記する活動。
    pub activities: Vec<SynthActivity>,
}
/// 連番ごとに独立して生成する反復子。
pub struct Synth {
    config: SynthConfig,
    index: u64,
}
impl Synth {
    /// 指定した条件で先頭から生成する。
    ///
    /// # Panics
    /// 合成番号帯の上限（3000億件）を超える場合。
    #[must_use]
    pub fn new(config: SynthConfig) -> Self {
        assert!(
            config.rows <= 300_000_000_000,
            "synthetic sequence out of range"
        );
        Self { config, index: 0 }
    }
}
struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn chance(&mut self, basis_points: u64) -> bool {
        self.below(10_000) < basis_points
    }
    fn weighted(&mut self, weights: &[u64]) -> usize {
        let mut x = self.below(weights.iter().sum());
        for (i, w) in weights.iter().enumerate() {
            if x < *w {
                return i;
            }
            x -= w;
        }
        unreachable!()
    }
}
const FRAGMENTS: &[&str] = &[
    "東",
    "西",
    "南",
    "北",
    "中央",
    "日本",
    "大和",
    "富士",
    "桜",
    "梅",
    "松",
    "竹",
    "新",
    "神",
    "山",
    "川",
    "海",
    "空",
    "星",
    "光",
    "風",
    "雲",
    "愛",
    "幸",
    "福",
    "豊",
    "栄",
    "アクア",
    "テック",
    "ソフト",
    "システム",
    "フーズ",
    "サービス",
    "ホーム",
    "エナジー",
    "ライフ",
    "メディカル",
    "ロジ",
    "デザイン",
    "建設",
    "商事",
    "工業",
    "運輸",
    "不動産",
    "産業",
    "開発",
    "電機",
    "精工",
    "食品",
    "薬局",
    "物産",
    "設計",
    "住宅",
    "興業",
    "企画",
    "総業",
];
const BASE_AT: i64 = 1_792_627_200;
// 乱数は各列の小さな上限で剰余済み。
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::too_many_lines
)]
fn record(config: &SynthConfig, i: u64) -> SynthRecord {
    let mut mix = SplitMix64(
        config
            .seed
            .wrapping_add(i.wrapping_mul(0x9e37_79b9_7f4a_7c15)),
    );
    let mut r = SplitMix64(mix.next());
    let dense = config.profile == Profile::Dense;
    let sole = r.chance(if dense { 1000 } else { 500 });
    let mut core = String::new();
    let parts = r.weighted(&[2, 5, 3]) + 1;
    for _ in 0..parts {
        core.push_str(FRAGMENTS[r.below(FRAGMENTS.len() as u64) as usize]);
    }
    let forms = [
        "株式会社",
        "有限会社",
        "合同会社",
        "一般社団法人",
        "社会福祉法人",
        "医療法人",
    ];
    let form = forms[r.weighted(&[70, 14, 10, 2, 2, 2])];
    let name = if sole {
        format!("{core}{}", if r.chance(5000) { "商店" } else { "食堂" })
    } else if r.chance(7500) {
        format!("{form}{core}")
    } else {
        format!("{core}{form}")
    };
    let mut weights = [12; 47];
    for (code, w) in [
        (13, 240),
        (27, 80),
        (14, 65),
        (23, 50),
        (11, 40),
        (12, 35),
        (28, 33),
        (1, 33),
        (40, 33),
        (22, 24),
    ] {
        weights[code - 1] = w;
    }
    let prefecture = (r.weighted(&weights) + 1) as u8;
    let municipality = Some(format!("{prefecture:02}{}", 100 + r.below(60)));
    let industry = if r.chance(if dense { 10_000 } else { 290 }) {
        Industry::from_code(
            (r.weighted(&[
                10, 2, 1, 140, 100, 5, 60, 30, 180, 20, 50, 60, 50, 60, 30, 140, 3, 50, 5,
            ]) + 1) as u8,
        )
    } else {
        None
    };
    let phone = r.chance(if dense { 9000 } else { 210 }).then(|| {
        format!(
            "0{:02}-{:04}-{:04}",
            r.below(100),
            r.below(10_000),
            r.below(10_000)
        )
    });
    let website = r
        .chance(if dense { 5000 } else { 90 })
        .then(|| format!("https://example.invalid/{i}"));
    let status = Status::from_code(r.weighted(if dense {
        &[800, 80, 50, 30, 30, 10]
    } else {
        &[960, 15, 10, 7, 3, 5]
    }) as u8)
    .unwrap();
    let owner = r
        .chance(if dense { 3000 } else { 100 })
        .then(|| UserId((r.below(5) + 1) as i64));
    let mut activities = Vec::new();
    if r.chance(if dense { 2500 } else { 100 }) {
        for _ in 0..=r.below(if dense { 3 } else { 2 }) {
            let kind = [
                ActivityKind::Call,
                ActivityKind::Visit,
                ActivityKind::Email,
                ActivityKind::Memo,
            ][r.below(4) as usize];
            let at = BASE_AT - r.below(90 * 86400) as i64;
            // 除外後に解除を必要とする活動を合成しない。
            let status_after = if status != Status::Excluded && r.chance(3000) {
                Status::from_code(r.below(5) as u8)
            } else {
                None
            };
            let next_action = if r.chance(if owner.is_some() { 8000 } else { 2000 }) {
                NextAction::Set(BASE_AT + r.below(90 * 86400) as i64)
            } else if r.chance(2000) {
                NextAction::Clear
            } else {
                NextAction::Keep
            };
            activities.push(SynthActivity {
                kind,
                at,
                status_after,
                next_action,
            });
        }
    }
    SynthRecord {
        company: CompanyInput {
            origin: DataOrigin::Synthetic,
            corporate_number: if sole {
                None
            } else {
                Some(houjin::generate(i).expect("validated sequence"))
            },
            name,
            prefecture,
            municipality,
            industry,
            phone,
            website,
            owner,
            status,
            is_sole_proprietor: sole,
            source_id: Some("synthetic-v1".into()),
            source_as_of: Some("2026-10-22".into()),
        },
        activities,
    }
}
impl Iterator for Synth {
    type Item = SynthRecord;
    fn next(&mut self) -> Option<Self::Item> {
        if self.index >= self.config.rows {
            return None;
        }
        let result = record(&self.config, self.index);
        self.index += 1;
        Some(result)
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.index = self.index.saturating_add(n as u64).min(self.config.rows);
        self.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match usize::try_from(self.config.rows - self.index) {
            Ok(n) => (n, Some(n)),
            Err(_) => (usize::MAX, None),
        }
    }
}
/// 全レコードのUTF-8 Debug表現と改行をFNV-1a 64で畳む。小規模の再現確認用。
#[must_use]
pub fn digest(config: &SynthConfig) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for row in Synth::new(*config) {
        for b in format!("{row:?}\n").bytes() {
            hash = (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;
    // REQ 8.1
    // REQ 8.2
    #[test]
    fn deterministic_and_independent() {
        let c = SynthConfig {
            rows: 1000,
            seed: 42,
            profile: Profile::Dense,
        };
        assert_eq!(
            Synth::new(c).collect::<Vec<_>>(),
            Synth::new(c).collect::<Vec<_>>()
        );
        assert_eq!(digest(&c), digest(&c));
        assert_ne!(digest(&c), digest(&SynthConfig { seed: 43, ..c }));
        assert_eq!(Synth::new(c).nth(900), Some(record(&c, 900)));
        for i in [0, 4_445_897, 4_449_999, 4_450_000] {
            let n = houjin::generate(i).unwrap();
            houjin::validate(&n).unwrap();
            assert!(matches!(n.as_bytes()[1], b'6' | b'8' | b'9'));
        }
    }
    // REQ 8.1
    // REQ 8.2
    #[test]
    fn distributions_and_valid_inputs() {
        for profile in [Profile::Sparse, Profile::Dense] {
            let mut counts = [0u32; 12];
            let c = SynthConfig {
                rows: 200_000,
                seed: 20_261_022,
                profile,
            };
            for (i, r) in Synth::new(c).enumerate() {
                let x = r.company;
                for (j, has) in [
                    x.industry.is_some(),
                    x.phone.is_some(),
                    x.website.is_some(),
                    x.owner.is_some(),
                    x.is_sole_proprietor,
                    !r.activities.is_empty(),
                ]
                .into_iter()
                .enumerate()
                {
                    counts[j] += u32::from(has);
                }
                counts[6 + x.status.code() as usize] += 1;
                if x.is_sole_proprietor {
                    assert!(x.corporate_number.is_none());
                } else {
                    let n = x.corporate_number.as_ref().unwrap();
                    houjin::validate(n).unwrap();
                    assert!(matches!(n.as_bytes()[1], b'6' | b'8' | b'9'));
                }
                assert_eq!(
                    x.municipality.as_ref().unwrap()[..2].parse::<u8>().unwrap(),
                    x.prefecture
                );
                if let Some(url) = &x.website {
                    assert!(url.starts_with("https://example.invalid/"));
                }
                if let Some(phone) = &x.phone {
                    assert!(phone.starts_with('0') && phone.contains('-'));
                }
                if i < 10_000 {
                    crate::write::validate_company_input(&x).unwrap();
                }
                if x.status == Status::Excluded {
                    assert!(r.activities.iter().all(|a| a.status_after.is_none()));
                }
            }
            let targets = if profile == Profile::Dense {
                [100., 90., 50., 30., 10., 25., 80., 8., 5., 3., 3., 1.]
            } else {
                [2.9, 2.1, 0.9, 1., 5., 1., 96., 1.5, 1., 0.7, 0.3, 0.5]
            };
            for (i, (n, target)) in counts.into_iter().zip(targets).enumerate() {
                let actual = f64::from(n) / 2000.;
                assert!(
                    (actual - target).abs()
                        <= if profile == Profile::Dense && i == 1 {
                            1.
                        } else {
                            0.5
                        },
                    "{profile:?} column {i}: {actual} vs {target}"
                );
            }
        }
    }
}
