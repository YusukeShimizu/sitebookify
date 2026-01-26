# ExecPlan: Book bundle → LLM translate → Export

## Goal

- `sitebookify book render` の出力（mdBook）から、全章を 1 つの Markdown ファイルに統合して出力できるようにする。
- 統合 Markdown を任意言語へ LLM で翻訳できるようにする（実装は外部コマンド連携を正とする）。
- 翻訳後 Markdown を `epub` / `pdf` 等へ変換できるようにする（外部ツール `pandoc` を利用する）。

### 非目的

- LLM プロバイダ（OpenAI 等）へ直接 API 接続する実装は、この計画では行わない。
- 既存の `crawl` / `extract` / `manifest` / `toc` / `book render` の仕様変更は行わない。
- TOC の自動整形や「教科書化（演習生成等）」は行わない。

## Scope

### 変更対象

- CLI: `book bundle`, `llm translate`, `export` の追加
  - `src/cli.rs`, `src/main.rs`
- 実装: bundle/translate/export の処理追加
  - `src/book.rs`（bundle 追加）または新規モジュール
  - `src/llm.rs`（translate 追加）
  - `src/export.rs`（pandoc 呼び出し）
  - `src/lib.rs`
- 代表操作の Integration Test 追加/拡張
  - `tests/sitebookify_pipeline.rs`
- 開発環境（Nix）に外部ツールを追加
  - `flake.nix`（`pandoc`、PDF 用に `tectonic` を追加）
- 仕様更新
  - `spec.md`, `protocol.md`, `README.md`

### 変更しないもの

- `proto/` のスキーマは変更しない。
- `docs/` の Mintlify 設定は変更しない。

## Milestones

1. `book bundle` を追加し、`book/src/SUMMARY.md` の順序で章を 1 つの Markdown に統合できる。
   - 観測可能な成果: `bundle.md` が生成され、章本文と `## Sources` を含む。
2. `llm translate` を追加し、統合 Markdown を外部コマンド経由で変換して別ファイルに保存できる。
   - 観測可能な成果: `translated.md` が生成される（Integration Test では `noop` を使用）。
3. `export` を追加し、統合/翻訳済み Markdown を `pandoc` で `epub`（任意で `pdf`）に変換できる。
   - 観測可能な成果: `book.epub` が生成される（Integration Test は `epub` のみ）。
4. `spec.md` / `protocol.md` / `README.md` を更新し、手順が再現できる。

## Tests

- `tests/sitebookify_pipeline.rs` に次を追加する（mock は使わない）。
  - `sitebookify book bundle` を実行し、生成ファイルに `## Sources` が含まれることを検証する。
  - `sitebookify llm translate` を `engine=noop` で実行し、出力が生成されることを検証する。
  - `sitebookify export --format epub` を実行し、`*.epub` が生成されることを検証する。

## Decisions / Risks

Decisions:

- **Bundle の順序は `book/src/SUMMARY.md` を正**とする（`toc.yaml` ではなく mdBook 出力に追従する）。
- 翻訳は **外部コマンド連携**とする。
  - `sitebookify` は stdin → stdout のフィルタとして翻訳コマンドを呼び出す。
  - 目標言語は環境変数 `SITEBOOKIFY_TRANSLATE_TO` として渡す。
- `epub/pdf` 変換は **`pandoc`** を呼び出す。
  - PDF は `tectonic` をデフォルトエンジンとして利用できるようにする。

Risks:

- 大規模な Markdown を一括で LLM に渡すとトークン制限により失敗しうる。
  - 緩和策: 将来 `--split`（H1 単位）等のチャンク化を追加する余地を残す（今回の範囲外）。
- `pandoc`/`tectonic` 依存により、Nix を使わない環境では追加インストールが必要。
  - 緩和策: エラーメッセージで必要ツールを明確にする。

## Progress

- 2026-01-26: 設計（CLI 追加方針、外部ツール方針）を決定し、実装とテストに着手する。
