# Cloudflareアカウントでログインする

Cloudflare Accessで本人を確認し、meiboでも署名を検証する。
招待済みの利用者は組織IDだけを入力し、既存の権限でCRMへ進む。
cloud buildで明示的に有効にする。設定を省いた起動は従来のpassword方式になる。

## 入口とアプリを用意する

Accessは認証を担当する。Rustアプリの実行先と[Turso保存先](hosting-remote.md)は別に必要になる。
[Cloudflare構成](hosting-cloudflare.md)でも通常のDockerホストでも利用できる。
Workersの有料契約や実行先の変更は、この認証機能の導入に含めない。

Accessのself-hosted applicationを公開hostに設定し、Cloudflare IdPだけを許可する。
pilotのAllow policyはメール、Cloudflareでのログイン、対象accountの所属をすべて要求する。
API tokenだけを持つservice identityはmeiboの利用者として扱わない。
アプリへ直接入れる経路も制限し、入口でのpolicy変更・失効を迂回させない。
Accessを使うだけでRustの配置や接続元制限が完成するわけではない。

## 利用者と設定を対応させる

管理者が利用者と所属を先に登録する。ログイン時の自動登録・権限付与は行わない。
以下は新しい利用者を作る場合の例。既存利用者には再実行しない。

```sh
meibo hosted-remote /private/remote.json user-add operator --cloudflare-access
meibo hosted-remote /private/remote.json member-set operator alpha admin
```

このuser-addはランダムなpasswordのhashだけを保存し、passwordを表示しない。
所属はmember-setで別途指定する。ローカル保存のhostedでも同じ操作を使える。
Access設定を専用ファイルへ保存し、権限を0600にする。

```json
{"version":1,"issuer":"https://example-team.cloudflareaccess.com","audience":"APPLICATION_AUDIENCE","users":[{"email":"operator@example.invalid","username":"operator"}]}
```

issuerとaudienceは対象のAccessアプリから取得する。usersはメールと既存利用者名の一対一対応。
未知項目、重複、不正なteam URL、公開権限のある設定ファイルは起動時に拒否する。
値をログやGitへ置かず、配置を変えたときも同じ対象か照合する。

```sh
meibo hosted-remote /private/remote.json serve --origin https://crm.example.test --bind 0.0.0.0:4317 --access-config /private/access.json
```

CloudflareのentrypointではMEIBO_ACCESS_CONFIGをsecretとして明示する。
Worker bindingは5120bytesまでで、公開前検査でもこの上限を確認する。
一時設定は0600で作成する。空・不正な設定はpassword方式へ切り替えず起動を拒否する。
この設定はDBのbackupに含まれないため、復旧時に別途再設定する。
ビルドにはCMakeが必要で、公開鍵の取得先は固定のCloudflare HTTPS endpointだけになる。

## sessionと確認範囲

health以外は毎回Accessの署名・宛先・期限・利用者を検証する。
CRM操作には所属組織のsessionも必要で、別人のcookieは利用・延長できない。
所属変更・利用停止・revokeと既存のCSRF/context検査を維持する。
CRMのログアウトはCRM sessionを失効する。CloudflareのSSO sessionは継続し得る。
他のAccessアプリも失効するCloudflare全体のlogoutへは自動で遷移しない。

合成RSA署名のHTTP試験と、同じCRMを操作するブラウザ試験を用意している。
ブラウザ試験はcrates/app/tests/cloudflare-access-browser.mjsで、cloud test executableとPlaywrightを指定する。
試験用の署名元はcfg(test)に限定し、配布するバイナリでは指定できない。
この試験は本物のCloudflare SSO/MFAの確認を代替しない。
本番hostを配備してから本人のログイン・期限切れ・許可対象を確認する。
host全体をAccessで保護すると、未認証の共有crawlerも通れない。
canonical/OGPの生成は維持するが、公開リンクのpreviewは実hostで別途確認する。

Cloudflare公式: [Cloudflare IdP](https://developers.cloudflare.com/cloudflare-one/integrations/identity-providers/cloudflare/)、
[JWT検証](https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/)、
[session管理](https://developers.cloudflare.com/cloudflare-one/access-controls/access-settings/session-management/)。
