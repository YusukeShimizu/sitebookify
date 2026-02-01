import { useEffect, useMemo, useState } from "react";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { anyUnpack } from "@bufbuild/protobuf/wkt";
import {
  CreateJobMetadataSchema,
  Engine,
  Job_State,
  SitebookifyService,
  type Job,
} from "./gen/sitebookify/v1/service_pb";

type UiState = {
  jobName: string | null;
  job: Job | null;
  bookMd: string | null;
  bookMdLoading: boolean;
  error: string | null;
  busy: boolean;
  copied: boolean;
};

export default function App() {
  const client = useMemo(() => {
    const transport = createGrpcWebTransport({
      baseUrl: "",
    });
    return createClient(SitebookifyService, transport);
  }, []);

  const [url, setUrl] = useState("https://example.com/docs/");
  const [{ jobName, job, bookMd, bookMdLoading, copied, error, busy }, setState] =
    useState<UiState>({
    jobName: null,
    job: null,
    bookMd: null,
    bookMdLoading: false,
    error: null,
    busy: false,
    copied: false,
  });

  const canStart = url.trim().length > 0 && !busy;

  function jobIdFromName(name: string): string {
    return name.startsWith("jobs/") ? name.slice("jobs/".length) : name;
  }

  async function fetchBookMd(name: string): Promise<string> {
    const jobId = jobIdFromName(name);
    const resp = await fetch(`/jobs/${jobId}/book.md`, {
      method: "GET",
      headers: { Accept: "text/plain" },
    });
    if (!resp.ok) {
      throw new Error(`failed to fetch book.md (${resp.status}): ${await resp.text()}`);
    }
    return await resp.text();
  }

  async function start() {
    setState((s) => ({
      ...s,
      busy: true,
      error: null,
      bookMd: null,
      bookMdLoading: false,
      copied: false,
      job: null,
      jobName: null,
    }));
    try {
      const op = await client.createJob({
        job: {
          spec: {
            sourceUrl: url.trim(),
            tocEngine: Engine.NOOP,
            renderEngine: Engine.NOOP,
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

      setState((s) => ({ ...s, jobName: resolvedJobName, busy: false }));
    } catch (e) {
      setState((s) => ({ ...s, busy: false, error: String(e) }));
    }
  }

  useEffect(() => {
    if (!jobName) return;
    if (job?.state === Job_State.ERROR) return;
    if (job?.state === Job_State.DONE) return;
    let stopped = false;

    const tick = async () => {
      try {
        const j = await client.getJob({ name: jobName });
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
  }, [client, jobName, job?.state]);

  useEffect(() => {
    if (!jobName) return;
    if (job?.state !== Job_State.DONE) return;
    if (bookMd || bookMdLoading) return;
    let stopped = false;

    setState((s) => ({ ...s, bookMdLoading: true }));
    const run = async () => {
      try {
        const md = await fetchBookMd(jobName);
        if (stopped) return;
        setState((s) => ({ ...s, bookMd: md, bookMdLoading: false }));
      } catch (e) {
        if (stopped) return;
        setState((s) => ({ ...s, bookMdLoading: false, error: String(e) }));
      }
    };

    void run();
    return () => {
      stopped = true;
    };
  }, [jobName, job?.state, bookMd, bookMdLoading]);

  const statusText = (() => {
    if (!job) return "—";
    switch (job.state) {
      case Job_State.QUEUED:
        return "queued";
      case Job_State.RUNNING:
        return "running";
      case Job_State.DONE:
        return "done";
      case Job_State.ERROR:
        return "error";
      default:
        return "unknown";
    }
  })();

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

  return (
    <div className="container">
      <div className="topbar">
        <div className="brand">{">_ sitebookify"}</div>
        <div className="pill">gRPC-Web • local FS • noop</div>
      </div>

      <div className="hero">
        <h1 className="title">
          Turn docs into a
          <br />
          textbook Markdown
        </h1>
        <p className="subtitle">
          Paste a start URL. Sitebookify crawls, extracts, builds an mdBook, then shows the bundled{" "}
          <code>book.md</code> here so you can review and copy it.
        </p>

        <div className="card">
          <div className="row">
            <input
              type="url"
              placeholder="https://example.com/docs/"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && canStart) void start();
              }}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
            />
            <button onClick={() => void start()} disabled={!canStart}>
              {busy ? "Starting…" : "Start"}
            </button>
          </div>

          <div className="status">
            <div className="row">
              <span className="pill">job</span>
              <span className="muted">{jobName ?? "—"}</span>
            </div>
            <div className="row">
              <span className="pill">status</span>
              <span className={job?.state === Job_State.ERROR ? "error" : "muted"}>
                {statusText}
              </span>
              {job ? <span className="muted">• {job.message}</span> : null}
            </div>
            <div className="row">
              <span className="pill">url</span>
              <span className="muted">{job?.spec?.sourceUrl ?? (url.trim() || "—")}</span>
            </div>

            <div className="progress" aria-label="progress">
              <div style={{ width: `${job?.progressPercent ?? 0}%` }} />
            </div>
            <div className="muted">{job ? `${job.progressPercent}%` : ""}</div>

            {job?.state === Job_State.DONE ? (
              <div className="output">
                <div className="row">
                  <span className="pill success">output</span>
                  <button
                    className="small"
                    onClick={() => void copyBookMd()}
                    disabled={!bookMd || bookMdLoading}
                  >
                    {copied ? "Copied" : "Copy"}
                  </button>
                  {bookMdLoading ? <span className="muted">Loading…</span> : null}
                </div>
                <textarea
                  className="outputText"
                  readOnly
                  value={bookMd ?? ""}
                  placeholder="Waiting for book.md…"
                />
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

        <div className="footer">
          Local MVP: `sitebookify-app` stores jobs under <code>workspace-app/jobs</code>. For Cloud Run,
          swap JobStore/ArtifactStore/Queue.
        </div>
      </div>
    </div>
  );
}
