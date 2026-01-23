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

ワークスペースの中身（MVP）は次の通り。

```text
workspace/
  raw/
  extracted/
  manifest.jsonl
  toc.yaml
  book/
```

手動で実行したい場合は、次の順に実行する。

```sh
sitebookify crawl --url https://example.com/docs/ --out raw
sitebookify extract --raw raw --out extracted
sitebookify manifest --extracted extracted --out manifest.jsonl
sitebookify toc init --manifest manifest.jsonl --out toc.yaml
sitebookify book init --out book --title "Example Docs Textbook"
sitebookify book render --toc toc.yaml --manifest manifest.jsonl --out book
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
