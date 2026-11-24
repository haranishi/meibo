//! 公開デモモードの設定と、起動時刻を基準にした営業デモ初期データの生成。
use std::{fs, io::ErrorKind, net::Ipv6Addr, path::Path, time::Duration};

use meibo_core::{
    DuplicatePolicy, Engine,
    crm::{OpportunityForecastPatch, OpportunityInput, OpportunityOwnerChange},
    synth::{Profile, Synth, SynthConfig},
    types::{
        ActivityInput, ActivityKind, CompanyId, CompanyInput, DataOrigin, Filters, NextAction,
        Status, UserId,
    },
};
use serde_json::Value;

use super::{HostedResult, Role, Store, auth, store::engine_options};

/// 公開デモのPOST本文上限（64KiB）。`/demo/enter` だけは256バイト。
pub(super) const MAX_POST_BODY_BYTES: usize = 64 * 1024;
/// 公開デモの既定アイドル終了時間（15分）。
pub(super) const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(900);
/// 公開デモの既定上限稼働時間（6時間）。
pub(super) const DEFAULT_MAX_UPTIME: Duration = Duration::from_secs(21_600);
/// 公開デモの既定停止検知時間（10秒）。
pub(super) const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(10);
/// `--demo-idle-seconds` の最小値（秒）。
pub(super) const MIN_IDLE_SECONDS: u64 = 1;
/// `--demo-idle-seconds` の最大値（秒）。
pub(super) const MAX_IDLE_SECONDS: u64 = 86_400;
/// `--demo-max-uptime-seconds` の最小値（秒）。
pub(super) const MIN_MAX_UPTIME_SECONDS: u64 = 1;
/// `--demo-max-uptime-seconds` の最大値（秒）。
pub(super) const MAX_MAX_UPTIME_SECONDS: u64 = 86_400;
/// `--demo-stall-seconds` の最小値（秒）。
pub(super) const MIN_STALL_SECONDS: u64 = 1;
/// `--demo-stall-seconds` の最大値（秒）。
pub(super) const MAX_STALL_SECONDS: u64 = 600;
/// 公開デモの既定入室頻度上限（1分あたりの回数）。
pub(super) const DEFAULT_ENTER_PER_MINUTE: u32 = 300;
/// 公開デモの既定取引先件数上限。
pub(super) const DEFAULT_MAX_COMPANIES: u64 = 1_000;
/// 公開デモの既定活動件数上限。
pub(super) const DEFAULT_MAX_ACTIVITIES: u64 = 5_000;
/// 公開デモの既定商談件数上限。
pub(super) const DEFAULT_MAX_OPPORTUNITIES: u64 = 200;
/// 公開デモの既定独自項目定義数上限。
pub(super) const DEFAULT_MAX_CUSTOM_FIELDS: u64 = 20;
/// 公開デモの既定担当者件数上限。
pub(super) const DEFAULT_MAX_CONTACTS: u64 = 2_000;
/// 公開デモの既定仕事（ToDo・予定）件数上限。
pub(super) const DEFAULT_MAX_WORK_ITEMS: u64 = 5_000;
/// 公開デモの既定共有架電リスト件数上限。
pub(super) const DEFAULT_MAX_CALLING_LISTS: u64 = 50;
/// 公開デモの既定保存先合計サイズ上限（32MiB）。
pub(super) const DEFAULT_MAX_STORAGE_BYTES: u64 = 32 * 1024 * 1024;

/// 公開デモで未認証のまま404で拒否する公開経路の接頭辞一覧。
pub(super) const DISABLED_PUBLIC_PREFIXES: &[&str] = &["/forms/", "/oauth/activity-return"];
/// 公開デモで認証後に403（`demo_disabled`）で拒否する経路の一覧。
/// 完全一致または `"{経路}/"` で始まる下位経路を拒否する。
pub(super) const DISABLED_AUTH_ROUTES: &[&str] = &[
    "/api/import",
    "/api/leads",
    "/api/lead-form",
    "/api/activity-sync",
];

/// 公開デモの組織ID。
pub(super) const DEMO_TENANT: &str = "demo";
/// 公開デモの入室用利用者名。
pub(super) const DEMO_USER: &str = "demo";
const DEMO_USER_B: &str = "demo-b";
const DEMO_COMPANY_COUNT: u64 = 150;
const DEMO_SEED: u64 = 42;
const NAME_PREFIX: &str = "合成・";
const ABSENCE_REQUEST_KEY: &str = "public-demo-initial-absence-callback";
const MAX_CONTACT_URL_LEN: usize = 2048;

const SECONDS_PER_DAY: i64 = 86_400;
const JST_OFFSET_SECS: i64 = 9 * 3600;
const MAX_TIMESTAMP: i64 = 253_402_300_799;

/// 公開デモモードの起動設定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DemoConfig {
    /// 入口や注意書きに出す運用者の連絡先URL（`http://` または `https://`）。
    pub(super) contact: Option<String>,
    /// 無操作で自己終了するまでの時間。
    pub(super) idle_timeout: Duration,
    /// 起動から自己終了するまでの上限時間。
    pub(super) max_uptime: Duration,
    /// ハートビートの停滞で自己終了するまでの時間。
    pub(super) stall_timeout: Duration,
    /// 1分あたりの全体入室上限回数。
    pub(super) enter_per_minute: u32,
    /// 取引先の件数上限。
    pub(super) max_companies: u64,
    /// 活動の件数上限。
    pub(super) max_activities: u64,
    /// 商談の件数上限。
    pub(super) max_opportunities: u64,
    /// 独自項目定義の件数上限。
    pub(super) max_custom_fields: u64,
    /// 担当者の件数上限。
    pub(super) max_contacts: u64,
    /// 仕事（ToDo・予定）の件数上限。
    pub(super) max_work_items: u64,
    /// 共有架電リストの件数上限。
    pub(super) max_calling_lists: u64,
    /// 保存先配下の合計ファイルサイズ上限（バイト）。
    pub(super) max_storage_bytes: u64,
}

impl DemoConfig {
    /// 起動引数から公開デモ設定を組み立て、連絡先URLを検証する。
    pub(super) fn new(contact: Option<String>) -> HostedResult<Self> {
        let contact = contact.map(|url| validate_contact_url(&url)).transpose()?;
        Ok(Self {
            contact,
            idle_timeout: DEFAULT_IDLE_TIMEOUT,
            max_uptime: DEFAULT_MAX_UPTIME,
            stall_timeout: DEFAULT_STALL_TIMEOUT,
            enter_per_minute: DEFAULT_ENTER_PER_MINUTE,
            max_companies: DEFAULT_MAX_COMPANIES,
            max_activities: DEFAULT_MAX_ACTIVITIES,
            max_opportunities: DEFAULT_MAX_OPPORTUNITIES,
            max_custom_fields: DEFAULT_MAX_CUSTOM_FIELDS,
            max_contacts: DEFAULT_MAX_CONTACTS,
            max_work_items: DEFAULT_MAX_WORK_ITEMS,
            max_calling_lists: DEFAULT_MAX_CALLING_LISTS,
            max_storage_bytes: DEFAULT_MAX_STORAGE_BYTES,
        })
    }

    /// アイドル終了時間を画面表示用の分数（1分以上）で返す。
    pub(super) const fn idle_timeout_minutes(&self) -> u64 {
        let minutes = self.idle_timeout.as_secs().div_ceil(60);
        if minutes == 0 { 1 } else { minutes }
    }
}

/// 公開デモ用の秒数引数を範囲付きで検証する。
pub(super) fn parse_seconds(raw: &str, flag: &str, min: u64, max: u64) -> HostedResult<Duration> {
    let valid = !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit());
    let parsed = if valid { raw.parse::<u64>().ok() } else { None };
    let Some(seconds) = parsed.filter(|value| (min..=max).contains(value)) else {
        return Err(format!("{flag}は{min}以上{max}以下の整数で指定してください。").into());
    };
    Ok(Duration::from_secs(seconds))
}

/// 公開デモで未認証のまま404で拒否すべき公開経路か判定する。
pub(super) fn is_disabled_public_path(path: &str) -> bool {
    DISABLED_PUBLIC_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

/// 公開デモで認証後に403（`demo_disabled`）で拒否すべき経路か判定する。
pub(super) fn is_disabled_auth_path(path: &str) -> bool {
    DISABLED_AUTH_ROUTES.iter().any(|route| {
        path == *route
            || path
                .strip_prefix(route)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// デモで本文を読む前に、ヘッダーと宣言された本文長を検査する。
/// `Expect` は417、POSTで `Content-Length` なし（分割転送を含む）は411、上限超過は413を返す。
pub(super) fn check_post_preflight(
    method: &str,
    headers: &[(String, String)],
    body_length: Option<usize>,
    max_body: usize,
) -> Option<(u16, &'static str)> {
    if headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("Expect"))
    {
        return Some((417, "Expectヘッダーには対応していません。"));
    }
    if method == "POST" {
        if headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("Transfer-Encoding"))
        {
            return Some((411, "Content-Lengthを指定してください。"));
        }
        let Some(length) = body_length else {
            return Some((411, "Content-Lengthを指定してください。"));
        };
        if length > max_body {
            return Some((413, "送信内容が上限を超えています。"));
        }
    }
    None
}

/// 保存先（ROOT）配下のすべての通常ファイル（SQLiteのWAL・共有メモリを含む）の合計サイズを返す。
pub(super) fn storage_bytes(root: &Path) -> HostedResult<u64> {
    let mut total = 0_u64;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            if meta.is_dir() {
                dirs.push(path);
            } else if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

/// 公開デモの入室回数を全体で1分の固定窓ごとに数える。
#[derive(Debug, Default)]
pub(super) struct EnterRateLimit {
    window_minute: Option<i64>,
    count: u32,
}

impl EnterRateLimit {
    /// 時刻 `at`（UNIX秒）の1分窓で、上限 `limit` 以内なら記録して `true` を返す。
    pub(super) fn allow(&mut self, at: i64, limit: u32) -> bool {
        let minute = at.div_euclid(60);
        if self.window_minute.is_none_or(|prev| minute > prev) {
            self.window_minute = Some(minute);
            self.count = 0;
        }
        if self.count >= limit {
            return false;
        }
        self.count += 1;
        true
    }
}

/// 連絡先URLが `http://` または `https://` の安全なURLであることを検証する。
pub(super) fn validate_contact_url(raw: &str) -> HostedResult<String> {
    let err = || "デモの連絡先URLはhttp://またはhttps://の有効なURLで指定してください。".into();
    if raw.is_empty() || raw.len() > MAX_CONTACT_URL_LEN {
        return Err(err());
    }
    let Some(rest) = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("http://"))
    else {
        return Err(err());
    };
    if !raw
        .bytes()
        .all(|b| b.is_ascii_graphic() && !matches!(b, b'<' | b'>' | b'"' | b'\'' | b'`' | b'\\'))
    {
        return Err(err());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(err());
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((addr, tail)) = after_open.split_once(']') else {
            return Err(err());
        };
        if addr.parse::<Ipv6Addr>().is_err() {
            return Err(err());
        }
        if !tail.is_empty() {
            let Some(port) = tail.strip_prefix(':') else {
                return Err(err());
            };
            if !valid_port(port) {
                return Err(err());
            }
        }
    } else {
        let mut parts = authority.split(':');
        let host = parts.next().unwrap_or_default();
        if let Some(port) = parts.next()
            && !valid_port(port)
        {
            return Err(err());
        }
        if parts.next().is_some() {
            return Err(err());
        }
        if host.is_empty()
            || host.len() > 253
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
            || host
                .split('.')
                .any(|label| label.is_empty() || label.starts_with('-') || label.ends_with('-'))
        {
            return Err(err());
        }
    }
    Ok(raw.to_owned())
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|value| value > 0)
}

/// 公開デモ用の保存先が、未作成または空の通常ディレクトリであることを確かめる。
pub(super) fn ensure_empty_root(root: &Path) -> HostedResult<()> {
    match fs::symlink_metadata(root) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(
                    "公開デモの保存先は未作成または空の通常ディレクトリだけを指定してください。"
                        .into(),
                );
            }
            if fs::read_dir(root)?.next().is_some() {
                return Err(
                    "公開デモの保存先に既存のデータがあります。空または未作成のディレクトリを指定してください。"
                        .into(),
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// 未作成または空の保存先を初期化し、起動時刻 `now`（UNIX秒）を基準に公開デモの初期データを作る。
pub(super) fn bootstrap(root: &Path, now: i64) -> HostedResult<Store> {
    bootstrap_with_password_source(root, now, || auth::random_hex(32))
}

fn bootstrap_with_password_source(
    root: &Path,
    now: i64,
    mut next_password: impl FnMut() -> HostedResult<String>,
) -> HostedResult<Store> {
    let today = JstDate::from_unix_secs(now)?;
    ensure_empty_root(root)?;
    let mut store = Store::initialize_for_demo(root)?;
    let guard = store.lock()?;
    store.add_tenant(DEMO_TENANT, DataOrigin::Synthetic, 0)?;
    for name in [DEMO_USER, DEMO_USER_B] {
        // OSの乱数から生成した資格情報はハッシュ化後に即座に破棄し、保存も出力もしない。
        let password = next_password()?;
        store.add_user(name, &password)?;
        drop(password);
        store.set_member(name, DEMO_TENANT, Some(Role::Editor))?;
    }
    let members = store.sales_members(DEMO_TENANT)?;
    let demo_id = find_member_id(&members, DEMO_USER)?;
    let demo_b_id = find_member_id(&members, DEMO_USER_B)?;

    let tenant_path = store.tenant_path(DEMO_TENANT)?;
    let mut engine = Engine::open(&tenant_path, &engine_options())?;
    seed_workspace(&mut engine, demo_id, demo_b_id, today)?;
    drop(engine);
    drop(guard);
    Ok(store)
}

fn find_member_id(members: &[Value], username: &str) -> HostedResult<i64> {
    members
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(username))
        .and_then(|entry| entry.get("id").and_then(Value::as_i64))
        .ok_or_else(|| "デモ用の営業メンバーを取得できません。".into())
}

fn seed_workspace(
    engine: &mut Engine,
    demo_id: i64,
    demo_b_id: i64,
    today: JstDate,
) -> HostedResult<()> {
    seed_companies(engine, demo_id, demo_b_id)?;
    let (deal_companies, followup_company) = select_targets(engine)?;
    seed_opportunities(engine, &deal_companies, demo_id, demo_b_id, today)?;
    let current_month = today.ym();
    engine.crm_set_sales_target(&current_month, UserId(demo_id), Some(400_000))?;
    engine.crm_set_sales_target(&current_month, UserId(demo_b_id), Some(200_000))?;
    seed_overdue_callback(engine, followup_company, demo_id, today)?;
    Ok(())
}

fn seed_companies(engine: &mut Engine, demo_id: i64, demo_b_id: i64) -> HostedResult<()> {
    let config = SynthConfig {
        rows: DEMO_COMPANY_COUNT,
        seed: DEMO_SEED,
        profile: Profile::Dense,
    };
    let companies: Vec<CompanyInput> = Synth::new(config)
        .enumerate()
        .map(|(index, record)| {
            let mut company = record.company;
            company.name = format!("{NAME_PREFIX}{}", company.name);
            company.phone = company.phone.map(|_| format!("000-0000-{:04}", index + 1));
            company.owner = company
                .owner
                .map(|UserId(id)| UserId(if id % 2 == 1 { demo_id } else { demo_b_id }));
            company
        })
        .collect();
    engine.import_companies(&companies, DuplicatePolicy::Error)?;
    Ok(())
}

fn select_targets(engine: &Engine) -> HostedResult<([CompanyId; 9], CompanyId)> {
    let mut eligible = Vec::new();
    engine.visit_companies(&Filters::default(), &Vec::new(), |company| {
        if !company.is_sole_proprietor && company.status != Status::Excluded {
            eligible.push((company.id, company.phone.is_some()));
        }
        Ok(())
    })?;
    eligible.sort_by_key(|(id, _)| id.0);
    if eligible.len() < 10 {
        return Err("デモ用の取引先候補が不足しています。".into());
    }
    let deal_companies = [
        eligible[0].0,
        eligible[1].0,
        eligible[2].0,
        eligible[3].0,
        eligible[4].0,
        eligible[5].0,
        eligible[6].0,
        eligible[7].0,
        eligible[8].0,
    ];
    let followup_company = eligible[9..]
        .iter()
        .find(|(_, has_phone)| *has_phone)
        .or_else(|| eligible.iter().find(|(_, has_phone)| *has_phone))
        .map(|(id, _)| *id)
        .ok_or("再架電の対象となる電話番号付きの取引先が見つかりません。")?;
    Ok((deal_companies, followup_company))
}

#[derive(Clone, Copy)]
enum DealOwner {
    Demo,
    DemoB,
    Unassigned,
}

#[derive(Clone, Copy)]
enum DealCloseDate {
    Today,
    ThisMonthPlus(u8),
    NextMonthDay(u8),
}

struct DealSpec {
    name: &'static str,
    amount_jpy: i64,
    stage: u8,
    probability: Option<u8>,
    owner: DealOwner,
    close_date: DealCloseDate,
    decided_today: bool,
}

const DEMO_DEALS: [DealSpec; 9] = [
    DealSpec {
        name: "合成・Webサイト制作",
        amount_jpy: 320_000,
        stage: 4,
        probability: None,
        owner: DealOwner::Demo,
        close_date: DealCloseDate::Today,
        decided_today: true,
    },
    DealSpec {
        name: "合成・旧サイトの追加提案",
        amount_jpy: 90_000,
        stage: 5,
        probability: None,
        owner: DealOwner::Unassigned,
        close_date: DealCloseDate::Today,
        decided_today: true,
    },
    DealSpec {
        name: "合成・受注管理画面の改修",
        amount_jpy: 300_000,
        stage: 2,
        probability: Some(60),
        owner: DealOwner::Demo,
        close_date: DealCloseDate::ThisMonthPlus(7),
        decided_today: false,
    },
    DealSpec {
        name: "合成・商品カタログ制作",
        amount_jpy: 180_000,
        stage: 1,
        probability: Some(25),
        owner: DealOwner::DemoB,
        close_date: DealCloseDate::ThisMonthPlus(7),
        decided_today: false,
    },
    DealSpec {
        name: "合成・営業リスト整理の相談",
        amount_jpy: 150_000,
        stage: 0,
        probability: Some(10),
        owner: DealOwner::DemoB,
        close_date: DealCloseDate::NextMonthDay(10),
        decided_today: false,
    },
    DealSpec {
        name: "合成・パンフレット刷新",
        amount_jpy: 80_000,
        stage: 0,
        probability: Some(10),
        owner: DealOwner::Demo,
        close_date: DealCloseDate::NextMonthDay(12),
        decided_today: false,
    },
    DealSpec {
        name: "合成・保守契約の見直し",
        amount_jpy: 250_000,
        stage: 2,
        probability: Some(50),
        owner: DealOwner::DemoB,
        close_date: DealCloseDate::ThisMonthPlus(10),
        decided_today: false,
    },
    DealSpec {
        name: "合成・展示会ブースの制作",
        amount_jpy: 400_000,
        stage: 3,
        probability: Some(75),
        owner: DealOwner::Demo,
        close_date: DealCloseDate::ThisMonthPlus(9),
        decided_today: false,
    },
    DealSpec {
        name: "合成・採用ページの制作",
        amount_jpy: 220_000,
        stage: 1,
        probability: Some(30),
        owner: DealOwner::DemoB,
        close_date: DealCloseDate::NextMonthDay(5),
        decided_today: false,
    },
];

fn seed_opportunities(
    engine: &mut Engine,
    companies: &[CompanyId; 9],
    demo_id: i64,
    demo_b_id: i64,
    today: JstDate,
) -> HostedResult<()> {
    let today_ymd = today.ymd();
    for (spec, &company) in DEMO_DEALS.iter().zip(companies.iter()) {
        let close_date = match spec.close_date {
            DealCloseDate::Today => today_ymd.clone(),
            DealCloseDate::ThisMonthPlus(days) => today.plus_days_clamped(days),
            DealCloseDate::NextMonthDay(day) => today.next_month_day(day),
        };
        let outcome_date = spec.decided_today.then_some(today_ymd.as_str());
        let owner = match spec.owner {
            DealOwner::Demo => OpportunityOwnerChange::Assign(UserId(demo_id)),
            DealOwner::DemoB => OpportunityOwnerChange::Assign(UserId(demo_b_id)),
            DealOwner::Unassigned => OpportunityOwnerChange::Unassigned,
        };
        let input = OpportunityInput {
            company,
            contact: None,
            name: spec.name.to_owned(),
            amount_jpy: spec.amount_jpy,
            stage: spec.stage,
            close_date,
        };
        let forecast = OpportunityForecastPatch {
            owner,
            probability: Some(spec.probability),
        };
        engine.crm_create_opportunity_forecast(&input, outcome_date, forecast)?;
    }
    Ok(())
}

fn seed_overdue_callback(
    engine: &mut Engine,
    company: CompanyId,
    demo_id: i64,
    today: JstDate,
) -> HostedResult<()> {
    let due_at = today.yesterday_morning_9_jst();
    let call_at = today.absence_call_at();
    let fingerprint = auth::hex(&auth::digest(
        format!("public-demo:absence:{}:{demo_id}:{due_at}", company.0).as_bytes(),
    ));
    let input = ActivityInput {
        company,
        contact: None,
        kind: ActivityKind::Call,
        at: call_at,
        note: Some("不在のため再架電を予約".into()),
        status_after: Some(Status::InProgress),
        next_action: NextAction::Set(due_at),
        reverses: None,
    };
    engine.crm_record_absence(&input, Some(demo_id), ABSENCE_REQUEST_KEY, &fingerprint)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct JstDate {
    day_index: i64,
    year: i32,
    month: u8,
    day: u8,
}

impl JstDate {
    fn from_unix_secs(now: i64) -> HostedResult<Self> {
        if !(2 * SECONDS_PER_DAY..=MAX_TIMESTAMP - 32 * SECONDS_PER_DAY).contains(&now) {
            return Err("起動時刻が有効範囲外です。".into());
        }
        let day_index = (now + JST_OFFSET_SECS).div_euclid(SECONDS_PER_DAY);
        let (year, month, day) = civil_from_days(day_index)?;
        Ok(Self {
            day_index,
            year,
            month,
            day,
        })
    }

    fn ymd(self) -> String {
        format!("{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }

    fn ym(self) -> String {
        format!("{:04}-{:02}", self.year, self.month)
    }

    fn plus_days_clamped(self, days: u8) -> String {
        let last = days_in_month(self.year, self.month);
        let target_day = self.day.saturating_add(days).min(last);
        format!("{:04}-{:02}-{target_day:02}", self.year, self.month)
    }

    fn next_month_day(self, day: u8) -> String {
        let (year, month) = if self.month == 12 {
            (self.year + 1, 1)
        } else {
            (self.year, self.month + 1)
        };
        format!("{year:04}-{month:02}-{day:02}")
    }

    /// 日本時間の昨日の朝9時（00:00 UTCと同値）をUNIX秒で返す。
    const fn yesterday_morning_9_jst(self) -> i64 {
        (self.day_index - 1) * SECONDS_PER_DAY
    }

    /// 昨日の朝9時の19時間前（一昨日の14:00 JST）をUNIX秒で返す。
    const fn absence_call_at(self) -> i64 {
        self.yesterday_morning_9_jst() - 19 * 3600
    }
}

fn civil_from_days(days_since_epoch: i64) -> HostedResult<(i32, u8, u8)> {
    let z = days_since_epoch + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(m <= 2);
    Ok((i32::try_from(year)?, u8::try_from(m)?, u8::try_from(d)?))
}

const fn is_leap_year(year: i32) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use meibo_core::{
        crm::OpportunityQuery,
        types::{Field, Filters},
    };

    use super::*;
    use crate::hosted::{Hosted, HostedConfig, cli, control::ControlStore, http::HostedStorage};

    fn jst_unix(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> i64 {
        let y = i64::from(year) - i64::from(month <= 2);
        let era = y.div_euclid(400);
        let yoe = y - era * 400;
        let m = i64::from(month);
        let d = i64::from(day);
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        let days = era * 146_097 + doe - 719_468;
        days * SECONDS_PER_DAY + i64::from(hour) * 3600 + i64::from(minute) * 60 - JST_OFFSET_SECS
    }

    fn collect_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect_files(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn demo_initial_sales_report_matches_expected_totals_across_month_boundaries() {
        let cases = [
            (
                "month_start",
                jst_unix(2026, 11, 1, 0, 30),
                "2026-11",
                "2026-12",
            ),
            (
                "mid_month",
                jst_unix(2026, 11, 15, 12, 0),
                "2026-11",
                "2026-12",
            ),
            (
                "month_end",
                jst_unix(2026, 11, 30, 23, 30),
                "2026-11",
                "2026-12",
            ),
            (
                "year_end",
                jst_unix(2026, 12, 31, 23, 30),
                "2026-12",
                "2027-01",
            ),
        ];
        for (label, now, expected_month, expected_next_month) in cases {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join(label);
            let store = bootstrap(&root, now).unwrap();
            let members = store.sales_members(DEMO_TENANT).unwrap();
            let demo_id = find_member_id(&members, DEMO_USER).unwrap();
            let demo_b_id = find_member_id(&members, DEMO_USER_B).unwrap();

            let engine =
                Engine::open(&store.tenant_path(DEMO_TENANT).unwrap(), &engine_options()).unwrap();
            let report = engine.crm_sales_report(expected_month).unwrap();
            assert_eq!(report.won_amount_jpy, 320_000, "{label}");
            assert_eq!(report.won_count, 1, "{label}");
            assert_eq!(report.lost_count, 1, "{label}");
            assert_eq!(report.win_rate_percent(), Some(50.0), "{label}");
            assert_eq!(report.undated_count, 0, "{label}");

            let weighted_total: u128 = report
                .projections
                .values()
                .map(|p| p.weighted_amount_jpy)
                .sum();
            let projected_total = report.won_amount_jpy + weighted_total;
            assert_eq!(weighted_total, 650_000, "{label}");
            assert_eq!(projected_total, 970_000, "{label}");

            let demo_proj = &report.projections[&Some(UserId(demo_id))];
            assert_eq!(demo_proj.won_amount_jpy, 320_000, "{label}");
            assert_eq!(demo_proj.weighted_amount_jpy, 480_000, "{label}");
            assert_eq!(demo_proj.projected_amount_jpy(), 800_000, "{label}");
            assert_eq!(demo_proj.target_jpy, Some(400_000), "{label}");

            let demo_b_proj = &report.projections[&Some(UserId(demo_b_id))];
            assert_eq!(demo_b_proj.won_amount_jpy, 0, "{label}");
            assert_eq!(demo_b_proj.weighted_amount_jpy, 170_000, "{label}");
            assert_eq!(demo_b_proj.projected_amount_jpy(), 170_000, "{label}");
            assert_eq!(demo_b_proj.target_jpy, Some(200_000), "{label}");

            let next_report = engine.crm_sales_report(expected_next_month).unwrap();
            let next_open_count: u64 = next_report.projections.values().map(|p| p.open_count).sum();
            assert_eq!(next_open_count, 3, "{label} next month deals");

            let deals = engine
                .crm_opportunities(&OpportunityQuery {
                    company: None,
                    after: None,
                    limit: 50,
                })
                .unwrap();
            assert_eq!(deals.total, 9, "{label}");
            let distinct_companies: BTreeSet<i64> = deals
                .items
                .iter()
                .map(|deal| deal.input.company.0)
                .collect();
            assert_eq!(distinct_companies.len(), 9, "{label}");
            for deal in &deals.items {
                assert!(
                    deal.company_name
                        .as_deref()
                        .is_some_and(|name| name.starts_with(NAME_PREFIX)),
                    "{label}: {:?}",
                    deal.company_name
                );
            }
            if label == "year_end" {
                let next_dates: BTreeSet<&str> = deals
                    .items
                    .iter()
                    .filter(|deal| deal.input.close_date.starts_with("2027-01-"))
                    .map(|deal| deal.input.close_date.as_str())
                    .collect();
                assert_eq!(
                    next_dates,
                    BTreeSet::from(["2027-01-05", "2027-01-10", "2027-01-12"])
                );
            }
            let verify = engine.verify().unwrap();
            assert!(verify.integrity_ok && verify.counts_match && verify.folds_match);
        }
    }

    #[test]
    fn demo_companies_use_synthetic_names_nonexistent_phones_and_invalid_urls() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("companies");
        let now = jst_unix(2026, 11, 15, 12, 0);
        let store = bootstrap(&root, now).unwrap();
        let engine =
            Engine::open(&store.tenant_path(DEMO_TENANT).unwrap(), &engine_options()).unwrap();
        assert_eq!(engine.origin(), DataOrigin::Synthetic);

        let filters = Filters {
            include_excluded: true,
            ..Filters::default()
        };
        let fields = vec![Field::Name, Field::Phone, Field::Website];
        let mut count = 0_u64;
        let mut with_phone = 0_u64;
        let mut with_website = 0_u64;
        engine
            .visit_companies(&filters, &fields, |company| {
                count += 1;
                let name = company.name.expect("all companies have a name");
                assert!(name.starts_with(NAME_PREFIX), "unexpected name: {name}");
                if let Some(phone) = company.phone {
                    with_phone += 1;
                    assert!(
                        phone.starts_with("000-0000-") && phone.len() == 13,
                        "unexpected phone: {phone}"
                    );
                    assert!(phone[9..].bytes().all(|b| b.is_ascii_digit()));
                }
                if let Some(website) = company.website {
                    with_website += 1;
                    assert!(
                        website.starts_with("https://example.invalid/"),
                        "unexpected website: {website}"
                    );
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(count, DEMO_COMPANY_COUNT);
        assert!(with_phone > 0);
        assert!(with_website > 0);
    }

    #[test]
    fn demo_overdue_followup_is_due_yesterday_morning_jst_for_demo_user() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("followups");
        let now = jst_unix(2026, 11, 1, 0, 30);
        let expected_due = jst_unix(2026, 10, 31, 9, 0);
        let store = bootstrap(&root, now).unwrap();
        let members = store.sales_members(DEMO_TENANT).unwrap();
        let demo_id = find_member_id(&members, DEMO_USER).unwrap();
        let demo_b_id = find_member_id(&members, DEMO_USER_B).unwrap();

        let engine =
            Engine::open(&store.tenant_path(DEMO_TENANT).unwrap(), &engine_options()).unwrap();
        let due = engine.crm_due_followups(Some(demo_id), now).unwrap();
        assert_eq!(due.total, 1);
        assert_eq!(due.items.len(), 1);
        assert_eq!(due.items[0].owner_id, Some(demo_id));
        assert_eq!(due.items[0].due_at, expected_due);
        assert!(
            due.items[0]
                .company_name
                .as_deref()
                .is_some_and(|name| name.starts_with(NAME_PREFIX))
        );

        let due_b = engine.crm_due_followups(Some(demo_b_id), now).unwrap();
        assert_eq!(due_b.total, 0);
        assert!(due_b.items.is_empty());
    }

    #[test]
    fn demo_users_are_sales_members_and_plaintext_passwords_never_appear_in_storage() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("users");
        let now = jst_unix(2026, 11, 15, 12, 0);
        let mut generated = Vec::new();
        let store = bootstrap_with_password_source(&root, now, || {
            let secret = auth::random_hex(32)?;
            generated.push(secret.clone());
            Ok(secret)
        })
        .unwrap();
        assert_eq!(generated.len(), 2);
        assert_ne!(generated[0], generated[1]);
        assert!(generated.iter().all(|pw| pw.len() == 64));

        let members = store.sales_members(DEMO_TENANT).unwrap();
        assert_eq!(members.len(), 2);
        let names: Vec<&str> = members
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec![DEMO_USER, DEMO_USER_B]);

        let hashes: Vec<String> = store
            .conn
            .prepare("SELECT password_hash FROM users ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(hashes.len(), 2);
        assert_ne!(hashes[0], hashes[1]);
        for hash in &hashes {
            assert!(hash.starts_with("$argon2id$"));
            for secret in &generated {
                assert!(!hash.contains(secret));
            }
        }
        drop(store);

        let mut files = Vec::new();
        collect_files(&root, &mut files);
        assert!(files.len() >= 2);
        for file in files {
            let bytes = fs::read(&file).unwrap();
            for secret in &generated {
                assert!(
                    !bytes
                        .windows(secret.len())
                        .any(|window| window == secret.as_bytes()),
                    "plaintext secret leaked into {}",
                    file.display()
                );
            }
        }
    }

    #[test]
    fn demo_root_accepts_missing_or_empty_directory_and_rejects_nonempty_or_file() {
        let temp = tempfile::tempdir().unwrap();
        let now = jst_unix(2026, 11, 15, 12, 0);

        let empty_dir = temp.path().join("empty");
        fs::create_dir(&empty_dir).unwrap();
        assert!(bootstrap(&empty_dir, now).is_ok());

        let populated_dir = temp.path().join("populated");
        fs::create_dir(&populated_dir).unwrap();
        fs::write(populated_dir.join("existing.txt"), b"real").unwrap();
        let err = bootstrap(&populated_dir, now).err().unwrap().to_string();
        assert!(err.contains("既存のデータ"));

        let existing_hosted = temp.path().join("existing-hosted");
        let mut store = Store::initialize(&existing_hosted).unwrap();
        store.add_tenant("real-org", DataOrigin::Real, 0).unwrap();
        drop(store);
        assert!(bootstrap(&existing_hosted, now).is_err());

        let regular_file = temp.path().join("remote.json");
        fs::write(&regular_file, b"{}").unwrap();
        let err = bootstrap(&regular_file, now).err().unwrap().to_string();
        assert!(err.contains("通常ディレクトリ"));

        #[cfg(unix)]
        {
            let target = temp.path().join("symlink-target");
            fs::create_dir(&target).unwrap();
            let link = temp.path().join("symlink-root");
            std::os::unix::fs::symlink(&target, &link).unwrap();
            let err = bootstrap(&link, now).err().unwrap().to_string();
            assert!(err.contains("通常ディレクトリ"));
        }
    }

    #[test]
    fn demo_cli_options_validate_contact_and_reject_incompatible_combinations() {
        let ok = cli::serve_config(&[
            "--origin".into(),
            "https://demo.example.invalid".into(),
            "--bind".into(),
            "127.0.0.1:4317".into(),
            "--public-demo".into(),
            "--demo-contact".into(),
            "https://example.invalid/issues?kind=demo#top".into(),
        ])
        .unwrap();
        assert_eq!(
            ok.demo,
            Some(
                DemoConfig::new(Some("https://example.invalid/issues?kind=demo#top".into()))
                    .unwrap()
            )
        );

        let loopback = cli::serve_config(&[
            "--origin".into(),
            "http://127.0.0.1:4317".into(),
            "--bind".into(),
            "127.0.0.1:4317".into(),
            "--allow-http-loopback".into(),
            "--public-demo".into(),
        ])
        .unwrap();
        assert!(loopback.development);
        assert_eq!(loopback.demo, Some(DemoConfig::new(None).unwrap()));

        let normal = cli::serve_config(&[
            "--origin".into(),
            "https://crm.example.invalid".into(),
            "--bind".into(),
            "127.0.0.1:4317".into(),
        ])
        .unwrap();
        assert!(normal.demo.is_none());

        // --public-demo と --access-config の併用を拒否する。
        let access_err = cli::serve_config(&[
            "--origin".into(),
            "https://demo.example.invalid".into(),
            "--public-demo".into(),
            "--access-config".into(),
            "access.json".into(),
        ])
        .err()
        .unwrap()
        .to_string();
        assert!(access_err.contains("--access-config"));

        // --demo-contact 単独は拒否する。
        assert!(
            cli::serve_config(&[
                "--origin".into(),
                "https://crm.example.invalid".into(),
                "--demo-contact".into(),
                "https://example.invalid/contact".into(),
            ])
            .is_err()
        );

        // 不正な --demo-contact URL を拒否する。
        for bad in [
            "",
            "javascript:alert(1)",
            "mailto:demo@example.invalid",
            "ftp://example.invalid/contact",
            "HTTPS://example.invalid/contact",
            "https://",
            "https:///path",
            "https://user:pass@example.invalid",
            "https://example.invalid:0",
            "https://example.invalid/has space",
            "https://example.invalid/\"quote",
            "https://example..invalid",
        ] {
            assert!(
                cli::serve_config(&[
                    "--origin".into(),
                    "https://demo.example.invalid".into(),
                    "--public-demo".into(),
                    "--demo-contact".into(),
                    bad.into(),
                ])
                .is_err(),
                "should reject contact URL: {bad}"
            );
        }
    }

    struct DummyRemoteStore(Store);
    impl std::ops::Deref for DummyRemoteStore {
        type Target = ControlStore;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }
    impl std::ops::DerefMut for DummyRemoteStore {
        fn deref_mut(&mut self) -> &mut Self::Target {
            &mut self.0
        }
    }
    impl HostedStorage for DummyRemoteStore {
        type Guard = ();
        fn acquire(&self) -> HostedResult<Self::Guard> {
            Ok(())
        }
        fn complete<T>(
            &self,
            _guard: Self::Guard,
            result: HostedResult<T>,
            _uncertain: bool,
        ) -> HostedResult<T> {
            result
        }
        fn open_engine(&self, tenant: &str) -> HostedResult<Engine> {
            self.0.open_engine(tenant)
        }
        fn remote(&self) -> bool {
            true
        }
    }

    #[test]
    fn demo_serve_rejects_remote_storage() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::initialize(&temp.path().join("remote-dummy")).unwrap();
        let mut config =
            HostedConfig::parse("https://demo.example.invalid", "127.0.0.1:0", false).unwrap();
        config.demo = Some(DemoConfig::new(None).unwrap());
        let err = Hosted::new(DummyRemoteStore(store), config)
            .serve()
            .err()
            .unwrap()
            .to_string();
        assert!(err.contains("遠隔保存"));
    }

    #[test]
    fn jst_date_clamps_month_end_and_rolls_over_year_and_leap_day() {
        let feb_non_leap = JstDate::from_unix_secs(jst_unix(2026, 2, 28, 23, 30)).unwrap();
        assert_eq!(feb_non_leap.ymd(), "2026-02-28");
        assert_eq!(feb_non_leap.plus_days_clamped(10), "2026-02-28");
        assert_eq!(feb_non_leap.next_month_day(5), "2026-03-05");
        assert_eq!(
            feb_non_leap.yesterday_morning_9_jst(),
            jst_unix(2026, 2, 27, 9, 0)
        );

        let feb_leap = JstDate::from_unix_secs(jst_unix(2028, 2, 25, 12, 0)).unwrap();
        assert_eq!(feb_leap.plus_days_clamped(7), "2028-02-29");

        let dec_end = JstDate::from_unix_secs(jst_unix(2026, 12, 31, 23, 30)).unwrap();
        assert_eq!(dec_end.ymd(), "2026-12-31");
        assert_eq!(dec_end.plus_days_clamped(9), "2026-12-31");
        assert_eq!(dec_end.next_month_day(10), "2027-01-10");
        assert_eq!(dec_end.next_month_day(12), "2027-01-12");
        assert_eq!(dec_end.next_month_day(5), "2027-01-05");

        let jan_start = JstDate::from_unix_secs(jst_unix(2027, 1, 1, 0, 30)).unwrap();
        assert_eq!(
            jan_start.yesterday_morning_9_jst(),
            jst_unix(2026, 12, 31, 9, 0)
        );
    }

    #[test]
    fn contact_url_validation_matches_shared_cases() {
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../../deploy/demo/cloudflare/tests/contact-url-cases.json"
        ))
        .unwrap();
        let valid_count = cases
            .iter()
            .filter(|entry| entry["valid"].as_bool() == Some(true))
            .count();
        let invalid_count = cases
            .iter()
            .filter(|entry| entry["valid"].as_bool() == Some(false))
            .count();
        assert!(valid_count >= 15, "valid cases={valid_count}");
        assert!(invalid_count >= 15, "invalid cases={invalid_count}");
        for entry in &cases {
            let url = entry["url"].as_str().expect("url string");
            let expected = entry["valid"].as_bool().expect("valid bool");
            let note = entry["note"].as_str().unwrap_or("");
            if note == "全体長ちょうど2048文字" {
                assert_eq!(url.len(), MAX_CONTACT_URL_LEN);
            }
            if note == "全体長2049文字超過" {
                assert_eq!(url.len(), MAX_CONTACT_URL_LEN + 1);
            }
            if note == "ホスト長ちょうど253文字" {
                assert_eq!(url.strip_prefix("https://").unwrap().len(), 253);
            }
            if note == "ホスト長254文字超過" {
                assert_eq!(url.strip_prefix("https://").unwrap().len(), 254);
            }
            assert_eq!(
                validate_contact_url(url).is_ok(),
                expected,
                "case ({note}): {url:?}"
            );
        }
    }
}
