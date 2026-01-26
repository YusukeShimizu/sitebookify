# Sitebookify MVP Plan（CLI / Markdown）

## 0. 目的（ゴール）

ログイン不要の公開サイトをクロールし、本文抽出済み Markdown 素材を生成する。
章立て（TOC）に従って mdBook 形式の教科書 Markdown を出力する。

## 1. スコープ

### 1.1 MVP に含める

- CLI のみを提供する。
- `crawl` → `extract` → `manifest` → `toc` → `book` の一連を実行できる。
- 出力は Markdown（mdBook プロジェクト）までとする。

### 1.2 MVP ではやらない

- robots.txt の尊重、解釈。
- ログインや認証が必要なサイト。
- PDF 出力。
- LLM による教科書化（文体統一、演習生成など）。
  - ただし、後で追加できるように入出力形式は固定する。

## 2. 不変条件（Hard constraints）

- `raw/` と `extracted/` はスナップショットである。
  - CLI は **既存ファイルを上書きしてはならない**。
  - 既に存在する場合はエラーにするか、明示フラグが必要である（MVP はエラーで良い）。
- `book/` のみが生成物である。
  - `book/` は再生成され得る。
- 生成される章 Markdown は必ず `## Sources` を含む。
  - 出典 URL 一覧を必ず残す。
- 事実を創作しない。
  - 情報不足は `TODO` を書き、どの Sources が不足したかを明記する。

## 3. 用語（Ubiquitous Language）

- **Raw**: クロールで取得した HTML 等の生データ。
- **Extracted Page**: Raw から本文抽出し、素材化した Markdown。
- **Manifest**: Extracted Page の台帳（JSONL）。
- **TOC**: 教科書の章立て（YAML）。
- **Book**: mdBook プロジェクト（`book/`）。

## 4. ディレクトリ構成（MVP）

```
raw/
  crawl.jsonl
  html/
    <host>/<path...>/index.html
extracted/
  pages/
    <page_id>.md
manifest.jsonl
toc.yaml
book/
  src/
    SUMMARY.md
    chapters/
      ch01.md
```

補足は次のとおり。

- `manifest.jsonl` と `toc.yaml` はリポジトリ直下に置く。
  - 将来、`--workspace <dir>` でルートを切り替えできるようにする。

## 5. URL 正規化とクロール範囲

### 5.1 正規化

- URL のフラグメント（`#...`）は常に無視する。
- URL のクエリ（`?...`）は破棄する。
- URL の末尾の `/` は正規化で除去する（ただし `/` 自体は除外）。
  - 例: `https://example.com/docs/` → `https://example.com/docs`
- 相対 URL は取得元ページの URL を基準に解決する。

### 5.2 追跡範囲

- same-origin のみを対象にする。
- 開始 URL の **パス配下のみ**を追跡する。
  - 例: 開始が `https://example.com/docs/` なら `/docs/**` のみ。

### 5.3 停止条件（デフォルト）

- `--max-pages 200`
- `--max-depth 8`

### 5.4 polite 設定（デフォルト）

- `--concurrency 4`
- `--delay-ms 200`

### 5.5 robots.txt

- MVP では未対応。
- `docs/` と `spec.md` に「未対応」であることを明記する。

## 6. CLI 仕様（提案）

バイナリ名は `sitebookify` とする。

### 6.0 `build`

`crawl` → `extract` → `manifest` → `toc init` → `book init` → `book render` を一括で実行する。

- 例:

```sh
sitebookify build \
  --url https://example.com/docs/ \
  --out workspace \
  --title "Example Docs Textbook"
```

- 出力:
  - `workspace/raw/**`
  - `workspace/extracted/**`
  - `workspace/manifest.jsonl`
  - `workspace/toc.yaml`
  - `workspace/book/**`

注意事項は次のとおり。

- `workspace/` は write-once とする（既に存在する場合は失敗して良い）。
- `crawl` と同じ polite 設定（`--max-pages` / `--max-depth` / `--concurrency` / `--delay-ms`）を受け取る。

### 6.1 `crawl`

サイトをクロールして Raw を作る。

- 例:

```sh
sitebookify crawl \
  --url https://example.com/docs/ \
  --out raw
```

- 主なフラグ:
  - `--url <URL>`（必須）
  - `--out <DIR>`（必須）
  - `--max-pages <N>`（default: 200）
  - `--max-depth <N>`（default: 8）
  - `--concurrency <N>`（default: 4）
  - `--delay-ms <N>`（default: 200）

- 出力:
  - `raw/html/<host>/<path...>/index.html`
  - `raw/crawl.jsonl`

`raw/crawl.jsonl` の 1 行は 1 取得を表す。

```json
{"url":"https://example.com/docs/intro","normalized_url":"https://example.com/docs/intro","depth":1,"status":200,"content_type":"text/html","retrieved_at":"2026-01-23T10:25:00Z","raw_html_path":"raw/html/example.com/docs/intro/index.html"}
```

注意事項は次のとおり。

- Content-Type が `text/html` 以外は保存しない（MVP）。
- 失敗（非 2xx）は記録する。
  - Raw HTML は保存しない。

### 6.2 `extract`

Raw HTML から本文抽出し、Extracted Page を作る。

- 例:

```sh
sitebookify extract \
  --raw raw \
  --out extracted
```

- 入力:
  - `raw/crawl.jsonl`
  - `raw/html/**.html`

- 出力:
  - `extracted/pages/<page_id>.md`

`<page_id>` は `sha256(normalized_url)` の hex を採用する。

Extracted Page の先頭は YAML front matter とする。

```markdown
---
id: "p_<hex>"
url: "https://example.com/docs/intro"
retrieved_at: "2026-01-23T10:25:00Z"
raw_html_path: "raw/html/example.com/docs/intro/index.html"
title: "Intro"
---

# Intro

...本文...
```

抽出アルゴリズム（MVP）は次のとおり。

- Mozilla Readability（Firefox Reader Mode）を `readability-js` 経由で利用する。
- `Readability` インスタンスは `extract` 実行中に再利用する。
- Readability の check が落ちるケースは options を調整して再試行する（二段構え）。
- 抽出結果の HTML を Markdown に変換する。

### 6.3 `manifest`

`extracted/pages/*.md` を走査して `manifest.jsonl` を作る。

- 例:

```sh
sitebookify manifest --extracted extracted --out manifest.jsonl
```

- 出力行の例:

```json
{"id":"p_<hex>","url":"https://example.com/docs/intro","title":"Intro","path":"/docs/intro","extracted_md":"extracted/pages/p_<hex>.md"}
```

### 6.4 `toc init`

Manifest から初期 TOC を作る。

- 例:

```sh
sitebookify toc init --manifest manifest.jsonl --out toc.yaml
```

初期 TOC は URL のパス階層で決める。

- `toc.yaml` 例:

```yaml
book_title: "Example Docs Textbook"
parts:
  - title: "Part 1"
    chapters:
      - id: "ch01"
        title: "Docs"
        sources:
          - "p_<hex>"
```

### 6.5 `book init`

mdBook の雛形を生成する。

- 例:

```sh
sitebookify book init --out book --title "Example Docs Textbook"
```

- 生成物:
  - `book/src/SUMMARY.md`
  - `book/src/chapters/ch01.md`（空でも良い）

### 6.6 `book render`

`toc.yaml` と `manifest.jsonl` から `book/src/` を生成する。

- 例:

```sh
sitebookify book render --toc toc.yaml --manifest manifest.jsonl --out book
```

MVP では以下を満たす。

- `book/src/SUMMARY.md` を生成する。
- 少なくとも `ch01.md` を生成する。
- 各章末尾に `## Sources` と URL 一覧を出力する。

章テンプレ（MVP）は次のとおり。

- Objectives
- Prerequisites
- Body
- Summary
- Sources

## 7. Rust 実装方針（モジュール案）

既存のテンプレ構造（`src/cli.rs`, `src/logging.rs` など）を置き換える。

- `src/main.rs`
  - clap で CLI parse。
  - `logging::init()`。
  - 各 subcommand handler を呼ぶ。
- `src/cli.rs`
  - `crawl` / `extract` / `manifest` / `toc` / `book` を定義。
- `src/crawl.rs`
  - URL 正規化。
  - same-origin とパス配下の制限。
  - `spider`（spider-rs）でクロールする。
  - `raw/crawl.jsonl` を出力する。
  - Raw HTML を `raw/html/**/index.html` に保存する（上書き禁止）。
  - `page_links` から BFS 深さを計算する。
- `src/raw_store.rs`
  - URL → `raw/html/...` の保存パス計算。
  - `create_dir_all`。
  - 上書き禁止チェック。
- `src/extract.rs`
  - `raw/crawl.jsonl` の読み込み。
  - Mozilla Readability による本文抽出。
  - HTML → Markdown 変換。
  - front matter 付与。
- `src/manifest.rs`
  - Extracted Page の front matter 解析。
  - `manifest.jsonl` の生成。
- `src/toc.rs`
  - URL パス階層から TOC 初期生成。
- `src/book.rs`
  - `SUMMARY.md` と章ファイル生成。

## 8. テスト方針（Integration Test）

モックは使わない。
外部ネットワークも使わない。

- `tiny_http` 等でローカル HTTP サーバを立てる。
- 2〜5 ページの HTML を配信する。
  - `/docs/index.html` が `/docs/intro` へリンクする。
  - クエリとフラグメントを含むリンクも含める。
- `sitebookify crawl` を実行し、`raw/html/**` と `raw/crawl.jsonl` ができることを確認する。
- `extract` → `manifest` → `toc init` → `book render` まで通す。
- `book/src/chapters/ch01.md` に `## Sources` が含まれることを確認する。

## 9. `spec.md` / `protocol.md` / `proto/` / `docs/` の整合方針

### 9.1 `spec.md`

- テンプレの `RustCLI/hello` を Sitebookify の CLI 概念に置き換える。
- `just ci` の同期（Sync）は維持する。

### 9.2 `protocol.md`（新規）

オンディスク形式の「互換性方針」を定義する。

- `raw/crawl.jsonl` の行スキーマ。
- Extracted Page front matter のキー。
- `manifest.jsonl` の行スキーマ。
- `toc.yaml` のスキーマ。
- 破壊的変更の扱い（バージョニング）。

### 9.3 `proto/`

CI が `buf format` / `buf lint` を必須にするため、proto は維持する。
MVP は「API」ではなく「オンディスク形式のスキーマ」として proto を置く。

- `proto/sitebookify/v1/manifest.proto`
- `proto/sitebookify/v1/toc.proto`

既存の `proto/template/v1/greetings.proto` は置換対象とする。

### 9.4 `docs/`

Mintlify の導線を Sitebookify に更新する。

- `docs/index.mdx` に CLI の概要と最短手順を書く。
- `docs/docs.json` の `name` と `description` を更新する。
- `docs/` に以下のページを追加する。
  - `docs/cli/overview.mdx`
  - `docs/formats/raw.mdx`
  - `docs/formats/extracted.mdx`
  - `docs/formats/manifest.mdx`
  - `docs/formats/toc.mdx`

## 10. 実装ステップ（最短の順）

1. バイナリ名を `sitebookify` に変更する。
2. `crawl` を実装し、Integration Test を追加する。
3. `extract` を実装し、fixture で安定させる。
4. `manifest` / `toc init` を実装する。
5. `book init` / `book render` を実装する。
6. `spec.md` / `protocol.md` / `proto/` / `docs/` を更新する。
7. `just ci` が通るまで修正する。
