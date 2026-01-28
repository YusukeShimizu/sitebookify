# LLM 介入設計（Page Rewrite / 本向け整形）

本書は、`sitebookify` の決定的なパイプライン（crawl→extract→manifest→toc→book）を土台にする。
LLM は **TOC 作成後**の「ページ書き換え」にのみ介入させ、サイト由来のページを「本として読みやすい体裁（文章中心）」に整える。

ユーザはプロンプトで体裁や言語を指定できる（例: 「日本語で簡潔にまとめて」）。

---

## Security & Architectural Constraints

この節は不変条件である（RFC 2119: MUST / MUST NOT / SHOULD / MAY）。

### 決定性と根拠

- 取得・抽出の正は `sitebookify` の出力（`raw/**`, `extracted/**`, `manifest.jsonl`）である（MUST）。
- LLM は **Web 取得に関与してはならない**（MUST NOT）。
  - 根拠データはスナップショット（`raw`/`extracted`）に固定する。
- LLM は **入力スナップショットに存在しない事実を追加してはならない**（MUST NOT）。
  - 不明な場合は「不明」と書く（SHOULD）。

### 対象と範囲

- 書き換え対象は **TOC に採用された page id のみ**である（MUST）。
  - `toc.yaml` から参照されない page id を出力してはならない（MUST NOT）。
- 書き換えは **ユーザプロンプト指定時のみ**有効である（MUST）。

### 出力品質（本の体裁）

- 出力は「サイトの形式」を保つ必要はない（MAY）。
- 出力は「普通の本」として読みやすい体裁を優先する（MUST）。
  - 文章中心にする（MUST）。
  - 表・図・コードは必要最小限に留める（SHOULD）。
- 生成された mdBook の各章は `## Sources` を含む（MUST）。

### 破壊しやすい要素の扱い

- コード、コマンド、識別子（例: `PeerManager`）は改変しない（MUST NOT）。
- URL は改変しない（MUST NOT）。
  - LLM 入力ではコード・URL 等をプレースホルダ（`{{SBY_TOKEN_000000}}`）で保護し、出力後に復元する（SHOULD）。

---

## Concepts

### Snapshot

クロール結果と抽出結果を固定した作業ディレクトリ。

- `raw/`: 取得 HTML と取得ログ（`crawl.jsonl`）
- `extracted/`: ページ本文抽出（`pages/*.md`）
- `manifest.jsonl`: page id ↔ URL ↔ 抽出 Markdown の対応表

### Manuscript

TOC に採用されたページを「本向け」に書き換えた素材ディレクトリ。

- `manuscript/pages/*.md`: 書き換え後の Markdown（YAML front matter + 本文）
- `manifest.manuscript.jsonl`: 書き換え後ページの台帳（任意。book 側で参照する）

### Page / Chapter

- Page: `manifest.jsonl` の 1 レコードに対応する単位。
- Chapter: mdBook の 1 章（`book/src/chapters/chXX.md`）。

### LLM Intervention（介入点）

LLM を呼び出す工程は次の 2 つに限定する。

1. **TOC Refine（任意）**: 章の順序・章タイトル・ページ省略（キュレーション）
2. **Page Rewrite（任意）**: TOC に採用されたページ本文を「本向け」に書き換える

---

## Flows

### Flow A: スナップショット作成（LLM なし）

入力: `start_url`, crawler params

```sh
sitebookify crawl --url https://example.com/docs/ --out raw
sitebookify extract --raw raw --out extracted
```

出力: Snapshot（`raw/**`, `extracted/**`）

### Flow B: Manifest（LLM なし）

```sh
sitebookify manifest --extracted extracted --out manifest.jsonl
```

### Flow C: TOC（LLM 任意）

```sh
sitebookify toc init --manifest manifest.jsonl --out toc.yaml
# 任意: toc refine
sitebookify toc refine --manifest manifest.jsonl --out toc.refined.yaml --engine openai
```

### Flow D: Page Rewrite（LLM 任意）

目的: TOC に採用されたページ本文を、ユーザプロンプトを加味して「本向け」に書き換える。

入力（ページごと、概念）は次のとおり。

- Extracted Page（front matter + 本文）
- ユーザプロンプト

出力（ページごと）は次のとおり。

- `manuscript/pages/<page_id>.md`

呼び出し単位は、実装では Markdown の `##` 見出し単位を基本とする（SHOULD）。

### Flow E: Manuscript Manifest（LLM なし）

```sh
sitebookify manifest --extracted manuscript --out manifest.manuscript.jsonl
```

### Flow F: mdBook 生成（LLM なし）

```sh
sitebookify book init --out book --title "Example Docs Textbook"
sitebookify book render --toc toc.yaml --manifest manifest.manuscript.jsonl --out book
sitebookify book bundle --book book --out book.md
```

### Flow G: 検証（LLM なし）

最低限の検証は次のとおり。

- `toc.yaml` が採用ページの page id を参照していること
- `book/src/chapters/*.md` が `## Sources` を含むこと
- textlint が通ること

```sh
nix develop -c just textlint
```

---

## Notes（運用上の工夫）

### キャッシュ（コスト最適化）

- ページ単位で LLM を呼ぶ。
- さらに `##` 単位に分割して呼ぶことで、入力サイズと失敗率を抑える。
- 将来的に、入力（抽出 Markdown + プロンプト）からハッシュを作り、同一なら再利用できる余地がある。

### 失敗時の扱い

- LLM の出力が空、または復元不能な場合は、該当箇所は原文（抽出 Markdown）にフォールバックする（SHOULD）。

