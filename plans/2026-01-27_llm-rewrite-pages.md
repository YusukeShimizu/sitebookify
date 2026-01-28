# ExecPlan: TOC 後に LLM でページを「本向け」に書き換える（翻訳/Export 削除）

## Goal

- 翻訳（`llm translate`）と Export（`export`）機能を削除し、CLI と仕様を簡素化する。
- TOC 作成後は、TOC で採用された各ページ（source page）を LLM で「本として読みやすい体裁」へ書き換えられるようにする。
  - 例: ユーザプロンプト「日本語で簡潔にまとめて」を加味して文章中心に整形する。
- 以降の mdBook 生成（`book render`）および `book bundle` は、この書き換え後のページ内容を入力として行う。

### 非目的

- `epub` / `pdf` 等の外部フォーマット出力（pandoc 経由）は行わない。
- robots.txt 対応やログインサイト対応等、クロール機能の拡張は行わない。
- LLM 出力の品質（文章校正、用語統一）を自動で完全保証することは目標にしない。
  - ただし最低限の安全策（コード/URL の保持、空出力拒否など）は入れる。

## Scope

### 変更対象

- 仕様更新
  - `spec.md`: `llm_translate` と `export` を削除し、TOC 後の「ページ書き換え」工程を追加する
  - `protocol.md`: workspace 構成から `book.<LANG>.md` / `book.epub` / `book.pdf` 等を削除し、新しい中間生成物（書き換えページ）を追記する
- CLI 変更
  - `src/cli.rs`, `src/main.rs`: `llm translate` と `export` を削除
  - `build` に「ページ書き換え」用のフラグを追加（prompt/engine 等）
  - 新規コマンド（案）: `sitebookify llm rewrite-pages`
- 実装
  - `src/llm.rs`: 翻訳実装を撤去し、ページ書き換え（rewrite）に置換
  - `src/build.rs`: TOC 後に rewrite を挟み、書き換え後の pages を元に `book render` へ渡す
  - `src/export.rs`: 削除（または未使用なら削除し `lib.rs` からも外す）
- Integration Test 更新
  - `tests/sitebookify_pipeline.rs`: 翻訳/Export 前提のテストを削除し、rewrite の代表ケースに置換
- ドキュメント更新
  - `README.md`, `docs/cli/overview.mdx`: 翻訳/Export 手順を削除し、rewrite の手順へ置換
  - 必要に応じて `llmdesign.md` を現状に合わせて更新（翻訳 Mode B 前提のままだと誤解を生むため）

### 変更しないもの

- `crawl` / `extract` / `manifest` / `toc init/refine` / `book bundle` の基本仕様（存在、I/O の非上書き方針）は維持する。
  - ただし `build` の内部フローに rewrite 工程を追加する。
- `proto/` は原則変更しない（オンディスク形式のスキーマ自体は維持）。

## Milestones

1. 仕様（`spec.md` / `protocol.md`）を更新し、「翻訳/Export 削除」と「rewrite 工程追加」を明文化する。
   - 観測可能な成果: `build` の pipeline と workspace 構成の説明が新要件と整合する。
2. Integration Test を更新し、翻訳/Export に依存しない形で E2E が通るようにする。
   - 観測可能な成果: `cargo test --all` が LLM API キー無しで通る（`engine=noop` を利用）。
3. CLI から `llm translate` と `export` を削除し、代わりに `llm rewrite-pages` を追加する。
   - 観測可能な成果: `sitebookify --help` から該当コマンドが消え、rewrite が現れる。
4. rewrite の実装を追加し、`build` が TOC 後に書き換えページを生成してから mdBook を生成する。
   - 観測可能な成果: `workspace/` 内に書き換え用ディレクトリ（例: `manuscript/pages/*.md`）が生成され、`book.md` がそれを反映する。
5. README / docs を更新し、手順が再現できる状態にする。
   - 観測可能な成果: `nix develop -c just ci` が通る（textlint/vale 含む）。

## Tests

- `tests/sitebookify_pipeline.rs` を次の観点で更新する（mock は使わない）。
  - `build` が `--rewrite-engine noop`（および `--rewrite-prompt ...`）で完走し、`book.md` が生成される。
  - `llm rewrite-pages` の `engine=noop` が入力をコピーし、出力が生成される。
  - `engine=openai` の場合に `OPENAI_API_KEY` 未設定でエラーになる（安全ゲート）。

## Decisions / Risks

Decisions:

- rewrite 対象は **TOC に採用された page id のみ**とする。
  - toc refine でページが省略された場合でも、book 側に不要ページを混入させない。
- rewrite は **プロンプト指定時のみ有効**とする。
  - `build` では `--rewrite-prompt <TEXT>` が与えられた時だけ LLM 工程を挟む。
- rewrite の入力は **抽出 Markdown（front matter + body）**を基本とし、front matter は保持する。
  - manifest 生成を再利用できるようにする。
- ユーザプロンプトは「追加の指示」として扱い、常に「本として読みやすい体裁（文章中心、必要最小限の表/図/コード）」の制約を優先する。
- LLM の呼び出し単位は **自然な単位**を優先し、実装はまず Markdown の `##` 見出し単位で分割して扱う。
  - ただし 1 セクションが大きい場合は、さらに安全に分割する余地を残す。

Risks:

- LLM が事実を捏造するリスク。
  - 緩和策: システム指示で「入力に無い事実を追加しない」「不明は不明と書く」を明示する。
- LLM がコード/URL を破壊するリスク。
  - 緩和策: 既存の placeholder トークン（`{{SBY_TOKEN_...}}`）方式でコード/URL を保護し、復元する。
  - ただし rewrite は要約を許容するため、**トークンの欠落（＝省略）は許容**する（翻訳と異なる）。
- コスト/時間（ページ数が多いと遅い）。
  - 緩和策: 将来的に入力ハッシュ + プロンプトでキャッシュする余地を残す（今回の MVP では必須にしない）。

## Progress

- 2026-01-27: 方針（翻訳/Export を削除し、TOC 後に rewrite を追加）を整理し、ExecPlan を作成する。
