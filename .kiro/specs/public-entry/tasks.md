# Implementation Plan

- [x] 1. 公開ログイン入口を仕上げる
- [x] 1.1 (P) 公開metadataとサービスURL共有を接続する
  - 既存の公開loginへcanonical/OGP/汎用画像とX・LINE・コピー・共有シートを追加し、安全なURL固定、既存境界、3幅とkeyboardを実HTTP/browserで確認する。
  - _Boundary: app hosted public_entry/http/module registration, login HTML/CSS, same-origin share script/image assets, tests and short verification instructions_
  - _Depends: remote-storage 3.3_
  - _Requirements: 1.1, 1.2, 2.1, 2.2, 2.3, 3.1, 3.2_

## Verification
独立レビューAPPROVED。11ファイルのSHA、app default/cloud/integration 62/68/100件、strict Clippy、実Rust/Cargo 1.88の3構成、通信禁止guard、リンク、限定秘密検査を確認。実HTTPS/browserで3幅・keyboard・copy・API模擬・login・PNG decodeを通過した。元REDの新規module archiveはなくtool履歴で追跡。実端末の共有シートと公開SNSプレビューは未確認。
