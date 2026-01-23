# LLM 介入設計（Mode B / 簡単な本 / 1ページ=1章）

本書は、`sitebookify` の決定的なパイプライン（crawl→extract→manifest→toc→book）を土台にしつつ、LLM を「必要なフェーズだけ」介入させて、日本語の「全体を網羅した簡単な本」体裁の mdBook を生成するための設計である。

本設計は **Mode B**（本文も翻訳する）かつ **1ページ=1章** を前提とする。

---

## Security & Architectural Constraints

この節は不変条件である（RFC 2119: MUST / MUST NOT / SHOULD / MAY）。

### 決定性と根拠

- 取得・抽出の正は `sitebookify` の出力（`raw/**`, `extracted/**`, `manifest.jsonl`）である。
- LLM は **Web 取得に関与してはならない**（MUST NOT）。
  - 根拠データはスナップショット（`raw`/`extracted`）に固定する。
- Mode B では、章本文は **翻訳する**（MUST）。
  - ただし、翻訳は「構造を保った変換」であり、情報を落とした要約にしてはならない（MUST NOT）。
  - LLM の出力は必ず、スナップショット（抽出 Markdown）に基づく（MUST）。

### 章構成とカバレッジ

- `manifest.jsonl` に含まれる各ページ（page id）は、**ちょうど 1 つの章に対応**する（MUST）。
- `toc.yaml` の章は **1ページ=1章** であり、`sources` は **必ず 1 つ**（その章に対応する page id）とする（MUST）。
- `toc.yaml` から参照されない page id を作ってはならない（MUST NOT）。
- `toc.yaml` 内で同一 page id を重複参照してはならない（MUST NOT）。

### 出力品質

- 生成された各章は `## Sources` を含む（MUST）。
  - `sitebookify book render` の保証に加え、LLM 介入後も保持する。
- LLM が生成する日本語本文・補助テキストは、入力（抽出 Markdown）にない事実を追加してはならない（MUST NOT）。
  - 不明な場合は「不明」と書くか、項目を空にする（SHOULD）。
- コード、コマンド、識別子（`PeerManager` のような型名）を改変してはならない（MUST NOT）。
  - コードブロックとインラインコードは原文のまま保持する（MUST）。
- リンク URL は原文どおりに保持する（MUST）。
  - 相対リンクは、基準 URL（各ページの `url`）に対して絶対 URL に正規化する（SHOULD）。

### セキュリティと秘密情報

- API キー等の秘密情報はリポジトリに書き込まない（MUST NOT）。
  - 実行時環境変数（例: `.envrc.local`）で注入する（MUST）。
- 入力サイトに機微情報が含まれる可能性がある場合、送信可否の判断は利用者が行う（MUST）。

---

## Concepts

### Snapshot

クロール結果と抽出結果を固定した作業ディレクトリ。

- `raw/`: 取得 HTML と取得ログ（`crawl.jsonl`）
- `extracted/`: ページ本文抽出（`pages/*.md`）
- `manifest.jsonl`: page id ↔ URL ↔ 抽出 Markdown の対応表

本設計では、翻訳前の抽出結果を `extracted.en/`、翻訳後を `extracted.ja/` のように別ディレクトリとして扱う。

### Page（ページ）

`manifest.jsonl` の 1 レコードに対応する単位。

- `id`: page id（例: `p_...`）
- `url`: 正規化 URL
- `title`: ページタイトル
- `extracted_md`: 抽出 Markdown のパス

### Chapter（章）

mdBook における章（`book/src/chapters/chXX.md`）。

本設計では **1ページ=1章** とし、章は必ず 1 つの page id を `sources` として持つ。

### Translation（翻訳）

抽出 Markdown（英語など）を日本語 Markdown に変換する工程である。
Mode B では本の中核であり、情報を落とさずに訳すことが最優先となる。

翻訳は「全文翻訳」だが、Markdown の構造は維持する。
このため、訳文はページ単位（Page）で生成し、`extracted.ja/pages/*.md` として保存する。

### Book Tone（本の体裁）

本設計が目指すのは「技術書」に限定された体裁ではなく、「全体を網羅した簡単な本」である。
具体的には、次を満たす。

- 目次が自然な順序である。
- 各章が独立して読める（タイトルが日本語で分かりやすい）。
- 各章末に出典が残る（Sources）。

必要に応じて、章の冒頭/末尾に短い補助テキスト（導入や要点）を付けてもよい。
ただし、補助テキストのために本文の情報を削ってはならない。

### LLM Intervention（LLM 介入点）

LLM を呼び出す工程。
本設計の介入点は次の 3 つに限定する。

1. **Page Translate**: 抽出 Markdown をページ単位で翻訳する（要約禁止）
2. **TOC Refine**: 章の順序と章タイトルの編集（1ページ=1章は維持）
3. **Chapter Polish（任意）**: 章の導入や要点など、短い補助テキストを付与する

---

## Flows

### Flow A: スナップショット作成（LLMなし）

入力: `start_url`, crawler params

```sh
sitebookify crawl --url https://example.com/docs/ --out raw
sitebookify extract --raw raw --out extracted.en
```

出力: Snapshot（`raw/**`, `extracted.en/**`）

### Flow B: Page Translate（LLMあり）

目的: `extracted.en/pages/*.md` を日本語へ翻訳し、`extracted.ja/pages/*.md` を生成する。

入力（ページごと）:

- `extracted.en/pages/<page_id>.md`（front matter + 本文）

出力（ページごと）:

- `extracted.ja/pages/<page_id>.md`

必須要件:

- front matter の `id`, `url`, `retrieved_at`, `raw_html_path` は原文からコピーする（MUST）。
- front matter の `title` は日本語化してよい（SHOULD）。
- 本文は「要約」ではなく「翻訳」である（MUST）。
- コードブロック、インラインコード、URL は改変しない（MUST NOT）。

推奨:

- 章タイトルのために `title_ja` / `title_en` のようなメタ情報を front matter に追加してもよい（MAY）。

### Flow C: Manifest（LLMなし）

```sh
sitebookify manifest --extracted extracted.ja --out manifest.jsonl
```

出力: `manifest.jsonl`（日本語の章タイトルが必要なら、ここでの `title` を日本語にしておく）

### Flow D: TOC 初期化（LLMなし）

```sh
sitebookify toc init --manifest manifest.jsonl --out toc.yaml
```

ここでの `toc.yaml` は「暫定の章立て」であり、次の Flow E で編集される前提とする。

### Flow E: TOC Refine（LLMあり）

目的: **1ページ=1章** を維持しつつ、読書順序と章タイトルを「簡単な本として自然」に整える。

入力（最小）:

- `manifest.jsonl` 由来の Page 一覧（`id`, `path`, `title`, `url`）

入力（推奨）:

- 各ページの見出し一覧（抽出 Markdown から機械的に抽出）
  - ページ本文全部を渡さずに設計できるため、コストと漏洩リスクを下げる。

出力（推奨: 構造化）:

- `TocPlan`（JSON）
  - `chapters[]` は必ず page id を 1 つずつ持つ。
  - 章の順序と章タイトルのみが編集対象である。

`TocPlan` の例（概念）:

```json
{
  "book_title": "Example Docs Textbook",
  "chapters": [
    { "chapter_id": "ch01", "title": "導入（Introduction）", "source_page_id": "p_..." },
    { "chapter_id": "ch02", "title": "アーキテクチャ（Architecture）", "source_page_id": "p_..." }
  ]
}
```

生成後は `TocPlan` を決定的に `toc.yaml` に変換する（YAML 変換は LLM に任せない）。

### Flow F: mdBook 生成（LLMなし）

```sh
sitebookify book init --out book --title "Example Docs Textbook"
sitebookify book render --toc toc.yaml --manifest manifest.jsonl --out book
```

出力:

- `book/src/SUMMARY.md`
- `book/src/chapters/*.md`（本文は抽出結果を元に生成され、補助テキスト部分が TODO の状態）

### Flow G: Chapter Polish（LLMあり、任意）

目的: 本文（翻訳済み）を維持したまま、「簡単な本」として読みやすくするための短い補助テキストを付ける。

入力（章ごと、最小）:

- 章に対応する翻訳済み本文（front matter を除いた本文）
- 章タイトル（`toc.yaml`）

出力（推奨: 構造化）:

- `ChapterPolish`（JSON）
  - `intro_ja`（短い導入、任意）
  - `points[]`（この章の要点、任意）
  - `terms[]`（用語、任意）
  - 章本文の引用が必要な場合は「短い引用」に限る（抜粋しすぎない）。

`ChapterPolish` の例（概念）:

```json
{
  "chapter_id": "ch01",
  "intro_ja": "本章は、対象ドキュメントの導入を扱う。",
  "points": ["主要な概念の位置づけを述べる。"],
  "terms": [{ "term": "PeerManager", "note_ja": "固有名は原文どおり保持する。" }]
}
```

適用（LLMなし）:

- `book/src/chapters/chXX.md` に、上記 JSON を使って短い導入/要点を挿入する。
- 章本文（翻訳済み）は変更しない。
- `## Sources` は保持する。

### Flow H: 検証（LLMなし）

最低限の検証:

- `toc.yaml` が page id を重複なく全件カバーしていること
- 章本文が翻訳済みであること（英語が過度に残っていないこと）
- 各章に `## Sources` があること
- textlint が通ること

```sh
nix develop -c just textlint
```

---

## Notes（運用上の工夫）

### キャッシュ（コスト最適化）

- ページ単位で LLM を呼ぶ。
- 入力（抽出 Markdown + プロンプトバージョン）からハッシュを作り、同一なら再利用する。
- TOC Refine も同様に、ページ一覧のハッシュでキャッシュする。

### 失敗時の扱い

- LLM の出力がスキーマに合わない場合は、リトライではなく「空配列で埋める」等の安全側に倒す（MUST）。
- Chapter Polish が空でも、翻訳済み本文は残るため、最低限「読める本」は常に生成できる（SHOULD）。

### Mode A へのフォールバック

全文翻訳のコストが高い場合は、本文を原文のままにし、補助テキストのみ日本語化する Mode A に切り替えてよい。
ただし、Mode A と Mode B を同一出力に混在させる場合は、章ごとに明示する（SHOULD）。
