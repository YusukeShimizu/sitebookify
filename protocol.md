# Sitebookify On-disk Protocol（MVP）

この文書は、Sitebookify のオンディスク形式（ファイルとディレクトリ）を定義する。
本書は「再現性」と「将来の拡張」を優先する。

## 互換性方針

- 本プロトコルは MVP のため、**後方互換性を保証しない**。
- 破壊的変更を行う場合は、次を必ず行う。
  - `protocol.md` と `spec.md` を更新する。
  - 既存の Integration Test（`tests/`）を更新する。
  - 互換性が必要な場合は、新しい出力先ディレクトリを使う。

## 用語

- **Raw**: クロールで取得した HTML 等の生データ。
- **Extracted Page**: Raw から本文抽出し、素材化した Markdown。
- **Manifest**: Extracted Page の台帳（JSONL）。
- **TOC**: 教科書の章立て（YAML）。
- **Book**: mdBook プロジェクト（`book/`）。

## ディレクトリ構成（MVP）

```
raw/
  crawl.jsonl
  html/
    <host_or_host_port>/<path...>/index.html
extracted/
  pages/
    <page_id>.md
manifest.jsonl
toc.yaml
book/
  book.toml
  src/
    SUMMARY.md
    chapters/
      ch01.md
```

## `raw/crawl.jsonl`

`raw/crawl.jsonl` は JSON Lines（1 行 1 JSON）である。
1 行は 1 回の取得を表す。

### 行スキーマ（MVP）

- `url`（string, required）: 取得対象の URL（正規化後）。
- `normalized_url`（string, required）: 正規化済み URL。
  - フラグメント（`#...`）は削除する。
  - クエリ（`?...`）は削除する。
  - URL の末尾の `/` は削除する（ただし `/` 自体は除外）。
- `depth`（number, required）: 開始 URL を 0 とした深さ。
- `status`（number, required）: HTTP ステータスコード。
  - 取得に失敗した場合は 0 を記録する。
- `content_type`（string, optional）: レスポンスの Content-Type。
- `retrieved_at`（string, required）: 取得時刻（RFC 3339）。
- `raw_html_path`（string, optional）: 保存した Raw HTML のパス。
  - `text/html` かつ 2xx の場合のみ設定する。

### パスの注意（host と port）

URL にポートが含まれる場合は、ファイルシステム上の host 表現として `:` の代わりに `_` を使う。
例: `127.0.0.1:12345` → `127.0.0.1_12345`

## `extracted/pages/*.md`（Extracted Page）

Extracted Page は Markdown ファイルである。
先頭は YAML front matter（`---`）である。

### Front matter（MVP）

- `id`（string, required）: `p_` + `sha256(normalized_url)` の hex。
- `url`（string, required）: 正規化済み URL。
- `retrieved_at`（string, required）: 取得時刻（RFC 3339）。
- `raw_html_path`（string, required）: Raw HTML のパス。
- `title`（string, required）: ページタイトル。

本文（Markdown）は、HTML から変換した内容である。

## `manifest.jsonl`

`manifest.jsonl` は JSON Lines（1 行 1 JSON）である。
1 行は 1 Extracted Page を表す。

行スキーマは `proto/sitebookify/v1/manifest.proto` の `ManifestRecord` と対応する。

### 行スキーマ（MVP）

- `id`（string, required）: Extracted Page の ID。
- `url`（string, required）: 正規化済み URL。
- `title`（string, required）: ページタイトル。
- `path`（string, required）: URL のパス（例: `/docs/intro`）。
- `extracted_md`（string, required）: Extracted Page のファイルパス。

## `toc.yaml`

`toc.yaml` は YAML である。
教科書の章立てを表す。

スキーマは `proto/sitebookify/v1/toc.proto` の `Toc` と対応する。

### スキーマ（MVP）

- `book_title`（string, required）: 書籍タイトル。
- `parts`（list, required）: Part の配列。
  - `title`（string, required）: Part タイトル。
  - `chapters`（list, required）: Chapter の配列。
    - `id`（string, required）: 章 ID（例: `ch01`）。
    - `title`（string, required）: 章タイトル。
    - `sources`（list[string], required）: `manifest.jsonl` の `id` の配列。

## `book/`（mdBook）

`book/` は生成物である。
再生成を前提とする。

### 章 Markdown の要件（MVP）

- 章 Markdown は必ず `## Sources` を含む。
- `## Sources` には、章に含まれる出典 URL を列挙する。
