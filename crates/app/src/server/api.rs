use std::{
    collections::HashMap,
    io::Write,
    time::{SystemTime, UNIX_EPOCH},
};

use meibo_core::{
    Engine,
    types::{
        ActivityId, ActivityInput, ActivityKind, Company, CompanyId, CompanyInput, ContactId,
        ContactInput, Cursor, DataOrigin, EngineError, Field, Filters, ListQuery, NextAction,
        Region, SearchOptions, Sort, Status,
    },
};
use meibo_import::{ExportOptions, export_csv};
use serde_json::{Value, json};

use super::{MAX_EXPORT, Reply};

pub(super) fn summary(engine: &Engine) -> Reply {
    let result = (|| {
        let total = engine
            .count(&Filters {
                include_excluded: true,
                ..Filters::default()
            })?
            .n;
        let active = engine.count(&Filters::default())?.n;
        let with_phone = engine
            .count(&Filters {
                has_phone: Some(true),
                ..Filters::default()
            })?
            .n;
        let status_counts = (0..=5)
            .map(|code| {
                engine
                    .count(&Filters {
                        status: vec![Status::from_code(code).expect("fixed status code")],
                        include_excluded: true,
                        ..Filters::default()
                    })
                    .map(|count| count.n)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let scheduled = engine
            .count(&Filters {
                next_action_before: Some(i64::MAX),
                ..Filters::default()
            })?
            .n;
        let now = current_time().ok_or_else(|| {
            EngineError::Storage(Box::new(std::io::Error::other("system clock unavailable")))
        })?;
        let due = engine
            .count(&Filters {
                next_action_before: Some(now),
                ..Filters::default()
            })?
            .n;
        Ok::<_, EngineError>(
            json!({"total":total,"active":active,"with_phone":with_phone,"origin":"synthetic",
                "status_counts":status_counts,"scheduled":scheduled,"due":due}),
        )
    })();
    respond(result)
}

pub(super) fn companies(engine: &Engine, query: &str) -> Reply {
    let Some(parameters) = query_parameters(query) else {
        return Reply::error(400, "検索条件が正しくありません。");
    };
    let limit = match parameters.get("limit") {
        None => 50,
        Some(value) => match value.parse::<u32>() {
            Ok(value) if (1..=200).contains(&value) => value,
            _ => return Reply::error(400, "表示件数は1〜200件で指定してください。"),
        },
    };
    let after = parameters
        .get("after")
        .filter(|value| !value.is_empty())
        .map(|token| Cursor {
            token: token.clone(),
        });
    let term = parameters.get("q").map_or("", |value| value.trim());
    let Some((filters, sort)) = list_filters(&parameters) else {
        return Reply::error(400, "表示するビューまたは絞り込み条件を確認してください。");
    };
    let filtered = parameters.get("view").is_some_and(|view| view != "all")
        || ["status", "prefecture", "has_phone"]
            .iter()
            .any(|key| parameters.contains_key(*key));
    if !term.is_empty()
        && (filtered || parameters.get("sort").is_some_and(|sort| sort != "default"))
    {
        return Reply::error(
            400,
            "社名検索はすべての取引先が対象です。ビューと絞り込みを解除してください。",
        );
    }
    let result = if term.is_empty() {
        engine.list(&ListQuery {
            filters: filters.clone(), sort, after, limit, fields: vec![],
        }).and_then(|page| {
            let count = if parameters.get("after").is_none_or(String::is_empty) {
                Some(engine.count(&filters)?.n)
            } else { None };
            Ok(json!({"rows":page.rows.iter().map(company_json).collect::<Vec<_>>(),"next":page.next.map(|cursor|cursor.token),"count":count}))
        })
    } else {
        engine.search_name(term, &SearchOptions {
            after, limit, fields: vec![], include_excluded: false, with_count: true,
        }).map(|page| json!({"rows":page.rows.iter().map(company_json).collect::<Vec<_>>(),"next":page.next.map(|cursor|cursor.token),"count":page.n}))
    };
    respond(result)
}

fn current_time() -> Option<i64> {
    i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()
}

fn list_filters(parameters: &HashMap<String, String>) -> Option<(Filters, Sort)> {
    let mut filters = Filters::default();
    match parameters.get("view").map_or("all", String::as_str) {
        "all" => {}
        "untouched" => filters.status = vec![Status::Untouched],
        "negotiating" => filters.status = vec![Status::Negotiating],
        "has_phone" => filters.has_phone = Some(true),
        "scheduled" => filters.next_action_before = Some(i64::MAX),
        "due" => filters.next_action_before = Some(current_time()?),
        _ => return None,
    }
    if let Some(code) = parameters.get("status") {
        let code = code.parse::<u8>().ok().filter(|code| *code <= 4)?;
        filters.status = vec![Status::from_code(code)?];
    }
    if let Some(prefecture) = parameters.get("prefecture") {
        let code = prefecture
            .parse::<u8>()
            .ok()
            .filter(|code| (1..=47).contains(code))?;
        filters.region = vec![Region::Prefecture(code)];
    }
    if let Some(value) = parameters.get("has_phone") {
        filters.has_phone = Some(match value.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        });
    }
    let sort = if filters.next_action_before.is_some() {
        Sort::NextAction
    } else {
        Sort::ShapeDefault
    };
    let sort = match parameters.get("sort").map(String::as_str) {
        None | Some("default") => sort,
        Some("next_action") => Sort::NextAction,
        Some("last_activity") => Sort::LastActivityDesc,
        _ => return None,
    };
    Some((filters, sort))
}

pub(super) fn create_company(engine: &mut Engine, body: &[u8]) -> Reply {
    let Some(company) = parse_company(body) else {
        return Reply::error(
            400,
            "取引先名・都道府県・任意項目の入力を確認してください。",
        );
    };
    match engine.register_company(&company) {
        Ok(id) => Reply::json(201, &json!({"id":id.0})),
        Err(error) => engine_error(&error),
    }
}

fn parse_company(body: &[u8]) -> Option<CompanyInput> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "name" | "prefecture" | "municipality" | "corporate_number" | "phone" | "website"
        )
    }) {
        return None;
    }
    let name = object.get("name")?.as_str()?.trim().to_owned();
    if name.is_empty() || name.len() > 4000 || name.contains('\0') {
        return None;
    }
    let prefecture = u8::try_from(object.get("prefecture")?.as_u64()?)
        .ok()
        .filter(|code| (1..=47).contains(code))?;
    let optional = |field: &str| -> Option<Option<String>> {
        match object.get(field) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(value)) => {
                let limit = match field {
                    "phone" => 100,
                    "website" => 2000,
                    "municipality" => 5,
                    _ => 13,
                };
                if value.len() > limit || value.contains('\0') {
                    return None;
                }
                Some((!value.trim().is_empty()).then(|| value.trim().to_owned()))
            }
            _ => None,
        }
    };
    Some(CompanyInput {
        origin: DataOrigin::Synthetic,
        corporate_number: optional("corporate_number")?,
        name,
        prefecture,
        municipality: optional("municipality")?,
        industry: None,
        phone: optional("phone")?,
        website: optional("website")?,
        owner: None,
        status: Status::Untouched,
        is_sole_proprietor: false,
        source_id: Some("manual-demo".into()),
        source_as_of: None,
    })
}

pub(super) fn update_company(engine: &mut Engine, id: CompanyId, body: &[u8]) -> Reply {
    let Some(mut update) = parse_company(body) else {
        return Reply::error(400, "取引先の入力内容を確認してください。");
    };
    let object: Value = serde_json::from_slice(body).expect("validated JSON");
    let previous = match engine.company_detail(id, &vec![Field::Name, Field::Phone, Field::Website])
    {
        Ok(detail) if detail.company.erased_at.is_none() => detail.company,
        Ok(_) => return missing(),
        Err(error) => return engine_error(&error),
    };
    update.origin = previous.origin;
    update.industry = previous.industry;
    update.owner = previous.owner;
    update.status = previous.status;
    update.is_sole_proprietor = previous.is_sole_proprietor;
    update.source_id = previous.source_id;
    update.source_as_of = previous.source_as_of;
    if object.get("corporate_number").is_none() {
        update.corporate_number = previous.corporate_number;
    }
    if object.get("municipality").is_none() {
        update.municipality = previous.municipality;
    }
    if object.get("phone").is_none() {
        update.phone = previous.phone;
    }
    if object.get("website").is_none() {
        update.website = previous.website;
    }
    match engine.update_company(id, &update) {
        Ok(()) => Reply::json(200, &json!({"id":id.0})),
        Err(error) => engine_error(&error),
    }
}

pub(super) fn detail(engine: &Engine, id: CompanyId, query: &str) -> Reply {
    let mut fields = vec![];
    let mut seen = std::collections::HashSet::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let Some((key, "1")) = pair.split_once('=') else {
            return Reply::error(400, "詳細の表示指定を確認してください。");
        };
        if !seen.insert(key) {
            return Reply::error(400, "詳細の表示指定を確認してください。");
        }
        match key {
            "include_contacts" => {
                fields.extend([Field::ContactName, Field::ContactPhone, Field::ContactEmail]);
            }
            "include_private" => fields.extend([Field::Name, Field::Phone, Field::Website]),
            _ => return Reply::error(400, "詳細の表示指定を確認してください。"),
        }
    }
    let result = engine.company_detail(id, &fields).and_then(|detail| {
        if detail.company.erased_at.is_some() { return Err(EngineError::NotFound); }
        let contacts = detail.contacts.iter().filter(|contact| contact.erased_at.is_none()).map(|contact|json!({
            "id":contact.id.0,"name":contact.name,"phone":contact.phone,"email":contact.email,
        })).collect::<Vec<_>>();
        let activities = detail.activities.iter().map(|activity|json!({
            "id":activity.id.0,"kind":activity.kind.code(),"at":activity.at,"contact_id":activity.contact.map(|id|id.0),
            "note":if activity.note_erased_at.is_none() {activity.note.as_deref()} else {None},
            "status_after":activity.status_after.map(Status::code),"reverses_id":activity.reverses.map(|id|id.0),
        })).collect::<Vec<_>>();
        Ok(json!({"company":company_json(&detail.company),"contacts":contacts,"activities":activities,"contacts_revealed":fields.contains(&Field::ContactName)}))
    });
    respond(result)
}

fn active_company(
    engine: &Engine,
    id: CompanyId,
) -> Result<meibo_core::types::Detail, EngineError> {
    let detail = engine.company_detail(id, &vec![])?;
    if detail.company.erased_at.is_some() {
        return Err(EngineError::NotFound);
    }
    Ok(detail)
}

pub(super) fn contact(
    engine: &mut Engine,
    company: CompanyId,
    id: Option<ContactId>,
    body: &[u8],
) -> Reply {
    let parsed = (|| {
        let value: Value = serde_json::from_slice(body).ok()?;
        let object = value.as_object()?;
        if object
            .keys()
            .any(|key| !matches!(key.as_str(), "name" | "phone" | "email"))
        {
            return None;
        }
        let name = object.get("name")?.as_str()?.trim().to_owned();
        if name.is_empty() || name.len() > 1000 || name.contains('\0') {
            return None;
        }
        let optional = |field: &str, limit: usize| -> Option<Option<String>> {
            match object.get(field) {
                None | Some(Value::Null) => Some(None),
                Some(Value::String(value)) if value.len() <= limit && !value.contains('\0') => {
                    Some((!value.trim().is_empty()).then(|| value.trim().to_owned()))
                }
                _ => None,
            }
        };
        Some(ContactInput {
            company,
            name,
            phone: optional("phone", 100)?,
            email: optional("email", 320)?,
        })
    })();
    let Some(parsed) = parsed else {
        return Reply::error(400, "担当者の氏名と連絡先を確認してください。");
    };
    let detail = match active_company(engine, company) {
        Ok(detail) => detail,
        Err(error) => return engine_error(&error),
    };
    if let Some(id) = id {
        if !detail
            .contacts
            .iter()
            .any(|contact| contact.id == id && contact.erased_at.is_none())
        {
            return missing();
        }
        match engine.update_contact(id, &parsed) {
            Ok(()) => Reply::json(200, &json!({"id":id.0})),
            Err(error) => engine_error(&error),
        }
    } else {
        match engine.add_contact(&parsed) {
            Ok(id) => Reply::json(201, &json!({"id":id.0})),
            Err(error) => engine_error(&error),
        }
    }
}

pub(super) fn erase_contact(
    engine: &mut Engine,
    company: CompanyId,
    id: ContactId,
    body: &[u8],
) -> Reply {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Reply::error(400, "消去の指定を確認してください。"),
    };
    let Some(object) = value.as_object() else {
        return Reply::error(400, "消去の指定を確認してください。");
    };
    if object.len() != 1 || !object.get("wipe_notes").is_some_and(Value::is_boolean) {
        return Reply::error(400, "消去の指定を確認してください。");
    }
    let detail = match active_company(engine, company) {
        Ok(detail) => detail,
        Err(error) => return engine_error(&error),
    };
    if !detail
        .contacts
        .iter()
        .any(|contact| contact.id == id && contact.erased_at.is_none())
    {
        return missing();
    }
    match engine.erase_contact(id, object["wipe_notes"].as_bool().expect("checked bool")) {
        Ok(()) => Reply::json(200, &json!({"ok":true})),
        Err(error) => engine_error(&error),
    }
}

pub(super) fn cancel_activity(
    engine: &mut Engine,
    company: CompanyId,
    id: ActivityId,
    body: &[u8],
) -> Reply {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return Reply::error(400, "取消の指定を確認してください。"),
    };
    if !value.as_object().is_some_and(serde_json::Map::is_empty) {
        return Reply::error(400, "取消の指定を確認してください。");
    }
    if let Err(error) = active_company(engine, company) {
        return engine_error(&error);
    }
    let Some(at) = current_time() else {
        return Reply::error(500, "現在日時を確認できません。");
    };
    match engine.record_activity(&ActivityInput {
        company,
        contact: None,
        kind: ActivityKind::Cancel,
        at,
        note: None,
        status_after: None,
        next_action: NextAction::Keep,
        reverses: Some(id),
    }) {
        Ok(_) => Reply::json(200, &json!({"ok":true})),
        Err(error) => engine_error(&error),
    }
}

pub(super) fn activity(engine: &mut Engine, id: CompanyId, body: &[u8]) -> Reply {
    let Some(input) = parse_activity(id, body) else {
        return Reply::error(400, "保存するメモ・状態・次回対応日を確認してください。");
    };
    // coreの詳細取得は消去行を返すため、書込み前も公開HTTP境界で拒否する。
    match engine.company_detail(id, &vec![]) {
        Ok(detail) if detail.company.erased_at.is_some() => return missing(),
        Err(error) => return engine_error(&error),
        _ => {}
    }
    if let Some(contact) = input.contact {
        match active_company(engine, id) {
            Ok(detail)
                if detail
                    .contacts
                    .iter()
                    .any(|value| value.id == contact && value.erased_at.is_none()) => {}
            Ok(_) => return Reply::error(400, "この取引先の未消去の担当者を選択してください。"),
            Err(error) => return engine_error(&error),
        }
    }
    match engine.record_activity(&input) {
        Ok(_) => Reply::json(200, &json!({"ok":true})),
        Err(error) => engine_error(&error),
    }
}

fn parse_activity(id: CompanyId, body: &[u8]) -> Option<ActivityInput> {
    let value: Value = serde_json::from_slice(body).ok()?;
    let object = value.as_object()?;
    if object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "note" | "status" | "next_action_at" | "kind" | "contact_id" | "at"
        )
    }) {
        return None;
    }
    let note = object.get("note")?.as_str()?.to_owned();
    if note.contains('\0') || note.trim().is_empty() || note.chars().count() > 10000 {
        return None;
    }
    let status_after = object
        .get("status")
        .map(|value| {
            value
                .as_u64()
                .and_then(|code| u8::try_from(code).ok())
                .and_then(Status::from_code)
        })
        .map_or(Some(None), |value| value.map(Some))?;
    let next_action = match object.get("next_action_at") {
        None => NextAction::Keep,
        Some(Value::Null) => NextAction::Clear,
        Some(value) => NextAction::Set(value.as_i64()?),
    };
    let at = match object.get("at") {
        None => current_time()?,
        Some(value) => value
            .as_i64()
            .filter(|value| (0..=253_402_300_799).contains(value))?,
    };
    let kind = match object.get("kind") {
        None => ActivityKind::Call,
        Some(value) => {
            ActivityKind::from_code(u8::try_from(value.as_u64()?).ok()?).filter(|kind| {
                matches!(
                    kind,
                    ActivityKind::Call
                        | ActivityKind::Visit
                        | ActivityKind::Email
                        | ActivityKind::Memo
                )
            })?
        }
    };
    let contact = match object.get("contact_id") {
        None | Some(Value::Null) => None,
        Some(value) => Some(ContactId(value.as_i64().filter(|id| *id > 0)?)),
    };
    Some(ActivityInput {
        company: id,
        contact,
        kind,
        at,
        note: Some(note),
        status_after,
        next_action,
        reverses: None,
    })
}

fn company_json(company: &Company) -> Value {
    json!({
        "id":company.id.0,"name":company.name,"corporate_number":company.corporate_number,
        "prefecture":company.prefecture,"municipality":company.municipality,
        "phone":company.phone,"website":company.website,"status":company.status.code(),
        "owner":company.owner.map(|owner|owner.0),"last_activity_at":company.last_activity_at,
        "next_action_at":company.next_action_at,"is_sole_proprietor":company.is_sole_proprietor,
    })
}

pub(super) fn export(engine: &Engine) -> Reply {
    let mut writer = LimitedWriter {
        bytes: Vec::new(),
        limit: MAX_EXPORT,
        exceeded: false,
    };
    match export_csv(engine, &mut writer, ExportOptions::default()) {
        Ok(_) => Reply {
            status: 200,
            mime: "text/csv; charset=utf-8",
            body: writer.bytes,
            attachment: true,
        },
        Err(_) if writer.exceeded => {
            Reply::error(413, "CSVが32MiBの上限を超えるため書き出せません。")
        }
        Err(_) => Reply::error(500, "CSVを書き出せませんでした。"),
    }
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(std::io::Error::other("CSV size limit"));
        }
        // Vecの倍増による上限超えを避け、HTTP出力の確保量も32MiBに収める。
        if self.bytes.capacity() - self.bytes.len() < bytes.len() {
            let required = self.bytes.len() + bytes.len();
            let target = self
                .bytes
                .capacity()
                .saturating_mul(2)
                .max(required)
                .min(self.limit);
            self.bytes.reserve_exact(target - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn respond(result: Result<Value, EngineError>) -> Reply {
    match result {
        Ok(value) => Reply::json(200, &value),
        Err(error) => engine_error(&error),
    }
}

fn missing() -> Reply {
    Reply::error(404, "対象が見つかりません。")
}

fn engine_error(error: &EngineError) -> Reply {
    match error {
        EngineError::NotFound => missing(),
        EngineError::ExcludedCompany => {
            Reply::error(409, "除外中の企業はこの画面から解除できません。")
        }
        EngineError::DuplicateCorporateNumber { .. } => Reply::error(
            409,
            "同じ法人番号の取引先が登録されています。入力内容を確認してください。",
        ),
        EngineError::InvalidInput { .. }
        | EngineError::InvalidCorporateNumber { .. }
        | EngineError::InvalidSearchTerm
        | EngineError::EmptyName
        | EngineError::ForeignContact
        | EngineError::InvalidCancel => {
            Reply::error(400, "入力内容または検索条件を確認してください。")
        }
        _ => Reply::error(500, "データを処理できませんでした。再試行してください。"),
    }
}

fn query_parameters(query: &str) -> Option<HashMap<String, String>> {
    let mut parameters = HashMap::new();
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let key = decode_component(key)?;
        let value = decode_component(value)?;
        if !matches!(
            key.as_str(),
            "q" | "after" | "limit" | "view" | "status" | "prefecture" | "has_phone" | "sort"
        ) || value.len() > 4096
            || value.contains('\0')
        {
            return None;
        }
        if parameters.insert(key, value).is_some() {
            return None;
        }
    }
    Some(parameters)
}

fn decode_component(value: &str) -> Option<String> {
    let mut output = Vec::with_capacity(value.len());
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        match byte {
            b'+' => output.push(b' '),
            b'%' => {
                let high = char::from(bytes.next()?).to_digit(16)?;
                let low = char::from(bytes.next()?).to_digit(16)?;
                output.push(u8::try_from(high * 16 + low).ok()?);
            }
            _ => output.push(byte),
        }
    }
    String::from_utf8(output).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_writer_stops_before_retaining_oversized_output() {
        let mut writer = LimitedWriter {
            bytes: Vec::new(),
            limit: 10,
            exceeded: false,
        };
        writer.write_all(b"12345678").unwrap();
        assert!(writer.write_all(b"901").is_err());
        assert!(writer.exceeded);
        assert_eq!(writer.bytes, b"12345678");
        assert!(writer.bytes.capacity() <= 10);
    }
}
