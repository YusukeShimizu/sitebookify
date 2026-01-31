# Sitebookify Web MVP – Execution Plan（Local-first / Cloud Run-ready / gRPC-Web）

この `exec-plan.md` は `plan.md` を元に、**将来的に “1つの Cloud Run サービスに全部入り” でデプロイできる**将来性を見越しつつ、  
**まずは全部ローカル（Nix）で動く MVP**を最短で作るための実行計画です。  
ブラウザからの呼び出しは **gRPC（= ブラウザ都合で gRPC-Web）**を採用します。

---

## 0. MVP のゴール / 非ゴール

### ゴール（MVP）
- ローカルで、ブラウザから **URL入力 → ジョブ開始 → 進捗表示 → 成果物DL** が完結する
- API は **gRPC-Web**（Rust: `tonic` + `tonic-web`、Web: Buf 生成 + gRPC-Web/Connect クライアント）
- ジョブは “受付” と “実行” を分離し、**非同期処理**として回る（UI はポーリングで追従）
- 既存の `sitebookify build` パイプラインを最大限再利用する
- （将来）同じ構成を **Cloud Run 1サービス**に載せ替えできる（Store/Queue を差し替え）

### 非ゴール（今はやらない）
- 認証/課金/権限制御（最初は public 想定。後で IAP 等に拡張）
- robots.txt や高度なクロール制御の網羅
- PDF/EPUB 生成（まずは `book.md + assets/` を zip にして配布。後で追加）
- ストリーミング進捗（`WatchJob`）の完全対応（まずは `GetJob` ポーリング）

---

## 1. 先に決めること（迷ったらデフォルトで進める）

1) **成果物形式（MVP）**
- `book.md + assets/` を zip 化して **ローカルに保存** → ダウンロードURLでDL
- （将来）GCS に保存 → 署名URLでDL

2) **クロール範囲（安全のための制限）**
- 起点 URL と同一 origin（scheme+host+port）配下のみ
- `http/https` のみ許可、ローカル/メタデータ系へのアクセスは弾く（最低限）

3) **LLM（TOC作成/本文書き換え）**
- MVP: `noop`（ローカル/Cloud Run のどちらでも外部依存なく回す）
- 後で: “サーバ向け” エンジン（OpenAI API など）を追加
  - ※現状の仕様（Codex CLI 呼び出し）は Cloud Run と相性が悪いので、Web 版は別実装前提にする

---

## 2. MVP アーキテクチャ（ローカル “全部入り” → Cloud Run “全部入り” に載せ替え）

MVP は **ローカルで “全部入り”**（Web + gRPC-Web API + job runner）を 1プロセスで動かします。  
将来的には **同じ “全部入り” を 1つの Cloud Run サービス**に載せ替えます。

ただし、クロール/生成は長時間になり得るため、**重処理は別リクエストで実行**できる形にします。

### コンポーネント（MVP）
- **Local process: `sitebookify-app`**
  - React 静的配信（`/` と `/assets/*`）
  - gRPC-Web API（`StartCrawl`, `GetJob`, `GetDownloadUrl`）
  - 内部用 “job runner” HTTP endpoint（同一プロセス内で実行される）
- **Local JobStore（FS）**: job の状態/進捗/成果物パス
- **Local ArtifactStore（FS）**: 成果物（zip）を保存
- **Local Queue（in-process）**: job runner を非同期に叩く（`tokio::spawn` 等）

### フロー（MVP）
1. ブラウザ → gRPC-Web `StartCrawl(url, options)`  
2. API: JobStore に job 作成（`QUEUED`）→ Queue に enqueue  
3. Queue → job runner endpoint を呼ぶ（job 実行）  
4. job runner: `sitebookify build` を走らせる → 進捗を JobStore 更新 → 成果物 zip を ArtifactStore に保存 → `DONE`  
5. ブラウザ → `GetJob` をポーリングして進捗表示  
6. 完了後、ブラウザ → `GetDownloadUrl` → ダウンロードURLでDL

### 将来の載せ替え（Cloud Run）
ローカル MVP の “差し替えポイント” をそのまま Cloud 側実装に置換する。

- `JobStore`: Local FS → Firestore
- `ArtifactStore`: Local FS → GCS（+ 署名URL）
- `Queue`: in-process → Cloud Tasks（同じ Cloud Run サービスの job runner endpoint を叩く）

---

## 3. gRPC API（MVP の最小）

ブラウザ向けは gRPC-Web を想定し、まずは 3 RPC で回します。

- `StartCrawl(StartCrawlRequest) returns (StartCrawlResponse)`
- `GetJob(GetJobRequest) returns (Job)`
- `GetDownloadUrl(GetDownloadUrlRequest) returns (GetDownloadUrlResponse)`
- （後で）`WatchJob(WatchJobRequest) returns (stream JobEvent)`

`StartCrawlRequest` には最低限 `url` と、必要なら `max_depth/max_pages/language/tone/engine` を含める。  
MVP は `engine=noop` をデフォルトにする。

---

## 4. マイルストーン（DoD 付き）

### M0: ベースライン固定（既存 CLI を壊さない）
**作業**
- `nix develop -c just ci` が通ることを確認
- `sitebookify build ... --engine noop` の E2E が通ることを確認

**DoD**
- 既存の CLI が壊れていない状態で拡張に着手できる

---

### M1: proto & コード生成（Rust/TS）
**作業**
- `proto/sitebookify/v1/service.proto` を追加（API 用）
- Buf で生成パイプラインを用意
  - Rust: `tonic`（サーバ実装用）
  - Web: gRPC-Web/Connect クライアント生成（型付き）

**DoD**
- Web 側から “型が付いた状態” で `StartCrawl` を呼べる土台ができる（実装は stub でも可）

---

### M2: ローカル App（API）実装 – 受付/状態/ダウンロードURL
**作業**
- Rust で gRPC-Web サーバを実装（`tonic` + `tonic-web`）
  - `accept_http1(true)`（gRPC-Web）
  - 同一プロセスで静的配信するなら CORS は不要
- Local JobStore に job 状態を保存（`QUEUED/RUNNING/DONE/ERROR` + progress/message）
- `GetDownloadUrl` はローカルのダウンロード endpoint URL を返す（`DONE` のときだけ）

**DoD**
- ローカルで “開始→状態取得→DL URL 発行” ができる

---

### M3: 非同期実行（同一プロセス内 job runner + in-process Queue）
**作業**
- `StartCrawl` で in-process Queue に task を enqueue（payload に `job_id`）
- 内部 endpoint（HTTP）で job を実行
  - `sitebookify build`（または同等のライブラリ呼び出し）を実行
  - 主要ステップ単位で JobStore を更新（粗い進捗でOK）
  - `book.md + assets/` を zip 化 → ArtifactStore に保存
  - job を `DONE` に更新（artifact path 保存）

**DoD**
- ローカルで “StartCrawl → 非同期で完了 → URL から成果物DL” が一気通貫で動く

---

### M4: Web UI（React + shadcn/ui）MVP
**作業**
- 1画面 UI（`plan.md` の Hero/CommandInput 相当）
  - URL input（Enter で開始でもOK）
  - 進捗（`GetJob` ポーリング）
  - DONE で Download（`GetDownloadUrl`）
  - ERROR 表示（Toast/Sonner）

**DoD**
- ローカルの UI だけで “開始→進捗→DL” が完結する（将来 Cloud Run でも同様）

---

### M5: デプロイ導線（後で IaC に寄せられる形）
**作業**
- Dockerfile（Web build + Rust build のマルチステージ）
- `just deploy`（または `infra/` に `gcloud` スクリプト）
  - Firestore / GCS / Cloud Tasks / Cloud Run / SA 権限のセットアップ手順を固定
- 環境変数・Secret 管理の方針を決める（MVP は `noop` なら最小）

**DoD**
- “手順書どおりに実行すると Cloud Run に上がる” が再現できる

---

## 5. ローカル開発（Nix）での最低ライン

まずは “全部ローカル” で回す導線を作る。

- `nix develop` で API/Web を起動できる
- MVP は `noop` エンジンで E2E をまず成立させる
- 成果物はローカル FS に保存して DL できる

---

## 6. 次フェーズ（育ったら）

- `WatchJob`（server streaming）導入
- 並列数制御（in-process の同時実行数制限 → Cloud Tasks の rate/dispatch に移行）
- 認証（IAP / OAuth）導入
- LLM エンジンのサーバ実装（Secret Manager・レート制限・監査ログ）
  - 必要なら、このタイミングで Worker を別サービス/Jobs に分離してスケール特性を分ける（任意）
