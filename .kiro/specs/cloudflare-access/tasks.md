# Tasks: Cloudflareログイン

本人のCloudflareログイン指定を根拠に、要件→設計→以下の実装/検証へ自律進行する。新規有料契約・未決定の実行先変更は含めない。

- [x] 1. 固定したAccess設定とJWT検証を追加する
  - 設定の厳密検査、固定JWKS取得、cache/更新制限、実RS256検証と時刻/issuer/audience/利用者の照合を実装する。
  - 偽造・期限・重複・別宛先・不正公開鍵・取得失敗を合成署名で拒否し、秘密を出さない。
  - _Requirements:1.1,2.1,2.2,2.3,2.4,4.3,4.4_
- [x] 2. 既存sessionと画面を接続する
  - 明示したserve設定を追加し、Access方式ではpasswordなしで既存sessionを作る。
  - 各要求でAccess利用者とcookieの利用者を照合し、権限・所属・失効・CSRF/contextとgateを維持する。
  - login UI/AJAXを対応し、他のAccessアプリのsessionへ影響するlogoutを自動実行しない。
  - _Requirements:1.1,1.2,1.3,2.5,3.1,3.2,3.3,3.4_
- [x] 3. 配備設定・pilot・証拠を整える
  - 起動時secretと一時設定、説明、公開前検査を対応させ、Liracの既存Access設定から対象アプリだけを準備する。
  - 合成HTTP/ブラウザ、default/cloud/MSRV/依存/秘密検査を実行し、必要なGit操作を完了する。
  - host未配備や本人のSSO/MFAが必要な本番確認は未実施と記録する。
  - _Requirements:4.1,4.2,4.3,4.4_

実施結果は[evidence](evidence.md)。公開hostでの実SSO/MFAと実行先の配置は別の残件として扱う。
