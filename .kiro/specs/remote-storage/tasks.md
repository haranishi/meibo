# Implementation Plan

- [x] 1. SQL adapterの土台と接続
- [x] 1.1 optional crate・依存・実server/fault fixtureを用意する
  - meibo-dbとcloud feature、libsql-hrana =0.10.0、reqwest =0.13.5（defaultなし、json/native-tls-no-alpn）、共有Tokio runtimeを追加する。実sqld/fault試験は明示integration-tests featureでgateし、runtime単体試験は通常実行する。旧SDK/Hyper0.14をtest helper/proxyからも除き、fixture起動終了・MSRV・依存監査を確認する。
  - 既存no_network試験の全lockfile走査を、default app/coreのnormal/build cargo treeへ限定して直す。default featuresを無効化せず、禁止package一覧とdeny.tomlの検査を保つ。全workspace試験を通してから次へ進む。
  - _Boundary: Cargo manifests/lock, crates/db/lib, tests/support, core/tests/process/no_network.rs default graph assertion integration_
  - _Requirements: 1.1, 2.6, 5.4_
- [x] 1.2 SQLの値・引数・行変換を揃える
  - Value/ToSql/FromSql、Row.get、params、OptionalExtension、errorの既存使用範囲を実装し、i64端値・NULL・Blob・有限実数・変換失敗の試験を通す。
  - _Boundary: crates/db params, row, error_
  - _Requirements: 2.1, 2.6_
- [x] 1.3 手元ConnectionとStatementをforwardする
  - 設計1のlocal APIとTransactionをforwardし、path/PRAGMA/反復prepare/query/rollback/backupがrusqliteと同じ結果になる試験を通す。localの借用行とprivate raw cell bufferは取得列だけを変換し、未取得の不正UTF-8列と取得/未取得のInfをnative同様に扱う。paramsと公開from_valuesの有限実数制約は保つ。
  - _Boundary: crates/db local, statement, rows, row.rs private local buffering/borrowed conversion, local tests and primitives.rs native local Inf expectation_
  - _Requirements: 1.1, 2.1_
- [x] 1.4 外部Connectionと上限付きの受信を実装する
  - 公式型のv2/pipelineで上限付き応答を読み、HTTP1/redirectなし/retryなし/TLS1.2以上、schemeとbase_urlのorigin制限を実装する。成功・error bodyのchunkをVec/JSON確保前に制限し、過大応答・期限・別host・URL/秘密の診断漏れを実fixtureで拒否する。
  - 実用の非transaction RemoteConnectionを先に完成させ、同じstreamでforeign_keysの有効化・照合に成功してから業務SQLを送り、streamを閉じる。公式型decode前に構造と作業領域を検査し、正当な500行ページを維持しつつ小さなJSONの過剰なVec確保を拒否する。共有Connectionへの統合は1.5で行う。
  - _Boundary: crates/db remote, statement, rows, row.rs private remote-only diagnostic redaction, remote tests/support, direct serde manifest edge/lock_
  - _Requirements: 2.1, 2.6_
- [x] 1.5 同streamのtransactionと結果不明を扱う
  - 元Connection経由を含む同batonのtransaction、commit/rollback/Drop/lastrowidを共有する。SequenceでSQL全文/triggerを分割せず実行し、v2 GetAutocommitを使わない。commit後の応答を落としOutcomeUnknownと再使用/再送拒否を確認する。
  - _Boundary: crates/db Connection local/remote backend integration, transaction, remote state, Statement private readonly metadata, error classification/adapter exports, fault tests_
  - _Requirements: 2.2, 2.3, 2.4, 2.6_

- [ ] 2. Coreの保存方針を分ける
- [x] 2.1 (P) SQL aliasを既存coreへ接続する
  - 内部試験を含むSQL参照をaliasへ寄せる。defaultの直接rusqlite経路と公開Engine APIを保ち、default試験とcloudの手元CLIをcompileする。
  - _Boundary: core lib, SQL imports and tests_
  - _Depends: 1.3_
  - _Requirements: 1.1, 5.4_
- [x] 2.2 remoteの初期化と識別を追加する
  - Engine create_remote/open_remoteとmarker/版/DB識別/種別を追加する。実sqldの空DBだけを初期化し、欠落・未来版・不一致・既存内容を拒否し、手元へfallbackしないことを確認する。
  - integration-onlyのremote_lifecycle.rsはCargo required-featuresとcrate-level cfgの両方で囲む。defaultのno_network source scanは、この正確なfileの二重gateとfeature設定を先に検査して除外し、他file・禁止symbol・default依存graph・deny検査を保つ。
  - _Boundary: core engine, schema/remote, extension version constant visibility/reuse, cloud-only Engine exports and explicit real-server integration test setup/no_network exact fixture gate_
  - _Depends: 2.1, 1.5_
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 5.4_
- [ ] 2.3 remoteの更新・snapshot方針へ業務処理を接続する
  - fs/WALに依存しないremote transactionを使い、CRUD・取消・件数・索引・独自項目・一括処理・論理消去とtombstoneの実sqld回帰、制約失敗時の全取消を通す。
  - _Boundary: core schema/write policy and domain regression_
  - _Requirements: 2.1, 2.2, 2.3, 5.1_
- [x] 2.4 (P) 型付きsnapshotと既知schemaの入出力を用意する
  - CRMのtable/column/key許可表、snapshot-format v1の共有codec、既知AUTOINCREMENT high-waterを実装する。未知sequence・欠落・負値・最大id未満・overflowを拒否する試験を通す。control許可表はappの4.1が所有する。
  - _Boundary: core remote_snapshot, cloud-only module registration and optional codec dependency setup_
  - _Depends: 2.1_
  - _Requirements: 2.6, 4.2_
- [ ] 2.5 全件scanとremote保守を接続する
  - verify/CSV/snapshotを500行以下のkeysetページで処理し、ページ境界を越えて抜け・重複がないことを確認する。手元compact/WAL/file backupを保ち、remoteは対応外を明示する。
  - _Boundary: core scans, erase/engine maintenance, snapshot reader_
  - _Depends: 2.3, 2.4_
  - _Requirements: 2.1, 2.6, 5.1, 5.2_

- [ ] 3. Hostedの組織対応と共有排他
- [ ] 3.1 remote controlと認証・設定を接続する
  - CONFIG版/env参照/slugとDB識別を実装し、storage層の2組織3権限・session失効・再起動試験を通す。不正な版、1000組織超、秘密の欠落、重複/違うDBを拒否する。serveは3.3まで有効化しない。
  - _Boundary: app hosted remote/store/auth, app manifest_
  - _Depends: 2.3_
  - _Requirements: 1.2, 1.3, 1.4, 3.1, 3.2_
- [ ] 3.2 非失効gateの取得・所有照合・復旧を実装する
  - 永続gateを条件付きで取得/解除する。別processの競合、panic/再起動/取得応答消失で占有を維持し、owner違いの解除と自動takeoverを拒否する。以前の処理終了を指定した復旧を監査する。
  - _Boundary: app hosted remote_gate and storage tests_
  - _Depends: 1.5, 3.1_
  - _Requirements: 2.5, 3.3, 3.4, 3.5_
- [ ] 3.3 HTTPとremote CLIを統合する
  - hosted-remote CONFIGの全操作とHTTPが同じgate/認証境界を通るよう接続する。Replyへ変換された障害も接続poisonから確認し、503/outcome_unknownを返す。password-stdinと通常hostedの回帰を通す。
  - _Boundary: app main, hosted cli/http/remote, stable error response integration_
  - _Requirements: 2.4, 2.5, 3.1, 3.3_

- [ ] 4. 保存・復旧と全体の検証
- [ ] 4.1 gate中に型付き保存と目録検証を行う
  - privateな新規directoryへ全DBの最新行を逐次保存し、件数/hash/形式を検証する。sessions/gate/秘密を除き、symlink・不正行・未知sequenceを拒否する実2組織試験を通す。
  - _Boundary: app hosted remote_backup creation/validation_
  - _Depends: 2.5, 3.3_
  - _Requirements: 4.1, 4.2_
- [ ] 4.2 新しい空DB一式へ検証して復旧する
  - 全targetを先に検査し、restoring→ready、instance/DB対応の再発行、sequence復元、FTS/fold/件数/FK照合を実装する。最大field IDの再利用、改変、既存target、中断を試し、失敗時はsourceを保ってcandidateをofflineにする。
  - _Boundary: app hosted remote_backup restore and tests_
  - _Requirements: 4.3, 4.4, 4.5_
- [ ] 4.3 process・CLI・browserの回帰を確認する
  - 実sqld/fault proxyで2組織3権限のlogin/更新/logout/旧tab、再起動保持、占有gateの復旧、保存先変更後の再loginを確認し、秘密が診断へ出ない証拠を残す。
  - _Boundary: app/tests remote_hosted and browser fixtures, db fault tests integration_
  - _Requirements: 5.3_
- [ ] 4.4 公開版を継続検証する入口とCIを接続する
  - 合成serverを起動するcloud test/build/lint/CIがintegration-tests featureを明示実行するよう接続する。default-localの通信禁止bansを保つgraphとcloud用監査policyを分け、双方のadvisory/license/source門、default check-all/MSRVとcloud検査を通す。未実施の提供者環境確認を合格扱いにしない。
  - _Boundary: xtask and CI validation integration_
  - _Requirements: 1.1, 5.4_

## Implementation Notes
旧libsql SDK/TLS connector案は依存監査不合格で置換済み。公式Hrana型+現行reqwestの組合せはRust1.88/online advisories/licenses/sourcesを通過した。1.4はchunk受信中の上限を試験し、Rowsは単一の上限付き応答だけを保持する。大量scanは2.5でページ化する。Cloudflare版のLinux build/runtimeにはOpenSSLとCAを配備側が用意する。
1.1のdefault graph試験修正はoptional packageがlockfileへ載ることと通常版へlinkされることを区別する。4.4の監査policy分離は別タスクであり、1.1で禁止packageや既存policyを緩めない。
1.3のrow.rsとprimitives.rsの変更は既存の列選択とlocal変換の互換性を保つ範囲である。query/query_row/query_mapの未取得の不正列とlocalのInf読取りをnativeと照合し、params/公開from_valuesの拒否試験を残す。
2.1は完成済みlocal adapterのalias接続として1.5と独立worktreeで並行できる。sqlite_versionは同梱rusqliteの直接呼出しを保ち、統合後にConnectionのSendとdefault/cloudの手元経路を再検証する。remote初期化2.2は両方の完了を待つ。
