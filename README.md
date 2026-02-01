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

## Docker（sitebookify-app）

ローカルで Web MVP を 1 コンテナで動かしたい場合は `sitebookify-app` を Docker で起動できる。

```sh
docker build -t sitebookify-app:local .
docker run --rm -p 8080:8080 sitebookify-app:local
```

ヘルスチェック（例）。

```sh
curl -fsS http://127.0.0.1:8080/healthz
```

`sitebookify-app` はデータ保存先として、デフォルトで `workspace-app/` を使う。  
コンテナでは `CMD` で `/tmp/workspace-app` を指定している。  
Cloud Run などの read-only FS を想定している。

## rust-analyzer（VS Code）

rust-analyzer が標準ライブラリ（`std`）を解析できるように、次を設定する。

- Nix devShell で `RUST_SRC_PATH` を設定する。
- direnv で `rust-lib-src`（標準ライブラリソースへの symlink）を作成する。
- `.vscode/settings.json` で `rust-analyzer.cargo.sysrootSrc` を `${workspaceFolder}/rust-lib-src` に向ける。

## 実行例

```sh
sitebookify build --url https://example.com/docs/ --out workspace
# --title は任意（省略時は toc.yaml / LLM から自動決定）
# sitebookify build --url https://example.com/docs/ --out workspace --title "Example Docs Textbook"
```

TOC 作成と本文の書き換えは OpenAI（CLI 経由）を利用する。
デフォルトでは `openai` エンジンを利用する。

```sh
# 言語とトーンを指定できる（ニュアンス可変）
sitebookify build --url https://example.com/docs/ --out workspace --language 日本語 --tone 丁寧
```

OpenAI エンジン（CLI 経由）のバイナリやモデルは環境変数で指定できる。

```sh
echo 'export SITEBOOKIFY_OPENAI_MODEL=o3' > .envrc.local
echo 'export SITEBOOKIFY_OPENAI_REASONING_EFFORT=high' >> .envrc.local
# 必要なら CLI バイナリ名も指定する（未指定の場合は `codex` を優先して自動検出する）
# echo 'export SITEBOOKIFY_OPENAI_BIN=codex' >> .envrc.local
direnv allow
```

ワークスペースの中身（MVP）は次の通り。

```text
workspace/
  raw/
  extracted/
  manifest.jsonl
  toc.yaml
  book/
  assets/
  book.md
```

手動で実行したい場合は、次の順に実行する。

```sh
sitebookify crawl --url https://example.com/docs/ --out raw
sitebookify extract --raw raw --out extracted
sitebookify manifest --extracted extracted --out manifest.jsonl
sitebookify toc create --manifest manifest.jsonl --out toc.yaml --language 日本語 --tone 丁寧 --engine openai
sitebookify book init --out book --title "Example Docs Textbook"
sitebookify book render --toc toc.yaml --manifest manifest.jsonl --out book --language 日本語 --tone 丁寧 --engine openai
```

OpenAI を使わずに動作確認したい場合は `noop` を使う。

```sh
sitebookify toc create --manifest manifest.jsonl --out toc.yaml --language 日本語 --tone 丁寧 --engine noop
sitebookify book render --toc toc.yaml --manifest manifest.jsonl --out book --language 日本語 --tone 丁寧 --engine noop
```

## Web MVP（ローカル）

`sitebookify-app`（Web 静的配信 + gRPC-Web API + in-process job runner）をローカルで起動できる。

### Dev（Vite）

ターミナルを 2 つ使う。

```sh
# terminal 1 (API)
direnv allow
just dev_app
```

```sh
# terminal 2 (Web)
direnv allow
just web_install
just web_gen
just web_dev
```

ブラウザで `http://127.0.0.1:5173` を開く。

### Build（静的配信）

```sh
direnv allow
just web_install
just web_gen
just web_build
just dev_app
```

ブラウザで `http://127.0.0.1:8080` を開く。

## 1ファイル出力（Bundle）

`book render` 後に、mdBook 出力を 1 つの Markdown に統合して出力できる。
また、内部リンクを可能な範囲で維持するために、次を行う。

- `book render` は、画像を `book/src/assets/` にダウンロードし、参照先をローカルパス（`../assets/...`）に書き換える。
- `manifest.jsonl` に存在するページへのリンクは、章内/章間リンク（`#p_...` / `chXX.md#p_...`）に書き換える。
- `book bundle` は、章間リンク（`chXX.md#p_...`）を 1 ファイル内のアンカー（`#p_...`）に書き換える。
- `book bundle` は、`book/src/assets/` を `out` の隣の `assets/` にコピーし、画像パスを `assets/...` に書き換える。

```sh
sitebookify book bundle --book book --out book.md
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

## CI: GCP Artifact Registry へ Docker image を push

GitHub Actions から GCP Artifact Registry に `sitebookify-app` のイメージを push する workflow を用意している。

- Workflow: `.github/workflows/image-gcp.yml`
- 認証: GitHub OIDC → GCP Workload Identity Federation（鍵ファイル不要）

GitHub Actions Variables で次を設定する。  
設定場所: Repository Settings → Secrets and variables → Actions → Variables。

- `GCP_PROJECT_ID`
- `GCP_REGION`（例: `asia-northeast1`）
- `GAR_REPOSITORY`（Artifact Registry のリポジトリ名）
- `GCP_WORKLOAD_IDENTITY_PROVIDER`（Workload Identity Provider のリソース名）
- `GCP_SERVICE_ACCOUNT`（例: `github-actions@<project>.iam.gserviceaccount.com`）

GCP 側のセットアップ例（概略）。

```sh
# 例: 事前に gcloud auth login 済み
PROJECT_ID="<your-project-id>"
REGION="<your-region>" # e.g. asia-northeast1
REPO_NAME="<your-ar-repo>" # e.g. sitebookify
SA_NAME="github-actions"

gcloud services enable artifactregistry.googleapis.com iamcredentials.googleapis.com iam.googleapis.com sts.googleapis.com

gcloud artifacts repositories create "${REPO_NAME}" \
  --repository-format=docker \
  --location="${REGION}"
gcloud iam service-accounts create "${SA_NAME}" \
  --project "${PROJECT_ID}"

gcloud artifacts repositories add-iam-policy-binding "${REPO_NAME}" \
  --location "${REGION}" \
  --member "serviceAccount:${SA_NAME}@${PROJECT_ID}.iam.gserviceaccount.com" \
  --role "roles/artifactregistry.writer"
```

Workload Identity Federation の手順は構成に依存する。  
`google-github-actions/auth` のドキュメントに従って設定する。  
GitHub OIDC 発行者（`https://token.actions.githubusercontent.com`）を許可する。  
対象リポジトリ（`<owner>/<repo>`）から `GCP_SERVICE_ACCOUNT` を impersonate できるように設定する。
