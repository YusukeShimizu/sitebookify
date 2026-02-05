# Sitebookify Specification

このリポジトリは、Sitebookify（公開サイトをクロールし、mdBook 形式の Markdown 素材を生成する CLI）である。
本書（`spec.md`）は、Sitebookify に含める要件と不変条件を定義する。

本書は、arXiv:2508.14511v2 に準拠する。
対象論文のタイトルは「What You See Is What It Does」である。
本書は Concept Specifications と Synchronizations で構造化して記述する。
参照: https://arxiv.org/html/2508.14511v2

## Concepts

```text
concept Shell
purpose
    外部（開発者・CI）からのコマンド実行を表現する。
state
    env: string -> string
actions
    request [ command: string ]
        => [ ]
```

```text
concept DevEnvironment
purpose
    開発環境を Nix Flakes で再現する。
    環境変数を direnv（`.envrc`）で管理する。
    rust-analyzer が標準ライブラリ（`std`）を読めるようにする。
state
    flake_nix: string
    flake_lock: string
    envrc: string
    envrc_local: string
    rust_src_path: string
    rust_lib_src_link: string
actions
    enter [ tool: "direnv" ]
        => [ ]
        load the dev shell via `use flake` in `.envrc`
        load local-only overrides from `.envrc.local` when present
        export `RUST_SRC_PATH` for Rust stdlib sources
        ensure `rust-lib-src` points to the stdlib sources
    enter [ tool: "nix" ]
        => [ ]
        enter the dev shell via `nix develop`
        export `RUST_SRC_PATH` for Rust stdlib sources
        ensure `rust-lib-src` points to the stdlib sources
operational principle
    after enter [ tool: "direnv" ]
        => [ ]
    then Shell/request [ command: "just ci" ]
        => [ ]
```

```text
concept SitebookifyCLI
purpose
    ログイン不要の公開サイトをクロールし、Markdown 素材を生成する。
    章立て（TOC）に従って mdBook 形式の教科書 Markdown を出力する。
    TOC 作成では、章ごとに「狙い」「読者が得るもの」を持たせる。
    TOC の各節は、材料にする page_id（元ページ）を参照する。
    TOC 作成時点で「重複ページは片方に寄せる」「似た話題は統合」などの編集判断を行う。
    本文の書き換えは OpenAI API（Responses API）をエンジンとして行い、ユーザは任意の言語とトーンを指定できる。
    robots.txt は MVP では未対応である。
state
    binary_name: string
    raw_dir: string
    extracted_dir: string
    manifest_path: string
    toc_path: string
    book_dir: string
    bundle_path: string
    epub_path: string
actions
    crawl [
        url: string
        out: string
        max_pages: number
        max_depth: number
        concurrency: number
        delay_ms: number
    ]
        => [ exit_code: 0 ]
        write `raw/crawl.jsonl` and `raw/html/**/index.html`
        do not overwrite existing snapshot files
    extract [ raw: string ; out: string ]
        => [ exit_code: 0 ]
        write `extracted/pages/*.md`
        strip known UI boilerplate sections (e.g. mdBook keyboard shortcuts help)
        do not overwrite existing snapshot files
    manifest [ extracted: string ; out: string ]
        => [ exit_code: 0 ]
        write `manifest.jsonl`
    toc_create [
        manifest: string
        out: string
        book_title: string (optional)
        language: string
        tone: string
        engine: string
    ]
        => [ exit_code: 0 ]
        write `toc.yaml`
        may omit manifest pages that are not suitable for a book
        may propose a better `book_title`
        each chapter MUST have `intent` and `reader_gains`
        each section MUST have `sources` (page ids)
        when engine is "openai", call OpenAI Responses API
            require env `OPENAI_API_KEY` (or `SITEBOOKIFY_OPENAI_API_KEY`)
            use env `SITEBOOKIFY_OPENAI_MODEL` (default: "gpt-5.2")
            use env `SITEBOOKIFY_OPENAI_REASONING_EFFORT` (default: "medium"; e.g. "minimal" | "low" | "medium" | "high" | "xhigh")
            optionally use env `SITEBOOKIFY_OPENAI_BASE_URL` (for testing/proxies)
        when engine is "noop", generate a deterministic TOC without using an LLM
        do not overwrite existing output files unless `force` is set
    book_init [ out: string ; title: string ]
        => [ exit_code: 0 ]
        write `book/book.toml` and `book/src/*`
    book_render [
        toc: string
        manifest: string
        out: string
        language: string
        tone: string
        engine: string
    ]
        => [ exit_code: 0 ]
        write `book/src/SUMMARY.md` and `book/src/chapters/*.md`
        ensure every chapter includes `## Sources`
        do not emit placeholder TODO sections
        preserve internal links and images where possible
        when a link target matches a manifest page URL, rewrite to an internal anchor link
        download referenced images into `book/src/assets/*` and rewrite image destinations to local relative paths
        include stable anchors (e.g. `<a id="p_..."></a>`) for each referenced source page id
        when engine is "openai", call OpenAI Responses API and rewrite each section into book-first prose
            require env `OPENAI_API_KEY` (or `SITEBOOKIFY_OPENAI_API_KEY`)
            use env `SITEBOOKIFY_OPENAI_MODEL` (default: "gpt-5.2")
            use env `SITEBOOKIFY_OPENAI_REASONING_EFFORT` (default: "medium"; e.g. "minimal" | "low" | "medium" | "high" | "xhigh")
            optionally use env `SITEBOOKIFY_OPENAI_BASE_URL` (for testing/proxies)
            headings are minimal; body is paragraph-first
            bullet lists are limited to key-point summaries
            avoid web/article vocabulary (e.g. prefer "本章では" over "この記事では")
            figures are included only when necessary; explain before placing a figure
            do not add facts that are not present in the input snapshot
        when engine is "noop", render sections by concatenating the extracted source material
    book_bundle [ book: string ; out: string ]
        => [ exit_code: 0 ]
        read `book/src/SUMMARY.md` and `book/src/**/*.md`
        write a single Markdown file that concatenates chapters in SUMMARY order
        rewrite internal chapter links to anchors where possible
        when `book/src/assets` exists, copy it to `assets/` next to `out` (without overwriting existing files)
        rewrite image paths from `../assets/*` to `assets/*`
        do not overwrite existing output files
    book_epub [ book: string ; out: string ]
        => [ exit_code: 0 ]
        read `book/src/SUMMARY.md` and `book/src/**/*.md`
        write an `.epub` file
        include `book/src/assets/**` into the EPUB under `OEBPS/assets/**`
        rewrite chapter links from `chXX.md#...` to `chXX.xhtml#...`
        rewrite image paths from `../assets/*` to `assets/*`
        do not overwrite existing output files
    build [
        url: string
        out: string
        title: string (optional)
        max_pages: number
        max_depth: number
        concurrency: number
        delay_ms: number
        language: string
        tone: string
        toc_engine: string
        render_engine: string
    ]
        => [ exit_code: 0 ]
        write `<OUT>/raw/**`, `<OUT>/extracted/**`, `<OUT>/manifest.jsonl`, `<OUT>/toc.yaml`, and `<OUT>/book/**`
        write `<OUT>/book.md`
        write `<OUT>/book.epub`
        write `<OUT>/assets/**`
        do not overwrite existing snapshot files
operational principle
    after crawl [ url: "http://127.0.0.1:<PORT>/docs/" ; out: "<TMP>/raw" ; max_pages: 20 ; max_depth: 8 ; concurrency: 2 ; delay_ms: 0 ]
        => [ exit_code: 0 ]
    then extract [ raw: "<TMP>/raw" ; out: "<TMP>/extracted" ]
        => [ exit_code: 0 ]
    then manifest [ extracted: "<TMP>/extracted" ; out: "<TMP>/manifest.jsonl" ]
        => [ exit_code: 0 ]
    then toc_create [ manifest: "<TMP>/manifest.jsonl" ; out: "<TMP>/toc.yaml" ; language: "日本語" ; tone: "丁寧" ; engine: "noop" ]
        => [ exit_code: 0 ]
    then book_init [ out: "<TMP>/book" ; title: "Test Book" ]
        => [ exit_code: 0 ]
    then book_render [ toc: "<TMP>/toc.yaml" ; manifest: "<TMP>/manifest.jsonl" ; out: "<TMP>/book" ; language: "日本語" ; tone: "丁寧" ; engine: "noop" ]
        => [ exit_code: 0 ]
        `book/src/chapters/ch01.md` contains `## Sources`
    then book_bundle [ book: "<TMP>/book" ; out: "<TMP>/book.md" ]
        => [ exit_code: 0 ]
        `<TMP>/book.md` contains `## Sources`
    then book_epub [ book: "<TMP>/book" ; out: "<TMP>/book.epub" ]
        => [ exit_code: 0 ]
        `<TMP>/book.epub` is a valid zip
    after build [ url: "http://127.0.0.1:<PORT>/docs/" ; out: "<TMP>/workspace" ; title: "Test Book" ; max_pages: 20 ; max_depth: 8 ; concurrency: 2 ; delay_ms: 0 ; language: "日本語" ; tone: "丁寧" ; toc_engine: "noop" ; render_engine: "noop" ]
        => [ exit_code: 0 ]
        `<TMP>/workspace/book/src/chapters/ch01.md` contains `## Sources`
        `<TMP>/workspace/book.md` contains `## Sources`
        `<TMP>/workspace/book.epub` is a valid zip
```

```text
concept Logging
purpose
    tracing による構造化ログを提供する。
    ログ詳細度を `RUST_LOG` で制御できるようにする。
state
    rust_log: string
actions
    init [ ]
        => [ ]
        configure `tracing_subscriber::EnvFilter` from `RUST_LOG`
        default to `info` when `RUST_LOG` is not set
        write logs to stderr
        when the effective filter enables debug, emit debug logs (e.g. `"parsed cli"`)
operational principle
    after init [ ]
        => [ ]
    then SitebookifyCLI/crawl [ url: "http://example.com/" ; out: "raw" ; max_pages: 1 ; max_depth: 0 ; concurrency: 1 ; delay_ms: 0 ]
        => [ exit_code: 0 ]
```

```text
concept RustToolchain
purpose
    Rust コードの代表的な品質ゲートを提供する。
state
    src_dir: string
    tests_dir: string
actions
    fmt_check [ ]
        => [ ok: boolean ]
        run `cargo fmt --all -- --check`
    clippy [ ]
        => [ ok: boolean ]
        run `cargo clippy --all-targets --all-features -- -D warnings`
    test [ ]
        => [ ok: boolean ]
        run `cargo test --all`
```

```text
concept IntegrationTests
purpose
    代表的な操作を Integration Test（`tests/`）で表現する。
    Integration Test では mock を使わない。
state
    tests_dir: string
actions
    run [ ]
        => [ ok: boolean ]
        run `cargo test --all`
        include an end-to-end pipeline test for `build` (internally runs `crawl` → `extract` → `manifest` → `toc create` → `book render` → `book bundle`)
```

```text
concept Protobuf
purpose
    Protobuf スキーマを Buf で管理する。
    Protobuf は API ではなく、オンディスク形式（Manifest/TOC）のスキーマとして扱う。
state
    proto_dir: string
    buf_yaml: string
    buf_lock: string
actions
    format_check [ ]
        => [ ok: boolean ]
        run `buf format -d --exit-code`
    lint [ ]
        => [ ok: boolean ]
        run `buf lint`
    dep_update [ ]
        => [ ok: boolean ]
        run `buf dep update` (writes `buf.lock`)
```

```text
concept Documentation
purpose
    ドキュメントを `docs/` 配下で管理し、文章とリンクの検査を行う。
state
    docs_dir: string
    docs_json: string
    vale_ini: string
actions
    vale [ ]
        => [ ok: boolean ]
        run `vale --config docs/.vale.ini --glob='*.mdx' docs`
    broken_links [ ]
        => [ ok: boolean ]
        run `mint broken-links` for `docs/`
```

```text
concept Textlint
purpose
    Markdown 文章を textlint で検査する。
state
    config: string
    prh: string
actions
    lint_markdown [ ]
        => [ ok: boolean ]
        run `textlint` for tracked `*.md` files (excluding `.codex/`)
```

## Synchronizations

```text
sync CI
when {
    Shell/request: [ command: "just ci" ]
        => [] }
then {
    RustToolchain/fmt_check: [ ]
    Protobuf/format_check: [ ]
    Protobuf/lint: [ ]
    RustToolchain/clippy: [ ]
    RustToolchain/test: [ ]
    Textlint/lint_markdown: [ ]
    Documentation/vale: [ ]
    Documentation/broken_links: [ ] }
```
