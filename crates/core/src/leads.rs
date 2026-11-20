//! 同意を得たWeb問い合わせを組織内で受け付け、営業担当が個別に引き継ぐ。
use serde::{Deserialize, Serialize};

use crate::{
    Engine, crm,
    query::{invalid, storage},
    schema::WRITE_BASE,
    sql::{Connection, OptionalExtension, Row, params},
    types::EngineError,
};

mod conversion;
pub use conversion::{Conversion, NewOpportunity};

/// 公開フォームから受け取れる項目。会社・担当者IDや割当先は受け付けない。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Inquiry {
    /// ブラウザが送信ごとに生成する乱数。失敗後の再送では同じ値を使う。
    pub submission_key: String,
    /// 会社名・団体名。
    pub company_name: String,
    /// 問い合わせた方の名前。
    pub contact_name: String,
    /// 返信先メールアドレス。
    pub email: String,
    /// 任意の問い合わせ内容。
    pub message: Option<String>,
    /// 問い合わせに対応するための連絡・記録への同意。
    pub consent: bool,
}

/// 組織内の受付記録。個人情報は明示的な個別表示のときだけ返す。
#[derive(Debug, Serialize)]
pub struct Lead {
    /// 組織内の受付番号。
    pub id: i64,
    /// 同意を得て受け付けた時刻（UNIX秒）。
    pub created_at: i64,
    /// 割り当てた営業担当。
    pub owner_id: Option<i64>,
    /// 競合を検知する版。
    pub revision: i64,
    /// 0 未処理、1 変換済み、2 破棄済み。
    pub state: u8,
    /// 引き継いだ取引先。
    pub company_id: Option<i64>,
    /// 引き継いだ担当者。
    pub contact_id: Option<i64>,
    /// 引き継いだ商談。
    pub opportunity_id: Option<i64>,
    /// 個別表示時の団体名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub company_name: Option<String>,
    /// 個別表示時の氏名。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contact_name: Option<String>,
    /// 個別表示時のメールアドレス。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// 個別表示時のメッセージ。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

const COLUMNS: &str = "id,created_at,owner_id,revision,state,company_id,contact_id,opportunity_id";

fn read_lead(row: &Row<'_>, reveal: bool) -> crate::sql::Result<Lead> {
    Ok(Lead {
        id: row.get(0)?,
        created_at: row.get(1)?,
        owner_id: row.get(2)?,
        revision: row.get(3)?,
        state: row.get(4)?,
        company_id: row.get(5)?,
        contact_id: row.get(6)?,
        opportunity_id: row.get(7)?,
        company_name: if reveal { row.get(8)? } else { None },
        contact_name: if reveal { row.get(9)? } else { None },
        email: if reveal { row.get(10)? } else { None },
        message: if reveal { row.get(11)? } else { None },
    })
}

fn find_lead(conn: &Connection, id: i64, reveal: bool) -> Result<Lead, EngineError> {
    let personal = if reveal {
        ",company_name,contact_name,email,message"
    } else {
        ""
    };
    conn.query_row(
        &format!("SELECT {COLUMNS}{personal} FROM crm_leads WHERE id=?1"),
        [id],
        |r| read_lead(r, reveal),
    )
    .optional()
    .map_err(storage)?
    .ok_or(EngineError::NotFound)
}

/// 128bitの公開IDまたは二重送信防止キーの共通形式。
#[must_use]
pub fn valid_key(key: &str) -> bool {
    key.len() == 32
        && key
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_text(value: &str, max: usize, multiline: bool) -> bool {
    !value.trim().is_empty()
        && value.chars().count() <= max
        && !value
            .chars()
            .any(|c| c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
}

fn validate(inquiry: &Inquiry) -> Result<(), EngineError> {
    let email = inquiry.email.trim();
    let valid_email = email.len() <= 254
        && email.is_ascii()
        && !email
            .bytes()
            .any(|b| b.is_ascii_whitespace() || b.is_ascii_control())
        && email.split_once('@').is_some_and(|(name, domain)| {
            !name.is_empty() && !domain.is_empty() && !domain.contains('@')
        });
    if !inquiry.consent
        || !valid_key(&inquiry.submission_key)
        || !valid_text(inquiry.company_name.trim(), 200, false)
        || !valid_text(inquiry.contact_name.trim(), 100, false)
        || !valid_email
        || inquiry
            .message
            .as_deref()
            .map(str::trim)
            .is_some_and(|v| !v.is_empty() && !valid_text(v, 1000, true))
    {
        return Err(invalid("inquiry", "invalid form submission"));
    }
    Ok(())
}

fn form_key(conn: &Connection) -> Result<Option<String>, EngineError> {
    if crm::version(conn)? < 3 {
        return Ok(None);
    }
    conn.query_row("SELECT public_key FROM crm_lead_form WHERE id=1", [], |r| {
        r.get(0)
    })
    .optional()
    .map_err(storage)
}

impl Engine {
    /// 有効な受付フォームの公開ID。旧データベースでは初期状態は無効。
    /// # Errors
    /// 保存先を読めない場合。
    pub fn lead_form(&self) -> Result<Option<String>, EngineError> {
        form_key(self.db.conn())
    }

    /// 管理者の操作。再度有効にしても同じURLを返し、停止後に再開するとURLを変える。
    /// # Errors
    /// 不正な乱数、版の不一致、保存の失敗。
    pub fn set_lead_form(&mut self, key: Option<&str>) -> Result<Option<String>, EngineError> {
        if key.is_some_and(|k| !valid_key(k)) {
            return Err(invalid("form", "invalid public identifier"));
        }
        self.db.write(WRITE_BASE, |tx| {
            crm::ensure_sales_ops(tx)?;
            if let Some(key)=key {
                tx.execute("INSERT INTO crm_lead_form(id,public_key) VALUES(1,?1) ON CONFLICT(id) DO NOTHING",[key]).map_err(storage)?;
            } else {
                tx.execute("DELETE FROM crm_lead_form WHERE id=1",[]).map_err(storage)?;
            }
            form_key(tx)
        })
    }

    /// 同意済み問い合わせだけを受信し、営業記録を作る前に担当者の確認を待つ。
    /// # Errors
    /// フォーム停止、不正な入力、受付上限、保存の失敗。
    pub fn receive_lead(
        &mut self,
        key: &str,
        inquiry: &Inquiry,
        at: i64,
    ) -> Result<(), EngineError> {
        if at < 0 || !valid_key(key) {
            return Err(EngineError::NotFound);
        }
        validate(inquiry)?;
        let company = inquiry.company_name.trim();
        let name = inquiry.contact_name.trim();
        let email = inquiry.email.trim();
        let message = inquiry
            .message
            .as_deref()
            .map(str::trim)
            .filter(|v| !v.is_empty());
        self.db.write(WRITE_BASE + 32768, |tx| {
            if form_key(tx)?.as_deref() != Some(key) { return Err(EngineError::NotFound); }
            // A completed or discarded receipt retains no personal data to compare.
            let replay: Option<bool> = tx.query_row(
                "SELECT state<>0 OR (company_name=?2 AND contact_name=?3 AND email=?4 AND message IS ?5)
                 FROM crm_leads WHERE submission_key=?1",
                params![inquiry.submission_key,company,name,email,message], |row| row.get(0)
            ).optional().map_err(storage)?;
            if let Some(same) = replay {
                return if same { Ok(()) } else { Err(EngineError::RevisionConflict) };
            }
            let full: bool = tx.query_row(
                "SELECT (SELECT count(*) FROM (SELECT 1 FROM crm_leads WHERE created_at>=?1 LIMIT 30))>=30
                 OR (SELECT count(*) FROM (SELECT 1 FROM crm_leads WHERE state=0 LIMIT 1000))>=1000",
                [at.saturating_sub(900)], |r| r.get(0)
            ).map_err(storage)?;
            if full { return Err(invalid("inbox_capacity","form intake limit")); }
            tx.execute(
                "INSERT INTO crm_leads(submission_key,created_at,company_name,contact_name,email,message)
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![inquiry.submission_key,at,company,name,email,message]
            ).map_err(storage)?;
            Ok(())
        })
    }

    /// 新着順に受付記録を取得する。個人の名前・連絡先は返さない。
    /// # Errors
    /// 不正な状態・カーソル・保存先の読取失敗。
    pub fn leads(&self, state: u8, after: Option<i64>) -> Result<Vec<Lead>, EngineError> {
        if state > 2 || after.is_some_and(|v| v <= 0) {
            return Err(invalid("leads", "invalid list options"));
        }
        let conn = self.db.conn();
        if crm::version(conn)? < 3 {
            return Ok(vec![]);
        }
        let mut statement = conn
            .prepare(&format!(
                "SELECT {COLUMNS} FROM crm_leads WHERE state=?1 AND id<?2 ORDER BY id DESC LIMIT 51"
            ))
            .map_err(storage)?;
        statement
            .query_map(params![state, after.unwrap_or(i64::MAX)], |r| {
                read_lead(r, false)
            })
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)
    }

    /// 1件を開く。個人情報を読むときは明示的な操作を必要とする。
    /// # Errors
    /// 未登録の受付番号・保存先の読取失敗。
    pub fn lead(&self, id: i64, reveal: bool) -> Result<Lead, EngineError> {
        let conn = self.db.conn();
        if crm::version(conn)? < 3 {
            return Err(EngineError::NotFound);
        }
        find_lead(conn, id, reveal)
    }

    /// 未処理の問い合わせを同一組織内の担当に割り当てる。所属は入口で解決する。
    /// # Errors
    /// 担当・受付・版が無効、処理済み、または書き込み失敗。
    pub fn assign_lead(
        &mut self,
        id: i64,
        rev: i64,
        owner: Option<i64>,
    ) -> Result<Lead, EngineError> {
        if owner.is_some_and(|v| v <= 0) {
            return Err(invalid("owner_id", "must be positive"));
        }
        self.db.write(WRITE_BASE, |tx|{
            crm::ensure_sales_ops(tx)?;
            let changed=tx.execute("UPDATE crm_leads SET owner_id=?1,revision=revision+1 WHERE id=?2 AND revision=?3 AND state=0",params![owner,id,rev]).map_err(storage)?;
            if changed!=1 {return Err(EngineError::RevisionConflict);}
            find_lead(tx,id,false)
        })
    }

    /// 受付情報を消去し、同じ送信キーの再受付を防ぐ。
    /// # Errors
    /// 競合、変換済み、保存の失敗。
    pub fn discard_lead(&mut self, id: i64, rev: i64) -> Result<Lead, EngineError> {
        self.db.write(WRITE_BASE, |tx|{
            crm::ensure_sales_ops(tx)?;
            let lead=find_lead(tx,id,false)?;
            if lead.state==2 {return Ok(lead);}
            if lead.state!=0 || lead.revision!=rev {return Err(EngineError::RevisionConflict);}
            tx.execute("UPDATE crm_leads SET company_name=NULL,contact_name=NULL,email=NULL,message=NULL,owner_id=NULL,state=2,revision=revision+1 WHERE id=?1",[id]).map_err(storage)?;
            find_lead(tx,id,false)
        })
    }
}

#[cfg(test)]
mod tests;
