# Cloud Run 版の運用と緊急時の手順

`P`=プロジェクト、`R`=地域、`S`=サービス名。管理命令は
[管理の道具](hosting-cloud-run.md#管理の道具)を使う。

```sh
A="cargo xtask cloud-run-admin --project P --region R --service S --"
```

例は zsh で書く（bash では `${=A}` を `$A` にする）。

## 状況を見る

```sh
gcloud logging read 'resource.type="cloud_run_revision" AND resource.labels.service_name="S" AND severity>=WARNING' \
  --project=P --freshness=1d --limit=50
```

## すぐ止める・再開する

```sh
gcloud run services update S --region=R --project=P --ingress=internal   # 外からの要求を止める
gcloud run services update S --region=R --project=P --ingress=all        # 再開する
```

最小0台なので、要求が来なければ計算の料金はかからない。秘密の漏えいが疑われるときは、
止めてから鍵を差し替える。

## 支出上限に達した

アプリがエラーになり、請求先の管理者とプロジェクトのオーナーにメールが届く。翌月の
初めに自動で再開する。早く戻す前に、ログで要求が増えた原因を確かめる。

早く戻すときは、Google Cloud の「予算とアラート」で該当の予算を開き、支出上限を
解除する（反映に最大1時間ほど）。**解除すると、金額を上げ直すまでその月は上限が
効かない。** 解除した直後に、受け入れられる新しい金額を設定する。
詳しくは[公式の説明](https://docs.cloud.google.com/billing/docs/how-to/budgets-spend-caps)。

## 「処理結果を確認できません」

読み取りで通信が切れた場合は、アプリが終了して次の要求で立ち上がり直す。少し待って
画面を開き直す。続く場合や、保存・管理操作で出た場合は、排他（gate）が残っている
可能性がある。操作を繰り返さず、次の順に確かめる。

1. `--ingress=internal` で要求を止める。
2. 古い実行を終わらせる。
   `gcloud run services update S --region=R --project=P --update-labels=restarted=$(date +%Y%m%d%H%M)`
3. `${=A} gate-status` で占有と `owner` を確かめる。占有が無ければ6へ。
4. 進行中の処理が無いことを確かめる。
5. `${=A} gate-recover OWNER --prior-work-terminated`
6. `--ingress=all` で再開し、直前に保存した内容が入っているかを画面で確かめる。
   入っていなければ、もう一度入力する。

## 鍵の期限と差し替え

DB の鍵には期限がある。切れる前に差し替える。鍵は画面に出さず、そのまま保管庫へ渡す。

```sh
T=$(turso db tokens create DB_NAME --expiration 90d) && [ -n "$T" ] \
  && printf %s "$T" | gcloud secrets versions add SECRET_NAME --data-file=- --project=P
unset T
gcloud run services update S --region=R --project=P --update-labels=rotated=$(date +%Y%m%d%H%M)
```

鍵の作成に失敗したときは版を足さない（空の版を足すと、次の起動で失敗する）。アプリは
起動時にDBへ接続するので、鍵が誤っていると新しい版は切り替わらない。参照が `latest` の
ため、古い版の実行も立ち上がり直すと新しい鍵を読む。切り替わらないときは、鍵を作り直して
版を足し直す。

画面で動作を確かめたら、古い版を消す。有効と無効の版はどちらも料金の対象で、無料は
6版まで（超えた分は1版ごとに月0.06ドル）。消した版は戻せない。

```sh
gcloud secrets versions list SECRET_NAME --project=P
gcloud secrets versions destroy VERSION --secret=SECRET_NAME --project=P
```

## バックアップ

週に1回と、まとまった取り込みの前に実行する。保存先は、すでにある場所の下の
新しいディレクトリにする（途中にシンボリックリンクがある場所は拒否される）。

```sh
mkdir -p -m 700 "$HOME/meibo-backups"
${=A} backup "$HOME/meibo-backups/$(date +%Y%m%d-%H%M)"
```

`backed-up` が出れば、件数・型・hash の照合まで済んでいる。保存物にはパスワードの
照合用データと営業の情報が入るので、本人だけが読める場所に置き、共有や公開をしない。
実行中は排他を取るため、画面の操作は一時的に断られる。古い保存物は自動で消さない。
定期実行は、保存先と頻度を決めてから設定する。

## バックアップから戻す

元の DB は消さない。空の新しい DB（control と各組織）を作り、それぞれの鍵を保管庫に
登録する。元と**違う参照名**（例: `MEIBO_DB_RESTORE_CONTROL_URL`）で記述ファイルを作る。

```sh
cargo xtask cloud-run-admin --project P --region R --service S --target-describe TARGET_FILE \
  -- restore BACKUP_DIRECTORY
```

`restored` を確認してから、サービスの接続先URLと鍵の参照を新しい DB に切り替える。
再ログインして件数と記録を確かめるまで、元の DB と保存物を残す。
