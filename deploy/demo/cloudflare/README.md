# meibo 公開デモ用 Cloudflare Worker・コンテナ設定

合成データだけで動く公開デモ（`meibo hosted --public-demo`）を、Cloudflare Workers と Cloudflare Containers 上で動かすための設定と最小の逆プロキシ Worker です。

## 構成と役割

```text
訪問者 → Cloudflare（カスタムドメイン・Worker）→ Durable Object（DemoContainer）→ Container（meibo hosted --public-demo）
```

- **遅い送信からの保護**: コンテナ内のサーバーは単一スレッドで順番に処理するため、Worker が POST の本文（`Content-Length` 明示・64KiB 以下）を時間上限つきで最後まで受け切ってからコンテナへ渡します。分割転送・上限超過・途中切断・タイムアウトはコンテナへ届きません。
- **停止の切替**: 環境変数 `DEMO_ENABLED` が `"true"` 以外のときは、キャッシュ・レート制限・コンテナへ触れず 503（`Retry-After: 300`）を返します。
- **接続元ごとの頻度制限**: `CF-Connecting-IP` をキーに Workers の Rate Limiting バインディングで制限します。IPv6 の接続元は先頭 64 ビット（`xxxx:xxxx:xxxx:xxxx::/64`）に丸めたキーにします（1 台が /64 内のアドレスを替えて制限を逃れないため）。IPv4 射影（`::ffff:a.b.c.d`）は IPv4 として扱います（`POST /demo/enter` 20 回/分、その他 POST 60 回/分、GET 600 回/分。教室や会場のように、同じ接続元の複数人が同時に開く場面で締め出さないための余裕を持たせています）。超過時は 429（`Retry-After: 60`）を返します。
- **静的応答の保持**: `GET` の `/login`（300 秒）と `/login.css`・`/login.js`・`/public-entry.js`・`/public-entry.png`（各 3600 秒、クエリ無視）のみ、`Cache-Control: public, max-age=...` を付け直して Cache API に保持します。`Set-Cookie` を含む応答と上記以外の経路は一切保持しません。保持済みの応答はコンテナ休止中でもコンテナを起こさず返します。
- **Host の維持とヘッダー制限**: `getTcpPort(4317).fetch(...)` へ渡す要求は URL のホストと `Host` ヘッダーを公開ドメイン（`MEIBO_DOMAIN`）に揃え、`Cookie`・`Origin`・`Content-Type`・`X-Meibo-*` など許可したヘッダーだけを渡します。`CF-*`・`X-Forwarded-*`・`Forwarded` は渡しません。
- **単一インスタンスとアイドル停止**: `max_instances: 1`、既定は `instance_type: "lite"`（256MiB、メモリ実測しだいで `"basic"` 1GiB へ変更可能）。最後の要求から 15 分後に Durable Object のアラームでコンテナを停止し、次回要求で新しい初期状態のコンテナを起動します。アラームは要求のたびには書き込みません。期限はメモリ上で延ばし、アラームが未設定のときだけ設定します。発火したときに期限がまだ先なら、その期限で設定し直します（Durable Object が退避して印が消えたら、次の要求で設定し直します）。

## 前提条件

- Node.js `24.22.1` / npm `12.2.1`（`package.json` の `engines` に合わせる）
- ローカルでコンテナをビルド・起動する場合は Docker（`wrangler` は内部で `docker buildx` を使うため、buildx が要ります）
- 開発用の依存（wrangler 経由）に、LGPL-3.0-or-later のライブラリ（`@img/sharp-*` の libvips）が含まれます。手元の検査にだけ入り、デモの配布物（Worker・イメージ）には入りません。
- Cloudflare へ実配備する場合は Workers Paid プランと、対象ドメインのゾーンがある Cloudflare アカウント（契約・費用上限と通知・DNS 変更は運用者の承認後に実施）

## 手元での検査

```sh
cd deploy/demo/cloudflare
npm ci
npm run check
```

`npm run check` は次を順に実行します。

1. `npm run types:check`（`wrangler.jsonc` と `src/worker-configuration.d.ts` の一致確認）
2. `npm run typecheck`（TypeScript の型検査と Workers 型契約検査）
3. `npm test`（`node --test` による Worker・Durable Object・設定・Dockerfile 一致の単体試験）

Docker が使える環境でローカル起動を確かめる例（Cloudflare へ配備はしません）：

```sh
cd deploy/demo/cloudflare
npx wrangler dev --local-protocol https --port 8787 \
  --var MEIBO_DOMAIN:127.0.0.1:8787 \
  --var DEMO_ENABLED:true
```

## 配備時に渡す値の一覧

公開ドメインや連絡先 URL はリポジトリへ書かず、配備時に渡します（`wrangler deploy --help` で確認した現行オプションを使用）。

| 渡し方 | 項目 | 必須 | 説明 |
| --- | --- | --- | --- |
| `--domain <公開ドメイン>` | カスタムドメイン | 必須 | Worker を割り当てる公開ドメイン。`workers_dev: false` のためカスタムドメイン経由のみで公開する。Cache API（`caches.default`）もカスタムドメイン上で有効になる。 |
| `--var MEIBO_DOMAIN:<公開ドメイン>` | 公開ドメイン変数 | 必須 | `--domain` と同じ小文字ホスト名（スキームなし）。Worker の Host 検査・Cache キー・コンテナ起動時の `--origin https://${MEIBO_DOMAIN}` に使う。 |
| `--var DEMO_ENABLED:true` | 稼働フラグ | 任意（既定 `"true"`） | `"true"` で稼働、`"false"` で即座にメンテナンス応答（503）へ切替。 |
| `--var MEIBO_DEMO_CONTACT:<連絡先URL>` | 連絡先 URL | 公開するときは必須（既定 `""`） | デモ入口に表示する運用者の連絡先（小文字の `https://` または `http://` の URL）。空のときはコンテナへ渡さない。検査はアプリと同じ規則で、両方の試験が `tests/contact-url-cases.json` を読んで一致を確かめる。不正な値のときはコンテナを起動せず 503 を返す。 |

### 配備前の確認事項

- `wrangler.jsonc` の `ratelimits[].namespace_id`（`"9901"`・`"9902"`・`"9903"`）は Cloudflare アカウント内で一意である必要があります。同じアカウントの別 Worker で既に使っている番号があれば重複しない正の整数文字列へ変更してください。
- `wrangler.jsonc` の `containers[0].instance_type` は既定で `"lite"`（256MiB）です。実測メモリが不足する場合は `"basic"`（1GiB）へ変更してから配備します。
- `wrangler deploy` は `--keep-vars` を付けない限り、`wrangler.jsonc` と `--var` に指定した変数で既存の変数を置き換えます。`MEIBO_DEMO_CONTACT` を使う場合は毎回同じ `--var` を渡すか、`--keep-vars` を併用してください。

## 初回配備（運用者の承認後に実行）

```sh
cd deploy/demo/cloudflare
npx wrangler deploy \
  --domain <公開ドメイン> \
  --var MEIBO_DOMAIN:<公開ドメイン> \
  --var DEMO_ENABLED:true \
  --var MEIBO_DEMO_CONTACT:<連絡先URL>
```

連絡先 URL を設定しない場合は `--var MEIBO_DEMO_CONTACT:...` 行を省略します。

## 停止・再開・費用の通知

停止（設定値 `DEMO_ENABLED` を1つ変える）と再開の手順、費用の通知の設定は [OPERATIONS.md](OPERATIONS.md) にあります。

## データの扱いと非保持（設計上の事実）

- Worker の `observability.enabled` は `false` に設定し、訪問者の入力やヘッダーを Worker ログへ出力しません。
- Durable Object の保存領域へ営業データ・セッション・Cookie は一切書き込みません（アイドル停止タイマー用のアラームのみを使用します）。
- 営業データはコンテナ内の揮発領域（`/tmp/meibo-demo-<時刻>-<pid>`）の SQLite にのみ置かれ、アイドル 15 分（アラーム停止またはアプリ自己終了）、稼働上限 6 時間、または保存量の上限に達したままのとき（アプリ自己終了）にコンテナが終了すると消去され、次回起動時に初期状態へ戻ります。
- コンテナは `enableInternet: false` で起動するため、コンテナ内部から外部ネットワークへ送信できません。
- 公開ドメインへのアクセス自体は Cloudflare のネットワークを通過するため、プラットフォーム側の通信記録が残り得る点はデモ入口の注意書きで案内します。
