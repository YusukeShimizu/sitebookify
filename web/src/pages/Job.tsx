import type { Client } from "@connectrpc/connect";
import { useEffect, useMemo, useState } from "react";
import { MarkdownPreview } from "../MarkdownPreview";
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

function statusText(state?: Job_State): string {
  switch (state) {
    case Job_State.QUEUED:
      return "queued";
    case Job_State.RUNNING:
      return "running";
    case Job_State.DONE:
      return "done";
    case Job_State.ERROR:
      return "error";
    default:
      return "—";
  }
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

export function JobPage({ client, jobId, navigate }: Props) {
  const cleanJobId = jobId.trim();
  const jobResourceName = useMemo(() => jobName(cleanJobId), [cleanJobId]);

  const [outputView, setOutputView] = useState<"preview" | "raw">("preview");
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

  return (
    <div className="hero">
      <div className="row wrap">
        <button className="small" type="button" onClick={() => navigate("/")}>
          ← Back
        </button>
        <span className="pill">job</span>
        <span className="muted">{cleanJobId || "—"}</span>
      </div>

      <div className="card">
        <div className="status">
          <div className="row">
            <span className="pill">status</span>
            <span className={job?.state === Job_State.ERROR ? "error" : "muted"}>
              {statusText(job?.state)}
            </span>
            {job ? <span className="muted">• {job.message}</span> : null}
          </div>

          <div className="row">
            <span className="pill">url</span>
            <span className="muted">{job?.spec?.sourceUrl ?? "—"}</span>
          </div>

          <div className="progress" aria-label="progress">
            <div style={{ width: `${progressPercent}%` }} />
          </div>
          <div className="muted">{job ? `${progressPercent}%` : ""}</div>

          {job?.state === Job_State.DONE ? (
            <div className="output">
              <div className="row wrap">
                <span className="pill success">output</span>
                <button
                  className="small"
                  onClick={() => void copyBookMd()}
                  disabled={!bookMd || bookMdLoading}
                >
                  {copied ? "Copied" : "Copy"}
                </button>
                {download?.url ? (
                  <a className="pillLink" href={download.url}>
                    Download zip
                  </a>
                ) : (
                  <span className="muted">{downloadLoading ? "Preparing download…" : ""}</span>
                )}

                <div className="segmented">
                  <button
                    className={`small ${outputView === "preview" ? "active" : ""}`}
                    type="button"
                    onClick={() => setOutputView("preview")}
                  >
                    Preview
                  </button>
                  <button
                    className={`small ${outputView === "raw" ? "active" : ""}`}
                    type="button"
                    onClick={() => setOutputView("raw")}
                  >
                    Markdown
                  </button>
                </div>
                {bookMdLoading ? <span className="muted">Loading…</span> : null}
              </div>
              {outputView === "preview" ? (
                bookMd ? (
                  <MarkdownPreview markdown={bookMd} />
                ) : (
                  <div className="markdownFrame muted">Waiting for book.md…</div>
                )
              ) : (
                <textarea
                  className="outputText"
                  readOnly
                  value={bookMd ?? ""}
                  placeholder="Waiting for book.md…"
                />
              )}
            </div>
          ) : null}

          {error ? (
            <div className="row">
              <span className="pill error">error</span>
              <span className="error">{error}</span>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  );
}
