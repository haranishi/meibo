# Brief: remote-storage

## Problem
公開版をCloudflareへ配備するには、一時的なcontainer diskへ営業情報と認証状態を依存させられない。本人はTurso Cloud利用と既存の招待制ログインを承認した。

## Current State
coreはrusqliteの同期API、app/hostedはcontrolと組織別SQLite fileを使う。289試験成功のローカル動作を引き継ぐ。libSQL実serverで既存DDL・trigram・生成列・trigger・rollbackは成立した。PRAGMAの版/識別子更新は拒否される。

## Desired Outcome
cloud featureでTurso Cloud libSQLを正本として使い、認証・組織別DB・CRMの既存契約を維持する。通常buildは現在のrusqliteを直接使い、通信せず同じ保存形式・性能を維持する。

## Approach
optional meibo-db adapterを同期SQL境界に置き、業務SQLを共有する。remoteはHTTPのみでファイル複製を正本にしない。接続・transaction・結果不明の書込みを明示的に扱う。

## Scope
- In: adapter、core remote lifecycle、hosted remote control/tenant設定と運用、合成の実sqld回帰。
- Out: Worker/Container配備、課金、signup、外部OAuthログイン、新Rust製Turso engine、実企業DBの移動。

## Boundary Candidates
- crates/db: SQL/transport/transaction/row/valueのみ。業務・認証を知らない。
- core: 同じdomain SQLとbackend policy。app/hosted: 認証・許可された組織対応・管理/復旧。

## Upstream / Downstream
- Upstream: list-engine、team-access、現行SDK公式資料と制約。
- Downstream: cloudflare-hosting。

## Constraints
同じremote streamでBEGINから確定/rollbackまで実行する。書込みを自動再試行しない。URL/鍵/SQL本文を利用者向けerrorやlogへ含めない。無期限の待機・無制限の読取りbufferを避ける。filesystem/WAL/PRAGMA依存を黙って無効化せず保存方針を分ける。backup/復旧・組織分離・既存権限を検証する。
