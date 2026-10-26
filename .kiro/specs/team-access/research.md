# Team access research

確認日: 2026-10-26。以下は公開一次資料の確認とコード調査。実装前の判断を残す。

## 既存資産
- tiny_http 0.12.0、rusqlite 0.40.2（backup有効）、serde_json、fs4 0.13.1を利用中。
- getrandom 0.4.3はlock内に存在。直接依存として追加しOS乱数を再利用する。
- core Engineは1 DBを所有し、オンラインbackupと整合性検証を持つ。HTTPの業務処理を再利用できる。
- 認証/権限は存在しない。CRMはSynthetic固定のため、実DB対応は親工程で先に直す。

## Passwordとsession
- [Argon2 0.5.3公式API](https://docs.rs/argon2/0.5.3/argon2/): Rust 1.65以上、MIT OR Apache-2.0、Argon2id既定、PHC文字列のhash/verify。
- [Argon2 Params](https://docs.rs/argon2/0.5.3/argon2/struct.Params.html): 既定t=2/p=1。実装ではm=19456KiBも明示する。
- [OWASP Password Storage](https://cheatsheetseries.owasp.org/cheatsheets/Password_Storage_Cheat_Sheet.html): Argon2idを採用し、暗号化や高速な汎用hashでpasswordを代用しない。
- [getrandom公式API](https://docs.rs/getrandom/0.4.3/getrandom/fn.fill.html): OSの乱数を取得し、失敗時はエラーを返す。
- [SHA-2公式API](https://docs.rs/sha2/0.10.9/sha2/): sessionのlookup用digestにSha256を使う。passwordには使わない。
- [OWASP Session Management](https://cheatsheetseries.owasp.org/cheatsheets/Session_Management_Cheat_Sheet.html): 乱数ID、Secure/HttpOnly/SameSite cookie、サーバー側期限/失効を設計へ反映する。
- [OWASP CSRF Prevention](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html): SameSiteだけへ依存せず、既存の同一Originと専用headerも維持する。

## 公開運用
- [Caddy reverse proxy](https://caddyserver.com/docs/quick-starts/reverse-proxy): 既存HTTP backendの前段に置ける。
- [Caddy Automatic HTTPS](https://caddyserver.com/docs/automatic-https): public domainのDNS/80/443と、書込み可能で永続したcertificate保存が必要。
- [Docker Volumes](https://docs.docker.com/engine/storage/volumes/): コンテナを再作成しても保持するnamed Volumeを採用する。
- [fs4公式API](https://docs.rs/fs4/0.13.1/fs4/): 既存依存のfile lockを保守lockへ再利用する。
- [Rust公式image](https://hub.docker.com/_/rust)の1.99.1-bookwormと、[Caddy公式image](https://hub.docker.com/_/caddy)の2.11.7-alpineを確認して構成に指定した。
- [Caddy global options](https://caddyserver.com/docs/caddyfile/options)のbody/header timeoutと、[request_body](https://caddyserver.com/docs/caddyfile/directives/request_body)のmax_sizeを前段へ設定した。

## 判断
外部認証サービスは初回招待ベータに必要なcredential・送信・別契約を増やすため導入しない。
既存ライブラリへ小さなcontrol DBを追加し、組織DBを物理分離する。
この仕様だけで実運用の安全性や法令適合を保証したとは扱わず、実検証した範囲をtasksへ記録する。
