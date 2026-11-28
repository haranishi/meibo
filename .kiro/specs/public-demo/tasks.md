# Tasks: 誰でも触れる公開デモ

1から5は、手元での実装と検証で、費用も公開も伴わない。6は実行先への配備で、運用者の明示の承認（実行先・月額上限・ドメイン・公開）を得てから行う。承認前は、準備済みの設定や試験の合格を「公開済み」と記録しない。

- [x] 1. 起動と入室の土台を作る
- [x] 1.1 起動指定と保存先の検査
  - `meibo hosted ROOT serve --public-demo [--demo-contact URL]` を受け付ける。ROOTが存在しないか空のときだけ起動し、中身がある・遠隔保存・Access設定がある場合は、理由つきで拒否する。
  - 指定しない起動は、現在の動作・引数・出力を変えない。既存のhostedの試験をすべて通す。
  - _Boundary: hosted/cli.rs, hosted/http.rs の設定_
  - _Requirements: 3.4, 7.1, 7.2, 7.3_
  - 検証（2026-11-24）: `cargo test -p meibo-app --locked hosted` 19件・`--test hosted` 4件・`--test public_entry` 3件が合格。`cargo fmt --check`・`clippy -D warnings`（通常・cloud）は警告0。併用の拒否は、通常・`hosted-remote` の両方で試験した。

- [x] 1.2 営業デモの初期データ
  - `hosted/demo.rs` を新設し、組織 `demo`、利用者 `demo`・`demo-b`（乱数のパスワードを保存せず破棄）、合成の取引先150社（すべて先頭に「合成・」）、商談9件、今月の目標、期限が過ぎた再架電1件を、起動時刻（日本時間）を基準に作る。
  - 初期状態の月次集計が、受注額32万円・受注率50%・確度加重見込み65万円・着地予測97万円になることを、月初・月末・月途中の時刻で試験する。実在しうる名前・電話番号・メール・URLを含まないことを検査する。
  - _Boundary: hosted/demo.rs, 既存のEngine APIの呼び出しだけ（core の変更なし）_
  - _Depends: 1.1_
  - _Requirements: 2.1, 2.2, 2.3, 2.4, 4.1_
  - 検証（2026-11-24）: 月次集計の4つの値（受注32万円・受注率50%・確度加重見込み65万円・着地予測97万円）と、担当別の値、来月の未決着3件を、月初・月途中・月末・12月末の4つの時刻で試験して合格。社名の接頭辞・電話・URL・再架電の期限・パスワードの非保存も試験済み。

- [x] 1.3 入室
  - 未認証の `GET /`・`/login` にデモ入口、`POST /demo/enter` に共有sessionの受け渡し（組織 `demo` の編集者・起動ごとに1つ。4.4で、訪問者ごとのsession発行から変更）、`POST /auth/login` に404を返す。入室の全体の上限（300回/分。初版は30回/分）を超えたら429。既存のHost・Origin・CSRFの検査を維持する。
  - 初期化の後の旧sessionは `/login?reset=1` へ戻す。
  - _Boundary: hosted/http.rs, hosted/auth.rs（デモ時の分岐だけ）, hosted/public_entry.rs_
  - _Depends: 1.1, 1.2_
  - _Requirements: 1.1, 1.2, 3.1, 3.3, 4.4_
  - 検証（2026-11-24。初版の記録で、sessionは4.4で共有に変えた）: `POST /demo/enter`（256バイト・`{}` だけ・30回/分・session300件・通常ログインは20件のまま）、`POST /auth/login` の404、初期化後の旧sessionの `/login?reset=1` への戻りを、dispatchレベルの試験（hosted 27件・追加8件）と、実ブラウザ（実バイナリ＋自己署名HTTPS前段）の試験で確認。

- [x] 2. 止める操作と、自己終了を作る
- [x] 2.1 (P) 止める操作と上限
  - `/api/import`・`/api/leads*`・`/api/lead-form`・`/api/activity-sync*`・`/forms/*`・利用者と所属の変更を、理由つきで拒否する。取引先・活動・商談・独自項目の件数と、保存先32MiBの上限を置き、超えた書き込みを拒否する。
  - POSTは、Content-Lengthが明示された64KiB以下だけ本文を読む。分割転送・`Expect`・上限超過は、業務処理をせずに拒否する（本文の読み捨ては tiny_http に任される。4.4で訂正）。
  - _Boundary: hosted/http.rs の要求の境界, 保存先の設定_
  - _Depends: 1.1_
  - _Requirements: 3.1, 3.2, 3.5_
  - 検証（2026-11-24）: 公開禁止経路（`/forms/*`・`/oauth/activity-return*` の404）、認証後禁止経路（`/api/import`・`/api/leads*`・`/api/lead-form`・`/api/activity-sync*` の403 `demo_disabled`・未認証401）、64KiB上限と事前検証（411/417/413）、作成件数上限（取引先・活動/v1/再架電・商談・独自項目・担当者・仕事・共有リストの403 `demo_limit`、更新・取消・完了は通過）、保存先32MiB上限（書き込みPOSTの507 `demo_storage_full`、読み取り・検索POST・`/auth/logout`・`/demo/enter` は通過）を dispatch 試験（hosted 39件）と実HTTP試験（`--test hosted` 7件）で確認。`cargo fmt --check`・`clippy -D warnings`（通常・cloud・core）は警告0。
  - 再検証（2026-11-24・別の確認）: `cargo test -p meibo-app --locked hosted` 39件・`--test hosted` 7件・`--features cloud` の `access_login` 3件と `remote_serve_rejects_public_demo` が合格。`fmt`・`clippy -D warnings`（通常・cloud・core）は警告0。`crates/core` の追加は読み取り専用の件数取得2つ（`activity_count`・`contact_count`）だけで、coreの試験130件が合格。実ブラウザからも、入室後の `/api/import`・`/api/leads`・`/api/lead-form`・`/api/activity-sync*` が403（`demo_disabled`）、`/forms/*`・`/oauth/activity-return` が404であることを確認（幅390・768・1440px）。作成件数の上限は、取引先・活動・商談・独自項目に加え、担当者（2,000）・仕事（5,000）・共有架電リスト（50）まで広げた。
- [x] 2.2 (P) 自己終了と見張り
  - 待ち受けを `recv_timeout(500ms)` の周回にし、アイドル（既定900秒・`/health` を除く）、上限時間（既定21600秒）、見張り（最後に動いた時刻から10秒）で、理由の1行を出して終了する。終了コードは、アイドルと上限が0、見張りが70。
  - 時刻の判定と終了処理は、差し込める部品にして、実際に終了せずに試験する。本文を1バイトずつ送る接続で、10秒以内に終了することを、実プロセスの試験で示す。
  - _Boundary: hosted/http.rs の serve ループ, 新しい小さな部品_
  - _Depends: 1.1_
  - _Requirements: 3.6, 4.2, 4.3_
  - 検証（2026-11-24）: `--demo-idle-seconds`・`--demo-max-uptime-seconds`・`--demo-stall-seconds` の検証、`DemoLifecycle`・`WatchdogState`・`HealthProbeTracker`・`probe_health` の単体試験6件、実プロセス試験（アイドル終了・`/health` 継続でのアイドル終了・`/login` 継続後のアイドル終了・上限時間終了・未送信本文による停滞終了コード70と `demo-reset reason=stalled`）を確認。`ui/login.js` の1.5秒遅延表示・503自動再試行は `node --check` 通過（実ブラウザ確認は未実施）。
  - 再検証（2026-11-24・別の確認）: 実プロセスの試験で、アイドル終了（`/health` だけでは延びない・普通の要求を続けている間は終了しない）、上限時間での終了、分割転送・`Expect`・過大な本文の拒否、本文を送らない接続での自己終了（コード70・`demo-reset reason=stalled`）を確認。**tiny_http 0.12.0 は、`Content-Length` が1,024バイト以下の本文を接続スレッドが先に読み切る**（crateのソースで確認）。そのため処理のループが止まるのは1,024バイトを超える本文（上限64KiBまで）で、停止の試験は `POST /api/companies`（`Content-Length: 2048`）で行う。公開ゲートの遅い本文の検査も、同じ理由で1KiB超にした。
- [x] 3. 画面を整える
- [x] 3.1 デモ入口と常時の注意書き
  - `login.html` のデモ用の変種（「デモを開く」・説明・連絡先・共有欄）と、認証後の画面上部の注意書き（合成データ・初期化・個人情報を入力しない）を作る。デモで使えない操作の導線は隠す。幅390・768・1440pxで、キーボード操作を含めて確かめる。
  - titleとdescriptionはデモ用にし、canonical・OGP・Twitter cardを維持する。画像と共有の文面に、競合の名前や、実際にはない機能を書かない。
  - _Boundary: ui/login.html, ui/login.js, ui/login.css, ui/index.html, ui/app.js（デモ時の表示だけ）, hosted/public_entry.rs_
  - _Depends: 1.3_
  - _Requirements: 1.1, 1.3, 1.4, 8.1, 9.1_
  - 検証（2026-11-24）: 実ブラウザ試験 `deploy/demo/browser/`（13件×幅390・768・1440px）が合格。入口の説明・OGP・canonical・X/LINE/コピー・キーボードだけの入室とフォーカス・注意書き帯の固定表示・使えない導線の非表示・Cookie（`__Host-` 付き・Secure・HttpOnly・SameSite=Strict）・横あふれ0を確認。通常モードとAccessモードのログイン画面は、既存の `public_entry` の試験で不変。
  - 追加（2026-11-24）: 実行先の起動待ち（503と `Retry-After`）に備え、入口のJSが、1.5秒で「起動しています。数秒かかることがあります…」を出し、最大4回まで自動で再試行する。**メンテナンス中の503（`code: maintenance`）は待たずにすぐ案内し、待ち時間は最大10秒に抑える**（運用者が停止の切替を使ったとき、訪問者が5分待たされないため）。実ブラウザ試験で、再試行して入室できる・メンテナンスはすぐ案内・503が続けば4回の再試行のあとに諦める、を確認。
- [ ] 4. 実行物とCIを作る
- [x] 4.1 イメージ
  - `Dockerfile` に `demo` ターゲットを足す。`MEIBO_DOMAIN`（必須）と `PORT`（既定4317）を展開し、起動のたびに新しい作業用ディレクトリを指定して起動する。root以外のユーザーで動くこと。
  - 起動後のメモリを実測して記録する（コンテナの大きさを決めるため）。
  - _Boundary: Dockerfile（既存のターゲットを変えない）_
  - 検証（2026-11-24）: 直下の `Dockerfile` の `demo` ステージ（最終の `default` は不変）と、`deploy/demo/cloudflare/Dockerfile`（BUILDPLATFORMのままコンパイルし、配備先がamd64でビルド側がarm64のときだけクロスコンパイル）で、arm64とlinux/amd64のイメージを作れた（amd64は31MB・`readelf` で機種確認）。どちらも非root（uid 10001）で起動し、実ブラウザ試験13件が合格（amd64はエミュレーション上）。起動（初期データ込み）から `/health` 200まで約0.15秒。**メモリはピーク約45MB**（`VmHWM`）で、Cloudflareの `lite`（256MiB）で足りる。`sh` と `dash` で起動コマンドの展開（連絡先の空白・`&`、`MEIBO_DOMAIN` 欠落時のエラー）を確認済み。注意: wranglerのビルド（`docker build --load --platform linux/amd64`）には手元のDockerクライアントに `buildx` が要る（この環境では未導入。VM内のDockerには入っている）。
  - _Depends: 1.1_
  - _Requirements: 5.2, 6.1_

- [x] 4.2 (P) デモ用のCloudflare Workerとコンテナ設定
  - `deploy/demo/cloudflare/` に、小さな逆プロキシのWorkerとwranglerの設定を作る。公開側のHostをそのままコンテナへ渡す。静的な応答の保持、POSTの接続元ごとの頻度の制限、設定値1つの停止の切替を持つ。コンテナは1つ（`max_instances=1`）、無通信15分で休止する。
  - 既存の `cloudflare/` のWorkerは使わない。Workerの単体試験と、手元の `wrangler dev`（Docker）での起動確認を通す。実環境の契約・配備は行わない。
  - _Boundary: deploy/demo/cloudflare/（新規）_
  - 検証（2026-11-24・途中。A4後は `node --test` 32件）: `npm run check`（`wrangler types` の一致・`tsc --noEmit`・`node --test` 25件→32件）が合格。本文を先に読み切る・Hostを公開ドメインで渡す・保持の対象と対象外・頻度の制限・停止の切替・単一飛行の起動・準備待ちの超過・終了後の再起動・アイドル停止を、差し込み式の実行環境で試験済み。A4で、アラームの書き込みの間引き（2分以上先のときだけ）・IPv6の/64の接続元キー・連絡先URLの規則（アプリと共有の事例ファイル）を足した。`lite`（256MiB）相当のコンテナ（`--memory 256m`）で3周×3幅のブラウザ試験が通り、メモリ不足の停止は無い（起動後は約45MB）。
  - 検証（2026-11-24・手元の `wrangler dev` の通し）: Docker（amd64のイメージ・`--local-protocol https`）で、Worker → Durable Object → コンテナを通した。最初の要求でコンテナが起動して `/health` が200、`/login` にWorkerの保持ヘッダー（5分）とCSPが付き、Host違いは403。遅い本文の接続（長さ2,048バイトと宣言して1バイトも送らない）は、他の訪問者の要求を止めず、放置もされない。ブラウザ試験（公開URL向けの形）が幅390・768・1440pxで各24件合格。接続元ごとの上限は、1台のPCから全試験を流しても当たらず、同じ接続元の複数人（教室・会場）も締め出さないよう、入室20回/分・GET 600回/分に直した（POSTは60回/分）。**実Cloudflareでの確認は公開前の検査（5.2）で行う**: `Host` の受け渡し、`caches.default` の保持（カスタムドメイン上だけで働く）、Rate Limitingの数え方。
  - _Depends: 4.1_
  - _Requirements: 3.3, 5.2, 5.3, 5.4_

- [ ] 4.3 イメージの検査（CI）
  - `.github/workflows/demo-image.yml`（手動実行と `demo-v*` タグ）で、linux/amd64のイメージを作り、公開と同じ起動引数で起動して、5.1の試験を通す。検査のためのもので、レジストリへは置かない。
  - _Boundary: .github/workflows/_
  - _Depends: 4.1, 5.1_
  - _Requirements: 6.1, 6.3_
  - 検証（2026-11-24・手元で同じ手順を再現。GitHub Actions上の実行は未了）: ワークフローと同じく、直下の `Dockerfile` の `demo` ターゲットでイメージを作り（arm64・約31MB）、`--memory 256m` のコンテナへ公開と同じ引数で起動して、ブラウザ試験を幅390・768・1440pxで3周（9回すべて合格・OOMKilledなし）。amd64は、配備用のDockerfile（クロスコンパイル）でビルドし、エミュレーション下で同じ試験が合格。**未了**: GitHub Actions上の実行（PRを出したあとに確認する）。
  - 幅768pxのCIで初期化の案内が出なかった。traceではコンテナ起動の約2.2秒後、`/login?reset=1` と `/public-entry.js` は200だったが、案内を表示する `/login.js` とスタイルの `/login.css` は `net::ERR_NETWORK_CHANGED` で取得に失敗した。過去に幅1440pxで入室が止まった件は原因未確認。
  - 幅ごとの仮想インターフェース（veth）の作り直しが通信中断のきっかけとみて、CIを `--network host` に変更した。追加のブラウザコンテキストでも、失敗した要求のURLとエラー、ページ例外を該当試験の失敗レポートに残す。
  - 2026-11-28に `node --check` とYAML・シェルの構文検査が通った。通信失敗とページ例外を模擬すると、どちらもPlaywrightの失敗レポートに載った。前段（4442）からバックエンド（4317）への転送処理はメモリ上で確認した。修正後のCI実行は未確認。
  - 親の確認（2026-11-28）: 直した試験は手元の実行ファイルで幅390・768・1440pxとも24件が合格（公開URL専用の1件は対象外）。`/login.js` の通信を意図的に失敗させると、失敗レポートに要求URLとエラーが残った。

- [x] 4.4 独立レビュー（Opus・読み取り専用・2026-11-24）の指摘への対応
  - 共有session（入室で増やさない・logoutで消さない・全体の入室上限300回/分）、保存量の上限が続いたときの自己終了（理由 storage）、デモでは監査を書かない、活動の取消を件数の上限に数える、効かない `Connection: close` の削除、連絡先URLの検査をWorkerとアプリでそろえる、Workerの接続元キー（IPv6は/64）とアラームの書き込みの間引き、E2Eの固定窓への依存の解消、文書の訂正。
  - _Boundary: crates/app/src/hosted/・deploy/demo/・docs/_
  - _Depends: 4.2, 5.1_
  - _Requirements: 1.2, 3.2, 3.3, 3.5, 3.7, 4.4, 4.5_
  - 検証（2026-11-24）: 1回目の独立レビュー（Opus・読み取り専用）の重大1・中2・軽微7を、親がコードとtiny_httpのソースで確かめたうえで直した。2回目（修正の差分だけ・同じ方式）は重大なし・中1（アラームの間引きで、DOの退避後に発火すると期限より早く止まりうる）で、設定済みのアラームより2分以上先のときだけ書き込む形（`ALARM_REFRESH_SLACK_MS`）に直し、退避後に発火する試験を足した。軽微の文書の食い違いも直した。
  - 確認: `cargo xtask check-all`（408件・失敗なし）、REUSE（444/444）、`npm run check`（32件）、実バイナリのブラウザ試験（幅390・768・1440pxで各24件）、256MiBのコンテナで3周×3幅（9/9・OOMKilledなし）、`cargo xtask docs-check`・`check-links`。初期データの保存先は0.44MiB（上限32MiBの約1.4%）。
  - `cargo xtask check-cloud` の未通過は、すべて変更前の `origin/main` でも同じ条件で再現する既存の事象で、今回の変更は原因ではない: ①macOSの一時ディレクトリが別名（`/var`→`/private/var`）を含み、`remote_backup` の14件が落ちる（本物のパスの `TMPDIR` を渡すと通る）②全体実行のときだけ落ちるsqld起動の2件（`restore_first_control_commit_…`・`restore_lost_tenant_write_…`。変更前のmainでも130件中2件が同じ名前で落ちる。単独では3回通る）③新しく公開された脆弱性情報 RUSTSEC-2026-0592（`url` 2.5.8・`cloud-deny`・`Cargo.lock` は未変更）。Linuxの `ci.yml` での確認は、PRを出したあとに行う。

- [ ] 5. 検証する
- [x] 5.1 ブラウザ試験と悪用の試験
  - 入口 → ボタン → 再架電のある画面 → 架電モードで保存（翌日朝9時）→ 商談ボード → 月次集計の値 → 注意書き → 共有欄を、幅390・768・1440pxで確かめる。
  - 入室の連打、巨大・分割転送の本文、取り込みの拒否、遅い本文の接続による自己終了を確かめる。
  - _Boundary: deploy/demo/browser/（新規）, Rustの結合試験_
  - _Depends: 1.3, 2.1, 2.2, 3.1_
  - _Requirements: 1.4, 2.2, 2.3, 3.1, 3.2, 3.3, 3.5, 3.6, 6.1_
  - 検証（2026-11-24）: `deploy/demo/browser/`（実バイナリ＋自己署名HTTPS前段）で、`demo-flow.spec.mjs`（入口・キーボード・初期化後の戻り・起動待ち・メンテナンス・入室・止める操作・再架電・商談ボード・月次集計・段階変更・入室の連打）と `public-gate.spec.mjs`（HTTPの境界）を、幅390・768・1440pxのそれぞれで24件合格（公開URL専用の1件は省略）。A4で、共有session（別々に入室した2人が両方操作でき、片方のログアウトで切れない）と入室の全体上限（429の直後に新しい訪問者へ「混み合っています」・入室済みの画面は動き続ける・分をまたいだらやり直す）の試験を足した。同じ試験を、arm64とamd64（エミュレーション）のコンテナにも向けて合格。自己終了・分割転送・過大な本文・取り込みの拒否は、Rustの実プロセス試験で確認（上の2.1・2.2）。
- [ ] 5.2 公開URL用の検査と記録
  - 公開URLを引数に、HTTPS・Host不一致の拒否・未認証APIの拒否・入室・紹介動画の流れ・幅390pxの操作・OGPと共有・遅い本文の接続を確かめる検査を作る。結果を `docs/public-demo-verification.md` に残す。
  - 検査は `deploy/demo/browser/`（`MEIBO_DEMO_URL=https://…`）に作成済みで、手元の `wrangler dev` 越しに幅3種で予行が通っている（4.2）。公開URLでは、接続元ごとの上限（入室20回/分）に当たらないよう、幅ごとに1分以上あけて流す。自己署名の証明書の手元の確認だけ `MEIBO_DEMO_INSECURE_TLS=1`（公開URLでは使わない）。残りは、公開URLでの実行と結果の記録。
  - _Boundary: deploy/demo/browser/, docs/_
  - _Depends: 5.1_
  - _Requirements: 6.2, 6.3_

- [ ] 6. 配備する（運用者の承認後）
- [ ] 6.1 実行先・月額上限・ドメインの決定と承認の記録
  - 第1候補はCloudflare Containers（Workers Paid 月$5の契約が要る）、代替はFly.io。運用者の明示の承認（契約・月額の上限と通知・DNSの変更・公開）を得て、`spec.json` の `execution_authorization` に記録する。承認の前は、契約・カード登録・DNSの変更・公開を行わない。
  - 記録（2026-11-24）: 運営者は、ブランチのpushとPRの作成（マージは運営者）、公開URLのDNS（Workerのカスタムドメイン）の追加（実行は配備のとき）、手元のDockerへのbuildxの導入を承認した。**Workers Paid の契約は「まだ契約しない」**とした。そのため、配備と公開（6.2以降）は、契約の承認が出るまで進めない。
  - _Requirements: 5.1_

- [ ] 6.2 費用上限と停止・再開の手順
  - 実行先の月額上限と、超えそうなときの通知または停止を設定する。1つの操作でデモを止める手順と、再開する手順を `docs/` に書き、実際に止めて再開できることを確かめる。
  - _Depends: 6.1_
  - _Requirements: 5.1, 5.2, 5.4_

- [ ] 6.3 配備と公開後の検証
  - 検査済みの版のソースから、wranglerでコンテナとWorkerを配備し、独自ドメインをWorkerへ割り当てる（DNSの変更は、内容を見せて承認を得てから）。5.2の検査を公開URLで通す。1つでも失敗したら公開しない。
  - _Depends: 4.2, 4.3, 5.2, 6.2_
  - _Requirements: 5.3, 6.2, 6.3_

- [ ] 6.4 案内
  - READMEと、紹介ページ（100日チャレンジのDay99）に、公開デモのURLと「共有の合成データ・定期的に初期化」の説明を足す。実際に公開された後の記述だけを書く。
  - _Depends: 6.3_
  - _Requirements: 8.2, 9.2_
