# Sitebookify

Sitebookify は、ログイン不要の公開サイトをクロールし、本文抽出済み Markdown 素材を生成する。
章立て（TOC）に従って mdBook 形式の教科書 Markdown を出力する。

本文抽出は Mozilla Readability（Firefox Reader Mode）を利用する。

開発環境は Nix Flakes を正とする。
ローカルの環境変数は direnv（`.envrc`）で管理する。

## Quick start

```sh
direnv allow
just ci
```

direnv を使わない場合は、次を実行する。

```sh
nix develop -c just ci
```

## Build（バイナリ）

direnv を使う場合は、次でビルドできる。

```sh
cargo build
cargo build --release
```

direnv を使わない場合は、Nix devShell 経由で実行する。

```sh
nix develop -c cargo build
nix develop -c cargo build --release
```

生成されたバイナリは `target/release/sitebookify` に出力される。

## Tests

```sh
nix develop -c cargo test --all
```

## rust-analyzer（VS Code）

rust-analyzer が標準ライブラリ（`std`）を解析できるように、次を設定する。

- Nix devShell で `RUST_SRC_PATH` を設定する。
- direnv で `rust-lib-src`（標準ライブラリソースへの symlink）を作成する。
- `.vscode/settings.json` で `rust-analyzer.cargo.sysrootSrc` を `${workspaceFolder}/rust-lib-src` に向ける。

## 実行例

```sh
sitebookify build --url https://example.com/docs/ --out workspace --title "Example Docs Textbook"
```

翻訳まで含める場合は、`--translate-to` を指定する。

```sh
sitebookify build \
  --url https://example.com/docs/ \
  --out workspace \
  --title "Example Docs Textbook" \
  --translate-to ja \
  --translate-engine openai \
  --openai-model gpt-4.1
```

章立て（chapter と順序）も LLM で自動調整したい場合は `--toc-refine` を指定する。

```sh
sitebookify build \
  --url https://example.com/docs/ \
  --out workspace \
  --title "Example Docs Textbook" \
  --toc-refine \
  --toc-refine-engine openai \
  --openai-model gpt-4.1 \
  --translate-to ja \
  --translate-engine openai
```

ワークスペースの中身（MVP）は次の通り。

```text
workspace/
  raw/
  extracted/
  manifest.jsonl
  toc.yaml
  book/
  book.md
  book.<LANG>.md
```

手動で実行したい場合は、次の順に実行する。

```sh
sitebookify crawl --url https://example.com/docs/ --out raw
sitebookify extract --raw raw --out extracted
sitebookify manifest --extracted extracted --out manifest.jsonl
sitebookify toc init --manifest manifest.jsonl --out toc.yaml
# 章立てを LLM で調整したい場合（任意）
sitebookify toc refine --manifest manifest.jsonl --out toc.refined.yaml --book-title "Example Docs Textbook" --engine openai --openai-model gpt-4.1
sitebookify book init --out book --title "Example Docs Textbook"
# toc refine を実行しない場合は `--toc toc.yaml` を指定する
sitebookify book render --toc toc.refined.yaml --manifest manifest.jsonl --out book
```

## 1ファイル出力（Bundle）

`book render` 後に、mdBook 出力を 1 つの Markdown に統合して出力できる。

```sh
sitebookify book bundle --book book --out book.md
```

## 翻訳（LLM）

`book bundle` の出力（例: `book.md`）を翻訳できる。
翻訳時は、できるだけ元の Markdown 形態を保つ。

- 翻訳コマンドは **stdin で Markdown を受け取り、stdout に Markdown を返す**フィルタとして動作する必要がある。
- 目標言語は環境変数 `SITEBOOKIFY_TRANSLATE_TO` で渡される。

```sh
# 例: 翻訳エンジンとして外部コマンドを呼び出す
sitebookify llm translate --in book.md --out book.ja.md --to ja --engine command --command <TRANSLATOR> -- <ARGS...>
```

OpenAI API で翻訳する場合は `openai` を使う。
API キーは環境変数 `OPENAI_API_KEY` で渡す。

```sh
echo 'export OPENAI_API_KEY=...' > .envrc.local
direnv allow
sitebookify llm translate --in book.md --out book.ja.md --to ja --engine openai --openai-model gpt-4.1
```

入力が大きい場合は `--openai-max-chars` で分割サイズを調整する。

翻訳せずに（動作確認用に）入力をそのまま出力したい場合は `noop` を使う。

```sh
sitebookify llm translate --in book.md --out book.copy.md --to ja --engine noop
```

## 出力（Export）

統合/翻訳済み Markdown を `pandoc` 経由で `epub` / `pdf` 等に変換できる。

```sh
sitebookify export --in book.ja.md --out book.epub --format epub --title "Example Docs Textbook"
sitebookify export --in book.ja.md --out book.pdf --format pdf --title "Example Docs Textbook"
```

## Logging

`RUST_LOG` でログの詳細度を切り替える。

```sh
echo 'export RUST_LOG=debug' > .envrc.local
direnv allow
sitebookify crawl --url https://example.com/docs/ --out raw
```

## Protobuf（Buf）

Protobuf スキーマは `proto/` 配下で管理し、Buf で lint/format する。

```sh
buf lint
buf format -w
```

MVP では、Protobuf は API ではなくオンディスク形式（Manifest/TOC）のスキーマとして扱う。
スキーマは `proto/sitebookify/v1/` に置く。

## ドキュメント（Mintlify）

Mintlify で動かすことを前提に、ドキュメントは `docs/` 配下に置く。

- 設定: `docs/docs.json`
- Vale: `docs/.vale.ini`

CI では `just ci` がドキュメントの検査も実行する。
