# Requirements: CloudflareアカウントでCRMへログインする

本人の2026-10-27の指定を引き継ぐ。Cloudflare Accessで本人確認し、Turso等の既存control DBにある利用者・組織・権限でCRMを利用する。アプリ実行先の変更や新しい有料契約は含めない。

## 1. 認証方式を明示して選ぶ
1.1 公開運用者がAccess設定を指定した場合、Cloudflareログイン専用で起動する。不正設定や未対応buildでは起動を拒否し、password認証へfallbackしない。
1.2 Access設定を指定しない手元版・既存自己ホスト版のpassword認証を維持する。
1.3 Cloudflare方式の画面は組織IDだけを受け取り、アプリ独自のpassword・利用者名入力を求めない。

## 2. Cloudflareの証明を検証する
2.1 各保護要求の単一Cf-Access-Jwt-Assertionを検証する。署名、RS256、固定issuer/audience、exp/iat/任意nbf、非空subject/email、人のapp tokenを確認する。
2.2 emailは署名検証後だけ参照し、明示した一対一のemail→既存利用者名の対応で解決する。未登録・停止済み・所属なしを拒否し、利用者やadmin権限を自動作成しない。
2.3 JWT内のURLを接続先へ使わない。固定team domainの公開鍵だけをHTTPSで取得し、期限・容量・鍵数・更新頻度を制限する。検証不能時は拒否する。
2.4 重複header、偽造、別issuer/audience、期限切れ、未来の有効開始、service token、別利用者のapp cookieを拒否する。
2.5 health以外の入口でAccess検証を業務DBの処理前に行う。Host/Origin/method/bodyの既存境界も維持する。

## 3. 既存の組織・権限と失効を保つ
3.1 検証した利用者の所属をcontrol DBで確認して既存形式のsessionを発行する。viewer/editor/admin、期限、停止・権限変更・revoke、CSRF/contextを維持する。
3.2 各session要求でCloudflareの利用者とapp sessionの利用者を照合し、別の利用者のcookieで旧組織を参照しない。
3.3 logoutはCRM sessionを失効し、ブラウザの旧画面・保存内容を既存規則で破棄する。他のAccessアプリのsessionを自動で失効させない。
3.4 SQLの結果不明・永続gate・backup/restoreを変更せず、Access用の運用設定はDBの保存物へ含めない。

## 4. 提供者設定と検証を分けて示す
4.1 Liracの既存Cloudflare IdPを使い、新しいアプリだけを招待したpilotへ制限する。既存アプリ・全体IdP・契約は変更しない。
4.2 公開host・利用者対応・issuer/audienceをprivate設定で照合する。未配備のhostや未実施の実ログインを成功扱いしない。
4.3 合成RSA署名を使って拒否条件、2組織3権限、旧cookie、logout/revoke、password版の回帰とMSRVを検証する。
4.4 token・秘密・認証header・SQLをログやGitへ含めない。本人のSSO/MFA操作が必要な本番確認は残件として明示する。
