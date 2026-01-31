以下は、**Rust + gRPC-Web + React + shadcn/ui**で、画像のような「1画面ランディング + URL入力でジョブ開始」UIを作り、**Cloud Run**で運用する前提の“現実的に回る”構成案です。
（Sitebookify っぽく、入力→ジョブ開始→進捗→成果物DL までを想定しています）

---

## 1) 全体アーキテクチャ（推奨：Cloud Run で “Web + API” を分離、処理は Worker に逃がす）

ドキュメントクロール＆PDF/EPUB生成は **同期HTTPで完結しない**ことが多いので、**APIはジョブ受付**、**生成は非同期Worker**に分けるのが安定です。

### 構成図（最小で強い）

```
[React(shadcn/ui) on Cloud Run]  --- gRPC-Web --->  [API (Rust/tonic + tonic-web) on Cloud Run]
        |                                                          |
        | (進捗表示: Watch or Poll)                                | (ジョブ状態/メタ)
        |                                                          v
        |                                                     [Firestore]
        |                                                          |
        | (DLボタン)                                                | (キュー投入)
        v                                                          v
 [Signed URLでGCSからDL]  <--- 成果物 ---  [Worker (Rust) on Cloud Run Jobs or Cloud Run service]
                                       |
                                       v
                                  [GCS Bucket]
```

* **Web**: React + shadcn/ui（静的配信でもSSRでもOK）
* **API**: Rust（tonic）で **gRPC** を提供しつつ、ブラウザ用に **gRPC-Web** を有効化
* **Worker**: クロール＆レンダリング等の重処理（タイムアウト/メモリ/CPUをここに寄せる）
* **Firestore**: job状態（queued/running/done/error）、進捗、成果物パス
* **GCS**: PDF/EPUBを保存、APIが署名付きURLを返す

---

## 2) gRPC-Web どうする？（Rust側は Envoy不要にできる）

### 推奨：`tonic-web` で Rust 単体で gRPC-Web 対応

ブラウザ→API間の変換プロキシ（Envoy等）を挟まず、Rustで完結できます。

* tonic (gRPC) + **tonic-web** (gRPC-Web対応)
* Cloud Run は HTTP/1.1 でもOKなので、サーバ側を **`accept_http1(true)`** で起動
* WebとAPIを別サービスにするなら **CORS** を適切に許可

> Envoyを挟む構成も可能ですが、まずは tonic-web が一番シンプルです（運用部品が減る）。

---

## 3) リポジトリ構成（モノレポ推奨）

proto共有＆型生成を素直に回すために、monorepo が楽です。

```
repo/
  proto/
    sitebookify/v1/sitebookify.proto
    buf.yaml
    buf.gen.yaml
  backend/
    Cargo.toml
    src/
      main.rs
      grpc.rs
      jobs.rs
      storage_gcs.rs
      db_firestore.rs
  worker/
    Cargo.toml
    src/main.rs
  web/
    package.json
    src/
      app/
      components/
      lib/
    tailwind.config.ts
  infra/
    terraform/ (またはgcloud run deployスクリプト)
```

### コード生成（おすすめ）

* **Buf**でproto管理
* Rust: `prost` / `tonic-build`
* Web(TS): `ts-proto` か BufのTS生成（好み）
* gRPC-Webクライアントは `grpc-web` 系 or connect系（「gRPC-Web」を明示するなら grpc-web でOK）

---

## 4) gRPC設計（UIに必要な最小API）

画像のUIは入力欄が主役なので、最低限はこれで動きます。

* `StartCrawl(url, options)` → `job_id`
* `GetJob(job_id)` → status/progress
* `WatchJob(job_id)` →（できれば）server streaming で進捗イベント
* `GetDownloadUrl(job_id)` → 署名付きURL（GCS）

### proto例（イメージ）

```proto
syntax = "proto3";
package sitebookify.v1;

service SitebookifyService {
  rpc StartCrawl(StartCrawlRequest) returns (StartCrawlResponse);
  rpc GetJob(GetJobRequest) returns (Job);
  rpc WatchJob(WatchJobRequest) returns (stream JobEvent);
  rpc GetDownloadUrl(GetDownloadUrlRequest) returns (GetDownloadUrlResponse);
}

message StartCrawlRequest {
  string url = 1;            // "https://docs.rs/tokio/latest/"
  uint32 max_depth = 2;
  enum OutputFormat { PDF = 0; EPUB = 1; }
  OutputFormat format = 3;
}

message StartCrawlResponse { string job_id = 1; }

message GetJobRequest { string job_id = 1; }
message WatchJobRequest { string job_id = 1; }

message Job {
  string job_id = 1;
  enum Status { QUEUED=0; RUNNING=1; DONE=2; ERROR=3; }
  Status status = 2;
  uint32 progress_percent = 3;
  string message = 4;
  string artifact_path = 5; // "gs://bucket/xxx.pdf" など
}

message JobEvent {
  uint32 progress_percent = 1;
  string message = 2;
  bool done = 3;
}

message GetDownloadUrlRequest { string job_id = 1; }
message GetDownloadUrlResponse { string url = 1; uint32 expires_sec = 2; }
```

---

## 5) Rust(API)の実装方針（tonic-web + CORS）

### APIサーバ責務

* リクエスト検証（URL形式・ドメイン制限など）
* Firestoreにjob作成（QUEUED）
* Pub/Sub or Cloud Tasks に投入（Worker起動）
* `GetJob`/`WatchJob` で状態を返す
* DONEなら `GetDownloadUrl` で署名URL発行

### gRPC-Web有効化の要点

* `tonic_web::enable(...)`
* `Server::builder().accept_http1(true)`（ブラウザ対応）
* 別ドメインなら `tower_http::cors::CorsLayer` で

  * `POST, OPTIONS`
  * `content-type, x-grpc-web, grpc-timeout` 等を許可
  * `expose-headers: grpc-status, grpc-message` 等を許可

---

## 6) Workerの実装方針（Cloud Run Jobs 推奨）

クロール・PDF化は「時間がかかる」「メモリ食う」「再試行したい」ので、Workerは分離。

### Worker責務

* job_id を受け取る（Pub/Sub メッセージなど）
* クロール実行→中間成果保存→PDF/EPUB生成
* GCSにアップロード
* Firestoreに進捗更新（RUNNING→DONE/ERROR）

### 実行基盤の選択

* **Cloud Run Jobs**：バッチ向き（おすすめ）
* Cloud Run service + Pub/Sub push：常駐ワーカー的にもできる

---

## 7) React + shadcn/ui の画面構成（画像のUIをそのまま分解）

画像はほぼこのコンポーネント分割で再現できます。

### ページ構造

* `<TopNav />`：左上の `>_ sitebookify_` みたいなロゴ
* `<Hero />`：

  * 大見出し（2行）
  * サブコピー（URL/* をオフラインブックに）
  * `<CommandInput />`（コマンドライン風Input）
  * `<FeatureBadges />`（RECURSIVE CRAWL / AUTO-FLATTENING / RUST POWERED）
* `<Footer />`：Architecture / Source Code / License

### shadcn/ui で使う部品

* `Input`（コマンド入力）
* `Button`（右端に控えめな実行ボタン置くなら）
* `Badge`（特徴のラベル）
* `Separator`（フッターの区切り）
* `Toast / Sonner`（開始・完了通知）

### クライアント状態（最小）

* `url`（入力）
* `jobId`（StartCrawlで取得）
* `job`（GetJob/WatchJobで更新）
* `downloadUrl`（DONEになったら取得）

進捗の取り方は2案：

* **案A（堅実）**：`GetJob` を数秒ごとにポーリング（実装簡単）
* **案B（リッチ）**：`WatchJob` を server streaming（gRPC-Webでストリーム対応が必要）

---

## 8) Cloud Run デプロイパターン（おすすめ2択）

### パターンA：Web / API / Worker を全部 Cloud Run（分離）

**本番でスケールしやすい**、責務が明確。

* `sitebookify-web`：Reactビルド成果物を配信
* `sitebookify-api`：tonic + tonic-web（gRPC-Web）
* `sitebookify-worker`：Cloud Run Jobs（ジョブ処理）

**注意点**

* Web→API が別ドメイン/別サービスになるのでCORS必須

---

### パターンB：Cloud Run 1サービスに “Web静的配信 + gRPC-Web API” を同梱

**最速でシンプル**、CORS不要。最初のMVPに強い。

* Dockerビルド時に `web/` を `npm run build`
* 生成された `dist/` を Rustバイナリと同じコンテナに同梱
* Rust側で `/` は `dist/index.html`、`/assets/*` を配信
* `/sitebookify.v1.SitebookifyService/*` を gRPC-Web で処理

**注意点**

* 単体サービスに集約するので、後で分離したくなったら少し作業が必要

---

## 9) まずの“おすすめ結論”（迷ったらこれ）

* **MVP**：パターンB（1 Cloud Runサービス同梱） + `tonic-web` + Firestore + GCS
  → 画像のUIはすぐ作れて、APIも同一オリジンで楽
* **育ってきたら**：パターンAに移行（Web/API/Worker分離、キュー導入強化）

---

もしよければ、次のどれか1つだけ教えてください（確認というより、最適案を選ぶための前提です。答えがなくても上の構成で進められます）：

* **成果物はPDFのみ**？それとも **EPUBも必須**？
* クロールは「同一ドメイン配下のみ」など制限したい？
* 進捗は「バー表示が欲しい」 or 「完了通知だけでOK」？

不要なら、このまま **(1) Buf設定例 + (2) tonic-web付きRust起動コード骨子 + (3) React(shadcn)のHero/CommandInputの雛形**まで一気に具体化して提示できます。
