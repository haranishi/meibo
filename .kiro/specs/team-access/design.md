# Team access design

## 1. 境界と保存
`hosted ROOT COMMAND`を独立入口とし、既存ローカルserveを変更しない。
ROOT/control.sqlite3: meta、users、tenants、memberships、sessions、login_attempts、audit。
ROOT/tenants/{slug}.sqlite3: 組織別の既存Engine。slugは`[a-z0-9-]`の1〜48文字、DBパスはslugからだけ作る。
root/子DBのsymlinkを拒否し、専用ディレクトリとファイルのUnix権限を制限する。
controlのapplication_id/版を確認し、不明なDBは開かない。SQLiteはWAL/FULL/foreign_keys/busy_timeoutを設定する。
serverと管理CLIはROOTの同じ保守lockを使い、Web要求・管理変更・backupを直列化する。

## 2. 認証
passwordは12〜1024bytes。Argon2 0.5.3のArgon2id v19、m=19456KiB/t=2/p=1、OS乱数salt16bytesを使う。
username不在時もdummy hash検証を行う。失敗応答は利用者/組織/停止/所属の有無を区別しない。
sessionはgetrandomで32bytes生成しhex化、controlにはSHA-256 digestとuser/tenant/created/last_seen/expiresを保存する。
認証の都度usersとmembershipsをJOINして再確認する。sessionが指すroleを永続化して権限変更を取り逃さない。
失敗制限はaccount枠5回/15分と全体枠100回/15分。期限切れを削除し、追跡枠は4096件まで。
cookieは`__Host-meibo-session`、HttpOnly/Secure/SameSite=Strict/Path=/、絶対寿命8h、無操作30m。
開発のHTTPはloopback bind/originを同時に要求し、`meibo-dev-session`へ分離する。

## 3. HTTP adapter
外部のHost、Origin、専用header、Content-Type、method/path、bodyサイズを認証前に検査する。
GET /health、/login、/login.css、/login.jsとPOST /auth/loginだけは未認証でも使える。
GET /auth/session、POST /auth/logoutはsession操作。GET /auth/auditはadminの所属組織に限定する。
他のpathはsessionとroleを検証してから対応するEngineを開く。viewerの更新POSTと非adminの`/erase`は403。
viewerにも読み取り専用のPOST `/api/custom-fields/{正整数ID}/search`だけを許す。CSRF/context検査は維持し、近似path・定義/値の変更・deleteは許さない。
内部委譲は信頼済みInputへHost/Origin/X-Meibo-Localを正規化して既存server::dispatchを呼ぶ。
この正規化は公開境界検査後だけ実行し、認証を迂回する外部入口を作らない。
同時Engineは最大4、cacheは各32MiB、Full durability。古い接続を閉じて上限を守る。
hosted HTMLは既存の業務scriptを外しlogin.jsだけを読み込む。同scriptが/auth/sessionを確認してからapp.js→crm.js→list-tools.jsを順に読み込む。
contextが確定するまでは業務領域を隠し、失敗時はloginへ戻す。passwordやsessionをWeb Storageへ保存しない。
`/auth/session`はinstance/user/tenantから導いた非認証用storage_scopeを返し、`window.MeiboContext.storageKey(base)`で利用者・組織別keyを提供する。
親のUI統合でapp.jsのrecent keyとlist-tools.jsのview keyをこの関数へ接続する。localモードは従来keyを維持する。
hosted開始時に旧`meibo-recent-companies`/`meibo-list-views-v1`を破棄し、logout/切替時にhostedの保存内容と表示中の下書きを破棄する。
login.jsのsame-origin fetch adapterは業務APIへ`X-Meibo-Context: storage_scope`を付け、serverは現sessionのscopeと不一致ならCRMを開く前に409/context_changedを返す。
初回確認中・pagehide・persisted pageshowではbody全体をhidden/inertとし、CSSでもbody/全子孫を非表示にしてheader/nav/dialog/通知を含める。titleはmeiboへ中立化し、同じcontextの確認成功後だけ画面/titleを戻す。
401/context_changedとpageshowでのcontext変更は非表示を維持し再loginへ戻す。旧tabやbfcacheで別組織の同じIDを操作しない。所属/role/logoutはlogin.jsが表示する。

## 4. CLIと復旧
init、tenant-add、user-add、password-set、user-disable、member-set、revoke、audit、backup、restore、serve。
password操作は`--password-stdin`を必須にする。公開signup・email送信・課金は作らない。
backup先は新規directoryのみ。共通lock中にcontrol/各tenantをSQLite online backupし、SHA-256 manifestと整合性を確認する。
backup側のsessionを削除する。restoreも新規ROOTのみ、manifestは64KiB/tenant1000以内、導出したDB名だけ許す。
全fileのsymlink/sha256/SQLite integrityとEngine整合性を確かめ、コピー後のcontrol sessionも空にする。
backup失敗で元ROOTを変更/削除せず、不完全な保存先は成功と扱わない。

## 5. 配備とUX
Dockerはbuilderでlocked release build、runtimeはnon-root。アプリの公開portは持たずCaddyだけ80/443を公開する。
CRM/control、Caddy証明書と設定に別のnamed Volumeを用意する。Caddyにrequest body/headerのtimeoutを設定する。
ユーザーストーリー: 招待された担当者が所属組織へloginし、その業務だけを処理してlogoutする。
UX: 戦略=組織の分離、要件=login/失敗復帰/所属表示、構造=login→既存CRM、骨格=label付き1column form、表層=既存配色。
料金の選択や技術構成を利用者画面へ表示しない。操作権限と組織名だけを示す。

## 6. 検証と非目標
単体: hashと認証、期限と失効、header/CSRF、roles、2tenant isolation、symlink/path、backup/restoreと改竄。
統合: CLI標準入力、HTTP cookie、再起動後の保持、未認証static/API、既存回帰、実ブラウザ3幅。
同一browserのA→logout→B、別利用者、旧tab/bfcacheでrecent企業・保存view・下書き・APIが混ざらないことも確認する。復帰時のauth応答を止め、header/検索語/レコード名/title/dialog/通知が隠れたままであることを確認する。
hostedのhigh availability・分散構成・MFA/SSO・課金・公開signup・API tokenはこの仕様へ含めない。
TLS/DNS/Volumeの実配備は対象環境が確定してから同じ構成で検証し、未実施と区別する。
