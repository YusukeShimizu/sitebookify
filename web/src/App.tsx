import { useEffect, useMemo, useState } from "react";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { JobStatus, SitebookifyService, type Job } from "./gen/sitebookify/v1/service_pb";

type UiState = {
  jobId: string | null;
  job: Job | null;
  downloadUrl: string | null;
  error: string | null;
  busy: boolean;
};

export default function App() {
  const client = useMemo(() => {
    const transport = createGrpcWebTransport({
      baseUrl: "",
    });
    return createClient(SitebookifyService, transport);
  }, []);

  const [url, setUrl] = useState("https://example.com/docs/");
  const [{ jobId, job, downloadUrl, error, busy }, setState] = useState<UiState>({
    jobId: null,
    job: null,
    downloadUrl: null,
    error: null,
    busy: false,
  });

  const canStart = url.trim().length > 0 && !busy;

  async function start() {
    setState((s) => ({ ...s, busy: true, error: null, downloadUrl: null, job: null }));
    try {
      const res = await client.startCrawl({
        url: url.trim(),
        tocEngine: "noop",
        renderEngine: "noop",
      });
      setState((s) => ({ ...s, jobId: res.jobId, busy: false }));
    } catch (e) {
      setState((s) => ({ ...s, busy: false, error: String(e) }));
    }
  }

  useEffect(() => {
    if (!jobId) return;
    let stopped = false;

    const tick = async () => {
      try {
        const j = await client.getJob({ jobId });
        if (stopped) return;
        setState((s) => ({ ...s, job: j }));

        if (j.status === JobStatus.DONE) {
          const dl = await client.getDownloadUrl({ jobId });
          if (stopped) return;
          setState((s) => ({ ...s, downloadUrl: dl.url }));
        }
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
  }, [client, jobId]);

  const statusText = (() => {
    if (!job) return "—";
    switch (job.status) {
      case JobStatus.QUEUED:
        return "queued";
      case JobStatus.RUNNING:
        return "running";
      case JobStatus.DONE:
        return "done";
      case JobStatus.ERROR:
        return "error";
      default:
        return "unknown";
    }
  })();

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
          Paste a start URL. Sitebookify crawls, extracts, builds an mdBook, then bundles it into a
          single <code>book.md</code> (+assets).
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
              <span className="muted">{jobId ?? "—"}</span>
            </div>
            <div className="row">
              <span className="pill">status</span>
              <span className={job?.status === JobStatus.ERROR ? "error" : "muted"}>
                {statusText}
              </span>
              {job ? <span className="muted">• {job.message}</span> : null}
            </div>

            <div className="progress" aria-label="progress">
              <div style={{ width: `${job?.progressPercent ?? 0}%` }} />
            </div>
            <div className="muted">{job ? `${job.progressPercent}%` : ""}</div>

            {downloadUrl ? (
              <div className="row">
                <span className="pill success">artifact</span>
                <a href={downloadUrl}>Download zip</a>
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
