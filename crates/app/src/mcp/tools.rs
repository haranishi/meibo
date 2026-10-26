use serde_json::{Value, json};

pub(super) fn list(allow_write: bool) -> Vec<Value> {
    let paging = || {
        json!({
            "limit":{"type":"integer","minimum":1,"maximum":200,"default":50},
            "after":{"type":"string","maxLength":4096}
        })
    };
    let mut list_properties = paging();
    for (name, property) in [
        (
            "view",
            json!({"type":"string","enum":["all","untouched","negotiating","has_phone","scheduled","due"]}),
        ),
        ("status", json!({"type":"integer","minimum":0,"maximum":4})),
        (
            "prefecture",
            json!({"type":"integer","minimum":1,"maximum":47}),
        ),
        ("has_phone", json!({"type":"boolean"})),
        (
            "sort",
            json!({"type":"string","enum":["default","next_action","last_activity"]}),
        ),
    ] {
        list_properties[name] = property;
    }
    let mut search_properties = paging();
    search_properties["query"] = json!({"type":"string","minLength":1,"maxLength":4096});
    let mut result = vec![
        tool(
            "list_companies",
            "List active CRM companies with bounded pagination. Personal fields are hidden by default.",
            list_properties,
            &[],
            true,
        ),
        tool(
            "search_companies",
            "Search company names. Use the returned next cursor only with the same query.",
            search_properties,
            &["query"],
            true,
        ),
        tool(
            "get_company",
            "Get one company and its recorded activity history. Reveal contact or sole-proprietor personal fields only when explicitly needed.",
            json!({
                "id":{"type":"integer","minimum":1},
                "include_contacts":{"type":"boolean","default":false},
                "include_private":{"type":"boolean","default":false}
            }),
            &["id"],
            true,
        ),
    ];
    if allow_write {
        result.push(tool("record_activity", "Append a CRM activity without contacting anyone. This is not idempotent: after an uncertain response, inspect company history before retrying.", json!({
            "company_id":{"type":"integer","minimum":1},
            "note":{"type":"string","minLength":1,"maxLength":10000},
            "kind":{"type":"integer","enum":[1,2,3,4]},
            "status":{"type":"integer","minimum":0,"maximum":5},
            "next_action_at":{"type":["integer","null"],"minimum":0,"maximum":253_402_300_799_i64},
            "contact_id":{"type":["integer","null"],"minimum":1},
            "at":{"type":"integer","minimum":0,"maximum":253_402_300_799_i64}
        }), &["company_id","note"], false));
    }
    result
}

fn tool(
    name: &str,
    description: &str,
    properties: Value,
    required: &[&str],
    read_only: bool,
) -> Value {
    let mut tool = json!({
        "name":name,"description":description,
        "inputSchema":{"type":"object","required":required,"additionalProperties":false},
        "annotations":{"readOnlyHint":read_only,"destructiveHint":false,"idempotentHint":read_only,"openWorldHint":false}
    });
    tool["inputSchema"]["properties"] = properties;
    tool
}
