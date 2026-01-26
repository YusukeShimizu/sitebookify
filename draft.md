「サイト（階層構造）→ 教科書（章立て・練習問題つき）→ PDF/HTMLで配布」までを **Rust中心＋既存OSS最大活用** でやるなら、私はこう設計します。

---

## ゴールを「mdBookプロジェクト」に固定するのが強い

教科書づくりは、最終的に次の要素を揃える。

* 章立て（TOC）
* 本文（統一された文体・用語）
* 図/数式/コード
* 演習・小テスト
* 参考リンク・出典

これらを「本としてビルドできる形」に揃えることが重要です。

そこで **“本のソース”は mdBook（Markdown 群＋SUMMARY.md）に固定** する。
mdBook は Rust 製で、本/教材/チュートリアルに向いている。
プラグイン（preprocessor/backend）も多い。 ([GitHub][1])

---

## Rust中心で組む「おすすめOSSスタック」

### 1) クロール（サイト階層を集める）

* **spider / spider_cli**（Rust）

  * CLIで `crawl`/`download` でき、URL収集やHTML保存ができます。 ([Docs.rs][2])

### 2) 本文抽出（ナビ・広告・余計なものを落とす）

ここが品質の肝です。おすすめは2段構え。

* 第一候補: **readability-js（Rust + Mozilla Readability）**

  * Firefoxのリーダーモードと同系のアルゴリズムで「本文だけ」を抜きやすい。
  * **CLI（`readable`）があって、URL/HTMLファイル/STDINから“Markdown”で出力**できる。 ([Docs.rs][3])
  * 大量処理では `Readability` の初期化コストは高い。
  * 最終的にはライブラリに組み込み、インスタンスを再利用できる設計が望ましい（初期化~30ms、パース~10msの記述あり）。 ([Docs.rs][3])

* 代替（抽出が崩れるサイト向け）: **html-to-markdown（Rustコア）**

  * 抽出ではなく「HTML→Markdown変換」に強い。表・コードブロックなどを保ちたいときの保険。 ([Docs.rs][4])

### 3) 教科書としての本体（章立て・検索・見た目）

* **mdBook**（Rust） ([GitHub][1])
* 教科書向けのプラグイン（必要に応じて）

  * **mdbook-admonish**（NOTE/IMPORTANT などの囲み） ([Docs.rs][5])
  * **mdbook-quiz**（小テスト） ([GitHub][6])
  * **mdbook-exercises**（演習・ヒント・解答など） ([GitHub][7])
  * **mdbook-linkcheck**（リンク切れ検査） ([GitHub][8])
  * 数式/図:

    * **mdbook-katex**（HTMLで数式） ([GitHub Wiki][9])
    * **mdbook-mermaid**（図） ([GitHub Wiki][9])
  * 用語集:

    * **mdbook-termlink**（用語の自動リンク） ([GitHub Wiki][9])

### 4) PDF出力（“教科書の見た目”）

PDFは「見栄え」と「安定性」で2系統を用意すると楽です。

* 見栄え重視（推し）: **mdbook-typst + Typst**

  * mdBookをTypstに変換して、PDF/PNG/SVG/Typstソースを出せます。 ([GitHub][10])
  * `book.toml` に `[output.typst]` を足して `format="pdf"` にできる。 ([GitHub][10])

* 速攻でPDF化（Web印刷系）: **mdbook-pdf**

  * headless Chrome/DevTools ProtocolでPDF生成。Chrome/Chromium/Edgeが必要。 ([GitHub][11])

（他にも **mdbook-pandoc** という「Pandoc経由で多形式」もあるので、表現力や既存テンプレ資産があるなら候補。 ([GitHub][12])）

### 5) LLM（Rust寄りで回すなら）

* **mistral.rs**（RustのLLM推論エンジン）

  * OpenAI互換HTTPサーバー等がある、と明記されています。 ([GitHub][13])
* もちろんAPI利用（OpenAI等）でもOK。ここは「運用コスト/速度/精度」で選べばいいです。

---

## 全体パイプライン設計（おすすめ）

### フェーズA: クロールして「ページ集合」を確定

出力物（例）は次のとおり。

* `data/urls.jsonl`（URL一覧、親子関係、タイトル、取得日時、hash）
* `data/html/...`（HTML保存）

ポイントは次のとおり。

* 階層構造は **URLパス** と **パンくず/サイドバー** の両方を材料にする
* サイトに `sitemap.xml` があれば最強（まずそれを種にする）
* 同一内容の重複URL（`?ref=` や `#anchor`）を正規化して潰す

### フェーズB: 抽出して「教材素材Markdown」にする

出力物（例）は次のとおり。

* `data/extracted/*.md`（1ページ=1素材）
* 各ファイル先頭にメタデータ（出典URL・取得日・タイトル・見出し構造）

抽出戦略は次のとおり。

* まず `readable`（readability-js-cli）で本文Markdown化 ([Docs.rs][3])
* うまく抜けないページだけ html-to-markdown 系にフォールバック ([Docs.rs][4])

### フェーズC: 章立て（TOC）を決める

ここが「サイト階層→教科書」変換の本丸。

最初は割り切りで OK とする。

* **URLの階層 = 暫定の章立て**

  * `/guide/getting-started/...` → 「Guide → Getting Started → ...」
* ただし教科書は「読む順番」が重要なので、

  * “参照用ページ” と “学習順ページ” が混ざっていたら、
  * **学習順を優先して並び替える**（LLMが得意）

出力物は次のとおり。

* `book/src/SUMMARY.md`（mdBookの目次）
* `book/src/**/*.md`（章ファイル）

### フェーズD: LLMで「教科書化」する

LLMの使いどころは「本文を丸ごと生成」ではなく、次の編集タスクに寄せると安定します。

* 章の狙い（学習目標）を作る
* 用語を統一する（表記ゆれ、言い回し）
* 前提知識を明記する
* 例題 → 演習 → 小テストを作る
* “何を覚えるべきか” を箇条書きで締める
* 重要事項を admonish（囲み）に落とす

mdBook 側で演習/小テストを実装したいなら、次を使う。

* 演習: mdbook-exercises ([GitHub][7])
* 小テスト: mdbook-quiz ([GitHub][6])

**超重要**: 出典（URL）を章末に必ず残す
→ LLMの“教科書化”がうまくいっても、後で検証・更新できない本は運用が死にます。

### フェーズE: ビルド & QA

* HTML: `mdbook build`
* PDF:

  * typst経由: mdbook-typst ([GitHub][10])
  * Chrome印刷: mdbook-pdf ([GitHub][11])
* QA:

  * リンク切れ: mdbook-linkcheck ([GitHub][8])

---

## まずは最短で動くMVP（CLI寄せ）

「まず1冊出る」までを最短でやるルートです（後でRust実装に置き換え）。

```sh
# 1) クローラ
cargo install spider_cli --features smart

# 2) 本文抽出（Markdown化）
cargo install readability-js-cli

# 3) 本（mdBook）とPDF出力
cargo install mdbook
cargo install mdbook-typst
# typst CLIはmdbook-typst READMEの手順に従う（例: cargo install --git ...） :contentReference[oaicite:25]{index=25}
```

クロールして HTML を保存する例は次のとおり。

```sh
spider --url https://example.com/docs download -t data/html
```

（`download` でHTMLを保存できる例が示されています） ([Docs.rs][14])

HTML→Markdown 抽出の例は次のとおり。

```sh
# 例: ファイルごとに readable を通す
find data/html -name '*.html' -print0 | xargs -0 -I{} sh -c '
  out="data/extracted/$(basename "{}" .html).md"
  readable "{}" > "$out"
'
```

`readable` はファイル/URL/STDINを処理してMarkdownを出せます。 ([Docs.rs][3])

mdBook に取り込む例は次のとおり。

```sh
mdbook init book
cp data/extracted/*.md book/src/
# SUMMARY.md を作る（最初は手でもいい）
mdbook build book
```

PDF を Typst で出すには、`book/book.toml` に以下を追加する（最小）。

```toml
[output.typst]
[output.typst.output]
format = "pdf"
```

（この設定例がREADMEにあります） ([GitHub][10])

---

## 「Rustベース」に寄せた最終形（おすすめ構成）

大量ページを処理しはじめると、CLIを何千回も呼ぶのがボトルネックになります。そこで最終形はこう。

* `textbookify`（自作Rust CLI）

  * spider（ライブラリ）でクロール（または spider_cli を呼ぶ）
  * readability-js（ライブラリ）を **1回初期化して使い回す**（コストが明記されているので、ここが効く） ([Docs.rs][3])
  * 変換結果を `data/extracted/*.md` に保存
  * URL階層から `SUMMARY.md` を自動生成
  * LLMで章を生成/修正（mistral.rs などのOpenAI互換サーバーにHTTP） ([GitHub][13])
  * 最後に `mdbook build` を叩く

---

## 教科書としての“型”（LLMに作らせるテンプレ）

各章をこの型に揃えると、教材っぽさが一気に出ます。

* 章タイトル
* この章でできるようになること（3〜5個）
* 前提（知らないと詰む用語/概念）
* 本文（見出しは深くしすぎない）
* よくある間違い（admonishで）
* 例題（1〜2）
* 演習（3〜10、難易度別）
* 小テスト（5問）
* まとめ（箇条書き）
* 参考（出典URLの一覧）

mdBook 側の表現は次のプラグインで実装できる。

* 囲み: mdbook-admonish ([Docs.rs][5])
* 演習: mdbook-exercises ([GitHub][7])
* 小テスト: mdbook-quiz ([GitHub][6])



---

## 難所（先に潰しておくと強い）

1. **階層＝学習順ではない**

* サイトは「辞書順/参照順」になりがち
  → 最初はURL階層でTOCを作り、次にLLMで「学習順への並び替え提案」を出すのが現実的

2. **ページ抽出の品質がサイトごとに違う**

* readabilityは強いが万能ではない
  → 失敗ページだけフォールバック（html-to-markdown 等）を用意しておく ([Docs.rs][4])

3. **PDFは“見た目”で評価が決まる**

* 可能なら typst 経由（mdbook-typst）を最初から試す ([GitHub][10])
* ダメなら mdbook-pdf（Chrome印刷）で逃げ道 ([GitHub][11])

4. **権利/利用条件**

* 教科書化は二次利用・派生物になりやすい
  → 最低限「出典 URL の保持」「利用規約/ライセンスの確認」「社内利用と公開の線引き」は最初に決める。

---

## 次にやると良い“最小ステップ”

1. 1つの小さなサイト（またはサイトの一部ディレクトリ）に絞って
2. `spider_cli download` → `readable` → mdBook → PDF（typst）まで通す
3. 抽出が崩れるページのパターンを3つ集める
4. そこから「抽出ルール（除外CSS/フォールバック）」「章立てルール」を固める

この順番だと、最短で「本が出る」→「品質を上げる」に移れます。

---

必要なら、この設計を **“実装しやすい Rust プロジェクト構成（crate 分割・データ形式・コマンド設計）”** まで落とし込む。
さらに、`textbookify` のコマンド案（例: `crawl` / `extract` / `toc` / `draft` / `build`）も具体化する。

[1]: https://github.com/rust-lang/mdBook?utm_source=chatgpt.com "GitHub - rust-lang/mdBook: Create book from markdown files. Like ..."
[2]: https://docs.rs/crate/spider_cli/latest?utm_source=chatgpt.com "spider_cli 2.38.88 - Docs.rs"
[3]: https://docs.rs/readability-js "readability_js - Rust"
[4]: https://docs.rs/crate/html-to-markdown-cli/latest?utm_source=chatgpt.com "html-to-markdown-cli 2.22.5 - Docs.rs"
[5]: https://docs.rs/crate/mdbook-admonish/latest?utm_source=chatgpt.com "mdbook-admonish 1.20.0 - Docs.rs"
[6]: https://github.com/cognitive-engineering-lab/mdbook-quiz?utm_source=chatgpt.com "mdbook-quiz: interactive quizzes for Markdown - GitHub"
[7]: https://github.com/guyernest/mdbook-exercises?utm_source=chatgpt.com "GitHub - guyernest/mdbook-exercises: Parser and renderer for exercise ..."
[8]: https://github.com/Michael-F-Bryan/mdbook-linkcheck?utm_source=chatgpt.com "Michael-F-Bryan/mdbook-linkcheck - GitHub"
[9]: https://github-wiki-see.page/m/rust-lang/mdBook/wiki/Third-party-plugins " Third party plugins - rust-lang/mdBook GitHub Wiki "
[10]: https://github.com/LegNeato/mdbook-typst "GitHub - LegNeato/mdbook-typst: An mdBook backend to output Typst markup, pdf, png, or svg"
[11]: https://github.com/HollowMan6/mdbook-pdf "GitHub - HollowMan6/mdbook-pdf: A backend for mdBook written in Rust for generating PDF based on headless chrome and Chrome DevTools Protocol. (用 Rust 编写的 mdBook 后端，基于headless chrome和Chrome开发工具协议生成PDF)"
[12]: https://github.com/max-heller/mdbook-pandoc?utm_source=chatgpt.com "GitHub - max-heller/mdbook-pandoc: A mdbook backend powered by Pandoc."
[13]: https://github.com/EricLBuehler/mistral.rs?utm_source=chatgpt.com "GitHub - EricLBuehler/mistral.rs: Blazingly fast LLM inference."
[14]: https://docs.rs/crate/spider_cli/latest/source/README.md?utm_source=chatgpt.com "spider_cli 1.89.13 - Docs.rs"

いい流れです。spider → readability で「素材（ページ群）」を作り、goose / Codex で「教科書（章立て＋編集＋演習）」にする、が一番現実的。
「もう少し詰める」なら、次の2点を先に固定すると一気に前に進みます。


LLM（goose/Codex）が触る範囲を厳密に分ける（素材は不変、教科書は生成物）


教科書化の“入出力フォーマット”を決める（manifest + toc + chapter packets）


以下、その前提で、実務で回る形に落とし込みます。

1) 3レイヤ構成にすると破綻しない
レイヤA：Raw（取得物、再現性のために保存）


raw/html/...（spiderで落としたHTML）


raw/assets/...（画像やCSS、必要なら）


レイヤB：Extracted（本文抽出済みの“素材”＝不変）


extracted/pages/<page_id>.md


先頭に メタデータ（出典URL、取得日時、タイトル、推定階層） を入れる
→ これがないと、後で更新・検証・差分追跡が死にます。


レイヤC：Book（教科書＝生成物。ここだけLLMが触る）


book/src/...（mdBook想定。最初は単にMarkdown束でもOK）


book/src/SUMMARY.md（目次）


book/src/chapters/*.md（章本文）



コツ: LLM は Extracted を編集しない。必ず Book 側へ新規で書く。


2) “詰める”ための中間成果物：manifest と toc を先に作る
(A) manifest.jsonl（ページ台帳）
1 行 1 ページで OK です。最低限、次の項目を入れると強いです。

```json
{"id":"p_001","url":"...","title":"...","path":"/docs/guide/intro","breadcrumbs":["Docs","Guide"],"extracted_md":"extracted/pages/p_001.md"}
```


これがあると、**「未収録ページ」「重複」「章への割当」**が機械的に管理できます。

(B) toc.yaml（教科書の章立て＝編集方針）
ここが “サイト階層 → 学習順” の変換レイヤ。

```yaml
book_title: "XXX 教科書"
parts:
  - title: "Part 1: 基礎"
    chapters:
      - id: "ch01"
        title: "全体像と最初の一歩"
        sources: ["p_001","p_002","p_010"]
      - id: "ch02"
        title: "主要概念"
        sources: ["p_011","p_012"]
  - title: "Part 2: 実践"
    chapters:
      - id: "ch10"
        title: "実装パターン"
        sources: ["p_101","p_102"]
```

この toc.yaml が決まれば、あとは自動化できます。

3) goose と Codex の使い分け（ここも詰めると迷わない）
Codex CLI（OpenAI）


リポジトリを読んで、ファイル編集・コマンド実行できるローカルのコーディングエージェント。公式に案内されています。 


AGENTS.md による指示の自動読み込みができ、探索順序も仕様化されています（AGENTS.override.md→AGENTS.md→…）。 


OSS（Apache-2.0）。 


→ “Book生成”だけに集中させるなら、まずCodex単体が最短。
goose（Block）


ローカルで動くオープンソースAIエージェント（Apache-2.0）。 


goose session でセッション開始できる。 


さらに「CLIプロバイダ」として Codex CLI等をgoose経由で使うこともでき、セッション管理やワークフローに寄せられる。 


ただし、CLIプロバイダ経由だと goose の拡張（MCPなど）にはアクセスできないという制約が明記されています。 


→ 定期更新やレシピ化（毎週再生成）まで見据えるならgoose、
→ まずは Codexで教科書化の型を固めるのが早いです。

4) “教科書化”を失敗させないための生成フロー（LLM側の仕事を分割）
LLMにいきなり「全部まとめて教科書にして」は、ほぼ確実に破綻します（情報量/整合性/重複）。
ステップ1：ページ要約カードを作る（Extracted→Cards）
各ページに対して、まず 200〜500字のカードを作ります。


```md
# cards/<page_id>.md

## 何が書いてあるか（要点3つ）
## 前提知識
## 重要用語
## “このページの位置づけ”（導入/詳細/リファレンス/FAQ）
```




これをやると、章の設計精度が跳ねます。
ステップ2：toc.yaml を LLM に“提案”させ、人間が確定する。


LLM は manifest + cards から章立て案を作る。


人間は「学習順になってるか」「重複がまとまってるか」だけ見る
ここが最小の“編集者作業”。


ステップ3：章ごとの “packet” を作ってから章本文生成
chapter_packets/ch01.md みたいに、章に紐づく素材だけを結合した入力を作ります。


```md
# chapter_packets/ch01.md

## 章の学習目標（toc から）
## 含めるページのカード
## 含めるページの本文（必要部分だけでも OK）
## 出典 URL 一覧
```


→ LLMは ch01 packetだけ読んで ch01 を書く。これでコンテキスト爆発を防ぎます。
ステップ4：章本文はテンプレ固定（教科書の型）
各章はこのテンプレに必ず合わせる。

```text
- 学習目標（3〜5）
- 前提
- 本文（概念→例→注意点）
- よくある間違い
- ミニ演習（3問）
- 小テスト（5問）
- まとめ
- Sources（出典 URL）
```



5) AGENTS.md を用意して、エージェントの暴走を防ぐ（超重要）
Codex は AGENTS.md を自動検出して指示として使える。
最初からこれを書いておくと、生成の品質が安定する。
例（かなり実用寄り）は次のとおり。

```md
# AGENTS.md - Textbook Builder Rules

## Goal
Create a textbook-style document in `book/src/` using source materials in `extracted/pages/` and the mapping in `toc.yaml`.

## Hard constraints
- Never edit files under `extracted/` or `raw/`. They are immutable snapshots.
- Every chapter MUST include a "Sources" section listing source URLs (from front-matter).
- Do not invent facts. If information is missing, write "TODO" and cite which source pages were insufficient.
- Keep code blocks verbatim when quoting from sources. Do not “fix” APIs unless the source confirms it.

## Output format
- Markdown only.
- Chapter template:
  - Objectives
  - Prerequisites
  - Core explanation
  - Examples
  - Common pitfalls
  - Exercises
  - Quiz
  - Summary
  - Sources

## Build / QA commands
- Build HTML: `mdbook build book` (if mdBook is present)
- Link check: (if configured) `mdbook test book`
- Ensure all pages in `toc.yaml` are referenced in some chapter.

## Workflow
1) Read `toc.yaml` to know chapter mapping.
2) For each chapter, read only the listed source pages.
3) Write/update `book/src/chapters/<chapter_id>.md`.
4) Update `book/src/SUMMARY.md`.

```


6) まずの“完成定義”（MVP）を1つ決める
あなたの今の段階なら、最初の MVP はこれが最強です。
MVP-1（最短で形が出る）


- spider → readability 済みで `extracted/pages/*.md` ができている。
- `manifest.jsonl` ができている。
- `toc.yaml` ができている（章 5〜10 程度）。
- `book/src/chapters/ch01.md` だけ教科書テンプレで完成している。
- Sources が付いている。


これができれば、あとは「章を増やす」だけの作業に落ちます。

7) 次の一手：あなたの案を“詰める”具体的TODO
あなたの「spider→readability→goose/codexでまとめる」を、実装タスクに落とすとこうです。


- 出力ディレクトリ設計を確定する（上の 3 レイヤ）。
- extract 結果に必ずメタデータを入れる（url/title/取得日）。
- `manifest.jsonl` を生成する（自動）。
- `toc.yaml` を作る（LLM 提案→人間確定）。
- `chapter_packets/` を作る（章ごとに素材を束ねる）。
- Codex または goose で `book/src/chapters/` を生成する。
- QA を行う（Sources 必須、未収録ページ検出）。



もし「詰める」をさらに一段進めるなら、次は **“toc.yaml をどう自動で初期生成するか（URL階層/パンくず/ナビ）”**が一番効きます。
ただ、ここでは質問で止めずに進める案を示す。


- まず URL パス階層で toc を自動生成する（雑で OK）。


- その後、カード要約→toc 再編を LLM に行わせる。


この順番が最短で「学習順の目次」に到達します。
必要なら、manifest.jsonl と toc.yaml を生成するための Rust CLI（最小）を設計する。
コマンド案は `crawl` / `extract` / `manifest` / `toc-draft` / `packetize` である。
