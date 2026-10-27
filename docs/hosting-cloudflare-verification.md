# Cloudflare結合検証の記録

接続障害は未解決。
2026-10-27の追加検証で、公開先での検証はまだ実施していない。
対象のRust実行物とWorker bundleは、PR7で取り込んだものと同じ。

## 確認できたこと

- 実Wrangler SDKのHTTPS入口から、合成SQLDに接続する構成で、6並列要求を3回、計18件実行し、すべて200だった。
- 同じ構成のブラウザ検証は8項目を通過。2組織3権限、更新、CSRF/context拒否、旧tab、logout、3画面幅を確認した。想定外の400、資産取得失敗、外部要求、page errorは0だった。
- 別の実行でも8項目を通過し、再起動後のセッション・storage scope・保存内容を確認した。
- SDKは外部からのDocker再起動後にコンテナを再作成した。検証ではコンテナIDの維持を求めず、新しいプロセスと保存内容を確認した。
- 実Tursoのsource用secret参照と設定の一致、直接Docker CLIによるgate-statusを確認した。

## 未解決の観察

合成SQLDへの接続では、別の実行でAPIが約33秒後に503/outcome_unknownとなった。<!-- ok -->
このときDB入口はTCP接続を1件受けたが、TLSのsecureConnectionイベントは発生せず、
ECONNRESETを記録した。対応するHTTP/SQLのエラー記録はなかった。
原因は未確定だ。アプリ・SDK・検証用ネットワークをさらに切り分ける必要がある。

実Tursoへ切り替えた構成では、SDKの起動確認が30秒後に503/unavailableとなった。<!-- ok -->
その後のコンテナ内部のhealthは503/outcome_unknownだった一方、
同じコンテナのOpenSSLによるTursoへのTLS接続と証明書検証は成功した。
ブラウザ検証はhealthで停止。実Tursoを使う構成は未合格。

## 保持・復旧と公開の残件

再起動後の保持は確認できた。70秒の無要求観測では休止を確認できなかった。<!-- ok -->
[公式API](https://developers.cloudflare.com/containers/api/durable-object-container/#setinactivitytimeout)の60秒はDOが非活動になった後から数える。<!-- ok -->
観測時間を延ばした試行はAPIタイムアウトで止まり、休止の再検証には至らなかった。
休止、明示的な再作成、故障gateの復旧、新DBへのrestoreは、SDK経由では未合格。
既存のnative経路での13項目成功を、この構成の証拠へ置き換えない。

最新の提供者照会ではWorkers PaidとContainers利用条件が不足していた。
Worker作成・image push・公開secret登録・契約変更は行っていない。
公開には接続障害の切り分け、残る保持・復旧検証、契約条件、fresh preflight、公開後の同じ検証が必要。
終了時は検証プロセスを停止し、実Tursoと今回の合成SQLDのgateが空いていることを確認した。
秘密値・SQL・cookie・batonを含まない診断記録を、private run `2026-10-27-cloud-runtime-r2`へ保存した。
