//! Public service metadata and manual sharing, independent of account data.
use std::fmt::Write as _;

use super::{demo::DemoConfig, http::HostedConfig};
use crate::server::Reply;

const TITLE: &str = "meibo · 営業ワークスペース";
const DESCRIPTION: &str = "企業情報と営業活動を、チームのワークスペースで。";
const DEMO_TITLE: &str = "meibo · 公開デモ";
const DEMO_DESCRIPTION: &str = "合成データの営業CRMを、そのまま触って試せる公開デモです。";
const SHARE_START: &str = "<!-- public-entry:share:start -->";
const SHARE_END: &str = "<!-- public-entry:share:end -->";
const LOGIN_START: &str = "<!-- auth:login:start -->";
const LOGIN_END: &str = "<!-- auth:login:end -->";
const DEMO_START: &str = "<!-- demo:enter:start -->";
const DEMO_END: &str = "<!-- demo:enter:end -->";
const DEMO_CONTACT: &str = "<!-- demo:contact -->";

pub(super) fn login(config: &HostedConfig) -> Reply {
    let template = login_template(config);
    let html = if config.development {
        remove_block(&template, SHARE_START, SHARE_END).replace("<!-- public-entry:head -->", "")
    } else {
        // Only the validated configuration enters the template, never a request or identity.
        let (title, description) = if config.demo.is_some() {
            (DEMO_TITLE, DEMO_DESCRIPTION)
        } else {
            (TITLE, DESCRIPTION)
        };
        let canonical = format!("{}/login", config.origin);
        let image = html_value(&format!("{}/public-entry.png", config.origin));
        let encoded_url = url_value(&canonical);
        let encoded_text = url_value(title);
        let canonical = html_value(&canonical);
        let metadata = format!(
            r#"<meta name="description" content="{description}">
  <link rel="canonical" href="{canonical}">
  <meta property="og:title" content="{title}">
  <meta property="og:type" content="website">
  <meta property="og:url" content="{canonical}">
  <meta property="og:description" content="{description}">
  <meta property="og:image" content="{image}">
  <meta property="og:image:type" content="image/png">
  <meta property="og:image:width" content="1200">
  <meta property="og:image:height" content="630">
  <meta property="og:image:alt" content="{title}">
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
    if let Some(demo) = &config.demo {
        let minutes = demo.idle_timeout_minutes().to_string();
        let contact = demo.contact.as_deref().map_or_else(String::new, |url| {
            format!(
                "\n    <p class=\"demo-contact\"><a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">不具合や質問はこちら</a></p>",
                html_value(url)
            )
        });
        return remove_block(template, LOGIN_START, LOGIN_END)
            .replace(&format!("{DEMO_START}\n    "), "")
            .replace(DEMO_START, "")
            .replace(DEMO_END, "")
            .replace("{{DEMO_IDLE_MINUTES}}", &minutes)
            .replace(DEMO_CONTACT, &contact)
            .replace(
                "<title>ログイン · meibo</title>",
                &format!("<title>{DEMO_TITLE}</title>"),
            )
            .replace("aria-label=\"meibo ログイン\"", "aria-label=\"meibo 公開デモ\"")
            .replace(
                "<h2 id=\"public-share-heading\">ログインURLを共有</h2>",
                "<h2 id=\"public-share-heading\">デモのURLを共有</h2>",
            )
            .replace(
                "<p class=\"public-share-help\">ログイン画面のURLを共有します。ご利用には管理者からの案内が必要です。</p>",
                "<p class=\"public-share-help\">このデモのURLを共有できます。</p>",
            )
            .replace(
                "<label for=\"public-share-url\">ログインURL（選択してコピー）</label>",
                "<label for=\"public-share-url\">デモのURL（選択してコピー）</label>",
            );
    }
    let template = remove_block(template, DEMO_START, DEMO_END)
        .replace(LOGIN_START, "")
        .replace(LOGIN_END, "");
    if config.access_mode() {
        remove_block(&template, START, END)
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

/// 認証後の業務画面HTMLへ、公開デモ用の注意書き帯と専用CSSを差し込む。
pub(super) fn apply_demo_workspace(html: &str, demo: &DemoConfig) -> String {
    let minutes = demo.idle_timeout_minutes();
    let contact = demo.contact.as_deref().map_or_else(String::new, |url| {
        format!(
            r#" <a href="{}" target="_blank" rel="noopener noreferrer">不具合を知らせる</a>"#,
            html_value(url)
        )
    });
    let banner = format!(
        r#"<div class="demo-banner" role="note"><span>公開デモ · データはすべて合成です · {minutes}分操作がないと初期化されます · 個人情報を入力しないでください</span>{contact}</div>"#
    );
    html.replacen(
        "</head>",
        "  <link rel=\"stylesheet\" href=\"/demo.css\">\n</head>",
        1,
    )
    .replacen("<body>", &format!("<body>\n  {banner}"), 1)
}

fn remove_block(template: &str, start: &str, end: &str) -> String {
    let (before, rest) = template.split_once(start).expect("fixed block start");
    let (_, after) = rest.split_once(end).expect("fixed block end");
    format!("{before}{after}")
}

pub(super) fn asset(path: &str) -> Option<Reply> {
    match path {
        "/login.js" => Some(Reply::text(
            "text/javascript; charset=utf-8",
            include_str!("../../ui/login.js"),
        )),
        "/login.css" => Some(Reply::text(
            "text/css; charset=utf-8",
            include_str!("../../ui/login.css"),
        )),
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
