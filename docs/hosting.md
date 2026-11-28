# 招待制ベータを運用する

hostedは無料・招待制の初期構成です。利用者/所属は保守者CLIで作り、各組織を別のSQLiteへ保存します。公開signup、メール送信、課金、SSO/MFAは含みません。ソースの公開だけでは、このサービスが配備されたことにはなりません。

## 初期化と起動

Docker Composeを使える永続サーバーと、保守するdomainを用意します。DNSをサーバーへ向け、外部から80/443へ到達できる状態にします。[Caddyの条件](https://caddyserver.com/docs/automatic-https)を満たすとHTTPSが有効になります。

```sh
export MEIBO_DOMAIN=crm.example.com
docker compose build app
docker compose run --rm app hosted /data/service init
docker compose run --rm app hosted /data/service tenant-add example --origin real
python3 -c 'import getpass,sys;sys.stdout.write(getpass.getpass("Password: "))' | docker compose run --rm -T app hosted /data/service user-add admin --password-stdin
docker compose run --rm app hosted /data/service member-set admin example admin
docker compose up -d
```

passwordは12〜1024bytesです。引数や設定ファイルへ書かず、上の例では端末の非表示入力から標準入力へ渡します。組織ID・利用者名・passwordを招待した相手へ安全に渡してください。

SQLite版では、`/auth/login` への大量の要求が続くと、正しいパスワードでも429が返ることがあります。要求が収まれば数秒で再試行できます。よくあるパスワードを多くの利用者名で試す攻撃は、最初の100件の後も毎秒5件まで続けられます。長く推測されにくいパスワードを使ってください。

`MEIBO_DOMAIN`のdomainで開くとlogin画面へ進みます。アプリportをホストへ公開しないでください。`meibo_data`が認証/組織DB、`caddy_data`が証明書と鍵、`caddy_config`が前段設定の永続Volumeです。[Docker Volumes](https://docs.docker.com/engine/storage/volumes/)の対象を保持して更新します。

## 権限と失効

viewerは参照、editorは営業データの更新、adminは個人情報消去と組織監査も利用できます。所属の変更は保守者CLIだけで行います。

```sh
docker compose run --rm app hosted /data/service member-set USER example viewer
docker compose run --rm app hosted /data/service revoke USER
docker compose run --rm app hosted /data/service user-disable USER
docker compose run --rm app hosted /data/service audit example
```

`member-set USER SLUG none`は所属を外します。password変更・所属変更・利用停止は既存sessionを失効させます。password変更は`password-set USER --password-stdin`です。sessionは絶対8時間・無操作30分で失効します。監査は本文・password・session値を記録しません。

## 保存と復旧を確かめる

```sh
docker compose run --rm app hosted /data/service backup /data/snapshot-001
```

成功すると `backed-up manifest-sha256=<64桁の指紋>` が1行表示されます。指紋は保存物と別の場所に、保存先の名前と一緒に控えてください。復元時はその控えから64桁を入力します。

```sh
read -r manifest_sha256
docker compose run --rm app hosted /data/restored-001 restore /data/snapshot-001 --manifest-sha256 "$manifest_sha256"
```

backup先と復元先には未存在の名前を使い、復元先は保存物の外に置きます。backupは共通保守lockを取ってSQLiteをオンライン複製し、整合性を検査します。その間のWeb要求は503になることがあります。復元時には複写先の全sessionを失効させます。復元コマンドが成功すると、検査と同期を終えたROOTを使えます。

復元を強制終了すると、復元先の隣に `.meibo-restore-*` が残ることがあります。復元処理が動いていないことを確かめてから、残った作業フォルダを削除できます。

復旧後は`MEIBO_ROOT=/data/restored-001 docker compose up -d app`で新ROOTへ切り替え、再ログインして各組織の企業/活動/商談を照合します。切替後の再起動でも内容が残ることを確認してください。元ROOTとsnapshotは照合が済むまで保持します。

Volume内のsnapshotだけではサーバー消失に備えられません。保守者が暗号化した別保管先へsnapshotを複製し、定期的に新ROOTへ復旧して照合します。DBやsnapshotをGitや公開配信ディレクトリへ置かないでください。

## 手元でHTTPを確認する

```sh
meibo hosted LOCAL_ROOT init
meibo hosted LOCAL_ROOT tenant-add trial --origin synthetic --demo-rows 100
meibo hosted LOCAL_ROOT serve --origin http://127.0.0.1:4319 --bind 127.0.0.1:4319 --allow-http-loopback
```

HTTPはこの明示したloopback試験だけで許可します。本番はSecure cookieと設定したHTTPS originを必須とし、Forwarded headerから組織や信頼先を決めません。既存ローカル`serve`とは保存先を分けてください。

## 公開前に実環境で確認すること

HTTPS到達、未認証APIの拒否、2組織/3権限、同一browserの利用者/組織切替、旧tab/戻る、logout後の要求拒否、Volumeを使った再起動と復旧を確認します。組織別のブラウザ保存は認証後にだけ初期化します。初回は単一プロセスの構成で、処理量を測って拡張してください。

利用者が入力する個人情報の取扱方針、運用窓口、障害時の復旧担当と保存期間は、実際の提供者が公開前に定めます。未配備のTLS/DNS/Volumeは検証済みとは扱いません。

Cloudflare向けの準備と公開ゲートは[配備手順](hosting-cloudflare.md)を参照してください。

[Cloudflareアカウントでログインする手順](hosting-cloudflare-access.md)。

自分用に Cloud Run と Turso で公開する場合は[配備と管理の道具](hosting-cloud-run.md)と
[運用と緊急時の手順](cloud-run-operations.md)を参照してください。
