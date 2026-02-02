import React, { useEffect, useId, useMemo, useState } from "react";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";
import mermaid from "mermaid";

let mermaidInitialized = false;
function initMermaid() {
  if (mermaidInitialized) return;
  mermaid.initialize({
    startOnLoad: false,
    theme: "dark",
    securityLevel: "strict",
  });
  mermaidInitialized = true;
}

type MermaidDiagramProps = {
  code: string;
};

function MermaidDiagram({ code }: MermaidDiagramProps) {
  const reactId = useId();
  const diagramId = useMemo(
    () => `mermaid-${reactId.replace(/[^a-zA-Z0-9_-]/g, "_")}`,
    [reactId],
  );

  const [svg, setSvg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let canceled = false;

    const render = async () => {
      setSvg(null);
      setError(null);
      try {
        initMermaid();
        const { svg } = await mermaid.render(diagramId, code);
        if (canceled) return;
        setSvg(svg);
      } catch (e) {
        if (canceled) return;
        setError(String(e));
      }
    };

    void render();
    return () => {
      canceled = true;
    };
  }, [code, diagramId]);

  if (error) {
    return (
      <pre>
        <code className="language-mermaid">{code}</code>
      </pre>
    );
  }

  if (!svg) {
    return <div className="muted">Rendering mermaid…</div>;
  }

  return (
    <div
      className="mermaid"
      // Mermaid already returns SVG markup. With `securityLevel: "strict"`,
      // this is the recommended way to embed it.
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}

export type MarkdownPreviewProps = {
  markdown: string;
};

export function MarkdownPreview({ markdown }: MarkdownPreviewProps) {
  return (
    <div className="markdownFrame">
      <Markdown
        remarkPlugins={[remarkGfm]}
        components={{
          pre({ children, ...props }) {
            if (Array.isArray(children) && children.length === 1) {
              const child = children[0];
              if (React.isValidElement(child)) {
                const className = String(child.props?.className ?? "");
                if (className.includes("language-mermaid")) {
                  const code = String(child.props?.children ?? "").trimEnd();
                  return <MermaidDiagram code={code} />;
                }
              }
            }
            return <pre {...props}>{children}</pre>;
          },
          a({ href, children, ...props }) {
            // Keep users inside the app by default while still letting them open docs.
            // In particular, `book.md` can contain absolute links back to the source site.
            const isExternal =
              typeof href === "string" && /^(https?:)?\\/\\//i.test(href.trim());
            return (
              <a
                href={href}
                target={isExternal ? "_blank" : undefined}
                rel={isExternal ? "noreferrer" : undefined}
                {...props}
              >
                {children}
              </a>
            );
          },
        }}
      >
        {markdown}
      </Markdown>
    </div>
  );
}
