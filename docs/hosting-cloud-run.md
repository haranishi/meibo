# Cloud Run と Turso で自分用に公開する

運用者本人が使う招待制の公開を想定する。Turso の利用規約（2024年3月版）は第三者の
ための利用を認めないので、他の人や会社に使ってもらう公開にはこの構成を使わない。
緊急時とバックアップは[運用の手順](cloud-run-operations.md)に分ける。

## 構成と費用の守り

- Cloud Run は最大1台・最小0台・リクエスト単位の課金で動かす。
- 配備の前に、そのプロジェクトの Cloud Run を対象にした支出上限と通知を作る。
  上限はクレジットや無料分を引く前の金額で判定し、反映には時間差がある。
- 確認用と本番は、プロジェクト・DB・鍵・実行アカウントを分ける。確認用は合成データだけ。
- 本番は Identity-Aware Proxy を前に置き、運用者のアカウントだけを通す。
- `main` への取り込みは PR と CI を必須にし、本番への反映は確認してから行う。

## 配備の部品

- イメージは `cloudflare/Dockerfile`（linux/amd64・`cloud` feature）と `cloudflare/entrypoint.sh`。
- 環境変数 `MEIBO_ORIGIN`（公開URL）と `MEIBO_REMOTE_CONFIG`（参照名だけの
  [遠隔保存の設定](hosting-remote.md)）を渡し、ポートは4317にする。
- DB の鍵は Secret Manager に置き、専用の実行アカウントにだけ読み取りを許可する。
  Secret Manager の無料は、有効・無効の版を合わせて6つまで。接続先URLは通常の
  環境変数にしてもよいが、記録やリポジトリには出さない。

## 管理の道具

`cargo xtask cloud-run-admin` は、配備の設定から接続先と鍵の参照を読み、鍵を
Secret Manager から取り出して `meibo hosted-remote` の子プロセスにだけ渡す。鍵は
画面にもファイルにも書かない。参照名だけの設定ファイルは作業中だけ置いて消す。
初回は `meibo` を鍵を読む前にビルドする（`--meibo` で指定もできる）。

```sh
cargo xtask cloud-run-admin --project PROJECT --region REGION --service SERVICE -- gate-status
```

`--service` は、最新の作成版が準備済みで通信の100%を受けているときに使う。切り戻し中や通信の分割中は `--service SERVICE --revision NAME` で対象の版を選ぶ。
実行する端末には、対象プロジェクトの秘密を読める `gcloud` が要る。復旧先は `--target-describe FILE` で指定し、別プロジェクトなら `--target-project PROJECT` も渡す。`serve` は配備先で実行する。

## 初回の準備（サービスを作る前）

サービスの代わりに、同じ形の記述ファイルを `--describe` で渡す。値を持つのは
接続先URLだけで、鍵は参照（秘密の名前と版）を書く。ファイルはリポジトリの外に置く。`status` を含む記述ファイルでは、配備済みサービスと同じ通信先の照合を行う。

```json
{"spec":{"template":{"spec":{"containers":[{"env":[
 {"name":"MEIBO_REMOTE_CONFIG","value":"{\"version\":1,\"control\":{\"url_env\":\"MEIBO_DB_CONTROL_URL\",\"token_env\":\"MEIBO_DB_CONTROL_TOKEN\"},\"tenants\":{\"main\":{\"url_env\":\"MEIBO_DB_MAIN_URL\",\"token_env\":\"MEIBO_DB_MAIN_TOKEN\"}}}"},
 {"name":"MEIBO_DB_CONTROL_URL","value":"libsql://CONTROL_HOST"},
 {"name":"MEIBO_DB_CONTROL_TOKEN","valueFrom":{"secretKeyRef":{"name":"CONTROL_SECRET","key":"latest"}}},
 {"name":"MEIBO_DB_MAIN_URL","value":"libsql://MAIN_HOST"},
 {"name":"MEIBO_DB_MAIN_TOKEN","valueFrom":{"secretKeyRef":{"name":"MAIN_SECRET","key":"latest"}}}
]}]}}}}
```

空の control と組織DBに対して、次の順に実行する。パスワードは端末で非表示のまま
入力し、シェルの履歴や引数に残さない（zsh の例）。利用者名は推測されにくいものにする。

```sh
A="cargo xtask cloud-run-admin --describe FILE --project PROJECT --"
${=A} init
${=A} tenant-add main
read -rs "PW?パスワード: "; echo
printf '%s\n' "$PW" | ${=A} user-add NAME --password-stdin
unset PW
${=A} member-set NAME main admin
```

## 配備

```sh
export CLOUDSDK_CORE_DISABLE_FILE_LOGGING=1 CLOUDSDK_CORE_LOG_HTTP=false CLOUDSDK_CORE_VERBOSITY=error
gcloud run deploy SERVICE --image=IMAGE --region=REGION --project=PROJECT \
  --service-account=RUN_ACCOUNT --port=4317 --cpu=1 --memory=512Mi --concurrency=10 \
  --min-instances=0 --max-instances=1 --max=1 --cpu-throttling \
  --env-vars-file=ENV_FILE --set-secrets=ENV_NAME=SECRET_NAME:latest
```

確認用は `--allow-unauthenticated` を付け、アプリのログインで守る。

本番は付けずに Identity-Aware Proxy を有効にし、通すアカウントを追加する。組織の
無いプロジェクトでは、独自の OAuth クライアントを一度設定する必要がある。手順は
[公式の説明](https://docs.cloud.google.com/run/docs/securing/identity-aware-proxy-cloud-run)
に従う。この構成での Identity-Aware Proxy は未検証で、配備時に確かめて追記する。
