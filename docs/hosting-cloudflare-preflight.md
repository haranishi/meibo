# Cloudflare公開前の照合

この入口はprivateな設定と報告を読み取り、条件の不一致を拒否する。
提供者への照会、secret登録、配備、契約変更は行わない。
合格は入力の照合結果であり、報告の出所や実環境の成功を保証しない。

## 実行

Cloudflare用の固定Node環境で、次の引数をすべて指定する。
`MEIBO_EXPECTED_CF_ACCOUNT_ID`は、planの選択先と別に確認して環境へ設定する。
入力3ファイルは本人だけが読める通常ファイルとし、秘密をGitへ置かない。

```sh
npm --silent --prefix cloudflare run preflight -- \
  --plan /private/plan.json --report /private/report.json \
  --secrets /private/secrets.json \
  --expected-account-env MEIBO_EXPECTED_CF_ACCOUNT_ID --kind fixture
```

`fixture`は合成試験、`live`は実測した報告の照合を表す。
どちらも出力の`deploymentAuthorized`は`false`であり、公開操作を許可するものではない。
配備側は直前の実照会・実行物から報告を作り、同じ`validatePreflight`を呼ぶ。
fixtureや保存済みの成功報告を配備の許可へ転用しない。
拒否時は固定codeだけを返し、URL・secret・アカウントID・入力内容を表示しない。

## Plan v1

全fieldを必須とし、未知fieldと重複したJSON keyを拒否する。
`remoteConfig`は[遠隔DBのCONFIG v1](hosting-remote.md)と同じ形式である。

| field | 内容 |
|---|---|
| `version` | `1` |
| `accountId` | 配備先として選択したCloudflare ID |
| `workerName` / `origin` | `meibo-beta`と対象のworkers.dev HTTPS origin |
| `sourceRevision` | Git commitの40桁、または凍結source snapshotのSHA-256 64桁 |
| `imageDigest` / `bundleSha256` | OCI image digestとWorker bundleのSHA-256 |
| `remoteConfig` | 現在使うcontrol/tenantの環境変数参照 |
| `restoreConfigs` | 復旧候補CONFIGの配列。未指定候補は空配列。上限8組 |
| `databases` | 下記のDB対応と追加容量・書込み予算 |

各DB対応は`key,id,additionalBytes,additionalWrites`を持つ。
`id`は提供者のDB識別で、数値は非負の安全な整数とする。
keyは`source:control`または`source:tenant:SLUG`、候補は`restore0:control`等とする。
全接続先とDB識別は重複させず、全source/候補を報告へ含める。
`secrets.json`は参照された環境変数名と実値のobjectとし、余分な値を渡さない。

## Report v1

top levelは`version,kind,checkedAtMs,planSha256,account,worker,turso,image,bundle`だけを持つ。
`kind`は実行引数と一致させ、`checkedAtMs`は採取時のUnixミリ秒とする。
古い報告と未来の時刻を拒否する。有効期限の値は検査実装で管理する。
`planSha256`は`planFingerprint(plan,secrets)`の結果で、入力変更後は再採取する。
このhashを含む報告自体もprivateに保管する。

| object | 必須fieldと確認内容 |
|---|---|
| `account` | `id,accessVerified,workersPaid,containersAllowed,workersSubdomain`。ID一致・実アクセス・契約/権限 |
| `worker` | `name,accountId,ownershipVerified,existingBindings`。選択先での所有、または未作成を確認 |
| `turso` | `organizationVerified,bytesUsed,bytesLimit,writesUsed,writesLimit,databases`。組織と共有容量/予算 |
| `image` | `os,architecture,digest,sourceRevision,bootVerified,nonRootUid,credentialsInImage`。linux/amd64・実起動・UID10001・秘密なし |
| `bundle` | `sha256,sourceRevision,buildVerified`。同じsourceの実bundle |

`existingBindings`各行は`name,type,providerAccepted`を持つ。
typeは`secret_text,plain_text,json,other`のいずれかで、提供者の現在の応答から作る。
更新対象の変数は置換して数え、既存の変数も最終数へ含める。
候補CONFIGは独立した上限を照合する。切替時は改めてsourceとして事前検査する。

TursoのDB行は`id,apiVerified,cliVerified,databaseTokenVerified,instances,cliUrl,bytesUsed,bytesLimit,writesUsed,writesLimit`を持つ。
`instances`は`role,hostname`を持つprimary1件だけとし、replicaや未知の役割を拒否する。
APIのhostname、CLIのinstance URL、設定URLを正規化して一致させる。
DBと共有枠の双方で、既存使用量とplanの追加予算が上限へ収まることを確認する。

## 提供者上限と検証範囲

[Workers上限](https://developers.cloudflare.com/workers/platform/limits/)のPaid128変数へ収める。
文書の5KBを越えないよう各実値/CONFIGを5000 bytesで事前拒否する。
[一括secret登録](https://developers.cloudflare.com/workers/configuration/secrets/)は一要求100件までとし、
結果の`secretBatchCount`に従い配備側で分割する。appのCONFIG上限を配備上限へ流用しない。
入力は各1MiB以内、通常ファイル・私有permission・strict JSONを要求する。
`npm --prefix cloudflare run check`は合成の成功/拒否・実CLI・既存部品を検証する。
これを実提供者への接続や公開成功の証拠とは扱わない。
