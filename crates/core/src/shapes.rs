//! 速さを宣言した形の目録。SQLの実行計画そのものの検査は後続段階で行う。
use crate::types::{Filters, Region, Sort};

/// 条件と並びから分類した結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 仕様の形番号（1〜22）。
    Declared(u8),
    /// 宣言外の条件。
    Slow,
}

/// 索引と並びを固定する静的な形の定義。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeSpec {
    /// 仕様の形番号。
    pub number: u8,
    /// 仕様の絞り込み記号。
    pub filters: &'static str,
    /// 索引名。登録順は「主キー」。
    pub index: &'static str,
    /// 仕様の ORDER BY 列（省略しない）。
    pub order_by: &'static str,
    /// 仕様の計画の期待。実SQLiteの出力に対する正規化は後続段階。
    pub expected_plan: &'static str,
    sort: Sort,
}

/// shapes.md の22形。表の順序と番号を保つ。
pub const SHAPES: [ShapeSpec; 22] = [
    ShapeSpec {
        number: 1,
        filters: "P",
        index: "K2",
        order_by: "P, S, T, id",
        expected_plan: "SEARCH K2 (P)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 2,
        filters: "PS",
        index: "K2",
        order_by: "P, S, T, id",
        expected_plan: "SEARCH K2 (P, S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 3,
        filters: "PST",
        index: "K2",
        order_by: "P, S, T, id",
        expected_plan: "SEARCH K2 (P, S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 4,
        filters: "PI",
        index: "K1",
        order_by: "P, I, S, T, id",
        expected_plan: "SEARCH K1 (P, I)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 5,
        filters: "PIS",
        index: "K1",
        order_by: "P, I, S, T, id",
        expected_plan: "SEARCH K1 (P, I, S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 6,
        filters: "PIST",
        index: "K1",
        order_by: "P, I, S, T, id",
        expected_plan: "SEARCH K1 (P, I, S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 7,
        filters: "M",
        index: "K4",
        order_by: "M, S, T, id",
        expected_plan: "SEARCH K4 (M)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 8,
        filters: "MS",
        index: "K4",
        order_by: "M, S, T, id",
        expected_plan: "SEARCH K4 (M, S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 9,
        filters: "MST",
        index: "K4",
        order_by: "M, S, T, id",
        expected_plan: "SEARCH K4 (M, S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 10,
        filters: "MI",
        index: "K3",
        order_by: "M, I, S, T, id",
        expected_plan: "SEARCH K3 (M, I)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 11,
        filters: "MIS",
        index: "K3",
        order_by: "M, I, S, T, id",
        expected_plan: "SEARCH K3 (M, I, S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 12,
        filters: "MIST",
        index: "K3",
        order_by: "M, I, S, T, id",
        expected_plan: "SEARCH K3 (M, I, S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 13,
        filters: "I",
        index: "K5",
        order_by: "I, S, T, id",
        expected_plan: "SEARCH K5 (I)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 14,
        filters: "IS",
        index: "K5",
        order_by: "I, S, T, id",
        expected_plan: "SEARCH K5 (I, S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 15,
        filters: "IST",
        index: "K5",
        order_by: "I, S, T, id",
        expected_plan: "SEARCH K5 (I, S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 16,
        filters: "S",
        index: "K6",
        order_by: "S, T, id",
        expected_plan: "SEARCH K6 (S)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 17,
        filters: "ST",
        index: "K6",
        order_by: "S, T, id",
        expected_plan: "SEARCH K6 (S, T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 18,
        filters: "T",
        index: "K7",
        order_by: "T, S, id",
        expected_plan: "SEARCH K7 (T)",
        sort: Sort::ShapeDefault,
    },
    ShapeSpec {
        number: 19,
        filters: "ON",
        index: "K8",
        order_by: "O, 次回対応日, S, id",
        expected_plan: "SEARCH K8 (O, 次回対応日<)",
        sort: Sort::NextAction,
    },
    ShapeSpec {
        number: 20,
        filters: "N",
        index: "K9",
        order_by: "次回対応日, S, id",
        expected_plan: "SEARCH K9 (次回対応日<)",
        sort: Sort::NextAction,
    },
    ShapeSpec {
        number: 21,
        filters: "",
        index: "K10",
        order_by: "最終活動日 DESC, S DESC, id DESC",
        expected_plan: "SCAN K10（LIMITで早期終了）",
        sort: Sort::LastActivityDesc,
    },
    ShapeSpec {
        number: 22,
        filters: "",
        index: "主キー",
        order_by: "id",
        expected_plan: "SCAN（LIMITで早期終了）",
        sort: Sort::ShapeDefault,
    },
];

/// 条件と並びを22形に分類する。宣言外の複数値・並びは `Slow`。
#[must_use]
pub fn classify(filters: &Filters, sort: &Sort) -> Shape {
    let region = match filters.region.first() {
        Some(Region::Prefecture(_)) => "P",
        Some(Region::Municipality(_)) => "M",
        None => "",
    };
    if filters.region.iter().any(|r| {
        !matches!(
            (region, r),
            ("P", Region::Prefecture(_)) | ("M", Region::Municipality(_))
        )
    }) {
        return Shape::Slow;
    }
    let lengths = [
        filters.region.len(),
        filters.industry.len(),
        filters.status.len(),
        filters.owner.len(),
    ];
    if lengths.iter().any(|n| *n > 10) || lengths.iter().filter(|n| **n > 1).count() > 1 {
        return Shape::Slow;
    }
    let mut signature = region.to_owned();
    if !filters.industry.is_empty() {
        signature.push('I');
    }
    if !filters.status.is_empty() {
        signature.push('S');
    }
    if filters.has_phone.is_some() {
        signature.push('T');
    }
    if !filters.owner.is_empty() {
        signature.push('O');
    }
    if filters.next_action_before.is_some() {
        signature.push('N');
    }
    // 複数値は先頭の項目か状態だけが宣言されている。
    if filters.industry.len() > 1 && !region.is_empty() {
        return Shape::Slow;
    }
    SHAPES
        .iter()
        .find(|shape| shape.filters == signature && shape.sort == *sort)
        .map_or(Shape::Slow, |shape| Shape::Declared(shape.number))
}
