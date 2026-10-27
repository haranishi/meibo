# 実装上の補足

旧libsql SDK/TLS connector案は依存監査不合格で置換済み。公式Hrana型と現行reqwestはRust1.88/online advisories/licenses/sourcesを通過した。1.4はchunk受信中の上限を試験し、Rowsは単一の上限付き応答だけを保持する。大量scanは2.5でページ化する。Cloudflare版のLinux build/runtimeにはOpenSSLとCAを配備側が用意する。

1.1のdefault graph試験修正はoptional packageがlockfileへ載ることと通常版へlinkされることを区別する。4.4の監査policy分離は別タスクであり、1.1で禁止packageや既存policyを緩めない。

1.3のrow.rsとprimitives.rsは既存の列選択とlocal変換の互換性を保つ範囲である。query/query_row/query_mapの未取得の不正列とlocalのInf読取りをnativeと照合し、params/公開from_valuesの拒否試験を残す。

2.1は完成済みlocal adapterのalias接続として1.5と独立worktreeで並行できる。sqlite_versionは同梱rusqliteの直接呼出しを保ち、統合後にConnectionのSendとdefault/cloudの手元経路を再検証する。remote初期化2.2は両方の完了を待つ。

2.3のfault fixtureはDB側の既存helperを再利用する。必要なTokio・reqwest=0.13.5（defaultなし、既存TLS/json）・libsql-hrana=0.10.0の既存lock内edgeはintegration-testsだけで有効にし、default graphと禁止symbol検査を保つ。

4.1はsource `ee237b2`（統合`a016c02`）を独立レビューで受け入れた。13ファイルのSHA照合、app default/cloud/integration 59/69/105件、strict Clippy、実Rust 1.88両profile、通信禁止guard、リンク、秘密検査を確認した。CLIのproduction OFFによるREDとGREENの記録はあるが、RED時の旧source archiveはなくtool記録で追跡した。復旧・全体process/browser・提供者環境の確認は4.2/4.3と配備仕様に残る。

4.2は11ファイルのSHAと独立した親レビュー、app全構成59/70/112件・strict Clippy/fmt・実Rust 1.88等で受け入れた。ホストのagent thread上限によりkiro-implの明示fallbackを使った。初回の既存CLI通信失敗は原因未確定のまま記録し、単独の全件実行では再現しなかった。Macの一時pathはsymlinkを含むため、厳密な保存先検査の試験には実体のあるSSD TMPDIRを明示する。

4.3の実SQLD/HTTPS/browser全13手順と2組織3権限の復旧後ログイン、全型付き表照合を確認した。短時間の大量通信でMacの一時portが枯渇したため、試験用の前段だけに遅延を置き、再送・本体・OS設定を変えず確認した。最終check-all331件/check-cloud479件（各既存測定3件除外）、実Rust 1.88両構成、依存監査とREUSEを通過した。配備は隣接仕様に残る。
