# Cloud Run 版の運用と緊急時の手順

例は zsh 用。P、R、S はプロジェクト、地域、サービス名に置き換える。このシェルで gcloud のファイル記録と HTTP ログを切ってから作業する。

```sh
set +x
export CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 CLOUDSDK_CORE_LOG_HTTP=false CLOUDSDK_CORE_VERBOSITY=error
A="cargo xtask cloud-run-admin --project P --region R --service S --"
```

旧版への切り戻し中や通信が複数の版に分かれているときは、通信先を確認し、`--service S --revision NAME` で対象を選ぶ。[管理の道具](hosting-cloud-run.md#管理の道具)

## 状況を見る・入口を止める

```sh
gcloud logging read 'resource.type="cloud_run_revision" AND resource.labels.service_name="S" AND severity>=WARNING' --project=P --freshness=1d --limit=50
gcloud run services update S --region=R --project=P --ingress=internal
```

`--ingress=internal` は外部からの新しい要求を止める。保守中は内部の書き込み元と管理ジョブも止める。再開時は同じ命令で元の入口設定（通常は `--ingress=all`）に戻す。最小0台なので、要求が来なければ計算の料金はかからない。

## 支出上限に達した

アプリがエラーになり、請求先の管理者とプロジェクトのオーナーにメールが届く。翌月初めに自動で再開する。早く戻すときは、ログで要求が増えた原因を確認し、Google Cloud の「予算とアラート」で該当の上限を解除する（反映に最大1時間ほど）。解除した直後に、その月に受け入れられる新しい上限を設定する。[公式の説明](https://docs.cloud.google.com/billing/docs/how-to/budgets-spend-caps)

## 「処理結果を確認できません」

読み取りの通信断なら、アプリの再起動後に画面を開き直す。エラーが続くときや保存・管理操作で出たときは、操作を繰り返す前に排他（gate）を確認する。業務が成功しても、その後の利用量保存で通信が切れると gate が残る。

1. `--ingress=internal` で入口を止める。
2. 古い実行を終わらせる。`gcloud run services update S --region=R --project=P --update-labels=restarted=$(date +%Y%m%d%H%M)`
3. `${=A} gate-status` で占有と `owner` を確認する。占有がなければ5へ。
4. 以前の処理が終了したことを確認してから `${=A} gate-recover OWNER --prior-work-terminated` を実行する。
5. 入口を戻して直前の操作が反映されたかを画面で確認する。結果が確定してから必要な更新だけを入力する。

## 鍵の期限と漏えい

期限前の更新は手順2から行う。漏えいが疑われる場合は手順1から順に行う。Secret Manager の版を消しても、発行済みの Turso の鍵は有効なまま残る。

1. `--ingress=internal` で入口を止め、対象DBを使う他の処理も止める。古い秘密の版番号を控え、`turso db tokens invalidate DB_NAME` を実行する。そのDBの鍵が**すべて**無効になる。
2. 新しい鍵を作り、作成できた場合だけ保管庫へ渡す。以下は冒頭の環境変数を設定したシェルで実行する。

```sh
( set +x; set -o pipefail
  T=$(turso db tokens create DB_NAME --expiration 90d) && [[ -n "$T" ]] &&
    builtin printf %s "$T" | gcloud secrets versions add SECRET_NAME --data-file=- --project=P )
```

3. 版が追加されたことを確認し、その版番号で再配備する。対象DBを使う各サービス・ジョブの参照も更新する。

```sh
gcloud run services update S --region=R --project=P --update-secrets=ENV_NAME=SECRET_NAME:NEW_VERSION
```

4. 新しい版の起動と通信先を確認し、許可した経路から `/health` と対象DBの読み取りを確かめる。漏えい時は、同じDBの HTTPS 接続先を非表示で入力し、古い版の鍵で読み取りを試す。鍵は標準入力で渡し、HTTP 状態コードだけを見る。

```sh
( set +x; set -o pipefail
  export CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 CLOUDSDK_CORE_LOG_HTTP=false CLOUDSDK_CORE_VERBOSITY=error
  read -rs 'DB_HTTPS?対象DBのHTTPS接続先: ' || exit 1
  OLD=$(gcloud secrets versions access OLD_VERSION --secret=SECRET_NAME --project=P 2>/dev/null) && [[ -n "$OLD" ]] || exit 1
  builtin printf 'Authorization: Bearer %s\n' "$OLD" | curl -q -s -o /dev/null -w '%{http_code}\n' -H @- \
    -H 'Content-Type: application/json' --data '{"requests":[{"type":"execute","stmt":{"sql":"SELECT 1"}},{"type":"close"}]}' \
    --proto '=https' --config <(builtin printf 'url = "%s/v2/pipeline"\n' "${DB_HTTPS%/}") )
```

5. 漏えい時は 401 または 403 で古い鍵の拒否を確認してから `--ingress=all` で再開する。通信エラーや他の状態コードなら入口を止めたまま原因を確かめる。
6. 動作を確認したら古い版を消す。削除は取り消せない。有効・無効の版はどちらも保管料の対象で、無料は6版まで、超過分は1版あたり月0.06ドル。

```sh
gcloud secrets versions list SECRET_NAME --project=P
gcloud secrets versions destroy OLD_VERSION --secret=SECRET_NAME --project=P
```

## バックアップ

週1回と、まとまった取り込みの前に実行する。保存先は既存の親ディレクトリの下に新しく作る。途中にシンボリックリンクがあるパスは使えない。

```sh
mkdir -p -m 700 "$HOME/meibo-backups"
${=A} backup "$HOME/meibo-backups/$(date +%Y%m%d-%H%M)"
```

`backed-up` は件数・型・hashの照合まで済んだことを示す。保存物にはパスワードの照合用データと営業情報が入るため、本人だけが読める場所に置く。保存中は排他を取り、画面の操作は一時的に断られる。古い保存物は自動で消さない。定期実行は保存先と頻度を決めてから設定する。

## バックアップから戻す

1. `--ingress=internal` で入口を止め、内部の書き込みと管理ジョブも止める。進行中の処理が終了したことを確認し、元のDBで `${=A} gate-status` が `"occupied":false` になるのを確かめる。占有が残る場合は上の gate 復旧手順に従い、入口を閉じたままにする。
2. **保存物の作成後に行った更新は復旧先には戻らない。** 戻す時点を確認する。空の control と各組織DBを用意し、新しい鍵を復旧先の保管庫に登録する。元と異なる環境変数名で記述ファイルを作る。同じプロジェクトでは秘密名も分ける。
3. 復旧を実行する。`--target-project` を省略すると元と同じプロジェクトを使う。

```sh
cargo xtask cloud-run-admin --project P --region R --service S --target-describe TARGET_FILE \
  --target-project TARGET_PROJECT -- restore BACKUP_DIRECTORY
```

4. `restored` を確認したら、入口を閉じたまま接続先と鍵の参照を新しいDBに切り替える。新しい版へ通信を切り替え、内部経路から再ログインして件数と記録を照合する。
5. 照合後に `--ingress=all` で入口を再開し、停止した書き込み元も戻す。元のDBと保存物は保管する。
