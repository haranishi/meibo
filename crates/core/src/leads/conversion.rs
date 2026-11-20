use serde::Deserialize;

use super::{Lead, find_lead};
use crate::{
    Engine,
    crm::{self, OpportunityForecastPatch, OpportunityInput, OpportunityOwnerChange},
    query::{invalid, storage},
    schema::WRITE_BASE,
    sql::{OptionalExtension, Transaction, params},
    types::{
        ActivityInput, ActivityKind, CompanyId, CompanyInput, ContactId, ContactInput, DataOrigin,
        EngineError, NextAction, Status, UserId,
    },
    write,
};

/// 初回の引き継ぎに含める商談。初期段階は「見込み」。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewOpportunity {
    /// 商談名。
    pub name: String,
    /// 商談金額（円）。
    pub amount_jpy: i64,
    /// 完了予定日。
    pub close_date: String,
}

/// 担当者が確認した変換先。既存取引先の指定、新規取引先の都道府県のどちらかを使用。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversion {
    /// 確認済みの受付版。
    pub revision: i64,
    /// 確認済みの担当。同一組織に所属することをホストが確認する。
    pub owner_id: Option<i64>,
    /// 既存取引先を使うときのID。
    pub company_id: Option<i64>,
    /// 新規取引先の都道府県。
    pub prefecture: Option<u8>,
    /// 新規取引先に個人事業の表示・消去ルールを適用する。
    #[serde(default)]
    pub sole_proprietor: bool,
    /// 必要に応じて作成する商談。
    pub opportunity: Option<NewOpportunity>,
}

fn company_for(
    tx: &Transaction<'_>,
    origin: DataOrigin,
    lead: &Lead,
    input: &Conversion,
) -> Result<CompanyId, EngineError> {
    match (input.company_id, input.prefecture) {
        (Some(id), None) if id > 0 && !input.sole_proprietor => {
            let (status, erased): (u8, bool) = tx
                .query_row(
                    "SELECT status,erased_at IS NOT NULL FROM companies WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(storage)?
                .ok_or(EngineError::NotFound)?;
            if erased {
                return Err(EngineError::NotFound);
            }
            if status == Status::Excluded.code() {
                return Err(EngineError::ExcludedCompany);
            }
            Ok(CompanyId(id))
        }
        (None, Some(prefecture)) => {
            let company = CompanyInput {
                origin,
                corporate_number: None,
                name: lead.company_name.clone().ok_or(EngineError::Corrupt)?,
                prefecture,
                municipality: None,
                industry: None,
                phone: None,
                website: None,
                owner: lead.owner_id.map(UserId),
                status: Status::Untouched,
                is_sole_proprietor: input.sole_proprietor,
                source_id: Some("web-inquiry".into()),
                source_as_of: None,
            };
            let normalized = write::validate_company_input(&company)?;
            let duplicate:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM companies WHERE name_key=?1 AND prefecture_code=?2 AND erased_at IS NULL)",
                params![normalized.name_key,prefecture],|r|r.get(0)).map_err(storage)?;
            if duplicate {
                return Err(invalid("existing_company", "select the existing account"));
            }
            write::insert_company_tx(tx, &normalized)
        }
        _ => Err(invalid(
            "company_id",
            "choose an existing company or a prefecture",
        )),
    }
}

fn contact_for(
    tx: &Transaction<'_>,
    company: CompanyId,
    lead: &Lead,
) -> Result<ContactId, EngineError> {
    let name = lead.contact_name.as_deref().ok_or(EngineError::Corrupt)?;
    let email = lead.email.as_deref().ok_or(EngineError::Corrupt)?;
    let existing:Option<i64>=tx.query_row("SELECT id FROM contacts WHERE company_id=?1 AND name=?2 AND email=?3 COLLATE NOCASE AND erased_at IS NULL ORDER BY id LIMIT 1",
        params![company.0,name,email],|r|r.get(0)).optional().map_err(storage)?;
    if let Some(id) = existing {
        return Ok(ContactId(id));
    }
    let contact = ContactInput {
        company,
        name: name.into(),
        phone: None,
        email: Some(email.into()),
    };
    write::validate_contact_input(&contact)?;
    write::add_contact_tx(tx, &contact)
}

impl Engine {
    /// 未処理リードを取引先・担当者・任意の商談に引き継ぐ。確定後の再送には同じ結果を返す。
    /// # Errors
    /// 受付版、担当、対象、商談情報が不正な場合、またはトランザクションが失敗した場合。
    pub fn convert_lead(&mut self, id: i64, input: &Conversion) -> Result<Lead, EngineError> {
        let origin = self.origin();
        let estimate = self.db.local_growth_estimate(|| {
            let row_bytes = if let Some(company) = input.company_id {
                write::company_row_bytes(self.db.conn(), CompanyId(company))?
            } else {
                0
            };
            Ok(WRITE_BASE
                .saturating_mul(5)
                .saturating_add(64 * 1024)
                .saturating_add(row_bytes.saturating_mul(4)))
        })?;
        self.db.write(estimate,|tx|{
            crm::ensure_sales_ops(tx)?;
            let lead=find_lead(tx,id,true)?;
            if lead.state==1 {return Ok(lead);}
            if lead.state!=0 || lead.revision!=input.revision || lead.owner_id!=input.owner_id {return Err(EngineError::RevisionConflict);}
            let company=company_for(tx,origin,&lead,input)?;
            let contact=contact_for(tx,company,&lead)?;
            let opportunity=if let Some(deal)=&input.opportunity {
                let forecast=lead.owner_id.map_or_else(OpportunityForecastPatch::default,|owner|OpportunityForecastPatch{
                    owner:OpportunityOwnerChange::Assign(UserId(owner)),..Default::default()
                });
                Some(crm::insert_opportunity_tx(tx,&OpportunityInput{company,contact:Some(contact),name:deal.name.clone(),
                    amount_jpy:deal.amount_jpy,stage:0,close_date:deal.close_date.clone()},None,forecast)?)
            }else{None};
            if let Some(note)=lead.message {
                write::record_activity_tx(tx,&ActivityInput{company,contact:Some(contact),kind:ActivityKind::Memo,at:lead.created_at,
                    note:Some(note),status_after:None,next_action:NextAction::Keep,reverses:None},false)?;
            }
            tx.execute("UPDATE crm_leads SET state=1,revision=revision+1,company_name=NULL,contact_name=NULL,email=NULL,message=NULL,
                company_id=?2,contact_id=?3,opportunity_id=?4 WHERE id=?1",params![id,company.0,contact.0,opportunity]).map_err(storage)?;
            find_lead(tx,id,false)
        })
    }
}
