//! 保存・検索・活動・消去を束ねる公開口。
use std::path::Path;

use crate::{
    bulk::{self, BulkOptions},
    erase, lists, query,
    schema::{Db, OpenOptions, counts},
    search,
    synth::{Synth, SynthConfig},
    types::{
        ActivityId, ActivityInput, BulkChange, Company, CompanyId, CompanyInput, ContactId,
        ContactInput, Counted, Cursor, DataOrigin, Detail, DuplicatePolicy, EngineError, Fields,
        Filters, ImportBatchError, ImportBatchReport, ListId, ListQuery, Page, SearchOptions,
        SearchPage, Status, VerifyReport,
    },
    write,
};

/// 1つの保存先に対する操作。書き込みは可変借用で直列化する。
pub struct Engine {
    pub(crate) db: Db,
}

/// 合成データの追加結果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadReport {
    /// 追加した企業数。
    pub companies: u64,
    /// 追加した活動数。
    pub activities: u64,
}

impl Engine {
    /// Create a CRM schema on an empty, explicitly identified remote database.
    ///
    /// # Errors
    /// Rejects invalid configuration, existing content and storage failures.
    #[cfg(feature = "cloud")]
    pub fn create_remote(
        endpoint: meibo_db::RemoteOptions,
        identity: &crate::RemoteIdentity,
        origin: DataOrigin,
        opt: &OpenOptions,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            db: Db::create_remote(endpoint, identity, origin, opt)?,
        })
    }

    /// Open a remote CRM database only when its recorded identity is valid.
    ///
    /// # Errors
    /// Rejects missing, incompatible or mismatched metadata and storage failures.
    #[cfg(feature = "cloud")]
    pub fn open_remote(
        endpoint: meibo_db::RemoteOptions,
        identity: &crate::RemoteIdentity,
        opt: &OpenOptions,
    ) -> Result<Self, EngineError> {
        Ok(Self {
            db: Db::open_remote(endpoint, identity, opt)?,
        })
    }

    /// Whether the selected SQL connection forbids further operations.
    #[cfg(feature = "cloud")]
    #[must_use]
    pub fn is_poisoned(&self) -> bool {
        self.db.conn().is_poisoned()
    }

    /// 新しい保存先を作る。既存ファイルは上書きしない。
    /// # Errors
    /// `InvalidInput`（設定）、`Corrupt`（DB破損）、`Storage`（作成・初期化の失敗）。
    pub fn create(path: &Path, origin: DataOrigin, opt: &OpenOptions) -> Result<Self, EngineError> {
        Ok(Self {
            db: Db::create(path, origin, opt)?,
        })
    }
    /// 保存形式の版・種別と、設定に応じた整合性を確かめて保存先を開く。
    /// # Errors
    /// `InvalidInput`、`NeedsMigration`、`UnsupportedVersion`、`Corrupt`、`Storage`。
    pub fn open(path: &Path, opt: &OpenOptions) -> Result<Self, EngineError> {
        Ok(Self {
            db: Db::open(path, opt)?,
        })
    }
    /// 必要な移行経路があれば複製後に前方移行する。現行版は変更しない。
    /// # Errors
    /// `InvalidInput`、`NeedsMigration`、`UnsupportedVersion`、`InsufficientSpace`、`Corrupt`、`Storage`。
    pub fn migrate(path: &Path, opt: &OpenOptions) -> Result<(), EngineError> {
        Db::migrate(path, opt)
    }
    /// 同梱している SQLite の版。測定の記録に残すための口。
    #[must_use]
    pub fn sqlite_version() -> &'static str {
        rusqlite::version()
    }
    /// 測定用。背景の書き戻しを接続の待ち時間まで待つ。互換性は保証しない。
    /// 完了または背景スレッドなしなら `true`、`busy_timeout` の期限切れなら `false`。
    /// # Errors
    /// チェックポイントに失敗した場合は `Storage`。
    #[doc(hidden)]
    pub fn settle(&self) -> Result<bool, EngineError> {
        self.db.settle()
    }
    /// 保存先に記録されたデータ種別。
    #[must_use]
    pub fn origin(&self) -> DataOrigin {
        self.db.origin()
    }
    /// 保存ファイルのパス。
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.db.conn().path().map(Path::new)
    }
    /// 種別と入力を検証し、企業と検索キー・件数を同じ確定で登録する。
    /// # Errors
    /// `OriginMismatch`、`EmptyName`、`InvalidCorporateNumber`、`InvalidInput`、`DuplicateCorporateNumber`、`Corrupt`、`Storage`。
    pub fn register_company(&mut self, c: &CompanyInput) -> Result<CompanyId, EngineError> {
        write::register_company(&mut self.db, c)
    }
    /// 最大500社を全成功または全失敗で追加する。重複のスキップでも既存行は変えない。
    /// # Errors
    /// 入力・種別・重複・容量・保存の失敗は、バッチ内の行位置を添えて返す。
    /// 失敗したバッチの企業・全文索引・件数は残らない。
    pub fn import_companies(
        &mut self,
        companies: &[CompanyInput],
        duplicates: DuplicatePolicy,
    ) -> Result<ImportBatchReport, ImportBatchError> {
        bulk::import_companies(&mut self.db, companies, duplicates)
    }
    /// 現在の状態を保って企業情報と検索キー・件数を更新する。
    /// # Errors
    /// `NotFound`、`OriginMismatch`、`EmptyName`、`InvalidCorporateNumber`、`InvalidInput`、`StatusViaActivity`、`DuplicateCorporateNumber`、`Corrupt`、`Storage`。
    pub fn update_company(&mut self, id: CompanyId, c: &CompanyInput) -> Result<(), EngineError> {
        write::update_company(&mut self.db, id, c)
    }
    /// 未消去の企業に担当者を追加する。
    /// # Errors
    /// `EmptyName`（空の氏名）、`NotFound`（企業不在・消去済み）、`Corrupt`、`Storage`。
    pub fn add_contact(&mut self, c: &ContactInput) -> Result<ContactId, EngineError> {
        write::add_contact(&mut self.db, c)
    }
    /// 所属企業を固定したまま、未消去の担当者の氏名と連絡先を更新する。
    /// # Errors
    /// `EmptyName`、`InvalidInput`、`NotFound`、`ForeignContact`、`InsufficientSpace`、`Storage`。
    pub fn update_contact(&mut self, id: ContactId, c: &ContactInput) -> Result<(), EngineError> {
        write::update_contact(&mut self.db, id, c)
    }
    /// 活動を追記し、企業の状態・最終活動日・次回対応日を同じ確定で更新する。
    /// # Errors
    /// `NotFound`、`InvalidInput`、`ForeignContact`、`InvalidCancel`、`ExcludedCompany`、`Corrupt`、`Storage`。
    pub fn record_activity(&mut self, a: &ActivityInput) -> Result<ActivityId, EngineError> {
        write::record_activity(&mut self.db, a)
    }
    /// 除外中の企業に状態変更の活動を記録し、指定の状態へ解除する。
    /// # Errors
    /// `NotFound`、`InvalidInput`（除外状態・解除先・消去済み）、`Corrupt`、`Storage`。
    pub fn release_exclusion(
        &mut self,
        id: CompanyId,
        to: Status,
    ) -> Result<ActivityId, EngineError> {
        write::release_exclusion(&mut self.db, id, to)
    }
    /// 固定リストを削除し、企業と活動を残す。
    /// # Errors
    /// `NotFound`（リスト不在）、`Storage`（削除の失敗）。
    pub fn delete_list(&mut self, id: ListId) -> Result<(), EngineError> {
        lists::delete_list(&mut self.db, id)
    }
    /// リスト内の未消去の対象企業の担当営業と状態を一括変更し、対象数を返す。
    /// # Errors
    /// `NotFound`（リスト不在）、`ExcludedCompany`（除外の解除要求）、`Corrupt`、`Storage`。
    pub fn bulk_update(&mut self, list: ListId, change: &BulkChange) -> Result<u64, EngineError> {
        lists::bulk_update(&mut self.db, list, change)
    }
    /// 担当者の個人情報を消し、指定時には紐づく活動メモも消す。
    /// # Errors
    /// `NotFound`（担当者不在）、`Storage`（消去の失敗）。
    pub fn erase_contact(&mut self, id: ContactId, wipe_notes: bool) -> Result<(), EngineError> {
        erase::erase_contact(&mut self.db, id, wipe_notes)
    }
    /// 個人事業の企業を除外し、企業・担当者の個人情報と活動メモを消す。
    /// # Errors
    /// `NotFound`、`InvalidInput`（個人事業でない）、`Corrupt`、`Storage`。
    pub fn erase_company(&mut self, id: CompanyId) -> Result<(), EngineError> {
        erase::erase_company(&mut self.db, id)
    }
    /// 全文索引と保存ファイルを再構築し、WALの切り詰めまで行う。
    /// # Errors
    /// `InvalidInput`（ファイルなし）、`InsufficientSpace`、`CompactIncomplete`（読取中）、`Storage`。
    pub fn compact(&mut self) -> Result<(), EngineError> {
        erase::compact(&mut self.db)
    }
    /// 選択された個人情報を含む企業・担当者と、新しい順の活動を返す。
    /// # Errors
    /// `NotFound`（企業不在）、`Storage`（読取・値変換の失敗）。
    pub fn company_detail(&self, id: CompanyId, f: &Fields) -> Result<Detail, EngineError> {
        query::company_detail(&self.db, id, f)
    }
    /// 条件に一致する一覧ページと続きのカーソル・遅い条件の印を返す。
    /// # Errors
    /// `InvalidInput`（条件・並び・上限・カーソル）、`Storage`。
    pub fn list(&self, q: &ListQuery) -> Result<Page, EngineError> {
        query::list(&self.db, q)
    }
    /// 同じ読み取り時点の企業を逐次処理する。消去済みは常に省く。
    /// 個人情報の選択と除外の指定は一覧と同じ規則で扱う。
    /// # Errors
    /// `InvalidInput`（条件）、`Storage`（読取・値変換）、またはcallbackの失敗。
    pub fn visit_companies(
        &self,
        filters: &Filters,
        fields: &Fields,
        visitor: impl FnMut(Company) -> Result<(), EngineError>,
    ) -> Result<u64, EngineError> {
        query::visit_companies(&self.db, filters, fields, visitor)
    }
    /// 条件に一致する厳密な件数と遅い条件の印を返す。
    /// # Errors
    /// `InvalidInput`（条件）、`Storage`（集計の失敗）。
    pub fn count(&self, f: &Filters) -> Result<Counted, EngineError> {
        query::count(&self.db, f)
    }
    /// 正規化した社名の部分一致を登録順で返し、指定時は先頭ページに件数を付ける。
    /// # Errors
    /// `InvalidSearchTerm`、`InvalidInput`（上限・カーソル）、`Corrupt`（件数不整合）、`Storage`。
    pub fn search_name(&self, term: &str, o: &SearchOptions) -> Result<SearchPage, EngineError> {
        search::search_name(&self.db, term, o)
    }
    /// 空きを確認し、既存ファイルを上書きせずに保存先のオンライン複製を作る。
    /// # Errors
    /// `InsufficientSpace`（空き不足）、`Corrupt`、`Storage`（複製の失敗）。
    pub fn backup_to(&self, dest: &Path) -> Result<(), EngineError> {
        self.db.backup_to(dest)
    }
    /// 絞り込み結果を最大10万社の固定リストにする。
    /// # Errors
    /// `InvalidInput`（条件・名前・上限）、`Storage`（作成の失敗）。
    pub fn create_list(
        &mut self,
        name: &str,
        from: &Filters,
        limit: u32,
    ) -> Result<ListId, EngineError> {
        let (sql, args) = query::select_ids_sql(from, limit)?;
        lists::create_list_from_select(&mut self.db, name, &sql, &args, limit)
    }
    /// 指定件数・分布の合成企業と活動を同じ確定で追加する。
    /// # Errors
    /// `OriginMismatch`、`DuplicateCorporateNumber`、`InsufficientSpace`、`Corrupt`、`Storage`。
    pub fn load_synthetic(&mut self, config: SynthConfig) -> Result<LoadReport, EngineError> {
        if config.rows > 300_000_000_000 {
            return Err(EngineError::InvalidInput {
                field: "rows".into(),
                reason: "must be at most 300000000000".into(),
            });
        }
        self.db.check_origin(DataOrigin::Synthetic)?;
        let report = bulk::load(
            &mut self.db,
            Synth::new(config).map(Into::into),
            Some(config.rows),
            &BulkOptions::default(),
        )?;
        // 読み取り中の接続があっても追加の成功は取り消さない。
        if self.db.has_local_storage() {
            let _ = self
                .db
                .conn()
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
        }
        Ok(LoadReport {
            companies: report.companies,
            activities: report.activities,
        })
    }
    /// 整合性・件数表・活動の畳み込みを照合し、不一致を各フラグで返す。
    /// # Errors
    /// `Corrupt`（活動のコード不正）、`Storage`（整合性検査・活動照合の失敗）。
    pub fn verify(&self) -> Result<VerifyReport, EngineError> {
        // 3つの検査を同じ読み取り時点にそろえる。
        let tx = self
            .db
            .conn()
            .unchecked_transaction()
            .map_err(|e| EngineError::Storage(Box::new(e)))?;
        let integrity_ok = match self.db.verify_integrity() {
            Ok(()) => true,
            Err(EngineError::Corrupt) => false,
            Err(e) => return Err(e),
        };
        Ok(VerifyReport {
            integrity_ok,
            counts_match: counts::matches_recount(&tx),
            folds_match: write::verify_folds(&tx)?,
        })
    }
    /// 計画試験向けの実行計画を返す。互換性は保証しない。
    /// # Errors
    /// `InvalidInput`（条件・並び・上限・カーソル）、`Storage`。
    #[doc(hidden)]
    pub fn explain(&self, q: &ListQuery) -> Result<Vec<String>, EngineError> {
        query::explain(&self.db, q)
    }
    /// afterを無視し、先頭からoffset件の直後のカーソルを走査して返す。0・総数以上はなし。測定用で互換性は保証しない。
    /// # Errors
    /// `InvalidInput`（条件・並び・上限）、`Storage`。
    #[doc(hidden)]
    pub fn cursor_after_offset(
        &self,
        q: &ListQuery,
        offset: u64,
    ) -> Result<Option<Cursor>, EngineError> {
        query::cursor_after_offset(&self.db, q, offset)
    }
}
