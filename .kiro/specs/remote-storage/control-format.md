# Remote control保存形式 v1

## 識別と共有schema
既存hostedの利用者・所属・session・監査SQLを一つのcontrol実装で共有する。
手元版のpath/lock/header設定は手元Storeが所有し、remoteに適用しない。
remoteの初期化は空DBだけに行い、metaに次の必須値を保持する。

| key | 値 |
|---|---|
| remote_format | `meibo-control-remote-v1` |
| schema_version | `1` |
| database_kind | `control` |
| database_id | 新しい不透明な識別子 |
| instance | 既存形式の新しいサービス識別子 |
| state | 利用開始時は`ready`、復旧途中は`restoring` |
| dummy_hash | 既存の制限内のArgon2id password hash |

openは必須値と準備状態を検査し、欠落した値を補わない。
CRM DBまたは別形式のcontrolとして開かず、登録済みtenantをCoreのmarkerと照合する。

## tenant登録
`remote_tenants(slug TEXT PRIMARY KEY REFERENCES tenants(slug), database_id TEXT NOT NULL UNIQUE)`を追加する。
`tenants`と`remote_tenants`への登録は一つのtransactionで確定する。
CONFIGには未登録の空tenant接続先も置けるが、所属やHTTPから利用できるのは登録済みtenantだけである。
storage APIはtenant作成と登録を分け、作成結果の識別子とEngineを返す。
3.3のdemo投入はそのEngineへ行い、成功してから検証済み識別子を登録する。
空tenant追加の便宜APIも同じ作成・登録手順を使い、対応を直接挿入する別経路を作らない。
