//! Public service metadata and manual sharing, independent of account data.
use std::fmt::Write as _;

use super::http::HostedConfig;
use crate::server::Reply;

const TITLE: &str = "meibo · 営業ワークスペース";
const DESCRIPTION: &str = "企業情報と営業活動を、チームのワークスペースで。";
const SHARE_START: &str = "<!-- public-entry:share:start -->";
const SHARE_END: &str = "<!-- public-entry:share:end -->";

pub(super) fn login(config: &HostedConfig) -> Reply {
    let template = login_template(config);
    let html = if config.development {
        let (before, rest) = template.split_once(SHARE_START).expect("fixed share start");
        let (_, after) = rest.split_once(SHARE_END).expect("fixed share end");
        format!("{before}{after}").replace("<!-- public-entry:head -->", "")
    } else {
        // Only the validated configuration enters the template, never a request or identity.
        let canonical = format!("{}/login", config.origin);
        let image = html_value(&format!("{}/public-entry.png", config.origin));
        let encoded_url = url_value(&canonical);
        let encoded_text = url_value(TITLE);
        let canonical = html_value(&canonical);
        let metadata = format!(
            r#"<meta name="description" content="{DESCRIPTION}">
  <link rel="canonical" href="{canonical}">
  <meta property="og:title" content="{TITLE}">
  <meta property="og:type" content="website">
  <meta property="og:url" content="{canonical}">
  <meta property="og:description" content="{DESCRIPTION}">
  <meta property="og:image" content="{image}">
  <meta property="og:image:type" content="image/png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="meibo · 営業ワークスペース">
  <meta name="twitter:card" content="summary_large_image">
  <script src="/public-entry.js" defer></script>"#
        );
        template
            .replace("<!-- public-entry:head -->", &metadata)
            .replace(SHARE_START, "")
            .replace(SHARE_END, "")
            .replace("{{PUBLIC_CANONICAL}}", &canonical)
            .replace("{{PUBLIC_X}}", &html_value(&format!("https://twitter.com/intent/tweet?url={encoded_url}&text={encoded_text}")))
            .replace("{{PUBLIC_LINE}}", &html_value(&format!("https://social-plugins.line.me/lineit/share?url={encoded_url}&text={encoded_text}")))
    };
    Reply {
        status: 200,
        mime: "text/html; charset=utf-8",
        body: html.into_bytes(),
        attachment: false,
    }
}

fn login_template(config: &HostedConfig) -> String {
    const START: &str = "<!-- auth:password:start -->";
    const END: &str = "<!-- auth:password:end -->";
    let template = include_str!("../../ui/login.html");
    if config.access_mode() {
        let (before, rest) = template.split_once(START).expect("fixed password start");
        let (_, after) = rest.split_once(END).expect("fixed password end");
        format!("{before}{after}")
            .replace(
                "<form id=\"login-form\">",
                "<form id=\"login-form\" data-cloudflare-access>",
            )
            .replace(
                "招待された組織のアカウントでログインしてください。",
                "Cloudflareで確認したアカウントを使います。招待された組織IDを入力してください。",
            )
            .replace(">ログイン</button>", ">Cloudflareでログイン</button>")
    } else {
        template.replace(START, "").replace(END, "")
    }
}

pub(super) fn asset(path: &str) -> Option<Reply> {
    match path {
        "/public-entry.js" => Some(Reply::text(
            "text/javascript; charset=utf-8",
            include_str!("../../ui/public-entry.js"),
        )),
        "/public-entry.png" => Some(Reply {
            status: 200,
            mime: "image/png",
            body: include_bytes!("../../ui/public-entry.png").to_vec(),
            attachment: false,
        }),
        _ => None,
    }
}

fn html_value(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn url_value(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            encoded.push(char::from(byte));
        } else {
            write!(&mut encoded, "%{byte:02X}").expect("write to string");
        }
    }
    encoded
}
