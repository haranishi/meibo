# Design: Cloudflare Accessを既存sessionへ接続する

## 境界と採用理由
Cloudflare AccessのCloudflare IdPで本人確認し、Rustアプリでも署名を検証する。DBと画面の業務処理を再実装しない。既存の自己ホストpassword方式は既定のまま、公開運用でAccess設定を明示する。
所有範囲はAccess設定・公開鍵取得/検証・検証済み利用者によるsession発行・login画面・pilot設定。control schema、組織権限、永続gate、公開サーバーの選定は既存仕様を引き継ぐ。

## UX
利用者はCloudflareでログインし、招待された組織IDを入力してCRMへ進む。独自passwordの入力・管理を求めない。
戦略は招待制の本人確認、要件は既存組織/権限、構造はAccess→組織指定→CRM、骨格は既存login formの再利用、表層は既存の見た目と共有/OGP生成を維持する。Access保護後のcrawlerによるpreviewは実hostで確認する。

## 1. 設定と署名検証
cloud buildで`serve --access-config PRIVATE_JSON`を追加する。設定はversion、issuer、audience、usersの配列（email/username）で、未知field・重複対応・不正URLを拒否する。メールも利用者名も一対一とし、勝手な所属付与をしない。
issuerはHTTPSの単一team label付きcloudflareaccess.comだけ。JWKSはそこから固定の`/cdn-cgi/access/certs`へ取得する。JWTのjku等へ接続しない。
既存reqwest/native TLSとTokioを使い、redirect/proxy/retryなし、期限10秒、本文64KiB、RSA公開鍵16個まで。鍵は最大5分cacheし、更新要求の最短間隔を30秒にする。期限切れ鍵で取得失敗を隠さない。
jsonwebtoken11.1.0（MSRV1.88）のaws_lc_rsを固定してRS256署名・issuer/audienceを検証する。時刻は既存HTTP clockに合わせてexp/iat/nbfを確認する。非対称署名を自作しない。
初期候補rust_cryptoの依存検査でRUSTSEC-2026-0387とRUSTSEC-2023-0071を検出したため採用せず、公式対応のaws_lc_rsへ変更する。build段だけにCMakeを追加する。
tokenは16KiBまで。subject/emailとtype=appを要求し、検証後だけ許可した利用者名を取り出す。返すエラーは固定文言と分類だけ。

## 2. HTTPとsession
HostedConfigに任意のAccess verifierを持つ。Host/Origin等の検査後、healthを除く全要求でJWTを確認してからgate/DBへ進む。
POST/auth/loginはAccess方式ならtenantだけ受け、検証済み利用者からsessionを発行する。共通のsession発行部品をpassword方式と共有し、保存形式と失効条件を維持する。
認証済みAPIではsessionの利用者名とAccessの利用者名をDB更新前に照合する。別利用者のcookieを使ってもsessionを延長せず、CRMデータを返さない。
Access設定や対応表を変える場合は旧処理を終了して再起動する。設定欠落で別の認証方式を選ばない。

## 3. UIとlogout
既存login HTMLのpassword用fieldだけをAccess方式で除き、組織IDとCloudflareログインの案内を表示する。同じlogin.jsを使い、password無しの本文を作る。認証前の業務JS、CSP、canonical/OGPと共有は維持する。
AJAX要求へX-Requested-Withを付け、Accessの期限切れを401として扱えるようにする。login中の401では画面を再読込してAccessへ戻る。既存の旧tab/context保護を再利用する。
CRMのlogoutはapp sessionを失効する。Accessのlogout URLは他のAccessアプリも失効させるため自動遷移しない。再ログイン時にCloudflareのSSO sessionが継続していることは許容する。

## 4. 運用と検証
Cloudflare入口を使う場合、MEIBO_ACCESS_CONFIGを明示したsecretとしてcontainerへ渡し、一時設定をumask077で作る。未設定時の既存挙動を保つ。
既存LiracのAccess組織・Cloudflare IdPを照合済み。新規アプリだけを指定公開host・pilotメール・Cloudflare IdPへ制限し、他のアプリやIdPは更新しない。
公開hostは既存の公開計画から読み、値・account ID・emailをGitへ置かない。サーバー未配備なら設定準備と実ログイン確認を分けて報告する。
合成鍵は試験時に生成し、秘密鍵をcommitしない。実RSA署名・鍵更新/障害・偽造・別利用者cookieを単体/HTTPで検証し、Cloudflare方式の画面をブラウザで操作する。default/cloud/MSRV/denyと文書/秘密検査を通す。

## 公式根拠
- [CloudflareをIdPにする](https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/cloudflare/)
- [Access JWTの検証](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/)
- [logoutとAJAX](https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/)
- [jsonwebtoken11.1.0](https://docs.rs/jsonwebtoken/11.1.0/jsonwebtoken/)
