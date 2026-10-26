# Brief: APIとMCP

## Problem
営業担当とAIクライアントが、画面と同じ検索・企業詳細・活動記録を自動処理から利用したい。

## Current State
Engineと画面専用APIは動作済み。互換性を説明した汎用APIとMCPの入口がない。

## Desired Outcome
版付きHTTP APIとローカルMCPを通じ、同じ入力制限・個人情報既定・保存結果で操作できる。

## Approach
既存の画面APIを再利用する薄いv1入口を作り、MCP stdioもこの入口を呼ぶ。HTTPの認証と組織選択はteam-accessへ委ねる。

## Scope
- In: 一覧・社名検索・企業詳細・活動追記、MCPの現行版と従来版の接続、入力上限と回帰試験。
- Out: 遠隔MCP HTTP、外部認証サービス、メール送信、LLM実行、自動retry、企業DB収集。

## Boundary Candidates
- Public API: 版付きの入力・応答契約と既存業務処理への変換。
- MCP: JSON-RPCのframing・version・toolsとstdioの所有。

## Upstream / Downstream
- Upstream: list-engine、web-ui、crm-essentials。real DB対応はpublic-data-importers、hosted認証はteam-access。
- Downstream: ローカルAIクライアントと認証済みHTTP利用者。

## Constraints
2026-10-26本人の継続実装・一般公開依頼を根拠に自律実行する。実企業データと認証値をコード・ログへ含めない。
