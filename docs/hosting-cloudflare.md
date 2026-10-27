# Cloudflareへの配備準備

WorkerをHTTPS入口、ContainerをRustの実行先、Turso Cloudを保存先に使う。
現在は準備段階で、Cloudflareへの実配備と公開後の検証は完了していない。
[遠隔保存](hosting-remote.md)と[公開前の照合](hosting-cloudflare-preflight.md)も参照する。

## 前提

固定したNode/npmとDocker、Python、認証済みのWrangler/Turso CLIを使う。
Cloudflare ContainersにはWorkers Paidが必要で、新規契約は自動実行しない。
配備先のIDはplanと`CLOUDFLARE_ACCOUNT_ID`に設定し、独立に確認した期待IDを
`MEIBO_EXPECTED_CF_ACCOUNT_ID`へ渡す。Tursoの組織は
`MEIBO_TURSO_ORGANIZATION`で明示する。既定のアカウントへfallbackしない。

全control・tenant・復旧候補のprimaryを提供者APIとCLIで照合する。
容量上限が不明な応答、replica、照会失敗は拒否する。
`size_limit`が空の場合は、検証済みprimaryから`PRAGMA page_size`と
`PRAGMA max_page_count`を読み、DB実上限と組織枠の小さい方を使う。
この実測が欠ける場合も拒否する。DBの設定や業務データは変更しない。
作成時の引数だけで上限設定済みと判断せず、実APIの値を確認する。
初期化と利用者の招待は遠隔保存の管理CLIで行う。

## 検査と実行物

```sh
cd cloudflare
npm ci
npm run check
npm run artifacts:prepare -- \
  --plan /private/plan.json --secrets /private/secrets.json \
  --image meibo-beta:release --out /private/artifacts
```

入力は私有の通常JSONファイル、出力先は本人だけが読める既存directoryとする。
Docker用の認証・接続設定は環境で渡し、秘密をbuild argへ入れない。
prepareはsource snapshotを付けたLinux/amd64 imageとWorker bundleを作る。
OCIのmanifest digestとimage config IDを区別し、全blobと秘密の非混入を照合する。
prepareだけでは実起動や公開成功を記録しない。

```sh
npm run publication -- \
  --plan /private/artifacts/plan.json --secrets /private/secrets.json \
  --image meibo-beta:release --artifacts /private/artifacts --mode check
```

checkは最新の提供者情報を採取し、source・OCIを再検査する。
Workerを再bundleし、指定imageで実際に起動してhealth・login・CONFIG権限を確認する。
業務データへの更新要求は送らない。採取結果は同じ`validatePreflight`へ渡す。
契約条件も含むため、Workers Paid不足の場合はcheckも拒否する。
保存済みのreportは証拠の出力であり、次の配備許可へ転用しない。

## 初回公開

実環境の検査と契約条件を満たした後、同じ引数で`--mode publish`を指定する。
既存Workerへの更新はこの入口の対象外であり、上書きしない。

公開処理はimageをregistryへpushし、そのconfigと実起動したconfigを照合する。
registryのmanifest表現が異なる場合も対応を記録し、最新情報で再検査する。
秘密は一要求100件以内で`--secrets-file /dev/stdin`へ渡し、実コードと一緒に
workers.devを無効にした状態へ登録する。すべての参照名とbundleを照合してから公開する。
preview URLとobservabilityは無効にし、DBのURL・tokenはWorker secretだけに置く。
実配備後のWorker version・image digest・公開先の対応は私有の証拠へ保存する。

失敗した配備は自動で再送しない。公開要求後の応答消失は結果不明として確認する。
privateな初回配備が残った場合も、既存対象として拒否するため、状態を調べてから再開する。
旧処理の終了・gate状態・秘密更新後の再起動は遠隔保存の規則に従う。
公開HTTPへ管理・復旧用の迂回入口を追加しない。

## 残る検証

追加の[結合検証の記録](hosting-cloudflare-verification.md)に、成功した範囲と未解決の観察を残している。
本番相当のWorker/Container結合、同一実行物でのHTTPS・2組織3権限、
休止・再作成・復旧後の保持、公開先への同じ検査を完了してから公開検証済みとする。
`npm run check`の成功を提供者への配備成功として扱わない。

[Cloudflareアカウントでログインする手順](hosting-cloudflare-access.md)。
