# Goal

- Web 画面で生成された `book.md` を、見やすい Markdown プレビューとして表示する。
- ` ```mermaid ` コードブロックを図としてレンダリングする。

Non-goals:

- Markdown の編集 UI や mdBook の完全再現（テーマ/検索/ナビ）まではやらない。
- `book.md` の生成ロジック自体（サーバ側の Markdown 内容）は変更しない。

# Scope

- 変更対象（予定）
  - `web/src/App.tsx`（UI を「Raw + Preview」表示に拡張）
  - `web/src/style.css`（Markdown 表示用スタイル追加）
  - `web/package.json` / `web/package-lock.json`（Markdown/Mermaid 依存追加）
  - 必要なら `web/src/components/*`（プレビュー専用コンポーネント）
- 変更しないもの
  - gRPC / API スキーマ（`proto/`）
  - ジョブ実行/保存（`sitebookify-app` 側）

# Milestones

1. Markdown プレビューを追加する（GFM 対応: 見出し/表/リスト/コードが崩れない）。
2. `language=mermaid` のコードブロックを SVG に変換して表示する（失敗時はコード表示にフォールバック）。
3. ダークテーマで読みやすいスタイル調整（行間・見出し・コード・表・画像・Mermaid の幅）。
4. 動作確認: `book.md` 取得 → Web 表示（Preview）→ `curl` で API を確認。

# Tests

- Web build が通る: `cd web && npm run build`
- Web MVP（ローカル）を起動して表示確認:
  - `just dev_app`（API）
  - `just web_install && just web_gen && just web_dev`（Web）
- API 確認（例）:
  - `curl -fsS http://127.0.0.1:8080/healthz`
  - `curl -fsS http://127.0.0.1:8080/jobs/<jobId>/book.md | head`

# Decisions / Risks

- Markdown レンダラは `react-markdown` + `remark-gfm` を採用する（Raw HTML は既定で無効）。
- Mermaid はクライアント側で `mermaid` を使ってレンダリングする（XSS/壊れた図のリスク → try/catch でフォールバック）。
- 大きい `book.md` での描画コストが上がる可能性（対策: Mermaid のみ遅延レンダリング、UI は Raw/Preview 切替可能にする）。

# Progress

- 2026-02-02: ExecPlan 作成、現状調査開始。
- 2026-02-02: Web に Markdown プレビュー（GFM）と Mermaid レンダリングを追加、`web` の build まで確認。
- 2026-02-02: `sitebookify-app` を 18080 で起動し、`curl` で `/healthz` を確認。
- 2026-02-02: `/jobs/<id>/book.md` も確認（8080 はローカルで利用中）。
- 2026-02-02: `web/src/MarkdownPreview.tsx` の build エラーを修正し、`just web_build` が通ることを確認。
