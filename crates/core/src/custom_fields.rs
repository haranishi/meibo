//! 明示して要求した取引先の独自項目。既定の企業詳細やCSVには含めない。

use crate::sql::{Connection, OptionalExtension, Transaction, params, types::Value};
use crate::{
    Engine,
    schema::{Db, WRITE_BASE},
    types::{CompanyId, EngineError},
};

/// 取引先の独自項目の型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CustomFieldKind {
    /// 最大4000文字のテキスト。
    Text,
    /// 有限の数値。
    Number,
    /// 真偽値。
    Boolean,
}

/// 新しい項目の定義。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomFieldInput {
    /// 前後の空白を除いた名前。100文字まで。
    pub name: String,
    /// 値の型。作成後は変更しない。
    pub kind: CustomFieldKind,
    /// 専用一致検索の複合索引を使うか。
    pub indexed: bool,
}

/// 保存済みの項目定義。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomField {
    /// 項目ID。
    pub id: i64,
    /// 項目名。
    pub name: String,
    /// 値の型。
    pub kind: CustomFieldKind,
    /// 専用検索の索引の有無。
    pub indexed: bool,
}

/// 型を保つ独自項目の値。
#[derive(Debug, Clone, PartialEq)]
pub enum CustomFieldValue {
    /// テキスト。
    Text(String),
    /// 有限数。
    Number(f64),
    /// 真偽。
    Boolean(bool),
}

/// 明示して読み出した取引先の値。
#[derive(Debug, Clone, PartialEq)]
pub struct CompanyCustomValue {
    /// 項目ID。
    pub field_id: i64,
    /// 保存済みの値。
    pub value: CustomFieldValue,
}

/// 型の一致を検証する専用の完全一致検索。
#[derive(Debug, Clone, PartialEq)]
pub struct CustomFieldSearch {
    /// 検索する項目。
    pub field_id: i64,
    /// 完全一致する値。
    pub value: CustomFieldValue,
    /// 直前ページの末尾企業ID。
    pub after: Option<CompanyId>,
    /// 1〜200件。
    pub limit: u32,
}

/// 企業ID昇順の一致結果。既存一覧の性能契約とは別。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomFieldPage {
    /// 一致した未消去の企業ID。
    pub company_ids: Vec<CompanyId>,
    /// 続きがある場合の末尾ID。
    pub next: Option<CompanyId>,
    /// 値を含む複合索引を使用したか。
    pub indexed: bool,
}

const VERSION: u32 = 1;
const DDL: &str = r"
CREATE TABLE IF NOT EXISTS custom_field_definitions(
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(trim(name)) BETWEEN 1 AND 100),
 kind INTEGER NOT NULL CHECK(kind IN (1,2,3)),
 is_indexed INTEGER NOT NULL CHECK(is_indexed IN (0,1)),
 UNIQUE(id,kind,is_indexed)
);
CREATE TABLE IF NOT EXISTS custom_field_values(
 field_id INTEGER NOT NULL,
 company_id INTEGER NOT NULL REFERENCES companies(id),
 kind INTEGER NOT NULL,
 is_indexed INTEGER NOT NULL,
 text_value TEXT,
 number_value REAL,
 boolean_value INTEGER,
 PRIMARY KEY(field_id,company_id),
 FOREIGN KEY(field_id,kind,is_indexed)
  REFERENCES custom_field_definitions(id,kind,is_indexed) ON DELETE CASCADE,
 CHECK((kind=1 AND text_value IS NOT NULL AND length(text_value)<=4000
         AND number_value IS NULL AND boolean_value IS NULL)
    OR (kind=2 AND text_value IS NULL AND number_value IS NOT NULL
         AND number_value BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308
         AND boolean_value IS NULL)
    OR (kind=3 AND text_value IS NULL AND number_value IS NULL
         AND boolean_value IS NOT NULL AND boolean_value IN(0,1)))
);
CREATE INDEX IF NOT EXISTS custom_field_company ON custom_field_values(company_id,field_id);
CREATE INDEX IF NOT EXISTS custom_field_text_lookup
 ON custom_field_values(field_id,text_value,company_id) WHERE is_indexed=1 AND kind=1;
CREATE INDEX IF NOT EXISTS custom_field_number_lookup
 ON custom_field_values(field_id,number_value,company_id) WHERE is_indexed=1 AND kind=2;
CREATE INDEX IF NOT EXISTS custom_field_boolean_lookup
 ON custom_field_values(field_id,boolean_value,company_id) WHERE is_indexed=1 AND kind=3;
";

/// 呼出側の初期化トランザクションに互換拡張を加える。
pub(crate) fn initialize(tx: &Transaction<'_>) -> Result<(), EngineError> {
    let found: Option<String> = tx
        .query_row(
            "SELECT value FROM meta WHERE key='custom_fields_schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(found) = found {
        let found = found.parse::<u32>().map_err(|_| EngineError::Corrupt)?;
        if found > VERSION {
            return Err(EngineError::UnsupportedVersion {
                found,
                supported: VERSION,
            });
        }
        if found < VERSION {
            return Err(EngineError::NeedsMigration);
        }
    }
    tx.execute_batch(DDL).map_err(storage)?;
    tx.execute(
        "INSERT OR IGNORE INTO meta(key,value) VALUES('custom_fields_schema_version',?1)",
        [VERSION.to_string()],
    )
    .map_err(storage)?;
    Ok(())
}

/// 企業の消去と同じトランザクション内で、独自項目の値も消す。
pub(crate) fn erase_values(tx: &Transaction<'_>, company: CompanyId) -> Result<(), EngineError> {
    tx.execute(
        "DELETE FROM custom_field_values WHERE company_id=?1",
        [company.0],
    )
    .map_err(storage)?;
    Ok(())
}

/// 消去で触れる行・本文・索引とWALの余裕を見積もる。
pub(crate) fn erase_estimate(db: &Db, company: CompanyId) -> Result<u64, EngineError> {
    deletion_estimate(
        db,
        "SELECT count(*),coalesce(sum(length(CAST(text_value AS BLOB))),0) FROM custom_field_values WHERE company_id=?1",
        company.0,
    )
}

impl Engine {
    /// 項目を作成する。
    /// # Errors
    /// 名前・上限違反、同名、保存失敗。
    pub fn create_custom_field(&mut self, input: &CustomFieldInput) -> Result<i64, EngineError> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
            return Err(invalid(
                "name",
                "expected 1..100 characters without controls",
            ));
        }
        self.db.write(WRITE_BASE+8192, |tx| {
            let count:i64=tx.query_row("SELECT count(*) FROM custom_field_definitions",[],|r|r.get(0)).map_err(storage)?;
            if count>=100 {return Err(invalid("custom_fields", "definition limit exceeded"));}
            let exists:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM custom_field_definitions WHERE name=?1 COLLATE NOCASE)",[name],|r|r.get(0)).map_err(storage)?;
            if exists {return Err(invalid("name", "name already exists"));}
            tx.execute("INSERT INTO custom_field_definitions(name,kind,is_indexed) VALUES(?1,?2,?3)",params![name,input.kind.code(),input.indexed]).map_err(storage)?;
            Ok(tx.last_insert_rowid())
        })
    }
    /// 定義をID順で返す。
    /// # Errors
    /// 保存先の読み取り失敗。
    pub fn custom_fields(&self) -> Result<Vec<CustomField>, EngineError> {
        let mut statement = self
            .db
            .conn()
            .prepare("SELECT id,name,kind,is_indexed FROM custom_field_definitions ORDER BY id")
            .map_err(storage)?;
        statement
            .query_map([], read_definition)
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)
    }
    /// 項目とその値を消す。
    /// # Errors
    /// 不在、保存失敗。
    pub fn delete_custom_field(&mut self, id: i64) -> Result<(), EngineError> {
        valid_id(id)?;
        let estimate = deletion_estimate(
            &self.db,
            "SELECT count(*),coalesce(sum(length(CAST(text_value AS BLOB))),0) FROM custom_field_values WHERE field_id=?1",
            id,
        )?;
        self.db.write(WRITE_BASE.saturating_add(estimate), |tx| {
            definition(tx, id)?;
            tx.execute("DELETE FROM custom_field_definitions WHERE id=?1", [id])
                .map_err(storage)?;
            Ok(())
        })
    }
    /// 未消去企業へ型の合う値を保存する。
    /// # Errors
    /// 不在、型・上限違反、保存失敗。
    pub fn set_custom_value(
        &mut self,
        company: CompanyId,
        field: i64,
        value: &CustomFieldValue,
    ) -> Result<(), EngineError> {
        valid_id(field)?;
        validate_value(value)?;
        let estimate = WRITE_BASE
            .saturating_add(32_768)
            .saturating_add(match value {
                CustomFieldValue::Text(text) => (text.len() as u64).saturating_mul(16),
                _ => 0,
            });
        self.db.write(estimate,|tx|{
            target(tx,company)?;
            let field=definition(tx,field)?;
            matching_type(field.kind,value)?;
            let (text,number,boolean)=match value {
                CustomFieldValue::Text(text)=>(Some(text.as_str()),None,None),
                CustomFieldValue::Number(number)=>(None,Some(*number),None),
                CustomFieldValue::Boolean(boolean)=>(None,None,Some(*boolean)),
            };
            tx.execute("INSERT INTO custom_field_values(field_id,company_id,kind,is_indexed,text_value,number_value,boolean_value) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(field_id,company_id) DO UPDATE SET text_value=excluded.text_value,number_value=excluded.number_value,boolean_value=excluded.boolean_value",params![field.id,company.0,field.kind.code(),field.indexed,text,number,boolean]).map_err(storage)?;
            Ok(())
        })
    }
    /// 1つの値を解除する。
    /// # Errors
    /// 定義・企業不在、保存失敗。
    pub fn clear_custom_value(
        &mut self,
        company: CompanyId,
        field: i64,
    ) -> Result<(), EngineError> {
        valid_id(field)?;
        self.db.write(WRITE_BASE + 524_288, |tx| {
            target(tx, company)?;
            definition(tx, field)?;
            tx.execute(
                "DELETE FROM custom_field_values WHERE company_id=?1 AND field_id=?2",
                (company.0, field),
            )
            .map_err(storage)?;
            Ok(())
        })
    }
    /// 明示的な要求にだけ値を返す。
    /// # Errors
    /// 企業不在、読み取り失敗。
    pub fn custom_values(
        &self,
        company: CompanyId,
    ) -> Result<Vec<CompanyCustomValue>, EngineError> {
        target(self.db.conn(), company)?;
        let mut statement=self.db.conn().prepare("SELECT field_id,kind,text_value,number_value,boolean_value FROM custom_field_values WHERE company_id=?1 ORDER BY field_id").map_err(storage)?;
        statement
            .query_map([company.0], |row| {
                let value = match row.get::<_, i64>(1)? {
                    1 => CustomFieldValue::Text(row.get(2)?),
                    2 => CustomFieldValue::Number(row.get(3)?),
                    3 => CustomFieldValue::Boolean(row.get(4)?),
                    _ => return Err(crate::sql::Error::InvalidQuery),
                };
                Ok(CompanyCustomValue {
                    field_id: row.get(0)?,
                    value,
                })
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)
    }
    /// 独自項目の一致値で検索する。
    /// # Errors
    /// 定義不在、不正な値・件数・続き位置、読み取り失敗。
    pub fn search_custom_field(
        &self,
        query: &CustomFieldSearch,
    ) -> Result<CustomFieldPage, EngineError> {
        valid_id(query.field_id)?;
        if query.limit == 0 || query.limit > 200 {
            return Err(invalid("limit", "expected 1..200"));
        }
        if let Some(after) = query.after {
            valid_id(after.0)?;
        }
        validate_value(&query.value)?;
        let field = definition(self.db.conn(), query.field_id)?;
        matching_type(field.kind, &query.value)?;
        let sql = search_sql(field.kind, field.indexed);
        let mut statement = self.db.conn().prepare(sql).map_err(storage)?;
        let mut ids = statement
            .query_map(
                params![
                    field.id,
                    query.after.map_or(0, |id| id.0),
                    query.value.sql_value(),
                    i64::from(query.limit) + 1
                ],
                |row| row.get::<_, i64>(0).map(CompanyId),
            )
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?;
        let limit =
            usize::try_from(query.limit).map_err(|_| invalid("limit", "unsupported size"))?;
        let next = if ids.len() > limit {
            ids.truncate(limit);
            ids.last().copied()
        } else {
            None
        };
        Ok(CustomFieldPage {
            company_ids: ids,
            next,
            indexed: field.indexed,
        })
    }
}

impl CustomFieldKind {
    const fn code(self) -> i64 {
        match self {
            Self::Text => 1,
            Self::Number => 2,
            Self::Boolean => 3,
        }
    }
}

impl CustomFieldValue {
    fn sql_value(&self) -> Value {
        match self {
            Self::Text(text) => Value::Text(text.clone()),
            Self::Number(number) => Value::Real(*number),
            Self::Boolean(boolean) => Value::Integer(i64::from(*boolean)),
        }
    }
}

fn storage(error: crate::sql::Error) -> EngineError {
    EngineError::Storage(Box::new(error))
}
fn invalid(field: &str, reason: &str) -> EngineError {
    EngineError::InvalidInput {
        field: field.into(),
        reason: reason.into(),
    }
}
fn valid_id(id: i64) -> Result<(), EngineError> {
    if id > 0 {
        Ok(())
    } else {
        Err(invalid("id", "expected positive identifier"))
    }
}

fn read_definition(row: &crate::sql::Row<'_>) -> crate::sql::Result<CustomField> {
    let kind = match row.get::<_, i64>(2)? {
        1 => CustomFieldKind::Text,
        2 => CustomFieldKind::Number,
        3 => CustomFieldKind::Boolean,
        _ => return Err(crate::sql::Error::InvalidQuery),
    };
    Ok(CustomField {
        id: row.get(0)?,
        name: row.get(1)?,
        kind,
        indexed: row.get(3)?,
    })
}

fn definition(conn: &Connection, id: i64) -> Result<CustomField, EngineError> {
    conn.query_row(
        "SELECT id,name,kind,is_indexed FROM custom_field_definitions WHERE id=?1",
        [id],
        read_definition,
    )
    .optional()
    .map_err(storage)?
    .ok_or(EngineError::NotFound)
}

fn target(conn: &Connection, company: CompanyId) -> Result<(), EngineError> {
    valid_id(company.0)?;
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM companies WHERE id=?1 AND erased_at IS NULL)",
            [company.0],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if exists {
        Ok(())
    } else {
        Err(EngineError::NotFound)
    }
}

fn validate_value(value: &CustomFieldValue) -> Result<(), EngineError> {
    match value {
        CustomFieldValue::Text(text) if text.chars().count() > 4000 || text.contains('\0') => Err(
            invalid("value", "text exceeds 4000 characters or contains NUL"),
        ),
        CustomFieldValue::Number(number) if !number.is_finite() => {
            Err(invalid("value", "expected finite number"))
        }
        _ => Ok(()),
    }
}

fn matching_type(kind: CustomFieldKind, value: &CustomFieldValue) -> Result<(), EngineError> {
    if matches!(
        (kind, value),
        (CustomFieldKind::Text, CustomFieldValue::Text(_))
            | (CustomFieldKind::Number, CustomFieldValue::Number(_))
            | (CustomFieldKind::Boolean, CustomFieldValue::Boolean(_))
    ) {
        Ok(())
    } else {
        Err(invalid("value", "type does not match field definition"))
    }
}

fn deletion_estimate(db: &Db, sql: &str, id: i64) -> Result<u64, EngineError> {
    let (rows, bytes): (i64, i64) = db
        .conn()
        .query_row(sql, [id], |r| Ok((r.get(0)?, r.get(1)?)))
        .map_err(storage)?;
    let rows = u64::try_from(rows).map_err(|_| EngineError::Corrupt)?;
    let bytes = u64::try_from(bytes).map_err(|_| EngineError::Corrupt)?;
    if rows == 0 {
        return Ok(0);
    }
    let page_size: u32 = db
        .conn()
        .pragma_query_value(None, "page_size", |r| r.get(0))
        .map_err(storage)?;
    Ok(u64::from(page_size)
        .saturating_mul(rows.saturating_mul(8).saturating_add(16))
        .saturating_add(bytes.saturating_mul(16)))
}

fn search_sql(kind: CustomFieldKind, indexed: bool) -> &'static str {
    match (kind, indexed) {
        (CustomFieldKind::Text, true) => {
            "SELECT v.company_id FROM custom_field_values v INDEXED BY custom_field_text_lookup JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.text_value=?3 AND v.is_indexed=1 AND v.kind=1 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
        (CustomFieldKind::Number, true) => {
            "SELECT v.company_id FROM custom_field_values v INDEXED BY custom_field_number_lookup JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.number_value=?3 AND v.is_indexed=1 AND v.kind=2 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
        (CustomFieldKind::Boolean, true) => {
            "SELECT v.company_id FROM custom_field_values v INDEXED BY custom_field_boolean_lookup JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.boolean_value=?3 AND v.is_indexed=1 AND v.kind=3 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
        (CustomFieldKind::Text, false) => {
            "SELECT v.company_id FROM custom_field_values v JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.text_value=?3 AND v.is_indexed=0 AND v.kind=1 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
        (CustomFieldKind::Number, false) => {
            "SELECT v.company_id FROM custom_field_values v JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.number_value=?3 AND v.is_indexed=0 AND v.kind=2 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
        (CustomFieldKind::Boolean, false) => {
            "SELECT v.company_id FROM custom_field_values v JOIN companies c ON c.id=v.company_id WHERE v.field_id=?1 AND v.company_id>?2 AND v.boolean_value=?3 AND v.is_indexed=0 AND v.kind=3 AND c.erased_at IS NULL ORDER BY v.company_id LIMIT ?4"
        }
    }
}

#[cfg(test)]
#[path = "custom_fields_tests.rs"]
mod tests;
