//! Keep hosted navigation within the serialized remote-request budget.
use crate::server::Reply;

const CSS: &str = concat!(
    include_str!("../../ui/style.css"),
    "\n",
    include_str!("../../ui/crm.css"),
    "\n",
    include_str!("../../ui/list-tools.css"),
    "\n",
    include_str!("../../ui/custom-fields.css"),
    "\n",
    include_str!("../../ui/call-mode.css"),
    "\n",
    include_str!("../../ui/login.css"),
);
const JS: &str = concat!(
    include_str!("../../ui/custom-fields.js"),
    "\n;\n",
    include_str!("../../ui/call-mode.js"),
    "\n;\n",
    include_str!("../../ui/app.js"),
    "\n;\n",
    include_str!("../../ui/crm.js"),
    "\n;\n",
    include_str!("../../ui/list-tools.js"),
);

pub(super) fn asset(path: &str) -> Option<Reply> {
    match path {
        "/hosted-app.css" => Some(Reply::text("text/css; charset=utf-8", CSS)),
        "/hosted-app.js" => Some(Reply::text("text/javascript; charset=utf-8", JS)),
        _ => None,
    }
}

pub(super) fn compact(mut html: String) -> String {
    html = html.replace(
        "<link rel=\"stylesheet\" href=\"/style.css\">",
        "<link rel=\"stylesheet\" href=\"/hosted-app.css\">",
    );
    for name in ["crm", "list-tools", "custom-fields", "call-mode"] {
        html = html.replace(
            &format!("<link rel=\"stylesheet\" href=\"/{name}.css\">"),
            "",
        );
    }
    html = html.replace(
        "<script src=\"/custom-fields.js\" defer></script>",
        "<script src=\"/hosted-app.js\" defer></script>",
    );
    for name in ["call-mode", "app", "crm", "list-tools"] {
        html = html.replace(&format!("<script src=\"/{name}.js\" defer></script>"), "");
    }
    html
}
