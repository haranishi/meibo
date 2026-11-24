# 公開デモの運用（停止・再開・費用の通知）

配備と構成は [README.md](README.md)。ここは、運用者が止める・再開する・費用を見張るときの手順です。

## 停止と再開の手順

コンテナイメージを作り直さず Worker の設定だけを更新するため、`--containers-rollout none` を付けて切り替えます。

### 停止（メンテナンス応答 503 に切り替える）

```sh
cd deploy/demo/cloudflare
npx wrangler deploy \
  --containers-rollout none \
  --domain <公開ドメイン> \
  --var MEIBO_DOMAIN:<公開ドメイン> \
  --var DEMO_ENABLED:false \
  --var MEIBO_DEMO_CONTACT:<連絡先URL>
```

実行後、公開ドメインへのアクセスはコンテナを起動せず 503（`Retry-After: 300`、「メンテナンス中です。しばらくしてからお試しください。」）を返します。

### 再開

```sh
cd deploy/demo/cloudflare
npx wrangler deploy \
  --containers-rollout none \
  --domain <公開ドメイン> \
  --var MEIBO_DOMAIN:<公開ドメイン> \
  --var DEMO_ENABLED:true \
  --var MEIBO_DEMO_CONTACT:<連絡先URL>
```

### Cloudflare ダッシュボードから切り替える場合

1. 対象 Worker（`meibo-demo`）の **Settings → Variables and Secrets** を開く。
2. 変数 `DEMO_ENABLED` を `"false"`（停止）または `"true"`（再開）へ変更して保存する。
3. ダッシュボード側で変数を変更したあとに CLI から再配備する場合は、`--var` で同じ値を明示するか `--keep-vars` を付けて上書きを防いでください。

## 公開前の確認と、通報を受けたとき

- 公開する配備では、連絡先（`MEIBO_DEMO_CONTACT`）を必ず渡します。入口に表示され、違法・有害な内容の連絡や削除の依頼を受ける窓口になります。
- 通報を受けたら、上の手順で停止（`DEMO_ENABLED:false`）するか、コンテナを再起動して初期化します。訪問者の入力は初期化で消えます。
- 公開ドメインのゾーンでは、外部へ送る計測スクリプトを差し込む機能（Cloudflare Web Analytics の自動設定・Zaraz・メールアドレスの難読化など）を使わないでください。入口の CSP（`script-src 'self'`）が外部のスクリプトを止めますが、設定として入れない方が確実です。

## 費用の通知と上限（Cloudflareの公式の説明・2026-11-24）

- 常時起動した場合の最悪は月$9前後です（`lite`・730時間・CPUが常に動く場合の概算）。休止すれば、含まれる使用量（`lite` で約100時間分）の範囲で、Workers Paid の月$5のままです。
- Containers の使用料は、Cloudflare の費用の上限（Spend Management）の対象外です。上限があるのは Workers と Durable Objects だけで、アカウント全体に効きます。超えると、同じアカウントの他の Worker（問い合わせ用の関数など）も止まるため、上限は設定しません。
- 代わりに、次の3つで守ります。
  1. **予算アラート**：ダッシュボードの Billing → Billable Usage → Create budget alert で、アカウント全体の使用料が閾値（例：$8）を超えたらメールで知らせます（知らせるだけで、止まりません。従量課金のアカウントだけが対象）。
  2. **停止の切替**：上の手順で `DEMO_ENABLED:false`（設定値1つ）。
  3. **コンテナは1台固定**（`max_instances: 1`）で、無通信15分で休止します。
