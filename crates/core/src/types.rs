//! 公開する値・入出力とエラー。時刻は UNIX 秒、日付は YYYY-MM-DD。
use thiserror::Error;

/// 企業の内部識別番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CompanyId(pub i64);

/// 担当者の内部識別番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContactId(pub i64);

/// 活動の内部識別番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ActivityId(pub i64);

/// リストの内部識別番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ListId(pub i64);

/// 担当営業の内部識別番号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UserId(pub i64);

/// 企業の現在の状態。コードは固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Status {
    /// 未着手
    Untouched = 0,
    /// 対応中
    InProgress = 1,
    /// 商談
    Negotiating = 2,
    /// 受注
    Won = 3,
    /// 見送り
    Lost = 4,
    /// 除外
    Excluded = 5,
}

impl Status {
    /// 保存用の固定コード。
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
    /// 固定コードから復元する。未定義のコードは `None`。
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            0 => Some(Self::Untouched),
            1 => Some(Self::InProgress),
            2 => Some(Self::Negotiating),
            3 => Some(Self::Won),
            4 => Some(Self::Lost),
            5 => Some(Self::Excluded),
            _ => None,
        }
    }
}

/// 活動の種類。コードは固定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ActivityKind {
    /// 架電
    Call = 1,
    /// 訪問
    Visit = 2,
    /// メール
    Email = 3,
    /// メモ
    Memo = 4,
    /// 状態変更
    StatusChange = 5,
    /// 取り消し
    Cancel = 9,
}

impl ActivityKind {
    /// 保存用の固定コード。
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
    /// 固定コードから復元する。未定義のコードは `None`。
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Call),
            2 => Some(Self::Visit),
            3 => Some(Self::Email),
            4 => Some(Self::Memo),
            5 => Some(Self::StatusChange),
            9 => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// 日本標準産業分類の大分類 A〜T。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Industry {
    /// A
    A = 1,
    /// B
    B = 2,
    /// C
    C = 3,
    /// D
    D = 4,
    /// E
    E = 5,
    /// F
    F = 6,
    /// G
    G = 7,
    /// H
    H = 8,
    /// I
    I = 9,
    /// J
    J = 10,
    /// K
    K = 11,
    /// L
    L = 12,
    /// M
    M = 13,
    /// N
    N = 14,
    /// O
    O = 15,
    /// P
    P = 16,
    /// Q
    Q = 17,
    /// R
    R = 18,
    /// S
    S = 19,
    /// T
    T = 20,
}

impl Industry {
    /// 保存用の固定コード。
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }
    /// 固定コードから復元する。未定義のコードは `None`。
    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::A),
            2 => Some(Self::B),
            3 => Some(Self::C),
            4 => Some(Self::D),
            5 => Some(Self::E),
            6 => Some(Self::F),
            7 => Some(Self::G),
            8 => Some(Self::H),
            9 => Some(Self::I),
            10 => Some(Self::J),
            11 => Some(Self::K),
            12 => Some(Self::L),
            13 => Some(Self::M),
            14 => Some(Self::N),
            15 => Some(Self::O),
            16 => Some(Self::P),
            17 => Some(Self::Q),
            18 => Some(Self::R),
            19 => Some(Self::S),
            20 => Some(Self::T),
            _ => None,
        }
    }
}

/// 保存先と入力で照合するデータ種別。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataOrigin {
    /// 合成データ。
    Synthetic,
    /// 実データ。
    Real,
}

/// 所在地の一致条件。複数指定では同じ種類だけを使う。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Region {
    /// 都道府県コード（1〜47）。
    Prefecture(u8),
    /// 先頭の0を保つ JIS 5桁の市区町村コード。
    Municipality(String),
}

/// 次回対応日の変更指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NextAction {
    /// 変更しない。
    Keep,
    /// 解除する。
    Clear,
    /// UNIX 秒で設定する。
    Set(i64),
}

/// 一覧の並べ替え。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sort {
    /// 形ごとの索引順。条件なしでは登録順。
    ShapeDefault,
    /// 次回対応日の昇順。
    NextAction,
    /// 最終活動日の降順。
    LastActivityDesc,
}

impl Sort {
    /// 使用できる値の完全な一覧。候補の表示順で並ぶ。
    pub const ALL: [Sort; 3] = [Self::ShapeDefault, Self::NextAction, Self::LastActivityDesc];

    /// 文字列の入口で使う小文字 `snake_case` の名前。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ShapeDefault => "shape_default",
            Self::NextAction => "next_action",
            Self::LastActivityDesc => "last_activity_desc",
        }
    }

    /// 名前が完全に一致する値を返す。
    ///
    /// # Errors
    /// 未知の名前は使用できる名前の一覧を伴う `EngineError::FieldNotAllowed`。
    pub fn from_name(name: &str) -> Result<Self, EngineError> {
        Self::ALL
            .into_iter()
            .find(|value| value.name() == name)
            .ok_or_else(|| EngineError::FieldNotAllowed {
                allowed: Self::ALL
                    .into_iter()
                    .map(|value| value.name().to_owned())
                    .collect(),
            })
    }
}

/// 明示的に取得を指定できる項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    /// 企業ID。
    Id,
    /// 法人番号。
    CorporateNumber,
    /// 名称。個人事業では個人情報。
    Name,
    /// 都道府県。
    Prefecture,
    /// 市区町村。
    Municipality,
    /// 業種。
    Industry,
    /// 代表電話。個人事業では個人情報。
    Phone,
    /// ホームページ。個人事業では個人情報。
    Website,
    /// 担当営業。
    Owner,
    /// 状態。
    Status,
    /// 個人事業の印。
    IsSoleProprietor,
    /// 出典。
    SourceId,
    /// 出典の基準日。
    SourceAsOf,
    /// 登録日時。
    CreatedAt,
    /// 消去日時。
    ErasedAt,
    /// 最終活動日時。
    LastActivityAt,
    /// 次回対応日時。
    NextActionAt,
    /// 担当者氏名。
    ContactName,
    /// 担当者電話。
    ContactPhone,
    /// 担当者メール。
    ContactEmail,
}

impl Field {
    /// 使用できる値の完全な一覧。候補の表示順で並ぶ。
    pub const ALL: [Field; 20] = [
        Self::Id,
        Self::CorporateNumber,
        Self::Name,
        Self::Prefecture,
        Self::Municipality,
        Self::Industry,
        Self::Phone,
        Self::Website,
        Self::Owner,
        Self::Status,
        Self::IsSoleProprietor,
        Self::SourceId,
        Self::SourceAsOf,
        Self::CreatedAt,
        Self::ErasedAt,
        Self::LastActivityAt,
        Self::NextActionAt,
        Self::ContactName,
        Self::ContactPhone,
        Self::ContactEmail,
    ];

    /// 文字列の入口で使う小文字 `snake_case` の名前。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::CorporateNumber => "corporate_number",
            Self::Name => "name",
            Self::Prefecture => "prefecture",
            Self::Municipality => "municipality",
            Self::Industry => "industry",
            Self::Phone => "phone",
            Self::Website => "website",
            Self::Owner => "owner",
            Self::Status => "status",
            Self::IsSoleProprietor => "is_sole_proprietor",
            Self::SourceId => "source_id",
            Self::SourceAsOf => "source_as_of",
            Self::CreatedAt => "created_at",
            Self::ErasedAt => "erased_at",
            Self::LastActivityAt => "last_activity_at",
            Self::NextActionAt => "next_action_at",
            Self::ContactName => "contact_name",
            Self::ContactPhone => "contact_phone",
            Self::ContactEmail => "contact_email",
        }
    }

    /// 名前が完全に一致する値を返す。
    ///
    /// # Errors
    /// 未知の名前は使用できる名前の一覧を伴う `EngineError::FieldNotAllowed`。
    pub fn from_name(name: &str) -> Result<Self, EngineError> {
        Self::ALL
            .into_iter()
            .find(|value| value.name() == name)
            .ok_or_else(|| EngineError::FieldNotAllowed {
                allowed: Self::ALL
                    .into_iter()
                    .map(|value| value.name().to_owned())
                    .collect(),
            })
    }
}

/// 個人情報になりうる項目の完全な一覧。
pub const PERSONAL_FIELDS: [Field; 6] = [
    Field::Name,
    Field::Phone,
    Field::Website,
    Field::ContactName,
    Field::ContactPhone,
    Field::ContactEmail,
];
impl Field {
    /// 個人情報になりうる項目か。企業の3項目は個人事業の場合に適用する。
    #[must_use]
    pub const fn is_personal(self) -> bool {
        matches!(
            self,
            Self::Name
                | Self::Phone
                | Self::Website
                | Self::ContactName
                | Self::ContactPhone
                | Self::ContactEmail
        )
    }
    /// 企業の個人事業の印も含めた個人情報の判定。
    #[must_use]
    pub const fn is_personal_for(self, is_sole_proprietor: bool) -> bool {
        match self {
            Self::Name | Self::Phone | Self::Website => is_sole_proprietor,
            Self::ContactName | Self::ContactPhone | Self::ContactEmail => true,
            _ => false,
        }
    }
}
/// 明示的に取得する項目。空なら個人情報を追加しない。
pub type Fields = Vec<Field>;

/// 企業の登録・更新入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompanyInput {
    /// データ種別。
    pub origin: DataOrigin,
    /// 法人番号（13桁の文字列）。
    pub corporate_number: Option<String>,
    /// 入力どおりの名称。
    pub name: String,
    /// 都道府県コード。
    pub prefecture: u8,
    /// JIS 5桁の市区町村コード。
    pub municipality: Option<String>,
    /// 業種。
    pub industry: Option<Industry>,
    /// 代表電話。
    pub phone: Option<String>,
    /// ホームページ。
    pub website: Option<String>,
    /// 担当営業。
    pub owner: Option<UserId>,
    /// 登録時の状態。更新で変更するとエラー。
    pub status: Status,
    /// 個人事業の印。
    pub is_sole_proprietor: bool,
    /// 出典の識別子。
    pub source_id: Option<String>,
    /// 出典の基準日（YYYY-MM-DD）。
    pub source_as_of: Option<String>,
}

/// 法人番号が既存行または同じ取込バッチと重複したときの扱い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicatePolicy {
    /// バッチ全体を登録せずにエラーを返す。
    Error,
    /// 重複行を追加せず、既存行も変更しない。
    Skip,
}

/// 確定した1バッチの取込結果。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ImportBatchReport {
    /// 新しく追加した企業数。
    pub inserted: u64,
    /// 法人番号の重複で追加しなかった企業数。
    pub skipped: u64,
}

/// 全体を巻き戻した取込バッチの失敗。
#[derive(Debug, Error)]
#[error("import batch row {row}: {error}")]
pub struct ImportBatchError {
    /// バッチ内の1始まりの行位置。準備・容量・確定の失敗は0。
    pub row: usize,
    /// 入力値を含まない失敗の種類と理由。
    #[source]
    pub error: EngineError,
}

/// 担当者の登録入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContactInput {
    /// 所属企業。
    pub company: CompanyId,
    /// 氏名。
    pub name: String,
    /// 個人の電話。
    pub phone: Option<String>,
    /// 個人のメール。
    pub email: Option<String>,
}

/// 追記する活動。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityInput {
    /// 企業。
    pub company: CompanyId,
    /// 同じ企業の担当者。
    pub contact: Option<ContactId>,
    /// 種類。
    pub kind: ActivityKind,
    /// 活動日時。
    pub at: i64,
    /// メモ。
    pub note: Option<String>,
    /// 適用後の状態。
    pub status_after: Option<Status>,
    /// 次回対応日の指定。
    pub next_action: NextAction,
    /// 取り消し対象。
    pub reverses: Option<ActivityId>,
}

/// 文字列から選べる絞り込みの項目。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FilterKey {
    /// 地域。
    Region,
    /// 業種。
    Industry,
    /// 状態。
    Status,
    /// 電話の有無。
    HasPhone,
    /// 担当営業。
    Owner,
    /// 次回対応日の上限。
    NextActionBefore,
}

impl FilterKey {
    /// 使用できる値の完全な一覧。候補の表示順で並ぶ。
    pub const ALL: [FilterKey; 6] = [
        Self::Region,
        Self::Industry,
        Self::Status,
        Self::HasPhone,
        Self::Owner,
        Self::NextActionBefore,
    ];

    /// 文字列の入口で使う小文字 `snake_case` の名前。
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::Industry => "industry",
            Self::Status => "status",
            Self::HasPhone => "has_phone",
            Self::Owner => "owner",
            Self::NextActionBefore => "next_action_before",
        }
    }

    /// 名前が完全に一致する値を返す。
    ///
    /// # Errors
    /// 未知の名前は使用できる名前の一覧を伴う `EngineError::FieldNotAllowed`。
    pub fn from_name(name: &str) -> Result<Self, EngineError> {
        Self::ALL
            .into_iter()
            .find(|value| value.name() == name)
            .ok_or_else(|| EngineError::FieldNotAllowed {
                allowed: Self::ALL
                    .into_iter()
                    .map(|value| value.name().to_owned())
                    .collect(),
            })
    }

    /// この項目で使用できる演算。候補の表示順で並ぶ。
    #[must_use]
    pub const fn operations(self) -> &'static [&'static str] {
        match self {
            Self::Region | Self::Industry | Self::Status | Self::Owner => &["eq", "in"],
            Self::HasPhone => &["eq"],
            Self::NextActionBefore => &["lte"],
        }
    }
}

/// 絞り込みの項目名と演算を完全一致で検査し、演算の固定文字列を返す。
///
/// # Errors
/// 未知の項目は全項目の全演算、許可外の演算はその項目の演算を
/// `key:op` で示す `EngineError::FieldNotAllowed`。
pub fn check_filter(key: &str, op: &str) -> Result<(FilterKey, &'static str), EngineError> {
    let key = FilterKey::from_name(key).map_err(|_| EngineError::FieldNotAllowed {
        allowed: FilterKey::ALL
            .into_iter()
            .flat_map(filter_operations)
            .collect(),
    })?;
    key.operations()
        .iter()
        .copied()
        .find(|allowed| *allowed == op)
        .map(|operation| (key, operation))
        .ok_or_else(|| EngineError::FieldNotAllowed {
            allowed: filter_operations(key).collect(),
        })
}

fn filter_operations(key: FilterKey) -> impl Iterator<Item = String> {
    key.operations()
        .iter()
        .map(move |op| format!("{}:{op}", key.name()))
}

/// 一覧の条件。空のベクトルは条件なし。値の妥当性検査は公開操作で行う。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// 地域（同じ種類）。
    pub region: Vec<Region>,
    /// 業種。
    pub industry: Vec<Industry>,
    /// 状態。
    pub status: Vec<Status>,
    /// 電話の有無。
    pub has_phone: Option<bool>,
    /// 担当営業。
    pub owner: Vec<UserId>,
    /// 次回対応日の上限（UNIX 秒）。
    pub next_action_before: Option<i64>,
    /// 除外を含める明示指定。
    pub include_excluded: bool,
}

/// 続きの位置を表す不透明なトークン。符号化・条件照合は読み取り段階で実装する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// 呼び出し側は内容を解釈しない。
    pub token: String,
}

/// 一覧の入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuery {
    /// 絞り込み。
    pub filters: Filters,
    /// 並び順。
    pub sort: Sort,
    /// 前のページの続き。
    pub after: Option<Cursor>,
    /// 最大200件。
    pub limit: u32,
    /// 明示して取得する項目。
    pub fields: Fields,
}

/// 担当営業の変更指定。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OwnerChange {
    /// 変更しない。
    Keep,
    /// 解除する。
    Clear,
    /// 担当営業を設定する。
    Set(UserId),
}

/// リストに対する一括変更。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkChange {
    /// 担当営業の変更指定。
    pub owner: OwnerChange,
    /// 指定時だけ活動で状態を変更。
    pub status: Option<Status>,
    /// 除外を含める明示指定。
    pub include_excluded: bool,
}

/// 社名検索の入力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// 前のページの続き。
    pub after: Option<Cursor>,
    /// 最大200件。
    pub limit: u32,
    /// 明示して取得する項目。
    pub fields: Fields,
    /// 除外を含める明示指定。
    pub include_excluded: bool,
    /// 先頭ページで厳密な件数も求める。続きのページでは無視する。
    pub with_count: bool,
}

/// 企業の取得結果。個人情報を選ばなければ該当値は `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Company {
    /// 内部ID。
    pub id: CompanyId,
    /// データ種別。
    pub origin: DataOrigin,
    /// 法人番号（13桁の文字列）。
    pub corporate_number: Option<String>,
    /// 入力どおりの名称。
    pub name: Option<String>,
    /// 都道府県コード。
    pub prefecture: u8,
    /// JIS 5桁の市区町村コード。
    pub municipality: Option<String>,
    /// 業種。
    pub industry: Option<Industry>,
    /// 代表電話。
    pub phone: Option<String>,
    /// ホームページ。
    pub website: Option<String>,
    /// 担当営業。
    pub owner: Option<UserId>,
    /// 現在の状態。
    pub status: Status,
    /// 個人事業の印。
    pub is_sole_proprietor: bool,
    /// 出典の識別子。
    pub source_id: Option<String>,
    /// 出典の基準日（YYYY-MM-DD）。
    pub source_as_of: Option<String>,
    /// 登録日時。
    pub created_at: i64,
    /// 消去日時。
    pub erased_at: Option<i64>,
    /// 最終活動日時。なしは0。
    pub last_activity_at: i64,
    /// 次回対応日時。
    pub next_action_at: Option<i64>,
}

/// 担当者の取得結果。未選択または消去済みの個人情報は `None`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    /// 内部ID。
    pub id: ContactId,
    /// 所属企業。
    pub company: CompanyId,
    /// 氏名。
    pub name: Option<String>,
    /// 電話。
    pub phone: Option<String>,
    /// メール。
    pub email: Option<String>,
    /// 登録日時。
    pub created_at: i64,
    /// 消去日時。
    pub erased_at: Option<i64>,
}

/// 活動の取得結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    /// 内部ID（記録順）。
    pub id: ActivityId,
    /// 企業。
    pub company: CompanyId,
    /// 同じ企業の担当者。
    pub contact: Option<ContactId>,
    /// 種類。
    pub kind: ActivityKind,
    /// 活動日時。
    pub at: i64,
    /// メモ。
    pub note: Option<String>,
    /// 適用後の状態。
    pub status_after: Option<Status>,
    /// 次回対応日の指定。
    pub next_action: NextAction,
    /// 取り消し対象。
    pub reverses: Option<ActivityId>,
    /// メモの消去日時。
    pub note_erased_at: Option<i64>,
}

/// 一覧の1ページ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// 企業。
    pub rows: Vec<Company>,
    /// 続き。
    pub next: Option<Cursor>,
    /// 速さを宣言していない条件。
    pub slow: bool,
}

/// 厳密な件数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counted {
    /// 件数。
    pub n: u64,
    /// 速さを宣言していない条件。
    pub slow: bool,
}

/// 検索の1ページ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    /// 企業。
    pub rows: Vec<Company>,
    /// 続き。
    pub next: Option<Cursor>,
    /// 先頭ページだけの厳密な件数。
    pub n: Option<u64>,
}

/// 企業の詳細。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detail {
    /// 企業。
    pub company: Company,
    /// 新しい順の担当者。
    pub contacts: Vec<Contact>,
    /// 日時の新しい順、同時刻はID降順の活動。
    pub activities: Vec<Activity>,
}

/// 保存先の検査結果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyReport {
    /// 整合性検査の一致。
    pub integrity_ok: bool,
    /// 件数表と再集計の一致。
    pub counts_match: bool,
    /// 活動の畳み直しとの一致。
    pub folds_match: bool,
}

/// 公開操作が返すエラー。
#[derive(Debug, Error)]
pub enum EngineError {
    /// 入力項目の値が不正。
    #[error("invalid input for {field}: {reason}")]
    InvalidInput {
        /// 不正な項目。
        field: String,
        /// 拒否の理由。
        reason: String,
    },
    /// この実装より新しい保存形式。
    #[error("unsupported storage version {found}; supported: {supported}")]
    UnsupportedVersion {
        /// 保存先の版。
        found: u32,
        /// この実装が扱える版。
        supported: u32,
    },
    /// 法人番号の桁・文字・検査用数字が不正。
    #[error("invalid corporate number: {reason}")]
    InvalidCorporateNumber {
        /// 拒否の理由。
        reason: String,
    },
    /// 他社と法人番号が重複。
    #[error("duplicate corporate number: {existing:?}")]
    DuplicateCorporateNumber {
        /// 登録済み企業。
        existing: CompanyId,
    },
    /// 名称が空。
    #[error("empty name")]
    EmptyName,
    /// 対象が存在しない。
    #[error("not found")]
    NotFound,
    /// 別企業の担当者。
    #[error("contact belongs to another company")]
    ForeignContact,
    /// 項目や演算が許可されていない。
    #[error("field or operation not allowed; allowed: {allowed:?}")]
    FieldNotAllowed {
        /// 使用できる項目と演算の説明。
        allowed: Vec<String>,
    },
    /// 検索語に英数字等がない。
    #[error("search term must contain an alphanumeric character")]
    InvalidSearchTerm,
    /// 除外からの状態変更には明示操作が必要。
    #[error("explicit release of exclusion required")]
    ExcludedCompany,
    /// 取り消し先が不正。
    #[error("invalid cancellation")]
    InvalidCancel,
    /// 状態の更新には活動を使う。
    #[error("status must be changed via activity")]
    StatusViaActivity,
    /// 保存先と入力のデータ種別が違う。
    #[error("data origin mismatch")]
    OriginMismatch,
    /// 書き込みに必要な空きがない。
    #[error("insufficient storage space")]
    InsufficientSpace,
    /// 保存形式の移行が必要。
    #[error("migration required")]
    NeedsMigration,
    /// 保存先が壊れている。複製からの復元が必要。
    #[error("corrupt storage; restore from a verified backup")]
    Corrupt,
    /// チェックポイントが完了しなかった。
    #[error("compaction incomplete")]
    CompactIncomplete,
    /// 保存処理の失敗。
    #[error("storage: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}
