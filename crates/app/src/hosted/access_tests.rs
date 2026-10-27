use super::access::{Access, AccessError, Config, KeySource};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use serde_json::json;
use std::{
    cell::Cell,
    collections::VecDeque,
    io::Write,
    process::{Command, Stdio},
    rc::Rc,
    sync::OnceLock,
};

pub(super) const AT: i64 = 1_790_000_000;

pub(super) fn configuration() -> serde_json::Value {
    json!({"version":1,"issuer":"https://synthetic.cloudflareaccess.com","audience":"synthetic-application-audience",
        "users":[{"email":"alice@example.invalid","username":"admin"}]})
}

pub(super) struct Fixture {
    pub(super) key: EncodingKey,
    pub(super) jwks: serde_json::Value,
    pub(super) kid: String,
}
fn openssl(args: &[&str], input: Option<&[u8]>) -> Vec<u8> {
    let mut child = Command::new("openssl")
        .args(args)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("OpenSSL fixture tool");
    if let Some(input) = input {
        child.stdin.take().unwrap().write_all(input).unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "synthetic key generation failed");
    output.stdout
}
fn make_fixture(kid: &str) -> Fixture {
    let pem = openssl(
        &[
            "genpkey",
            "-algorithm",
            "RSA",
            "-pkeyopt",
            "rsa_keygen_bits:2048",
            "-pkeyopt",
            "rsa_keygen_pubexp:65537",
        ],
        None,
    );
    let der = openssl(&["rsa", "-traditional", "-outform", "DER"], Some(&pem));
    let modulus = String::from_utf8(openssl(&["rsa", "-noout", "-modulus"], Some(&pem))).unwrap();
    let hex = modulus.trim().strip_prefix("Modulus=").unwrap();
    let bytes: Vec<u8> = hex
        .as_bytes()
        .chunks(2)
        .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
        .collect();
    Fixture {
        key: EncodingKey::from_rsa_der(&der),
        kid: kid.into(),
        jwks: json!({"keys":[{"kty":"RSA","alg":"RS256","use":"sig","kid":kid,"n":URL_SAFE_NO_PAD.encode(bytes),"e":"AQAB"}]}),
    }
}
pub(super) fn fixtures() -> &'static (Fixture, Fixture) {
    static KEYS: OnceLock<(Fixture, Fixture)> = OnceLock::new();
    KEYS.get_or_init(|| (make_fixture("fixture-a"), make_fixture("fixture-b")))
}
pub(super) fn claims() -> serde_json::Value {
    json!({"iss":"https://synthetic.cloudflareaccess.com","aud":["synthetic-application-audience"],
        "sub":"synthetic-person-a","email":"alice@example.invalid","type":"app","iat":AT-10,"exp":AT+3600})
}
pub(super) fn sign(value: &serde_json::Value, fixture: &Fixture) -> String {
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(fixture.kid.clone());
    encode(&header, value, &fixture.key).unwrap()
}
fn sign_header(header: &serde_json::Value, value: &serde_json::Value, fixture: &Fixture) -> String {
    let message = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(header).unwrap()),
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap())
    );
    let signature =
        jsonwebtoken::crypto::sign(message.as_bytes(), &fixture.key, Algorithm::RS256).unwrap();
    format!("{message}.{signature}")
}
struct Source {
    replies: VecDeque<Result<Vec<u8>, AccessError>>,
    calls: Rc<Cell<usize>>,
}
impl KeySource for Source {
    fn fetch(&mut self) -> Result<Vec<u8>, AccessError> {
        self.calls.set(self.calls.get() + 1);
        self.replies
            .pop_front()
            .unwrap_or(Err(AccessError::Unavailable))
    }
}
pub(super) fn verifier(
    config: &serde_json::Value,
    replies: Vec<Result<serde_json::Value, AccessError>>,
) -> (Access, Rc<Cell<usize>>) {
    let calls = Rc::new(Cell::new(0));
    let source = Source {
        replies: replies
            .into_iter()
            .map(|v| v.map(|v| serde_json::to_vec(&v).unwrap()))
            .collect(),
        calls: calls.clone(),
    };
    (
        Access::with_source(
            Config::parse(&serde_json::to_vec(config).unwrap()).unwrap(),
            Box::new(source),
        ),
        calls,
    )
}

#[test]
fn access_verifies_an_actual_rsa_signature_and_caches_public_keys() {
    let fixture = &fixtures().0;
    let token = sign(&claims(), fixture);
    let (mut access, calls) = verifier(&configuration(), vec![Ok(fixture.jwks.clone())]);
    assert_eq!(access.verify(&token, AT).unwrap(), "admin");
    assert_eq!(access.verify(&token, AT + 10).unwrap(), "admin");
    assert_eq!(calls.get(), 1);
}

#[test]
fn access_rejects_wrong_signature_algorithm_and_critical_extensions() {
    let (a, b) = fixtures();
    let (mut access, _) = verifier(&configuration(), vec![Ok(a.jwks.clone())]);
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(a.kid.clone());
    let wrong_signature = encode(&header, &claims(), &b.key).unwrap();
    assert_eq!(
        access.verify(&wrong_signature, AT).unwrap_err(),
        AccessError::Invalid
    );
    let mut hmac = Header::new(Algorithm::HS256);
    hmac.kid = Some(a.kid.clone());
    let token = encode(
        &hmac,
        &claims(),
        &EncodingKey::from_secret(b"synthetic fixture only"),
    )
    .unwrap();
    assert_eq!(access.verify(&token, AT).unwrap_err(), AccessError::Invalid);
    let raw = json!({"alg":"RS256","kid":a.kid,"crit":["unrecognized"]});
    let critical = sign_header(&raw, &claims(), a);
    assert_eq!(
        access.verify(&critical, AT).unwrap_err(),
        AccessError::Invalid
    );
    assert_eq!(
        access.verify(&"x".repeat(16 * 1024 + 1), AT).unwrap_err(),
        AccessError::Invalid
    );
}

#[test]
fn access_never_uses_keys_or_urls_supplied_by_a_token() {
    let (a, b) = fixtures();
    let (mut access, calls) = verifier(&configuration(), vec![Ok(a.jwks.clone())]);
    let header = json!({"alg":"RS256","kid":a.kid,"jku":"http://127.0.0.1/internal"});
    assert_eq!(
        access
            .verify(&sign_header(&header, &claims(), a), AT)
            .unwrap(),
        "admin"
    );
    let header = json!({"alg":"RS256","kid":a.kid,"jwk":b.jwks["keys"][0]});
    assert_eq!(
        access
            .verify(&sign_header(&header, &claims(), b), AT)
            .unwrap_err(),
        AccessError::Invalid
    );
    assert_eq!(calls.get(), 1);
}

#[test]
fn access_requires_the_expected_issuer_audience_person_and_time() {
    let a = &fixtures().0;
    let (mut access, _) = verifier(&configuration(), vec![Ok(a.jwks.clone())]);
    let changes = [
        ("iss", json!("https://other.cloudflareaccess.com")),
        ("aud", json!(["foreign-application"])),
        ("exp", json!(AT)),
        ("iat", json!(AT + 1)),
        ("nbf", json!(AT + 1)),
        ("nbf", json!(null)),
        ("sub", json!("")),
        ("email", json!("unregistered@example.invalid")),
        ("type", json!("service")),
    ];
    for (field, value) in changes {
        let mut body = claims();
        body[field] = value;
        assert_eq!(
            access.verify(&sign(&body, a), AT).unwrap_err(),
            AccessError::Invalid,
            "claim {field}"
        );
    }
    for field in ["iss", "aud", "exp", "iat", "email", "sub", "type"] {
        let mut body = claims();
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            access.verify(&sign(&body, a), AT).unwrap_err(),
            AccessError::Invalid,
            "missing {field}"
        );
    }
}

#[test]
fn access_refreshes_rotated_keys_with_a_bounded_request_rate() {
    let (a, b) = fixtures();
    let (mut access, calls) = verifier(
        &configuration(),
        vec![Ok(a.jwks.clone()), Ok(b.jwks.clone())],
    );
    assert!(access.verify(&sign(&claims(), a), AT).is_ok());
    for _ in 0..4 {
        assert_eq!(
            access.verify(&sign(&claims(), b), AT + 1).unwrap_err(),
            AccessError::Invalid
        );
    }
    assert_eq!(calls.get(), 1);
    assert!(access.verify(&sign(&claims(), b), AT + 31).is_ok());
    assert_eq!(calls.get(), 2);
}

#[test]
fn access_does_not_accept_expired_cached_keys_when_the_provider_is_unavailable() {
    let a = &fixtures().0;
    let (mut access, calls) = verifier(
        &configuration(),
        vec![Ok(a.jwks.clone()), Err(AccessError::Unavailable)],
    );
    let token = sign(&claims(), a);
    assert!(access.verify(&token, AT).is_ok());
    assert_eq!(
        access.verify(&token, AT + 300).unwrap_err(),
        AccessError::Unavailable
    );
    assert_eq!(
        access.verify(&token, AT + 301).unwrap_err(),
        AccessError::Unavailable
    );
    assert_eq!(calls.get(), 2);
}

#[test]
fn access_rejects_ambiguous_or_inappropriate_provider_keys() {
    let a = &fixtures().0;
    let mut duplicate = a.jwks.clone();
    duplicate["keys"]
        .as_array_mut()
        .unwrap()
        .push(a.jwks["keys"][0].clone());
    let mut private = a.jwks.clone();
    private["keys"][0]["d"] = "not-a-private-key".into();
    let mut weak = a.jwks.clone();
    weak["keys"][0]["n"] = URL_SAFE_NO_PAD.encode([1u8; 128]).into();
    let mut algorithm = a.jwks.clone();
    algorithm["keys"][0]["alg"] = "HS256".into();
    let excessive = json!({"keys":vec![a.jwks["keys"][0].clone();17]});
    for keys in [
        duplicate,
        private,
        weak,
        algorithm,
        excessive,
        json!({"keys":[]}),
    ] {
        let (mut access, _) = verifier(&configuration(), vec![Ok(keys)]);
        assert_eq!(
            access.verify(&sign(&claims(), a), AT).unwrap_err(),
            AccessError::Unavailable
        );
    }
}

#[test]
fn access_configuration_only_uses_the_fixed_cloudflare_team_issuer() {
    let config = Config::parse(&serde_json::to_vec(&configuration()).unwrap()).unwrap();
    assert_eq!(
        config.certs_url(),
        "https://synthetic.cloudflareaccess.com/cdn-cgi/access/certs"
    );
    let mut longest = configuration();
    longest["issuer"] = format!("https://{}.cloudflareaccess.com", "a".repeat(63)).into();
    assert!(Config::parse(&serde_json::to_vec(&longest).unwrap()).is_ok());
    for issuer in [
        "http://synthetic.cloudflareaccess.com",
        "https://synthetic.cloudflareaccess.com.attacker.invalid",
        "https://synthetic.cloudflareaccess.com@attacker.invalid",
        "https://synthetic.cloudflareaccess.com:444",
        "https://synthetic.cloudflareaccess.com/path",
        "https://synthetic.cloudflareaccess.com?key=value",
        "https://synthetic.cloudflareaccess.com#fragment",
        "https://127.0.0.1",
        "https://a.b.cloudflareaccess.com",
    ] {
        let mut value = configuration();
        value["issuer"] = issuer.into();
        assert!(Config::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

#[test]
fn access_serve_configuration_is_explicit_private_and_never_falls_back() {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&serde_json::to_vec(&configuration()).unwrap())
        .unwrap();
    let args = vec![
        "--origin".into(),
        "https://crm.example.test".into(),
        "--access-config".into(),
        file.path().to_str().unwrap().into(),
    ];
    assert!(super::cli::serve_config(&args).unwrap().access_mode());
    let mut duplicate = args.clone();
    duplicate.extend([
        "--access-config".into(),
        file.path().to_str().unwrap().into(),
    ]);
    assert!(super::cli::serve_config(&duplicate).is_err());
    std::fs::write(file.path(), b"{}").unwrap();
    assert!(super::cli::serve_config(&args).is_err());
}

#[test]
fn access_user_provisioning_needs_no_password_and_grants_no_membership() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("service");
    let store = super::Store::initialize(&root).unwrap();
    drop(store);
    super::cli::run(vec![
        root.clone().into_os_string(),
        "user-add".into(),
        "cf-invite".into(),
        "--cloudflare-access".into(),
    ])
    .unwrap();
    let store = super::Store::open(&root).unwrap();
    let users:i64=store.conn.query_row("SELECT count(*) FROM users WHERE username='cf-invite' AND password_hash LIKE '$argon2id$%'",[],|r|r.get(0)).unwrap();
    let memberships: i64 = store
        .conn
        .query_row("SELECT count(*) FROM memberships", [], |r| r.get(0))
        .unwrap();
    assert_eq!(users, 1);
    assert_eq!(memberships, 0);
}

#[test]
fn access_identity_bindings_are_explicit_and_one_to_one() {
    for users in [
        json!([]),
        json!([{"email":"alice@example.invalid","username":"admin"},{"email":"ALICE@example.invalid","username":"editor"}]),
        json!([{"email":"alice@example.invalid","username":"admin"},{"email":"bob@example.invalid","username":"ADMIN"}]),
        json!([{"email":"*","username":"admin"}]),
        json!([{"email":"alice@example.invalid","username":"../../admin"}]),
    ] {
        let mut value = configuration();
        value["users"] = users;
        assert!(Config::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    }
}

#[test]
fn access_configuration_rejects_unknown_fields_and_duplicate_top_level_keys() {
    let mut value = configuration();
    value["jwks_url"] = "http://127.0.0.1/private".into();
    assert!(Config::parse(&serde_json::to_vec(&value).unwrap()).is_err());
    let bytes = serde_json::to_string(&configuration())
        .unwrap()
        .replace("\"version\":1", "\"version\":1,\"version\":1");
    assert!(Config::parse(bytes.as_bytes()).is_err());
}
