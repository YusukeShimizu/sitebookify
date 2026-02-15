import type { Client } from "@connectrpc/connect";
import { useEffect, useMemo, useState } from "react";
import {
  Job_State,
  SitebookifyService,
  type GenerateJobDownloadUrlResponse,
  type Job,
} from "../gen/sitebookify/v1/service_pb";
import { getJobById, upsertJob } from "../lib/jobHistory";

type Props = {
  client: Client<typeof SitebookifyService>;
  jobId: string;
  navigate: (to: string) => void;
};

type UiState = {
  job: Job | null;
  bookMd: string | null;
  bookMdLoading: boolean;
  download: GenerateJobDownloadUrlResponse | null;
  downloadLoading: boolean;
  error: string | null;
  copied: boolean;
};

type VisualPhase = "loading" | "processing" | "compiling" | "done" | "error";
type JobStageKey =
  | "queued"
  | "starting"
  | "crawl"
  | "extract"
  | "manifest"
  | "toc"
  | "book_init"
  | "book_render"
  | "book_bundle"
  | "book_epub"
  | "done"
  | "unknown";
type ErrorCategory = "network" | "extract" | "ai" | "render" | "unknown";

function jobName(jobId: string): string {
  return `jobs/${jobId}`;
}

async function fetchBookMd(jobId: string, signal?: AbortSignal): Promise<string> {
  const resp = await fetch(`/jobs/${jobId}/book.md`, {
    method: "GET",
    headers: { Accept: "text/plain" },
    signal,
  });
  if (!resp.ok) {
    throw new Error(`failed to fetch book.md (${resp.status}): ${await resp.text()}`);
  }
  return await resp.text();
}

function storedState(state?: Job_State) {
  switch (state) {
    case Job_State.QUEUED:
      return "queued" as const;
    case Job_State.RUNNING:
      return "running" as const;
    case Job_State.DONE:
      return "done" as const;
    case Job_State.ERROR:
      return "error" as const;
    default:
      return "unknown" as const;
  }
}

function getPhase(job: Job | null): VisualPhase {
  if (!job) return "loading";
  if (job.state === Job_State.ERROR) return "error";
  if (job.state === Job_State.DONE) return "done";
  if (job.state === Job_State.RUNNING && (job.progressPercent ?? 0) > 0) return "compiling";
  return "processing";
}

function normalizeJobStage(message?: string): JobStageKey {
  const normalized = (message ?? "").trim().toLowerCase();
  switch (normalized) {
    case "queued":
      return "queued";
    case "starting":
      return "starting";
    case "crawl":
      return "crawl";
    case "extract":
      return "extract";
    case "manifest":
      return "manifest";
    case "toc":
      return "toc";
    case "book init":
      return "book_init";
    case "book render":
      return "book_render";
    case "book bundle":
      return "book_bundle";
    case "book epub":
      return "book_epub";
    case "done":
      return "done";
    default:
      break;
  }

  if (normalized.includes("crawl")) return "crawl";
  if (normalized.includes("extract")) return "extract";
  if (normalized.includes("manifest")) return "manifest";
  if (normalized.includes("toc")) return "toc";
  if (normalized.includes("book init")) return "book_init";
  if (normalized.includes("book render")) return "book_render";
  if (normalized.includes("book bundle")) return "book_bundle";
  if (normalized.includes("book epub")) return "book_epub";
  return "unknown";
}

function stageLabel(stage: JobStageKey): string {
  switch (stage) {
    case "queued":
      return "Queued";
    case "starting":
      return "Starting";
    case "crawl":
      return "Crawling pages";
    case "extract":
      return "Extracting content";
    case "manifest":
      return "Building manifest";
    case "toc":
      return "Generating table of contents";
    case "book_init":
      return "Preparing renderer";
    case "book_render":
      return "Rendering book";
    case "book_bundle":
      return "Bundling output";
    case "book_epub":
      return "Creating EPUB";
    case "done":
      return "Done";
    default:
      return "Processing";
  }
}

function classifyErrorCategory(message?: string): ErrorCategory {
  const normalized = (message ?? "").toLowerCase();
  if (
    normalized.includes("timeout") ||
    normalized.includes("connection") ||
    normalized.includes("dns") ||
    normalized.includes("network") ||
    normalized.includes("failed to fetch")
  ) {
    return "network";
  }
  if (
    normalized.includes("extract") ||
    normalized.includes("readability") ||
    normalized.includes("front matter")
  ) {
    return "extract";
  }
  if (
    normalized.includes("openai") ||
    normalized.includes("responses api") ||
    normalized.includes("toc create")
  ) {
    return "ai";
  }
  if (
    normalized.includes("book render") ||
    normalized.includes("book bundle") ||
    normalized.includes("book epub") ||
    normalized.includes("mdbook")
  ) {
    return "render";
  }
  return "unknown";
}

export function JobPage({ client, jobId, navigate }: Props) {
  const cleanJobId = jobId.trim();
  const jobResourceName = useMemo(() => jobName(cleanJobId), [cleanJobId]);

  const [{ job, bookMd, bookMdLoading, download, downloadLoading, copied, error }, setState] =
    useState<UiState>({
      job: null,
      bookMd: null,
      bookMdLoading: false,
      download: null,
      downloadLoading: false,
      error: null,
      copied: false,
    });

  useEffect(() => {
    if (!cleanJobId) return;
    const existing = getJobById(cleanJobId);
    upsertJob({
      jobId: cleanJobId,
      createdAtMs: existing?.createdAtMs ?? Date.now(),
      sourceUrl: existing?.sourceUrl,
      state: existing?.state ?? "unknown",
      progressPercent: existing?.progressPercent,
      message: existing?.message,
    });
  }, [cleanJobId]);

  useEffect(() => {
    if (!cleanJobId) return;
    const existing = getJobById(cleanJobId);
    const createdAtMs = existing?.createdAtMs ?? Date.now();
    upsertJob({
      jobId: cleanJobId,
      createdAtMs,
      sourceUrl: job?.spec?.sourceUrl ?? existing?.sourceUrl,
      state: storedState(job?.state),
      progressPercent: job?.progressPercent ?? existing?.progressPercent,
      message: job?.message ?? existing?.message,
    });
  }, [cleanJobId, job?.message, job?.progressPercent, job?.spec?.sourceUrl, job?.state]);

  useEffect(() => {
    if (!cleanJobId) return;
    if (job?.state === Job_State.ERROR) return;
    if (job?.state === Job_State.DONE) return;
    let stopped = false;

    const tick = async () => {
      try {
        const j = await client.getJob({ name: jobResourceName });
        if (stopped) return;
        setState((s) => ({ ...s, job: j }));
      } catch (e) {
        if (stopped) return;
        setState((s) => ({ ...s, error: String(e) }));
      }
    };

    void tick();
    const id = window.setInterval(() => void tick(), 1500);
    return () => {
      stopped = true;
      window.clearInterval(id);
    };
  }, [client, cleanJobId, jobResourceName, job?.state]);

  useEffect(() => {
    if (!cleanJobId) return;
    if (job?.state !== Job_State.DONE) return;
    if (bookMd || bookMdLoading) return;
    let stopped = false;
    const controller = new AbortController();

    setState((s) => ({ ...s, bookMdLoading: true }));
    const run = async () => {
      try {
        const md = await fetchBookMd(cleanJobId, controller.signal);
        if (stopped) return;
        setState((s) => ({ ...s, bookMd: md, bookMdLoading: false }));
      } catch (e) {
        if (stopped) return;
        if (e instanceof DOMException && e.name === "AbortError") return;
        setState((s) => ({ ...s, bookMdLoading: false, error: String(e) }));
      }
    };

    void run();
    return () => {
      stopped = true;
      controller.abort();
    };
  }, [cleanJobId, job?.state]);

  useEffect(() => {
    if (!cleanJobId) return;
    if (job?.state !== Job_State.DONE) return;
    if (download || downloadLoading) return;
    let stopped = false;

    setState((s) => ({ ...s, downloadLoading: true }));
    const run = async () => {
      try {
        const resp = await client.generateJobDownloadUrl({ name: jobResourceName });
        if (stopped) return;
        setState((s) => ({ ...s, download: resp, downloadLoading: false }));
      } catch (e) {
        if (stopped) return;
        setState((s) => ({ ...s, downloadLoading: false, error: String(e) }));
      }
    };

    void run();
    return () => {
      stopped = true;
    };
  }, [client, job?.state, jobResourceName]);

  async function copyBookMd() {
    if (!bookMd) return;
    try {
      await navigator.clipboard.writeText(bookMd);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = bookMd;
      ta.style.position = "fixed";
      ta.style.top = "0";
      ta.style.left = "0";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.focus();
      ta.select();
      document.execCommand("copy");
      document.body.removeChild(ta);
    }

    setState((s) => ({ ...s, copied: true }));
    window.setTimeout(() => setState((s) => ({ ...s, copied: false })), 1200);
  }

  const progressPercent = job?.progressPercent ?? 0;
  const phase = getPhase(job);
  const sourceUrl = job?.spec?.sourceUrl;
  const currentStage = normalizeJobStage(job?.message);
  const currentStageLabel = stageLabel(currentStage);
  const errorCategory = classifyErrorCategory(job?.message);

  return (
    <div className="hero">
      {/* Loading phase */}
      {phase === "loading" ? (
        <div style={{ padding: "40px 0" }}>
          <div className="jobIcon">
            <span className="spinner" />
          </div>
          <h2 className="title" style={{ fontSize: 28 }}>
            Loading...
          </h2>
          <p className="muted">{cleanJobId || "\u2014"}</p>
        </div>
      ) : null}

      {/* Processing phase (queued / early running) */}
      {phase === "processing" ? (
        <div style={{ padding: "40px 0" }}>
          <div className="jobIcon">🌐</div>
          <h2 className="title" style={{ fontSize: 28 }}>
            {currentStageLabel}
          </h2>
          {sourceUrl ? <p className="muted">{sourceUrl}</p> : null}
          <p className="muted hint">phase: {currentStage}</p>
          {job?.message ? <div className="compileLog">{job.message}</div> : null}
        </div>
      ) : null}

      {/* Compiling phase (running with progress > 0) */}
      {phase === "compiling" ? (
        <div className="compileCard">
          <div className="jobIcon">📖</div>
          <h2 className="title" style={{ fontSize: 28 }}>
            {currentStageLabel}... {progressPercent}%
          </h2>
          {sourceUrl ? <p className="muted">{sourceUrl}</p> : null}
          <p className="muted hint">phase: {currentStage}</p>
          {job?.message ? <div className="compileLog">{job.message}</div> : null}
          <div className="compileProgress">
            <div className="progress" aria-label="progress">
              <div style={{ width: `${progressPercent}%` }} />
            </div>
          </div>
        </div>
      ) : null}

      {/* Done phase */}
      {phase === "done" ? (
        <div style={{ padding: "40px 0" }}>
          <div className="jobIcon jobIconDone">&#10003;</div>
          <h2 className="title" style={{ fontSize: 32 }}>
            Ready to Read.
          </h2>
          <p className="muted">
            Your custom e-book has been generated.
          </p>
          <p className="muted hint">Valid for 24 hours.</p>

          <div className="downloadGrid">
            <a
              className="downloadCard"
              href={`/jobs/${cleanJobId}/book.epub`}
              download={`sitebookify-${cleanJobId}.epub`}
            >
              <span className="icon">📕</span>
              <span>EPUB</span>
              <span className="label">E-book reader</span>
            </a>
            {download?.url ? (
              <a
                className="downloadCard"
                href={download.url}
                download={`sitebookify-${cleanJobId}.zip`}
              >
                <span className="icon">📦</span>
                <span>Markdown ZIP</span>
                <span className="label">Raw files + assets</span>
              </a>
            ) : (
              <div className="downloadCard" style={{ opacity: 0.5 }}>
                <span className="icon">📦</span>
                <span>{downloadLoading ? "Preparing..." : "Markdown ZIP"}</span>
                <span className="label">Raw files + assets</span>
              </div>
            )}
          </div>

          <div className="card" style={{ textAlign: "left", marginTop: 24 }}>
            <div className="row wrap outputActions">
              <button
                className="small"
                onClick={() => void copyBookMd()}
                disabled={!bookMd || bookMdLoading}
              >
                {copied ? "Copied!" : "Copy Markdown"}
              </button>
              <a
                className="pillLink"
                href={`/jobs/${cleanJobId}/book.md`}
                target="_blank"
                rel="noreferrer"
              >
                Open book.md
              </a>
              {bookMdLoading ? (
                <span className="muted">
                  <span className="spinner" /> Loading...
                </span>
              ) : null}
            </div>

            <textarea
              className="outputText"
              readOnly
              value={bookMd ?? ""}
              placeholder="Waiting for book.md..."
              style={{ marginTop: 10 }}
            />
          </div>

          <a
            className="convertAnother"
            href="/"
            onClick={(e) => {
              e.preventDefault();
              navigate("/");
            }}
          >
            Convert Another Site
          </a>
        </div>
      ) : null}

      {/* Error phase */}
      {phase === "error" ? (
        <div style={{ padding: "40px 0" }}>
          <div className="jobIcon" style={{ background: "#fef2f2" }}>
            &#9888;
          </div>
          <h2 className="title" style={{ fontSize: 28, color: "var(--danger)" }}>
            Something went wrong.
          </h2>
          <p className="muted">{job?.message || "An unknown error occurred."}</p>
          <p className="muted hint">category: {errorCategory}</p>
          <div className="heroButtons" style={{ marginTop: 20 }}>
            <button
              className="btnPrimary"
              onClick={(e) => {
                e.preventDefault();
                navigate("/");
              }}
            >
              Try Again
            </button>
          </div>
        </div>
      ) : null}

      {/* General error from polling */}
      {error && phase !== "error" ? (
        <div className="card" style={{ textAlign: "left", marginTop: 16 }}>
          <div className="row">
            <span className="pill error">error</span>
            <span className="error">{error}</span>
          </div>
        </div>
      ) : null}
    </div>
  );
}
