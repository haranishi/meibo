//! Keep hosted navigation within the serialized remote-request budget.
use crate::server::{Reply, assets};

pub(super) fn asset(path: &str) -> Option<Reply> {
    match path {
        "/hosted-app.css" => {
            let mut reply = Reply::text("text/css; charset=utf-8", assets::CSS);
            reply.body.extend_from_slice(b"\n");
            reply
                .body
                .extend_from_slice(include_bytes!("../../ui/login.css"));
            Some(reply)
        }
        "/hosted-app.js" => Some(Reply::text("text/javascript; charset=utf-8", assets::JS)),
        _ => None,
    }
}

/// 公開デモ専用の注意書き帯と非表示導線のCSSを返す。
pub(super) fn demo_css() -> Reply {
    Reply::text("text/css; charset=utf-8", include_str!("../../ui/demo.css"))
}

pub(super) fn hosted_paths(html: &str) -> String {
    html.replace("href=\"/workspace.css\"", "href=\"/hosted-app.css\"")
        .replace("src=\"/workspace.js\"", "src=\"/hosted-app.js\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosted_styles_keep_the_login_rules_after_the_workspace_styles() {
        let response = asset("/hosted-app.css").unwrap();
        let text = std::str::from_utf8(&response.body).unwrap();
        assert_eq!(
            text.strip_prefix(assets::CSS).unwrap().strip_prefix('\n'),
            Some(include_str!("../../ui/login.css"))
        );
        assert_eq!(asset("/hosted-app.js").unwrap().body, assets::JS.as_bytes());
    }
}
