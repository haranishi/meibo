# Team access tasks

承認根拠: 2026-10-26 本人の未完了機能実行・公開依頼。要件/設計/タスクは独立再審PASS後に確定した。
各チェックは実行証拠のある範囲だけ完了とする。秘密・DB・生ログはGitへ入れない。

- [x] 1.1 親integration: server公開口、hosted mod/CLI委譲、依存、UI storageKey hookを接続する。
  _Requirements: 1.1, 1.2, 3.1, 3.4, 4.1, 4.6, 4.7_ _Depends: none_ _Boundary: parent-owned server/main/args/commands/manifests/app.js/list-tools.js_
- [x] 2.1 control DB、ROOT保全、共通保守lock、組織・利用者・所属のCLIを追加する。
  _Requirements: 1.1, 1.2, 1.3, 1.4, 3.1, 3.3, 5.1, 5.2_ _Depends: 1.1_ _Boundary: hosted/store.rs, hosted/cli.rs_
- [x] 2.2 Argon2id、session/hash、roleの再検査、失効/期限/失敗制限を追加する。
  _Requirements: 1.2, 1.4, 2.1, 2.2, 2.3, 2.4, 2.5, 2.6_ _Depends: 2.1_ _Boundary: hosted/auth.rs_
- [x] 3.1 公開HTTP境界、login/context gate、role/組織dispatch、接続上限を追加する。
  _Requirements: 3.1, 3.2, 3.3, 3.4, 3.5, 3.6, 4.1, 4.2, 4.3, 4.4, 4.5, 4.6, 4.7, 5.1_ _Depends: 1.1, 2.2; parent real-origin CRM support_ _Boundary: hosted/http.rs, ui/login.*_
- [x] 4.1 共通lock下のonline backup、manifest、新ROOT restore、2組織の復旧実証を追加する。
  _Requirements: 5.2, 5.3, 5.4, 5.5_ _Depends: 3.1_ _Boundary: hosted/backup.rs_
- [x] 4.2 non-root Docker、HTTPS前段、永続Volume、運用手順を追加する。
  _Requirements: 6.1, 6.2_ _Depends: 3.1, 4.1_ _Boundary: Dockerfile, compose.yaml, deploy/, docs/hosting.md_
- [x] 5.1 2組織3role、CSRF/失効、同一browser/旧tab、復旧/再起動、3幅を検証し記録する。
  _Requirements: 6.3, 6.4, 6.5_ _Depends: 1.1, 2.1, 2.2, 3.1, 4.1, 4.2_ _Boundary: hosted tests; parent integration/browser/independent review_

## Implementation notes
認証コンテキストが確定するまで業務scriptを開始しない（4.6）。storage hookとX-Meibo-Contextの両方で、別組織の保存内容と旧tabの要求を分離する（4.7）。

## 実装と機械検証（2026-10-26）
2.1/2.2/3.1/4.1/4.2のコードを追加した。チェックは独立した実装レビュー後に親工程が確定する。
未実装stubに対する仕様試験のコンパイル失敗を確認後、Store/認証/HTTP/復旧を実装した。
`cargo test --offline --locked -p meibo-app hosted`: 単体6件とCLI/実HTTP/復旧の子プロセス1件が成功。
Argon2のsalt、session digest/期限/失効、所属/password/停止、失敗制限、2組織/3role、CSRF/context、backup/改竄を確認した。
viewerのPOSTは独自項目の正整数ID/searchだけ参照として許可し、定義/値変更・delete・近似pathは拒否する。
`cargo clippy --offline --locked -p meibo-app --all-targets -- -D warnings`成功、login.jsのNode構文検査成功。
Compose YAMLとアプリport非公開/Volumeの宣言は静的確認済み。Docker daemon/Compose/Caddyが利用できず、image build/実TLS/実Volumeは未検証。
親のMeiboAppStart/storage hookを利用する。認証後のscript順はHTML順に従い、全scriptの定義後に初期化する。
実ブラウザの同一browser/旧tab/3幅と独立実装レビューは親工程で確認する。実配備を完了したとは扱わない。
独立security reviewでmain外のheader/title/dialog/通知が復帰時に残る問題を再現したため、body全体のhidden/inert・CSS強制非表示・title中立化へ修正した。同一contextの確認成功後だけ復帰させる。遅延auth再現は親が再審する。

## Docker追加実証（2026-10-26）
SSDの識別子と空きを確認し、既存VMを変更せず専用Colima VM（4CPU/4GiB、data20/root10GiB）を作成した。
Compose5.6.1/Buildx0.38.1は公式asset SHA-256照合後、専用SSD領域へ導入。本番compose configと公式Caddy2.11.7によるCaddyfile validateが成功した。
公開用DockerfileをRust1.99.1/Linux arm64でlocked release buildし、non-root/readonly起動とapp port非公開を確認した。
合成2組織/3roleで、個別にCAを信頼したlocalhost HTTPS・cookie/context/logout・企業/活動の再起動保持を実証した。
永続Volume内のonline backup→新ROOT restore、container再作成後の再login/組織分離/企業・活動・証明書Volume保持が成功した。
証拠は `.agent-harness/runs/2026-10-26-hosted-container/result.json`。外部domain/クラウド配備と公開CA発行は未実施。
専用containerとVMは正常停止し、合成DB/Volume/imageを保持した。既存profile/SSH設定/active Docker contextとglobal CA信頼ストアは変更していない。

## 2026-10-26 最終統合の記録

2026-10-26: hosted6単体/CLI実HTTP復旧/新tenant公的取込、独立ブラウザ16項目、UI18/20・体験18/20が成功。認証再検証中のbody/header/title/dialog非表示、旧tab/context・3権限・組織storage分離・失効を再確認。Docker Linux arm64 image、HTTPS/2組織/3権限/再起動/Volume/backup/restoreを実証。外部クラウド配備・公開domain/CAは未実施。

全体289試験成功・大型測定3ignored、check-all/fmt/strict clippy/依存方針/オンラインadvisories/MSRV1.88成功。REUSE3.3準拠、秘密検査0。独立境界レビューAPPROVED。
