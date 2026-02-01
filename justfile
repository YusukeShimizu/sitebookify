fmt:
    cargo fmt --all -- --check

proto_fmt:
    buf format -d --exit-code

proto_lint:
    buf lint

aip_lint:
    tmp="$(mktemp)"; trap 'rm -f "$tmp"' EXIT; buf build --as-file-descriptor-set -o "$tmp"; api-linter --set-exit-status --output-format summary --descriptor-set-in "$tmp" -I proto sitebookify/v1/service.proto sitebookify/v1/manifest.proto sitebookify/v1/toc.proto

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all

textlint:
    textlint $(git ls-files '*.md' | grep -v '^\.codex/')

docs_links:
    cd docs && PUPPETEER_SKIP_DOWNLOAD=1 PUPPETEER_SKIP_CHROMIUM_DOWNLOAD=1 npx --yes mint@4.2.269 broken-links

docs_vale:
    cd docs && vale sync --config .vale.ini
    cd docs && vale --config .vale.ini --glob='*.mdx' .

ci: fmt proto_fmt proto_lint aip_lint clippy test textlint docs_vale docs_links

# --- Local Web MVP (optional) ---

dev_app port="8080":
    cargo run --bin sitebookify-app -- --addr 127.0.0.1:{{port}}

web_install:
    cd web && npm install

web_gen:
    cd web && npm run gen

web_dev:
    cd web && npm run dev

web_build:
    cd web && npm run build
