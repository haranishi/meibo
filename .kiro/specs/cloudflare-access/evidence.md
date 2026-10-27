# Cloudflareログインの検証記録

2026-10-27。本人の指定により、任意のcloud構成へCloudflare Access認証を追加した。
新しい有料契約や、未決定の実行先への移行は行っていない。

## 実装と提供者設定

固定issuer/audience、実RS256署名、期限、利用者対応を検証して既存sessionへ接続した。
組織所属・3権限・CSRF/context・失効・永続gateを引き継ぎ、自動登録は行わない。
設定を明示しない既存構成では従来のpassword方式を維持する。
Cloudflare側は既存のIdPを使い、所有者だけを許可する対象アプリを作成した。
作成後のGETで、対象host、許可IdP、メール・account所属の条件、8時間のsessionを照合した。
既存の他アプリ、全体IdP、有料契約は変更していない。値はprivate設定だけに保存した。

## 実行した確認

| 確認 | 結果 |
| --- | --- |
| 既定構成 `cargo xtask check-all` | 成功。331件成功、失敗0、既存の手動試験3件は除外 |
| cloud構成 `cargo xtask check-cloud` | 成功。497件成功、失敗0、手動試験4件は除外 |
| 最終UI変更後の認証重点試験 | 18件成功、失敗0。合成ブラウザ用serverの手動fixtureは除外 |
| Worker `npm --prefix cloudflare run check` | 成功。178件成功、型・dry-run・shell検査も成功 |
| Rust 1.88 | 全workspace/全target、既定・cloud integration構成とも成功 |
| ブラウザ | Access/password各8項目、login幅390/768/1440、Access期限切れ後の画面復帰が成功 |
| 本番用Rustの公開鍵取得 | 実CloudflareのHTTPS公開鍵を使い、実kidを付けた合成偽署名を401で拒否 |
| 依存・ライセンス | cargo-denyとREUSEが成功。新たな例外は追加していない |
| 秘密・文書 | gitleaksと実設定値の混入検査、内部リンク・仕様追跡・JSON・日本語quick検査が成功 |

ブラウザは合成署名元を使うローカルHTTPS試験で、実際のCloudflare SSOを通していない。
追加したserver fixtureはcfg(test)だけに存在し、配布バイナリから指定できない。
実公開鍵の試験も一時ローカルDBを使い、既存のTurso DBは初期化・変更していない。
実装した親エージェントがセキュリティと要件の2観点で確認した。独立レビューではない。

## 公開前に残る確認

アプリ本体は未配備で、本人の実SSO/MFA、実hostの接続元制限と共有previewは未確認。
Cloudflare Accessの設定作成だけでは、アプリの実行先は確保されない。
[公開仕様](../cloudflare-hosting/tasks.md)の2.2・2.3は未完了のままにする。
以前のartifactは認証追加前のsourceに対応するため、今回の公開には再利用できない。
再配備時は新しいsourceのartifactと、別途保存したAccess設定を改めて照合する。
