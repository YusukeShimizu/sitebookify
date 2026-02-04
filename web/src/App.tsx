import { useEffect, useMemo } from "react";
import { createClient } from "@connectrpc/connect";
import { createGrpcWebTransport } from "@connectrpc/connect-web";
import { SitebookifyService } from "./gen/sitebookify/v1/service_pb";
import { parseRoute, usePathname } from "./lib/router";
import { HomePage } from "./pages/Home";
import { JobPage } from "./pages/Job";

export default function App() {
  const client = useMemo(() => {
    const transport = createGrpcWebTransport({
      baseUrl: "",
    });
    return createClient(SitebookifyService, transport);
  }, []);

  const { pathname, navigate, replace } = usePathname();
  const route = useMemo(() => parseRoute(pathname), [pathname]);

  useEffect(() => {
    if (route.kind === "home" && pathname !== "/") {
      replace("/");
    }
  }, [pathname, replace, route.kind]);

  return (
    <div className="container">
      <div className="topbar">
        <a
          className="brand"
          href="/"
          onClick={(e) => {
            e.preventDefault();
            navigate("/");
          }}
        >
          {">_ sitebookify"}
        </a>
        <div className="pill">gRPC-Web • local FS • 24h TTL</div>
      </div>

      {route.kind === "home" ? (
        <HomePage client={client} navigate={navigate} />
      ) : (
        <JobPage client={client} jobId={route.jobId} navigate={navigate} />
      )}
    </div>
  );
}

