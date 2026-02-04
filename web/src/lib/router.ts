import { useCallback, useEffect, useState } from "react";

export type AppRoute =
  | { kind: "home" }
  | { kind: "job"; jobId: string };

export function parseRoute(pathname: string): AppRoute {
  const normalized = pathname.trim() || "/";
  if (normalized === "/" || normalized === "") return { kind: "home" };

  const m = normalized.match(/^\/jobs\/([^/]+)\/?$/);
  if (m) return { kind: "job", jobId: m[1] };

  return { kind: "home" };
}

export function routePath(route: AppRoute): string {
  switch (route.kind) {
    case "home":
      return "/";
    case "job":
      return `/jobs/${route.jobId}`;
  }
}

export function usePathname() {
  const [pathname, setPathname] = useState(() => window.location.pathname);

  useEffect(() => {
    const onPopState = () => setPathname(window.location.pathname);
    window.addEventListener("popstate", onPopState);
    return () => window.removeEventListener("popstate", onPopState);
  }, []);

  const navigate = useCallback((to: string) => {
    const path = to.trim() || "/";
    if (path === window.location.pathname) return;
    window.history.pushState(null, "", path);
    setPathname(path);
  }, []);

  const replace = useCallback((to: string) => {
    const path = to.trim() || "/";
    if (path === window.location.pathname) return;
    window.history.replaceState(null, "", path);
    setPathname(path);
  }, []);

  return { pathname, navigate, replace };
}

