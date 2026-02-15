import { anyUnpack } from "@bufbuild/protobuf/wkt";
import type { Client } from "@connectrpc/connect";
import { useEffect, useMemo, useState } from "react";
import {
  CreateJobMetadataSchema,
  Engine,
  SitebookifyService,
} from "../gen/sitebookify/v1/service_pb";
import {
  clearJobs,
  loadJobs,
  pruneJobsInStorage,
  removeJob,
  upsertJob,
  type StoredJob,
} from "../lib/jobHistory";

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
  total_characters: number;
  character_basis: "extracted_markdown";
  estimated_input_tokens_min: number;
  estimated_input_tokens_max: number;
  estimated_output_tokens_min: number;
  estimated_output_tokens_max: number;
  estimated_cost_usd_min: number | null;
  estimated_cost_usd_max: number | null;
  pricing_model: string;
  pricing_note: string | null;
};

function jobIdFromName(name: string): string {
  return name.startsWith("jobs/") ? name.slice("jobs/".length) : name;
}

function formatTimestamp(ms: number): string {
  try {
    return new Date(ms).toLocaleString();
  } catch {
    return String(ms);
  }
}

function formatInt(value: number): string {
  if (!Number.isFinite(value)) return "0";
  return Math.max(0, Math.round(value)).toLocaleString();
}

function formatUsd(value: number | null): string {
  if (typeof value !== "number" || !Number.isFinite(value)) return "n/a";
  const digits = value >= 1 ? 2 : 4;
  return `$${value.toFixed(digits)}`;
}

export function HomePage({ client, navigate }: Props) {
  const [url, setUrl] = useState("https://agentskills.io/");
  const [{ busy, error }, setState] = useState<UiState>({ busy: false, error: null });
  const [preview, setPreview] = useState<Preview | null>(null);
  const [previewLoading, setPreviewLoading] = useState(false);
  const [previewError, setPreviewError] = useState<string | null>(null);
  const [jobHistory, setJobHistory] = useState<StoredJob[]>(() => loadJobs());

  const canStart = url.trim().length > 0 && !busy;
  const canPreview = url.trim().length > 0 && !busy && !previewLoading;

  const recentJob = jobHistory.length > 0 ? jobHistory[0] : null;

  useEffect(() => {
    setJobHistory(pruneJobsInStorage());
  }, []);

  const banner = useMemo(() => {
    if (!recentJob) return null;
    const title =
      recentJob.state === "done"
        ? "Your last book is ready"
        : recentJob.state === "error"
          ? "Your last book failed"
          : "A book is being generated";
    const subtitle =
      recentJob.sourceUrl && recentJob.sourceUrl.length > 0
        ? recentJob.sourceUrl
        : `job_id: ${recentJob.jobId}`;
    return { title, subtitle, jobId: recentJob.jobId };
  }, [recentJob]);

  function refreshHistory() {
    setJobHistory(pruneJobsInStorage());
  }

  function clearHistory() {
    clearJobs();
    setJobHistory([]);
  }

  function removeFromHistory(jobId: string) {
    setJobHistory(removeJob(jobId));
  }

  async function start() {
    setState({ busy: true, error: null });
    try {
      const op = await client.createJob({
        job: {
          spec: {
            sourceUrl: url.trim(),
            languageCode: "日本語",
            tone: "簡潔で敬語",
            tocEngine: Engine.OPENAI,
            renderEngine: Engine.OPENAI,
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
      const nextHistory = upsertJob(stored);
      setJobHistory(nextHistory);

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
              View Progress
            </button>
          </div>
        </div>
      ) : null}

      <h1 className="title">
        Web to Book.
        <br />
        <span className="titleLight">Instantly.</span>
      </h1>
      <p className="subtitle">
        Convert any website documentation or blog into a perfectly formatted e-book.
      </p>

      <input
        className="urlInput"
        type="url"
        placeholder="https://example.com/docs"
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
        inputMode="url"
        enterKeyHint="go"
      />

      <div className="heroButtons">
        <button className="btnPrimary" onClick={() => void start()} disabled={!canStart}>
          {busy ? "Generating..." : "Generate Book"}
        </button>
        <button className="btnSecondary" onClick={() => void runPreview()} disabled={!canPreview}>
          {previewLoading ? "Previewing..." : "Preview Structure"}
        </button>
      </div>

      <div className="tagline">Free to use &middot; No Login Required</div>

      {error ? (
        <div className="card" style={{ textAlign: "left" }}>
          <div className="row">
            <span className="pill error">error</span>
            <span className="error">{error}</span>
          </div>
        </div>
      ) : null}

      {preview || previewLoading || previewError ? (
        <div className="card" style={{ textAlign: "left" }}>
          <div className="row">
            <span className="pill">preview</span>
            {previewLoading ? (
              <span className="muted">
                <span className="spinner" /> Analyzing...
              </span>
            ) : preview ? (
              <span className="muted">
                ~{preview.estimated_pages} pages &middot; {preview.estimated_chapters} chapters
                &middot; {preview.source}
              </span>
            ) : (
              <span className="muted">&mdash;</span>
            )}
          </div>

          {preview?.notes?.length ? (
            <div className="muted hint" style={{ marginTop: 8 }}>
              {preview.notes.join(" \u2022 ")}
            </div>
          ) : null}

          {preview ? (
            <div className="muted hint" style={{ marginTop: 8 }}>
              {formatInt(preview.total_characters)} chars ({preview.character_basis}) &middot; input{" "}
              {formatInt(preview.estimated_input_tokens_min)}-{formatInt(preview.estimated_input_tokens_max)} tok
              &middot; output {formatInt(preview.estimated_output_tokens_min)}-
              {formatInt(preview.estimated_output_tokens_max)} tok
              &middot; cost {formatUsd(preview.estimated_cost_usd_min)}-
              {formatUsd(preview.estimated_cost_usd_max)} &middot; {preview.pricing_model}
            </div>
          ) : null}

          {preview?.pricing_note ? (
            <div className="muted hint" style={{ marginTop: 4 }}>
              {preview.pricing_note}
            </div>
          ) : null}

          {preview?.chapters?.length ? (
            <div className="row wrap" style={{ marginTop: 8 }}>
              {preview.chapters.slice(0, 10).map((ch) => (
                <span key={ch.title} className="pill">
                  {ch.title} &middot; {ch.pages}p
                </span>
              ))}
            </div>
          ) : null}

          {previewError ? (
            <div className="row" style={{ marginTop: 8 }}>
              <span className="pill error">error</span>
              <span className="error">{previewError}</span>
            </div>
          ) : null}
        </div>
      ) : null}

      <div className="card" style={{ textAlign: "left" }}>
        <div className="row wrap">
          <span className="pill">history</span>
          <span className="muted">Stored in browser &middot; auto-deleted after 24h</span>
          <button className="small" type="button" onClick={refreshHistory}>
            Refresh
          </button>
          <button
            className="small"
            type="button"
            onClick={clearHistory}
            disabled={jobHistory.length === 0}
          >
            Clear
          </button>
        </div>

        {jobHistory.length === 0 ? (
          <div className="muted hint" style={{ marginTop: 8 }}>
            No jobs yet.
          </div>
        ) : (
          <div className="status">
            {jobHistory.map((j) => (
              <div key={j.jobId} className="historyItem">
                <div className="row wrap">
                  <button
                    className="small"
                    type="button"
                    onClick={() => navigate(`/jobs/${j.jobId}`)}
                  >
                    Open
                  </button>

                  <span className="pill">job</span>
                  <code className="muted">{j.jobId}</code>

                  <span className="pill">status</span>
                  <span
                    className={
                      j.state === "error" ? "error" : j.state === "done" ? "success" : "muted"
                    }
                  >
                    {j.state ?? "unknown"}
                  </span>

                  {typeof j.progressPercent === "number" ? (
                    <span className="muted">{j.progressPercent}%</span>
                  ) : null}

                  <span className="muted">{formatTimestamp(j.createdAtMs)}</span>

                  {j.state === "done" ? (
                    <a
                      className="pillLink"
                      href={`/jobs/${j.jobId}/book.epub`}
                      download={`sitebookify-${j.jobId}.epub`}
                      title="EPUB: book.epub"
                    >
                      EPUB
                    </a>
                  ) : null}

                  <button className="small" type="button" onClick={() => removeFromHistory(j.jobId)}>
                    Remove
                  </button>
                </div>

                <div className="muted hint">
                  {[j.sourceUrl?.trim(), j.message?.trim()]
                    .filter((v): v is string => Boolean(v && v.length > 0))
                    .join(" \u2022 ") || "\u2014"}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
