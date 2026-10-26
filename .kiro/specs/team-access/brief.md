# Brief: team-access

## Problem
ローカルの合成デモを、招待した複数組織が利用する無料ベータへ進める。
現在は認証・組織分離・権限・監査・公開運用と復旧の仕組みがない。

## Desired Outcome
利用者は所属組織を指定してログインし、その組織のCRMだけを使える。
保守者はCLIでユーザー・組織・権限を管理し、保存内容を復旧できる。

## Approach
既存tiny_httpとEngineを再利用し、認証用control DBと組織ごとのCRM DBを分ける。
Webのsessionはサーバー管理とし、HTTPS前段と永続Volumeを備える。
共有DBへtenant列を追加する方式は既存検索の全経路を変更するため採らない。

## Scope
- In: CLI管理、password認証、session、viewer/editor/admin、監査、ログアウト、保存と復旧、Docker構成。
- Out: 課金、公開signup、メール送信、SSO/MFA、API token、企業データの取得と変換。

## Upstream / Downstream
- Upstream: list-engine、web-ui、crm-essentials、親工程による実DBのCRM対応。
- Downstream: 認証後のHTTP API、招待制SaaS試用。

## Constraints
2026-10-26の本人指示を要件・設計・実装・公開準備の自律進行の根拠とする。
コード一般公開と、実際のインフラへSaaSを配備して到達確認する工程は区別する。
秘密、認証DB、業務DB、backupをリポジトリへ含めない。
