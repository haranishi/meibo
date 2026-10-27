# 招待制サービスのリモート保存

公開版のcontrolと組織DBをTurso CloudのlibSQL engineへ保存する。
Cloud featureを選んでビルドする。

```sh
cargo build --locked --release -p meibo-app --features cloud
```

controlと各組織に未使用のDBを用意する。初期の対応範囲は、primaryが1つ、
replicaが0の配置である。提供者APIの役割・hostnameとCLIのinstance URLが
一致することを確認し、全管理操作とサーバーで同じ設定を使う。
配置や接続先を変えるときは旧処理を終了し、改めて照合する。

CONFIGはversionと環境変数名だけを持つ。接続先とDB限定tokenは秘密管理から
環境変数へ渡す。CONFIGを専用ファイルへ保存し、権限を0600にする。

```json
{"version":1,"control":{"url_env":"MEIBO_DB_CONTROL_URL","token_env":"MEIBO_DB_CONTROL_TOKEN"},"tenants":{"alpha":{"url_env":"MEIBO_DB_ALPHA_URL","token_env":"MEIBO_DB_ALPHA_TOKEN"}}}
```

初期化と招待は管理CLIで行う。passwordは12〜1024bytesの1行を標準入力から
渡す。合成データを入れる場合は`--origin synthetic`を明示する。

```sh
meibo hosted-remote /private/remote.json init
meibo hosted-remote /private/remote.json tenant-add alpha --origin synthetic --demo-rows 100
meibo hosted-remote /private/remote.json user-add operator --password-stdin
meibo hosted-remote /private/remote.json member-set operator alpha admin
meibo hosted-remote /private/remote.json serve --origin https://crm.example.test --bind 0.0.0.0:4317
```

HTTPS前段から公開Host・Origin・Cookie・CSRF/contextを保持して転送する。
利用者は既存のログイン画面へ進み、所属組織の権限で操作する。
password変更、利用停止、所属変更、revokeは既存sessionを失効させる。
`sync-nta SLUG CATALOG [--limit COUNT]`は指定組織のreal DBへ許可項目を取り込む。
`audit [SLUG]`は最大200件の運用監査を表示する。

`backup NEW_DIRECTORY`は共通gateを保持し、認証DBと登録済みの全組織DBを
新しい専用ディレクトリへ保存する。既存ディレクトリの上書きはしない。
保存した全行の型・件数・SHA-256を検証してから`backed-up`を返す。
保存物にはpassword hashが含まれるため、秘密管理された場所で保管する。
session・gate・接続URL・token・CONFIGの環境変数名は含まれない。

```sh
meibo hosted-remote /private/remote.json backup /private/backups/service-20261027
```

復旧では元CONFIGと新しいtarget CONFIGを明示する。targetには保存された組織と
同じslugを設定し、controlを含む全接続先に未使用の空DBを割り当てる。
元・先の接続先重複、改変された保存物、1つでも空でないtargetを拒否する。

```sh
meibo hosted-remote /private/remote.json restore /private/backups/service-20261027 --target-config /private/restored.json
```

全DBの行・検索索引・件数・活動状態を照合してから利用可能にする。
新しいサービス/DB識別を発行し、旧sessionは引き継がない。復旧後は再ログインする。
途中失敗の候補は`restoring`のまま利用を拒否し、元のDBを保持する。
復旧は設定や稼働先を自動で切り替えない。成功したtarget CONFIGへ運用を切り替える。

Webと通常の管理操作は、controlの共通gateで直列化する。
`outcome_unknown`では確定を確認できないため、再送せず管理者へ連絡する。
管理者は以前のプロセスと進行中の処理を終了してから状態を確認する。

```sh
meibo hosted-remote /private/remote.json gate-status
meibo hosted-remote /private/remote.json gate-recover OWNER --prior-work-terminated
```

時刻だけで占有を解除しない。解除応答の消失ではDELETE済みの可能性もあるため、
新しい接続の`gate-status`で確認する。再起動は占有を自動解除しない。
Cloudflareでの配備と提供者環境の検証は、それぞれの公開ゲートで記録する。

## 合成ブラウザ検証

`crates/app/tests/remote-hosted-browser.mjs`はSQLD6個とHTTPS前段を一時起動し、
2組織3権限・旧tab・再起動・CLI中断時のgate保持/復旧・新DBへの復元を確認する。
Playwrightを導入した環境で、`integration-tests` featureのappをbuildし、
`MEIBO_BINARY`・`MEIBO_SQLD_BINARY`・`PLAYWRIGHT_MODULE`へ絶対pathを指定して
`node crates/app/tests/remote-hosted-browser.mjs`を実行する。`TMPDIR`にはsymlinkを含まない
既存の一時directoryを指定する。結果先は`MEIBO_BROWSER_OUTPUT`で指定できる。
実企業データは使用せず、生成証明書の例外は当該browser contextだけに限定する。
検証用DB前段には小さな通信遅延を置き、Mac上の一時port枯渇を避ける。SQLの再送はしない。
この結果はCloudflare/Tursoの提供環境や実端末での検証を示すものではない。

[Cloudflareアカウントでログインする手順](hosting-cloudflare-access.md)。
