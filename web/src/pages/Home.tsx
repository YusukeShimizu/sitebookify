import { anyUnpack } from "@bufbuild/protobuf/wkt";
import type { Client } from "@connectrpc/connect";
import { useEffect, useMemo, useState } from "react";
import {
  CreateJobMetadataSchema,
  Engine,
  SitebookifyService,
} from "../gen/sitebookify/v1/service_pb";
import { getMostRecentJob, upsertJob, type StoredJob } from "../lib/jobHistory";

type Props = {
  client: Client<typeof SitebookifyService>;
  navigate: (to: string) => void;
};

type UiState = {
  busy: boolean;
  error: string | null;
};

type Preview = {
  source: "sitemap" | "sitemap_index" | "links";
  estimated_pages: number;
  estimated_chapters: number;
  chapters: Array<{ title: string; pages: number }>;
  sample_urls: string[];
  notes: string[];
};

function jobIdFromName(name: string): string {
  return name.startsWith("jobs/") ? name.slice("jobs/".length) : name;
}

function engineValue(e: string): Engine {
  const parsed = Number(e);
  if (!Number.isFinite(parsed)) return Engine.UNSPECIFIED;
  switch (parsed) {
    case Engine.NOOP:
    case Engine.OPENAI:
    case Engine.UNSPECIFIED:
      return parsed;
    default:
      return Engine.UNSPECIFIED;
  }
}

export function HomePage({ client, navigate }: Props) {
  const [url, setUrl] = useState("https://agentskills.io/");
  const [languageCode, setLanguageCode] = useState("日本語");
  const [tone, setTone] = useState("丁寧");
  const [tocEngine, setTocEngine] = useState<Engine>(Engine.NOOP);
  const [renderEngine, setRenderEngine] = useState<Engine>(Engine.NOOP);
  const [{ busy, error }, setState] = useState<UiState>({ busy: false, error: null });
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);

  const canStart = url.trim().length > 0 && !busy;
  const canPreview = url.trim().length > 0 && !busy && !previewLoading;

  const engineLabel =
    tocEngine === Engine.OPENAI || renderEngine === Engine.OPENAI ? "openai" : "noop";

  const [recentJob, setRecentJob] = useState<StoredJob | null>(() => getMostRecentJob());

  useEffect(() => {
    setRecentJob(getMostRecentJob());
  }, []);

  const banner = useMemo(() => {
    if (!recentJob) return null;
    const title =
      recentJob.state === "done"
        ? "前回のブックが完了しています"
        : recentJob.state === "error"
          ? "前回のブックが失敗しています"
          : "作成中のブックがあります";
    const subtitle =
      recentJob.sourceUrl && recentJob.sourceUrl.length > 0
        ? recentJob.sourceUrl
        : `job_id: ${recentJob.jobId}`;
    return { title, subtitle, jobId: recentJob.jobId };
  }, [recentJob]);

  async function start() {
    setState({ busy: true, error: null });
    try {
      const op = await client.createJob({
        job: {
          spec: {
            sourceUrl: url.trim(),
            languageCode: languageCode.trim(),
            tone: tone.trim(),
            tocEngine,
            renderEngine,
          },
        },
        jobId: "",
      });

      const jobNameFromMetadata = op.metadata
        ? anyUnpack(op.metadata, CreateJobMetadataSchema)?.job ?? null
        : null;
      const jobNameFromOp = op.name.startsWith("operations/")
        ? `jobs/${op.name.slice("operations/".length)}`
        : null;

      const resolvedJobName = jobNameFromMetadata ?? jobNameFromOp;
      if (!resolvedJobName) {
        throw new Error(`CreateJob returned an operation without a job reference: ${op.name}`);
      }

      const jobId = jobIdFromName(resolvedJobName);
      const stored = {
        jobId,
        createdAtMs: Date.now(),
        sourceUrl: url.trim(),
        state: "queued" as const,
        progressPercent: 0,
        message: "queued",
      };
      upsertJob(stored);
      setRecentJob(stored);

      navigate(`/jobs/${jobId}`);
    } catch (e) {
      setState({ busy: false, error: String(e) });
      return;
    }
    setState({ busy: false, error: null });
  }

  async function runPreview() {
    const target = url.trim();
    if (target.length === 0) return;

    setPreviewError(null);
    setPreviewLoading(true);
    try {
      const resp = await fetch(`/preview?url=${encodeURIComponent(target)}`, {
        method: "GET",
        headers: { Accept: "application/json" },
      });
      if (!resp.ok) {
        throw new Error(`preview failed (${resp.status}): ${await resp.text()}`);
      }
      const data: Preview = await resp.json();
      setPreview(data);
    } catch (e) {
      setPreview(null);
      setPreviewError(String(e));
    } finally {
      setPreviewLoading(false);
    }
  }

  return (
    <div className="hero">
      {banner ? (
        <div className="banner">
          <div className="bannerTitle">{banner.title}</div>
          <div className="bannerSubtitle muted">{banner.subtitle}</div>
          <div className="row wrap bannerActions">
            <button className="small" onClick={() => navigate(`/jobs/${banner.jobId}`)}>
              進捗を見る
            </button>
          </div>
        </div>
      ) : null}

      <h1 className="title">
        Turn docs into a
        <br />
        textbook Markdown
      </h1>
      <p className="subtitle">
        URL を貼り付けるだけ。まずは無料で構成プレビューを確認し、そのまま生成ジョブを開始できます。
      </p>

      <div className="card">
        <div className="row">
          <input
            type="url"
            placeholder="https://agentskills.io/"
            value={url}
            onChange={(e) => {
              setUrl(e.target.value);
              setPreview(null);
              setPreviewError(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canStart) void start();
            }}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
          />
          <button onClick={() => void runPreview()} disabled={!canPreview}>
            {previewLoading ? "Preview…" : "Preview"}
          </button>
          <button onClick={() => void start()} disabled={!canStart}>
            {busy ? "Starting…" : "Start"}
          </button>
        </div>

        {preview || previewLoading || previewError ? (
          <div className="status">
            <div className="row">
              <span className="pill">preview</span>
              {previewLoading ? (
                <span className="muted">Analyzing…</span>
              ) : preview ? (
                <span className="muted">
                  約 {preview.estimated_pages} ページ • {preview.estimated_chapters} 章 •{" "}
                  {preview.source}
                </span>
              ) : (
                <span className="muted">—</span>
              )}
            </div>

            {preview?.notes?.length ? (
              <div className="muted hint">{preview.notes.join(" • ")}</div>
            ) : null}

            {preview?.chapters?.length ? (
              <div className="row wrap">
                {preview.chapters.slice(0, 10).map((ch) => (
                  <span key={ch.title} className="pill">
                    {ch.title} • {ch.pages}p
                  </span>
                ))}
              </div>
            ) : null}

            {previewError ? (
              <div className="row">
                <span className="pill error">preview</span>
                <span className="error">{previewError}</span>
              </div>
            ) : null}
          </div>
        ) : null}

        <div className="settings">
          <div className="row wrap">
            <span className="pill">toc</span>
            <select
              className="control"
              value={tocEngine}
              onChange={(e) => setTocEngine(engineValue(e.target.value))}
            >
              <option value={Engine.NOOP}>noop</option>
              <option value={Engine.OPENAI}>openai</option>
            </select>

            <span className="pill">render</span>
            <select
              className="control"
              value={renderEngine}
              onChange={(e) => setRenderEngine(engineValue(e.target.value))}
            >
              <option value={Engine.NOOP}>noop</option>
              <option value={Engine.OPENAI}>openai</option>
            </select>

            <span className="pill">{engineLabel}</span>
          </div>

          <div className="row wrap">
            <span className="pill">language</span>
            <input
              className="control"
              type="text"
              value={languageCode}
              onChange={(e) => setLanguageCode(e.target.value)}
              placeholder="日本語 / English / …"
            />

            <span className="pill">tone</span>
            <input
              className="control"
              type="text"
              value={tone}
              onChange={(e) => setTone(e.target.value)}
              placeholder="丁寧 / casual / …"
            />
          </div>

          {engineLabel === "openai" ? (
            <div className="muted hint">
              OpenAI engine requires the API key on the server (see <code>OPENAI_API_KEY</code> /{" "}
              <code>SITEBOOKIFY_OPENAI_*</code> in README).
            </div>
          ) : null}
        </div>

        {error ? (
          <div className="row">
            <span className="pill error">error</span>
            <span className="error">{error}</span>
          </div>
        ) : null}
      </div>

      <div className="footer">
        Local MVP: `sitebookify-app` stores jobs under <code>workspace-app/jobs</code>. For Cloud Run,
        swap JobStore/ArtifactStore/Queue.
      </div>
    </div>
  );
}
