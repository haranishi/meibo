# メールと予定を取引先に取り込む

ホームの「メール・予定の連携」から、自分のGoogleまたはMicrosoftアカウントを選び、読み取りを許可します。実データ用ワークスペースの編集者と管理者が使えます。担当者の登録メールアドレスに一意に一致したものだけが、組織の営業記録に加わります。

| 取り込み先 | 読み取り範囲 |
| --- | --- |
| 取引先の活動履歴 | 接続以降のGmail、またはOutlookの受信トレイ・送信済みアイテム。日時と件名を記録 |
| 取引先の予定 | 主カレンダー上で、接続の30日前から今後約6か月までの予定。件名、開始、終了を記録。変更・取消も反映 |

参加者が複数の登録担当者に一致する予定、非公開の予定、除外・消去済み企業、個人事業の連絡先は取り込みません。本文や添付を営業DBに保存する処理はありません。

接続したブラウザで営業画面を開いている間は約1分ごとに確認します。「今すぐ同期」でも更新できます。閉じていた間の変更は、次回の起動後に前回の取得位置から読み込みます。初めて使うブラウザでは「メール・予定の連携」を開いてください。最終同期は、そのサービスのメールと予定をすべて確認した時刻です。

「接続を解除」を押すと、保存していた認可情報と取得位置を削除します。取り込んだ活動と予定は組織の営業記録に残ります。サービス側の許可も取り消したい場合は、接続先のアカウントでアプリへのアクセス許可を管理してください。

## 管理者が接続を有効にする

1. Google CloudでGmail APIとGoogle Calendar APIを有効にし、ウェブアプリ用のOAuthクライアントを作成します。Microsoft EntraではWebプラットフォームのアプリを登録します。リダイレクトURIは両方とも営業画面のオリジンに `/oauth/activity-return` を加えたURLです。例は `https://crm.example.test/oauth/activity-return` です。
2. 次の表の権限を、この連携専用のアプリに設定します。Googleの外部公開に必要な審査や組織の同意ポリシーは、各社の管理画面で確認してください。
3. サーバーにNode.js 22以降を用意します。コネクターのプログラムはmeiboのバイナリに同梱されています。LinuxまたはmacOSで、サービスの実行ユーザーが所有する専用の永続ディレクトリを作り、権限を `0700` にします。営業DBのバックアップや共有フォルダからは独立させてください。
4. 下表の環境変数をサービスの起動設定に渡し、再起動します。クライアントシークレットは運用環境のシークレット管理機能に登録します。設定後、本人がホームから接続すると同期が始まります。

| アプリ | 委任される権限 |
| --- | --- |
| Google | `openid`、`https://www.googleapis.com/auth/gmail.metadata`、`https://www.googleapis.com/auth/calendar.events.owned.readonly` |
| Microsoft | `openid`、`email`、`offline_access`、`Mail.ReadBasic`、`Calendars.ReadBasic` |

| 環境変数 | 指定する値 |
| --- | --- |
| `MEIBO_ACTIVITY_SYNC_NODE` | Node実行ファイルの絶対パス |
| `MEIBO_SYNC_VAULT` | 上記の永続ディレクトリの絶対パス |
| `MEIBO_SYNC_ORIGIN` | 営業画面のHTTPSオリジン。ローカルでは固定ポートの `http://127.0.0.1:4317` も使用可 |
| `MEIBO_SYNC_GOOGLE_CLIENT_ID`、`MEIBO_SYNC_GOOGLE_CLIENT_SECRET` | GoogleのクライアントIDとシークレット |
| `MEIBO_SYNC_MICROSOFT_CLIENT_ID`、`MEIBO_SYNC_MICROSOFT_CLIENT_SECRET` | MicrosoftのアプリケーションIDとシークレットの値 |

片方のサービスだけを設定しても利用できます。Microsoftは接続元アカウントのメールアドレスを確認できることが条件です。GoogleのGmailメタデータは制限付きのスコープです。[GoogleのOAuthスコープ](https://developers.google.com/workspace/gmail/api/auth/scopes)・[Microsoftの権限一覧](https://learn.microsoft.com/en-us/graph/permissions-reference)

認可情報と同期位置は専用ディレクトリの `connections.sqlite` に保管します。復旧時には営業DBと整合する時点の専用ストアを用意してください。標準のCloudflare配備は一時ディスクを使うため、接点同期を無効のまま運用します。

### Docker Composeで有効にする

[ホスト環境の初期設定](hosting.md)を済ませ、対象サービスのクライアントIDとシークレットを起動環境に渡します。次のコマンドでNodeを含む `activity-sync` ターゲットを選びます。既定のターゲットは従来の軽量なイメージです。

```sh
docker compose -f compose.yaml -f compose.activity-sync.yaml build app
docker compose -f compose.yaml -f compose.activity-sync.yaml up -d
```

営業DBのボリュームは `meibo_data` です。認可情報には別の `meibo_activity_credentials` を使います。コンテナ内の専用ディレクトリは `/var/lib/meibo-activity`、所有者のUIDは `10001`、権限は `0700` です。更新時も同じ2つのComposeファイルを指定してください。

## 実アカウントを使わずに確かめる

`node --test crates/app/tests/activity-providers.cjs` は両社のAPI応答を合成データで置き換え、認可、取得範囲、ページ送り、変更・取消を確認します。`cargo test --locked -p meibo-app --bin meibo activity_sync` は同意の再利用防止、照合、消去、途中失敗後の再開を確認します。

CIでは390px幅のブラウザで、両社の同意画面、営業DBへの反映、画面を閉じている間に届いた予定の取得、接続の解除までを確かめます。同意画面とAPIの応答は試験プロセス内で再現します。
