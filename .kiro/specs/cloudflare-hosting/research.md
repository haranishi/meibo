# Research: Cloudflare配備の前提

確認日: 2026-10-26。MCP docs検索・公式文書・インストール済みWrangler schemaを照合した。アカウントIDや鍵は記載しない。

## 実行物とAPI
- [Get started](https://developers.cloudflare.com/containers/get-started/)はLinux/amd64を要求する。既存のarm64 Docker検証はCloudflare配備の証拠にならない。
- [Scheduling policies](https://developers.cloudflare.com/containers/configuration/scheduling-policy/)ではdefault policyがapplication全体のimage/instance/max_instancesを持つ。durable_object policyはmax_instances非対応で、初期の1台制限には使わない。
- [Durable Object Container API](https://developers.cloudflare.com/containers/api/durable-object-container/)のbuilt-in APIはdefault policyでもruntime envをstartへ渡せる。追加SDKを採らず、readiness・deadlineとqueueを小さく実装する。
- [Wrangler configuration](https://developers.cloudflare.com/containers/configuration/wrangler/)の新規DOはexportsでSQLite storageを宣言できる。legacy migrationsとは併用しない。手元のWrangler4.151.1のconfig schemaにも該当fieldがある。
- [Environment variables and secrets](https://developers.cloudflare.com/containers/examples/env-vars-and-secrets/)の公式例はWorker secretをruntime envへ渡す。ただし例の内部Host置換は現行meiboの厳密なHost検査に合わないため、公開Hostを検査した上で維持する。

## 永続化と公開条件
保存はremote-storageのTurso Cloud libSQLだけを正本とする。Container disk/snapshotへDBの確定を依存させない。DOのqueueだけで複数processの正しさを保証せず、共有gateを維持する。

上流のtransport検証で、`libsql-hrana 0.10.0`の型付きv2と`reqwest 0.13.5`（default-features=false、json/native-tls-no-alpn）を選定した。候補transportのRust1.88 buildとオンラインadvisory/license/source点検は成功した。Cloud imageはbuildに`pkg-config`/`libssl-dev`、runtimeに`libssl3`/`ca-certificates`を必要とする。統合したamd64 imageのbuildと起動は別途検証する。

対象アカウントの実照会はContainers利用にWorkers Paidが必要という拒否を返した。Workersのstandard usage model表示だけで有料契約を判断しない。契約変更は未実施で、preflightで同じ拒否を再現できるようにする。

[Pricing](https://developers.cloudflare.com/containers/platform/pricing/)ではWorkers Paidの月5米ドルに含む枠と、Containerのmemory/CPU/disk、ネットワーク、Worker/DO等の使用量を別に扱う。初期はlite・最大1台・無要求60秒休止を検証し、固定上限料金とは表示しない。金額は公開直前にも確認する。

## 既存の境界
JSON本文1MiB、CSV import32MiB、export32MiB、固定Host/Origin、__Host session cookie、context header、3権限と旧tab保護を引き継ぐ。Workerで小さな本文上限へ一律変更したり、認証前に内部Hostへ正規化したりしない。

## 未確認
Cloudflare上の実image起動・実secret受渡し・公開HTTPS到達・再作成保持と復旧はまだ行っていない。仕様・dry-run・ローカルruntimeだけでこれらを合格扱いしない。

Wrangler 4.151.1のMiniflare限定でUndiciを7.30.1へoverrideし、既知の依存脆弱性を修正した。npm ci・型/schema/shell・全依存auditを再検証し、独立再レビューAPPROVED。実imageの起動とは区別する。

2026-10-27に[Workers上限](https://developers.cloudflare.com/workers/platform/limits/)と[Secrets](https://developers.cloudflare.com/workers/configuration/secrets/)を再確認した。Paidはsecret/text合計128個、各5KB、bulk一要求100 secrets。CONFIGもこの小さい上限を受ける。事前検査部品のfixture検証と、最新の実測を入力する配備は別タスクとする。
