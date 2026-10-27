# Brief: cloudflare-hosting

## Problem
ソースは公開済みだが、ブラウザで使える招待制ベータは外部へ未配備。本人は事業用CloudflareとTurso Cloud、独自domain未定での進行を承認した。

## Current State
Rust tiny_httpとDocker、HTTPS/CSRF/Host/contextと組織分離の検証済み実装がある。永続Volumeを前提としている。remote-storageで正本DBを外部化する。

## Desired Outcome
明示した事業用Cloudflareアカウントにworkers.devのHTTPSで公開する。containerの停止・再作成後もTursoの認証/営業情報を保持し、2組織3権限と復旧を実証する。

## Approach
WorkerをHTTPS入口にし、Rust containerへ既存Host/Originを維持して転送する。公開版はremote-storageの接続を使い、空のローカルDBへfallbackしない。管理操作・backupが通常要求との排他を共有する経路を設計する。

## Scope
- In: Worker/Container設定、build、secret受渡し、公開ゲート、配備と合成の実環境検証、復旧手順。
- Out: DB adapter/業務SQL、既存Web UI再実装、signup/課金/外部ログイン、domain購入/変更、他サービス変更。

## Boundary Candidates
- cloudflare/: Workerとcontainer設定。既存Dockerは必要なcloud variantだけを足す。
- docs/hosting.md: 既存の自己ホスト手順を維持し、cloud手順を分割して追加する。

## Upstream / Downstream
- Upstream: remote-storage、team-access。
- Downstream: 独自domain、利用規模を広げる検討。

## Constraints
実データ・秘密・個人設定・アカウントIDをGitへ含めない。providerの認証/プランは実環境で確認する。未配備の構成を公開済みと報告しない。公開前と公開後に同じHost/Origin/未認証境界・DB保持を検査できるようにする。
