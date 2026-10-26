# Tasks: Cloudflare公開版

本人承認に基づく実装計画。remote-storageの完了を実際のcloud build/起動の前提とする。静的な設定の準備は先行できるが、未実装の依存や未実施の起動を成功扱いしない。新規の有料契約は承認範囲に含めず、公開ゲートで不足を示す。

- [ ] 1. 配備用runtimeと公開通信を準備する
- [ ] 1.1 実行物・依存・設定の土台を用意する
  - 固定したNode/Workers/Wranglerのmanifest、lock、型検査と試験入口を用意する。
  - cloud専用imageと起動処理を追加し、buildのpkg-config/libssl-dev、runtimeのlibssl3/ca-certificates、non-root、一時CONFIGと環境変数による秘密の参照を揃える。既存の自己ホスト構成は維持する。
  - Git/build contextから秘密を除外し、Linux/amd64のbuild・形式検査・実起動を通過したartifactを得る。
  - _Boundary: Runtime foundation, Cloud image（明示したsetup統合）_
  - _Depends: remote-storage_
  - _Requirements: 1.4, 3.1, 3.2, 4.3, 4.4_

- [ ] 1.2 公開先と要求の境界を維持する
  - 想定したHTTPS origin/Hostだけを受け、preview・不正な接続先・未知methodを処理前に拒否する。
  - Content-Lengthの有無によらず、逐次読取りでJSON1MiBとCSV import32MiBの上限を守る。
  - 検査済みの公開Host、Origin、cookieとCSRF/contextを内部HTTPへ維持し、forwarded headerを信頼しない。
  - spoofされた要求や過大本文は処理へ到達せず、正常な要求と保護されたcookie/CSPの契約が変わらないことを試験で示す。
  - _Boundary: Edge boundary_
  - _Requirements: 1.2, 1.3, 2.1, 2.2, 2.3, 2.4, 2.5_

- [ ] 1.3 起動・並行要求・通信失敗を制御する
  - 固定DO、readiness共有、待ち数/時間の上限と、明示したsecretだけのruntime受渡しを実装する。
  - queue待機中は本文を読まず、forward時だけ消費する。32MiB相当streamの待機中readが0回で、飽和時も先読みしないことを示す。
  - 要求を一度だけ転送し、応答本文完了まで順序を維持する。起動・forward・応答容量・休止の上限を試験する。
  - cold startの同時要求、秘密不足、DO再起動、timeoutと応答消失を試験し、更新の再送・強制停止・永続gateの自動解除が無いことを示す。
  - _Boundary: Container coordinator_
  - _Requirements: 2.4, 2.5, 3.1, 3.2, 3.3, 3.4, 3.5, 4.4_

- [ ] 2. 公開ゲートと実環境への統合を行う
- [ ] 2.1 事前検査と対象を固定した公開操作を用意する
  - 明示したアカウントと期待ID、公開URL、Containers契約/権限、Turso保存先/容量、秘密名と提供者の設定上限、amd64 artifactを検査する。
  - 別アカウント・不足した契約・不足した秘密をfixtureで拒否し、変更を始める前に理由を示す。既定先へのfallbackや新規契約を行わない。
  - 秘密は標準入力から登録し、実行物digest/Worker version/公開先と公開操作を対応付ける。private設定や秘密を通常ログへ出さない。
  - ローカルdry-runと検査済みの起動/復旧入口を揃え、条件が満たされれば対象だけへ公開できる実行経路を得る。
  - _Boundary: Publication gate_
  - _Depends: 1.1, 1.3_
  - _Requirements: 1.1, 2.4, 2.5, 4.1, 4.2, 4.3, 4.4, 5.1, 5.4_

- [ ] 2.2 同じ検査をローカルと公開先で実行する
  - remote-storage/team-accessの合成2組織3権限を使い、HTTPS入口・認証・CSRF/context・拒否・旧tabの試験を統合する。
  - 同一のcontrol/CRMについて、休止・再起動・container再作成後の保持を確認する。新しいDBへのbackup/restore後に再ログインし、件数と内容を照合する。
  - 結果不明の要求と占有gateで処理が拒否されること、default/cloudの品質検査とamd64 image/Worker bundleを確認する。
  - 実行物に対応した証拠を残す。本番の契約条件が不足する場合は未配備/未実施と記録し、公開後の検査を通過したとは報告しない。
  - _Boundary: Integration（Publication gate, Container coordinator, upstream runtime。業務SQLは変更しない）_
  - _Depends: 2.1, 1.3, remote-storage_
  - _Requirements: 1.2, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 3.4, 3.5, 5.1, 5.2, 5.3, 5.4_

## Review evidence
22要件を5実行タスクへ対応付けた。独立レビューで認証済み応答を禁じてしまう表現とqueueの本文先読みに関する不足を修正し、2回目のtask graph/final sanityはPASS。上流の保存仕様は承認済みのAPI・gate・復旧形式を参照する。
