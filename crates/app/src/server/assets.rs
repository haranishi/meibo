//! Embed the workspace in the same CSS and script order for local and hosted navigation.
use super::Reply;

macro_rules! list_tools {
    () => {
        concat!(
            include_str!("../../ui/calling-lists.js"),
            "\n;\n",
            include_str!("../../ui/list-tools.js")
        )
    };
}

pub(crate) const CSS: &str = concat!(
    include_str!("../../ui/style.css"),
    "\n",
    include_str!("../../ui/crm.css"),
    "\n",
    include_str!("../../ui/list-tools.css"),
    "\n",
    include_str!("../../ui/custom-fields.css"),
    "\n",
    include_str!("../../ui/call-mode.css"),
);
pub(crate) const JS: &str = concat!(
    include_str!("../../ui/custom-fields.js"),
    "\n;\n",
    include_str!("../../ui/call-mode.js"),
    "\n;\n",
    include_str!("../../ui/app.js"),
    "\n;\n",
    include_str!("../../ui/crm.js"),
    "\n;\n",
    list_tools!(),
);

const STYLE: &str = "text/css; charset=utf-8";
const SCRIPT: &str = "text/javascript; charset=utf-8";

pub(super) fn local(path: &str) -> Option<Reply> {
    let (mime, source) = match path {
        "/" => (
            "text/html; charset=utf-8",
            include_str!("../../ui/index.html"),
        ),
        "/workspace.css" => (STYLE, CSS),
        "/workspace.js" => (SCRIPT, JS),
        "/style.css" => (STYLE, include_str!("../../ui/style.css")),
        "/crm.css" => (STYLE, include_str!("../../ui/crm.css")),
        "/list-tools.css" => (STYLE, include_str!("../../ui/list-tools.css")),
        "/custom-fields.css" => (STYLE, include_str!("../../ui/custom-fields.css")),
        "/call-mode.css" => (STYLE, include_str!("../../ui/call-mode.css")),
        "/app.js" => (SCRIPT, include_str!("../../ui/app.js")),
        "/crm.js" => (SCRIPT, include_str!("../../ui/crm.js")),
        "/list-tools.js" => (SCRIPT, list_tools!()),
        "/custom-fields.js" => (SCRIPT, include_str!("../../ui/custom-fields.js")),
        "/call-mode.js" => (SCRIPT, include_str!("../../ui/call-mode.js")),
        _ => return None,
    };
    Some(Reply::text(mime, source))
}
