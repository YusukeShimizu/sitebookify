# ExecPlan: DBレス・自動販売機モデル UX（Preview → Job Key → Delivery）

## Goal

- 「URL を入れるだけで本（Markdown）を作る」体験を、**DBレス / あと腐れなし**の前提で Web UI に具現化する。
- 無料の **構成プレビュー**（推定ページ数 / 章数 / 目次イメージ）→ **Job ID 発行** → **/jobs/{uuid} で進捗表示** → **ダウンロード**、の流れを成立させる。
- ブラウザ再訪（タブを閉じる / リロード）でも LocalStorage の job_id から続きが追えるようにする。

### Non-goals

- 課金 / 認証 / 利用規約同意 / ユーザ管理（永続）を導入しない。
- 24 時間での完全削除をローカル FS 実装で厳密に保証しない（Cloud Run + GCS lifecycle / TTL DB を前提にする）。
- SSE / WebSocket による push 通知はやらず、今回はポーリングで成立させる。

## Scope

### In scope

- Web UX
  - URL 入力 → Preview 表示（無料）
  - Job 作成 → job_id を LocalStorage に保存
  - `/jobs/{uuid}` で進捗を表示（ポーリング）
  - 再訪時に「作成中のブックがあります」バナー → `/jobs/{uuid}` に誘導
  - TOP で「直近 24h のジョブ履歴」一覧を表示（Clear/Remove を含む）
  - 完了時にダウンロードボタンを表示
- API / Backend（MVP）
  - Preview 用の軽量エンドポイント（sitemap.xml or 1-hop link 収集）
  - 既存 Job API（CreateJob / GetJob / GenerateJobDownloadUrl）を UX から利用
- 検証手順（curl/ブラウザ）を ExecPlan に残す

### Out of scope

- 本番のメール配信（外部プロバイダ連携）を必須化しない（設計のみ残す）。
- Cloud Run / GCS / TTL DB の本番運用（Terraform 追加やデプロイ作業）はこの計画では扱わない。

## Milestones

1. **Job 追跡キーの LocalStorage 保存ができる**
   - Job 作成後に `job_id` と `timestamp` を保存する。
   - TOP に戻っても「作成中のブック」バナーが出て、ジョブページへ遷移できる。
2. **`/jobs/{uuid}` の専用画面で進捗が見える**
   - 直リンク / リロードでも job_id から `GetJob` ポーリングできる。
   - DONE / ERROR を表示できる。
3. **完了時にダウンロード導線が成立する**
   - `GenerateJobDownloadUrl` を呼び、ダウンロード URL を得てボタンで提供する。
4. **無料 Preview が見える**
   - `sitemap.xml` があればそれを使い、なければ TOP 1-hop だけ収集して推定ページ数/章数を返す。
   - Web で「約 XX ページ / YY 章」の表示と、章タイトルの簡易プレビューを出す。
5. **テストと動作確認が再現可能**
   - Rust 側は Preview ロジックのテスト（tiny_http など）を追加する。
   - `just ci` を通す。
   - `sitebookify-app` を起動し、curl で最低限のエンドポイントが動くことを確認する（8080 が使用中なら別 port）。

## Tests

### Rust（自動）

- `nix develop -c just test`
- Preview ロジックのユニットテスト（HTTP サーバで sitemap/HTML を返して期待値を検証）

### Web（手動）

- `just web_install`
- `just web_gen`
- `just web_dev`（または `just web_build` + `just dev_app 18080`）

### API（手動 / curl）

- `curl -fsS http://127.0.0.1:18080/healthz`
- Job 完了後に:
  - `curl -fsS http://127.0.0.1:18080/jobs/<jobId>/book.md | head`
  - `curl -fSL -o /tmp/sitebookify.zip http://127.0.0.1:18080/artifacts/<jobId>`

## Decisions / Risks

### Decisions

- ルーティングは SPA の history（`/jobs/{uuid}`）で行い、サーバは `index.html` fallback で受ける。
- 進捗は SSE ではなく `GetJob` のポーリングで実装する（MVP）。
- Preview は「sitemap 優先 / 無ければ 1-hop」の deterministic な推定とし、LLM は使わない（無料）。

### Risks / Mitigations

- sitemap が巨大だとメモリ/時間を食う。
  - Mitigation: 取得サイズと抽出件数に上限を設け、サンプリングして推定する。
- 任意 URL を fetch するため SSRF のリスクがある。
  - Mitigation: 本番では private IP / metadata などの拒否を追加する（別計画）。
- LocalStorage が消える / 別端末だと追跡できない。
  - Mitigation: job_id は URL で共有できる前提にする。

## Progress

- 2026-02-04: ExecPlan を作成した。
- 2026-02-04: Web を `/` と `/jobs/{uuid}` の 2 画面に分割し、LocalStorage に job 履歴を保持するようにした。
- 2026-02-04: 軽量 Preview の API（`GET /preview?url=...`）と Web UI の Preview ボタンを追加した。
- 2026-02-04: Preview ロジックを `src/app/preview.rs` に切り出し、ユニットテストを追加した。
- 2026-02-04: `/jobs/{uuid}` で `book.md` / download が Loading のままになる不具合を修正した（useEffect の依存配列を修正）。
- 2026-02-04: TOP に「直近 24h のジョブ履歴」一覧を追加し、LocalStorage の prune を保存するようにした（履歴の上限も拡張）。
