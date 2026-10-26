# Tasks: Cloudflare公開版

本人承認に基づく実装計画。1.1は静的なruntime準備、1.2/1.3はそれを使う独立したWorker部品の単体検査であり、remote-storageと並行して進められる。公開前検査の実行部品は2.1で先行できる。実HTTP入口の結合とLinux/amd64のbuild・inspect・実起動は、remote-storageとpublic-entry完了後の2.2で行う。準備済みの設定や単体試験をアプリ起動済みと扱わない。新規の有料契約は承認範囲に含めず、公開ゲートで不足を示す。

- [x] 1. 配備用runtimeと公開通信を準備する
- [x] 1.1 依存・型契約・設定の土台を用意する
  - 固定したNode/Workers/Wranglerのmanifest、lock、実際のWorker部品が共有するEnv/要求の型契約、型検査と試験入口を用意する。仮のHTTP応答や未実装アプリの代用は作らない。
  - cloud専用imageと起動処理を追加し、buildのpkg-config/libssl-dev、runtimeのlibssl3/ca-certificates、non-root、一時CONFIGと環境変数による秘密の参照を揃える。既存の自己ホスト構成は維持する。
  - Git/build contextから秘密を除外し、設定schema・型契約・shell構文の検査を通す。Dockerfileの準備をimageのbuild/起動成功とは記録しない。
  - _Boundary: Runtime foundation, Cloud image（明示したsetup統合）_
  - _Requirements: 1.4, 3.2, 4.4_

- [x] 1.2 (P) 公開先と要求の境界を維持する
  - 想定したHTTPS origin/Hostだけを受け、preview・不正な接続先・未知methodを処理前に拒否する。
  - Content-Lengthの有無によらず、逐次読取りでJSON1MiBとCSV import32MiBの上限を守る。
  - 検査済みの公開Host、Origin、cookieとCSRF/contextを内部HTTPへ維持し、forwarded headerを信頼しない。
  - 実HTTP入口へ組み込む境界部品の単体試験で、spoofされた要求や過大本文を拒否し、正常な要求とcookie/CSPの転送を維持することを示す。結合した入口は2.2で用意する。
  - _Boundary: Edge boundary_
  - _Depends: 1.1_
  - _Requirements: 1.2, 1.3, 2.1, 2.2, 2.3, 2.4, 2.5_

- [x] 1.3 (P) 起動・並行要求・通信失敗を制御する
  - 固定DO、readiness共有、待ち数/時間の上限と、明示したsecretだけのruntime受渡しを実装する。
  - queue待機中は本文を読まず、forward時だけ消費する。32MiB相当streamの待機中readが0回で、飽和時も先読みしないことを示す。
  - 要求を一度だけ転送し、応答本文完了まで順序を維持する。起動・forward・応答容量・休止の上限を試験する。
  - runtime依存を注入する単体試験でcold startの同時要求、秘密不足、DO再起動、timeoutと応答消失を再現し、更新の再送・強制停止・永続gateの自動解除が無いことを示す。実HTTP入口への結合や本物のContainer起動を前提にしない。
  - _Boundary: Container coordinator_
  - _Depends: 1.1_
  - _Requirements: 2.4, 2.5, 3.1, 3.2, 3.3, 3.4, 3.5, 4.4_

- [ ] 2. 公開ゲートと実環境への統合を行う
- [x] 2.1 (P) 公開前検査を独立した実行部品として完成させる
  - private planと提供者/実行物reportを厳密に検査するCLIを実装する。期待account ID、公開URL、契約/権限、容量、全control/tenant/復旧先のprimary一致、secret参照名と提供者上限、実行物との対応を検査し、不明・欠落・不一致を拒否する。
  - fixtureによるdry-runと拒否試験を通す。この段階では提供者API呼出・秘密登録・配備・契約変更を行わず、fixture合格を実環境確認や公開許可と記録しない。
  - _Boundary: publication preflight CLI/validator, package実行入口, tests and private-plan instructions_
  - _Depends: 1.1, 1.2, 1.3_
  - _Requirements: 1.1, 2.4, 2.5, 4.1, 4.2, 5.4_

- [ ] 2.2 実Workerと実行物を結合し、検査した対象へ公開する
  - boundary/coordinatorを実Worker/DO exportへ結合し、Linux/amd64 imageのbuild・inspect・実起動、Worker bundle、起動/復旧入口を検証する。
  - 最新の提供者照会と実artifactから得た結果を2.1の同じ検査実装へ渡し、対象・設定・digestを対応付ける。fixture、未検証、変更後の古い結果では公開しない。
  - 条件成立後に指定accountだけへ秘密を標準入力で登録し、配備対象のimage digest/Worker version/公開先を記録する。既定先へのfallbackや新規契約は行わない。
  - _Boundary: Worker entry, actual provider/artifact collection, deploy integration and evidence_
  - _Depends: 2.1, remote-storage, public-entry_
  - _Requirements: 1.1, 2.4, 2.5, 4.1, 4.2, 4.3, 4.4, 5.1, 5.4_

- [ ] 2.3 同じ検査をローカルと公開先で実行する
  - remote-storage/team-accessの合成2組織3権限を使い、HTTPS入口・認証・CSRF/context・拒否・旧tabの試験を統合する。
  - 同一のcontrol/CRMについて、休止・再起動・container再作成後の保持を確認する。新しいDBへのbackup/restore後に再ログインし、件数と内容を照合する。
  - 結果不明の要求と占有gateで処理が拒否されること、default/cloudの品質検査とamd64 image/Worker bundleを確認する。
  - 実行物に対応した証拠を残す。本番の契約条件が不足する場合は未配備/未実施と記録し、公開後の検査を通過したとは報告しない。
  - _Boundary: Integration（Publication gate, Container coordinator, upstream runtime。業務SQLは変更しない）_
  - _Depends: 2.2, 1.3, remote-storage_
  - _Requirements: 1.2, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 3.4, 3.5, 5.1, 5.2, 5.3, 5.4_

## Review evidence
22要件を6実行タスクへ対応付けた。静的準備・独立部品の単体検査と、保存実装を要するbuild/起動/公開を分け、依存順の修正と、実行可能な事前検査を先行させる分割を独立レビューでAPPROVEDと確認した。上流の保存仕様は承認済みのAPI・gate・復旧形式を参照する。
