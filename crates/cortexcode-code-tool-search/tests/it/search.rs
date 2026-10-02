//! Port of hoocode `test/search.test.ts` and the non-eval cases of
//! `test/hybrid-search.test.ts` (v0.5.89), plus a check of the lexical leg
//! against the `rg` binary hoocode shells out to.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cortexcode_code_tool_search::*;

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn write(&self, rel: &str, content: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hit(id: &str, rank: usize, source: RetrieverSource, score: Option<f64>) -> RankedHit {
    RankedHit {
        id: id.into(),
        rank,
        source,
        score,
    }
}

use RetrieverSource::{Bm25, Embed, Grep};

// --- rrfFuse ---

#[test]
fn rrf_rejects_invalid_k_and_ranks() {
    assert!(rrf_fuse(&[], -1.0)
        .unwrap_err()
        .contains("finite non-negative"));
    assert!(rrf_fuse(&[], f64::NAN)
        .unwrap_err()
        .contains("finite non-negative"));
    assert!(rrf_fuse(&[vec![hit("a", 0, Grep, None)]], DEFAULT_RRF_K)
        .unwrap_err()
        .contains("positive integer"));
}

#[test]
fn rrf_fusion_rules() {
    let fused = rrf_fuse(
        &[
            vec![hit("both", 2, Grep, None), hit("greponly", 1, Grep, None)],
            vec![
                hit("both", 2, Embed, Some(0.9)),
                hit("embedonly", 1, Embed, Some(0.95)),
            ],
        ],
        DEFAULT_RRF_K,
    )
    .unwrap();
    assert_eq!(fused[0].id, "both");
    assert_eq!(fused[0].ranks.0, vec![(Grep, 2), (Embed, 2)]);
    assert_eq!(fused[0].raw_scores.0, vec![(Embed, 0.9)]);

    let clean = rrf_fuse(
        &[
            vec![hit("a", 1, Grep, None)],
            vec![hit("b", 1, Embed, None)],
        ],
        DEFAULT_RRF_K,
    )
    .unwrap();
    let dupes = rrf_fuse(
        &[
            vec![hit("a", 1, Grep, None), hit("a", 3, Grep, None)],
            vec![hit("b", 1, Embed, None)],
        ],
        DEFAULT_RRF_K,
    )
    .unwrap();
    let score = |v: &[FusedHit], id: &str| v.iter().find(|h| h.id == id).unwrap().rrf_score;
    assert_eq!(score(&dupes, "a"), score(&clean, "a"));
    assert_eq!(
        dupes.iter().find(|h| h.id == "a").unwrap().ranks.get(Grep),
        Some(1)
    );

    let worst_first = rrf_fuse(
        &[vec![hit("a", 5, Grep, None), hit("a", 2, Grep, None)]],
        DEFAULT_RRF_K,
    )
    .unwrap();
    assert_eq!(worst_first[0].ranks.get(Grep), Some(2));

    let ties = rrf_fuse(
        &[
            vec![hit("zzz", 1, Grep, None)],
            vec![hit("aaa", 1, Embed, None)],
        ],
        DEFAULT_RRF_K,
    )
    .unwrap();
    assert_eq!(
        ties.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        ["aaa", "zzz"]
    );

    let single = rrf_fuse(
        &[vec![
            hit("first", 1, Embed, Some(0.9)),
            hit("second", 2, Embed, Some(0.5)),
        ]],
        DEFAULT_RRF_K,
    )
    .unwrap();
    assert_eq!(
        single.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
        ["first", "second"]
    );
}

// --- runLexicalRetriever glob filter ---

fn glob_repo() -> TempDir {
    let d = TempDir::new("glob-test-");
    d.write("src/a.ts", "const alpha = 1;");
    d.write("src/b.ts", "const beta = 2;");
    d.write("docs/readme.md", "alpha docs");
    d
}

fn rels(cwd: &Path, query: &str, glob: Option<&str>) -> Vec<String> {
    run_lexical_retriever(RunLexicalOptions {
        cwd,
        query,
        limit: 10,
        glob,
        signal: None,
        paths: None,
    })
    .unwrap()
    .into_iter()
    .map(|h| h.rel)
    .collect()
}

#[test]
fn lexical_retriever_glob_filters() {
    let d = glob_repo();
    let r = rels(&d.0, "alpha", Some("*.ts"));
    assert!(r.contains(&"src/a.ts".to_string()) && !r.contains(&"docs/readme.md".to_string()));

    d.write("src/nested/c.ts", "const alphaNested = 1;");
    let r = rels(&d.0, "alpha", Some("src/**/*.ts"));
    assert!(r.contains(&"src/nested/c.ts".to_string()));
    assert!(r.contains(&"src/a.ts".to_string()));
    assert!(!r.contains(&"docs/readme.md".to_string()));

    let r = rels(&d.0, "alpha", None);
    assert!(r.contains(&"src/a.ts".to_string()) && r.contains(&"docs/readme.md".to_string()));
}

// --- resolveSearchMode ---

#[test]
fn search_mode_resolution() {
    let mode =
        |q: &str, requested, available| resolve_search_mode(q, requested, available, None).mode;
    use ResolvedSearchMode as R;
    use SearchMode as S;
    assert_eq!(
        resolve_search_mode("how does compaction work", S::Auto, true, None),
        ModeResolution {
            mode: R::Hybrid,
            degraded_reason: None
        }
    );
    assert_eq!(mode("foo\\.bar\\(", S::Auto, true), R::Lexical);
    assert_eq!(mode("\"token budget exceeded\"", S::Auto, true), R::Hybrid);
    assert_eq!(mode("src/core/sdk.ts", S::Auto, true), R::Hybrid);
    assert_eq!(
        resolve_search_mode("anything", S::Auto, false, Some("not enabled")),
        ModeResolution {
            mode: R::Lexical,
            degraded_reason: None
        }
    );
    let res = resolve_search_mode("anything", S::Hybrid, false, Some("repo under threshold"));
    assert_eq!(res.mode, R::Lexical);
    assert!(res
        .degraded_reason
        .unwrap()
        .contains("repo under threshold"));
    assert!(resolve_search_mode("anything", S::Semantic, false, None)
        .degraded_reason
        .is_some());
    assert_eq!(mode("q", S::Lexical, true), R::Lexical);
    assert_eq!(mode("q", S::Semantic, true), R::Semantic);
    assert_eq!(mode("q", S::Hybrid, true), R::Hybrid);

    assert!(!has_strong_lexical_signals(
        "where is retrieval mode selected"
    ));
    assert!(!has_strong_lexical_signals(
        "parseTokenStream overflow behavior"
    ));
    assert!(!has_strong_lexical_signals("\"pattern must not be empty\""));
    assert_eq!(
        mode("\"pattern must not be empty\"", S::Auto, true),
        R::Hybrid
    );
    assert!(!has_strong_lexical_signals(
        "\"Theme not initialized. Call initTheme() first.\""
    ));
    assert!(!has_strong_lexical_signals(
        "\"Nothing to compact (session too small)\""
    ));
    assert!(has_strong_lexical_signals("parse.*Args\\("));
    assert!(has_strong_lexical_signals("foo|bar"));
    assert_eq!(mode("foo|bar", S::Auto, true), R::Lexical);
}

// --- adaptGrepHits ---

fn indexed_lookup(rel: &str, line: usize) -> Option<ChunkRef> {
    if rel != "src/indexed.ts" {
        return None;
    }
    let chunk = |id: &str, start, end| ChunkRef {
        id: id.into(),
        span: CandidateSpan {
            path: rel.into(),
            start_line: start,
            end_line: end,
        },
    };
    match line {
        1..=60 => Some(chunk("src/indexed.ts#0", 1, 60)),
        61..=120 => Some(chunk("src/indexed.ts#1", 51, 120)),
        _ => None,
    }
}

#[test]
fn adapter_collapses_maps_and_synthesizes() {
    let lookup: &dyn Fn(&str, usize) -> Option<ChunkRef> = &indexed_lookup;
    let adapted = adapt_grep_hits(
        &[
            GrepLineHit::new("src/indexed.ts", 10),
            GrepLineHit::new("src/indexed.ts", 20),
            GrepLineHit::new("src/indexed.ts", 100),
        ],
        Some(lookup),
    );
    assert_eq!(
        adapted
            .hits
            .iter()
            .map(|h| (h.id.as_str(), h.rank))
            .collect::<Vec<_>>(),
        [("src/indexed.ts#0", 1), ("src/indexed.ts#1", 2)]
    );
    assert_eq!(
        adapted.spans["src/indexed.ts#0"],
        CandidateSpan {
            path: "src/indexed.ts".into(),
            start_line: 1,
            end_line: 60
        }
    );

    let adapted = adapt_grep_hits(&[GrepLineHit::new("src/indexed.ts", 55)], Some(lookup));
    assert_eq!(adapted.hits[0].id, "src/indexed.ts#0");

    let adapted = adapt_grep_hits(
        &[
            GrepLineHit::new("src/unindexed.ts", 7),
            GrepLineHit::new("src/indexed.ts", 5),
        ],
        Some(lookup),
    );
    assert_eq!(
        adapted
            .hits
            .iter()
            .map(|h| h.id.as_str())
            .collect::<Vec<_>>(),
        ["src/unindexed.ts#L7", "src/indexed.ts#0"]
    );
    assert_eq!(
        adapted.spans["src/unindexed.ts#L7"],
        CandidateSpan {
            path: "src/unindexed.ts".into(),
            start_line: 2,
            end_line: 12
        }
    );

    let adapted = adapt_grep_hits(&[GrepLineHit::new("a.ts", 3)], None);
    assert_eq!(
        adapted.hits,
        [RankedHit {
            id: "a.ts#L3".into(),
            rank: 1,
            score: None,
            source: Grep
        }]
    );
}

// --- buildLexicalPattern ---

#[test]
fn lexical_patterns() {
    assert_eq!(
        build_lexical_pattern("find \"token budget exceeded\" in code").as_deref(),
        Some("token budget exceeded")
    );
    assert_eq!(
        build_lexical_pattern("\"a.b(c)\"").as_deref(),
        Some("a\\.b\\(c\\)")
    );
    let pattern = build_lexical_pattern("where does parseTokenStream handle overflow").unwrap();
    assert!(
        pattern.contains("parseTokenStream") && pattern.contains('|') && !pattern.contains("does|")
    );
    assert_eq!(build_lexical_pattern("   "), None);
}

// --- assembleContext ---

fn cand(
    id: &str,
    path: &str,
    start: usize,
    end: usize,
    ranks: &[(RetrieverSource, usize)],
    rrf: f64,
) -> FusedCandidate {
    FusedCandidate::new(
        FusedHit {
            id: id.into(),
            rrf_score: rrf,
            ranks: SourceMap(ranks.to_vec()),
            raw_scores: SourceMap::default(),
            merged_from: None,
        },
        CandidateSpan {
            path: path.into(),
            start_line: start,
            end_line: end,
        },
    )
}

fn assemble_repo() -> TempDir {
    let d = TempDir::new("search-assemble-");
    let lines: Vec<String> = (1..=40).map(|i| format!("const line{i} = {i};")).collect();
    d.write("src/a.ts", &lines.join("\n"));
    d
}

#[test]
fn assembler_headers_snippets_budget_and_clamping() {
    let both = [(Grep, 1), (Embed, 2)];
    let d = assemble_repo();
    let out = assemble_context(
        &[cand("src/a.ts#0", "src/a.ts", 1, 5, &both, 1.0)],
        &d.0,
        None,
    );
    assert!(out.text.contains("src/a.ts:1-5 [embed+grep]"));
    assert!(
        out.text.contains("  1: const line1 = 1;") && out.text.contains("  5: const line5 = 5;")
    );
    assert_eq!(out.snippet_count, 1);

    let out = assemble_context(
        &[cand("src/a.ts#L38", "src/a.ts", 35, 43, &both, 1.0)],
        &d.0,
        None,
    );
    assert!(out.text.contains("src/a.ts:35-40") && !out.text.contains("41:"));

    let many: Vec<_> = (0..6)
        .map(|i| cand(&format!("src/a.ts#{i}"), "src/a.ts", 1, 20, &both, 1.0))
        .collect();
    let out = assemble_context(&many, &d.0, Some(25));
    assert!(out.snippet_count < many.len());
    assert_eq!(out.text.matches("src/a.ts:1-20").count(), 6);

    let out = assemble_context(
        &[cand("gone.ts#0", "gone.ts", 1, 10, &both, 1.0)],
        Path::new("/nonexistent-root"),
        None,
    );
    assert_eq!(out.text, "gone.ts:1-10 [embed+grep]");
    assert_eq!(out.snippet_count, 0);

    let five: Vec<_> = (0..5)
        .map(|i| cand(&format!("src/a.ts#{i}"), "src/a.ts", 1, 20, &both, 1.0))
        .collect();
    let out = assemble_context(&five, &d.0, None);
    assert_eq!(out.text.matches("  20: const line20 = 20;").count(), 3);
    assert_eq!(out.text.matches("  8: const line8 = 8;").count(), 5);

    d.write("b.ts", "const x = 1;\n\n\n");
    let out = assemble_context(&[cand("b.ts#0", "b.ts", 1, 4, &both, 1.0)], &d.0, None);
    assert!(out.text.contains("  1: const x = 1;") && !out.text.contains("  2:"));
}

// --- rerankCandidates ---

#[test]
fn reranker_prefers_declarations_for_names_but_not_prose() {
    let d = TempDir::new("rerank-test-");
    d.write("a.ts", "import { parseArgs } from './args.js';\nconst result = parseArgs(input);\nlog(parseArgs);\n\nexport function parseArgs(input: string): string[] {\n\treturn input.split(' ');\n}");
    let candidates = [
        cand("a.ts#1", "a.ts", 1, 3, &[], 0.5),
        cand("a.ts#5", "a.ts", 5, 7, &[], 0.4),
    ];
    let ranked = rerank_candidates("parseArgs", &candidates, &d.0);
    assert_eq!(ranked[0].start_line, 5);
    assert_eq!(
        rerank_candidates("parseArgs", &candidates, &d.0)
            .iter()
            .map(|c| c.id().to_owned())
            .collect::<Vec<_>>(),
        ranked.iter().map(|c| c.id().to_owned()).collect::<Vec<_>>()
    );
    assert_eq!(
        rerank_candidates("parseArgs", &candidates[..1], &d.0),
        candidates[..1].to_vec()
    );

    let d = TempDir::new("rerank-prose-");
    d.write("a.ts", "// computed request budget for\nbudget = 1;\n");
    d.write(
        "b.ts",
        "// computed request budget for\nconst budget = 1;\n",
    );
    let candidates = [
        cand("a.ts#1", "a.ts", 1, 2, &[], 0.5),
        cand("b.ts#1", "b.ts", 1, 2, &[], 0.4),
    ];
    assert_eq!(
        rerank_candidates("budget", &candidates, &d.0)[0].path,
        "b.ts"
    );
    assert_eq!(
        rerank_candidates(
            "how is the budget computed for a request",
            &candidates,
            &d.0
        )[0]
        .path,
        "a.ts"
    );

    let d = TempDir::new("rerank-");
    d.write("weak.ts", "only budget here\n");
    d.write("strong.ts", "tokenBudget assembler snippet window\n");
    let g = [(Grep, 1)];
    let ranked = rerank_candidates(
        "tokenBudget assembler window",
        &[
            cand("weak.ts#0", "weak.ts", 1, 1, &g, 1.0),
            cand("strong.ts#0", "strong.ts", 1, 1, &g, 1.0),
        ],
        &d.0,
    );
    assert_eq!(ranked[0].id(), "strong.ts#0");

    d.write("src/other.ts", "mentions hybrid-search.ts in a comment\n");
    d.write("src/hybrid-search.ts", "export const x = 1;\n");
    let ranked = rerank_candidates(
        "src/hybrid-search.ts",
        &[
            cand("src/other.ts#0", "src/other.ts", 1, 1, &g, 1.0),
            cand(
                "src/hybrid-search.ts#0",
                "src/hybrid-search.ts",
                1,
                1,
                &g,
                1.0,
            ),
        ],
        &d.0,
    );
    assert_eq!(ranked[0].id(), "src/hybrid-search.ts#0");

    let ranked = rerank_candidates(
        "nomatch",
        &[
            cand("a.ts#0", "a.ts", 1, 1, &g, 1.0),
            cand("b.ts#0", "b.ts", 1, 1, &g, 1.0),
        ],
        Path::new("/nonexistent"),
    );
    assert_eq!(
        ranked.iter().map(|c| c.id()).collect::<Vec<_>>(),
        ["a.ts#0", "b.ts#0"]
    );
}

// --- hoistStaleCandidates / mergeOverlappingSpans ---

#[test]
fn hoisting_and_merging() {
    let c = |id: &str, p: &str| cand(id, p, 1, 5, &[], 0.1);
    let stale: HashSet<String> = ["fresh.ts", "fresh2.ts"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let out = hoist_stale_candidates(
        &[
            c("a", "indexed.ts"),
            c("b", "fresh.ts"),
            c("c", "indexed2.ts"),
            c("d", "fresh2.ts"),
        ],
        &stale,
    );
    assert_eq!(
        out.iter().map(|c| c.id()).collect::<Vec<_>>(),
        ["b", "d", "a", "c"]
    );
    let input = [c("a", "x.ts"), c("b", "y.ts")];
    assert_eq!(
        hoist_stale_candidates(&input, &HashSet::new()),
        input.to_vec()
    );
    let input: Vec<_> = (0..12)
        .map(|i| c(&format!("c{i}"), &format!("f{i}.ts")))
        .collect();
    let all: HashSet<String> = input.iter().map(|c| c.path.clone()).collect();
    assert_eq!(hoist_stale_candidates(&input, &all), input);

    let m =
        |id: &str, p: &str, s, e, ranks: &[(RetrieverSource, usize)]| cand(id, p, s, e, ranks, 0.1);
    let out = merge_overlapping_spans(&[m("a", "x.ts", 10, 40, &[]), m("b", "x.ts", 30, 60, &[])]);
    assert_eq!(out.len(), 1);
    assert_eq!(
        (out[0].id(), out[0].start_line, out[0].end_line),
        ("a", 10, 60)
    );
    let out = merge_overlapping_spans(&[m("a", "x.ts", 1, 20, &[]), m("b", "x.ts", 21, 40, &[])]);
    assert_eq!((out.len(), out[0].start_line, out[0].end_line), (1, 1, 40));
    let out = merge_overlapping_spans(&[m("a", "x.ts", 1, 20, &[]), m("b", "x.ts", 40, 60, &[])]);
    assert_eq!(out.iter().map(|c| c.id()).collect::<Vec<_>>(), ["a", "b"]);
    let out = merge_overlapping_spans(&[m("a", "x.ts", 10, 40, &[]), m("b", "y.ts", 10, 40, &[])]);
    assert_eq!(out.iter().map(|c| c.id()).collect::<Vec<_>>(), ["a", "b"]);
    let out = merge_overlapping_spans(&[
        m("a", "x.ts", 10, 40, &[(Embed, 3)]),
        m("b", "x.ts", 30, 60, &[(Grep, 1), (Embed, 9)]),
    ]);
    assert_eq!(out[0].hit.ranks.0, vec![(Embed, 3), (Grep, 1)]);
    assert_eq!(out[0].hit.merged_from, Some(vec!["b".to_string()]));
    let input = [
        m("a", "x.ts", 10, 40, &[(Embed, 1)]),
        m("b", "x.ts", 30, 60, &[]),
    ];
    merge_overlapping_spans(&input);
    assert_eq!((input[0].start_line, input[0].end_line), (10, 40));
}

#[test]
fn prose_detection() {
    for q in [
        "how does the agent decide between lexical and semantic retrieval",
        "where is the plan approval message assembled from plan sections",
        "which files are skipped when scanning the repository for indexing",
        "the index is stale",
    ] {
        assert!(query_is_prose(q), "{q}");
    }
    for q in [
        "rerankCandidates",
        "core/search/hybrid-search.ts",
        "hybrid search fusion",
        "\"Theme not initialized. Call initTheme() first.\"",
        "isForwardedRequest",
        "the index",
    ] {
        assert!(!query_is_prose(q), "{q}");
    }
}

// --- hybrid-search.test.ts (stubbed service) ---

fn hybrid_repo() -> TempDir {
    let d = TempDir::new("hybrid-search-");
    let mut indexed: Vec<String> = (1..=20).map(|i| format!("line {i}")).collect();
    indexed[4] = "function spendTokenBudget() {".into();
    indexed[14] = "const tokenBudget = 2000;".into();
    d.write("src/indexed.ts", &indexed.join("\n"));
    let mut unindexed: Vec<String> = (1..=10).map(|i| format!("filler {i}")).collect();
    for i in [2, 4, 6] {
        unindexed[i] = "tokenBudget".into();
    }
    d.write("src/unindexed.ts", &unindexed.join("\n"));
    d.write("src/other.ts", "conceptually related, no literal match\n");
    d
}

type SearchFn = dyn Fn(ChunkRetriever) -> Vec<ChunkHit> + Send + Sync;

struct StubService {
    available: bool,
    state: EmbsearchState,
    search: Box<SearchFn>,
    lookup: bool,
}

impl EmbsearchService for StubService {
    fn state(&self) -> EmbsearchState {
        self.state.clone()
    }
    fn is_available(&self) -> bool {
        self.available
    }
    fn search_chunks(
        &self,
        _: &str,
        _: usize,
        _: Option<&str>,
        retriever: ChunkRetriever,
    ) -> Result<Vec<ChunkHit>, String> {
        Ok((self.search)(retriever))
    }
    fn find_enclosing_chunk(&self, rel: &str, line: usize) -> Option<ChunkRef> {
        if !self.lookup || rel != "src/indexed.ts" {
            return None;
        }
        let (id, start, end) = if line <= 10 {
            ("src/indexed.ts#0", 1, 10)
        } else {
            ("src/indexed.ts#1", 11, 20)
        };
        Some(ChunkRef {
            id: id.into(),
            span: CandidateSpan {
                path: rel.into(),
                start_line: start,
                end_line: end,
            },
        })
    }
}

fn chunk(id: &str, path: &str, start: usize, end: usize, score: f64) -> ChunkHit {
    ChunkHit {
        id: id.into(),
        path: path.into(),
        start_line: start,
        end_line: end,
        score,
    }
}

fn ready_service() -> Arc<dyn EmbsearchService> {
    Arc::new(StubService {
        available: true,
        state: EmbsearchState::Ready { chunk_count: 3 },
        search: Box::new(|_| {
            vec![
                chunk("src/indexed.ts#0", "src/indexed.ts", 1, 10, 0.9),
                chunk("src/other.ts#0", "src/other.ts", 1, 1, 0.5),
            ]
        }),
        lookup: true,
    })
}

fn down_service() -> Arc<dyn EmbsearchService> {
    Arc::new(StubService {
        available: false,
        state: EmbsearchState::Unavailable {
            reason: "binary not found".into(),
        },
        search: Box::new(|_| Vec::new()),
        lookup: false,
    })
}

static AGENT_DIR: Mutex<()> = Mutex::new(());

/// Runs `f` with the agent dir (for search traces) in a temp dir.
fn with_agent_dir<T>(f: impl FnOnce(&Path) -> T) -> T {
    let _guard = AGENT_DIR.lock().unwrap_or_else(|e| e.into_inner());
    let dir = TempDir::new("search-agent-dir-");
    let saved = std::env::var_os("CORTEXCODE_CODING_AGENT_DIR");
    std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", &dir.0);
    let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&dir.0)));
    match saved {
        Some(v) => std::env::set_var("CORTEXCODE_CODING_AGENT_DIR", v),
        None => std::env::remove_var("CORTEXCODE_CODING_AGENT_DIR"),
    }
    out.unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn ids(candidates: &[FusedCandidate]) -> Vec<&str> {
    candidates.iter().map(|c| c.id()).collect()
}

#[test]
fn hybrid_fusion_rerank_and_degradation() {
    let d = hybrid_repo();
    let opts = |mode, service: Option<Arc<dyn EmbsearchService>>| RetrieveOptions {
        mode,
        service,
        ..RetrieveOptions::new(&d.0, "tokenBudget")
    };

    let r = retrieve_candidates(&RetrieveOptions {
        rerank: Some(false),
        ..opts(SearchMode::Hybrid, Some(ready_service()))
    })
    .unwrap();
    assert_eq!(r.resolved_mode, ResolvedSearchMode::Hybrid);
    assert_eq!(r.candidates[0].id(), "src/indexed.ts#0");
    let mut sources: Vec<&str> = r.candidates[0]
        .hit
        .ranks
        .iter()
        .map(|(s, _)| s.as_str())
        .collect();
    sources.sort();
    assert_eq!(sources, ["embed", "grep"]);
    assert!(ids(&r.candidates).contains(&"src/other.ts#0"));
    assert!(ids(&r.candidates).contains(&"src/unindexed.ts#L3"));

    let r = retrieve_candidates(&opts(SearchMode::Hybrid, Some(ready_service()))).unwrap();
    assert_eq!(r.candidates[0].id(), "src/indexed.ts#1");

    let bm25: Arc<dyn EmbsearchService> = Arc::new(StubService {
        available: true,
        state: EmbsearchState::Ready { chunk_count: 3 },
        search: Box::new(|retriever| match retriever {
            ChunkRetriever::Lexical => {
                vec![chunk("src/indexed.ts#1", "src/indexed.ts", 11, 20, 3.7)]
            }
            _ => vec![chunk("src/other.ts#0", "src/other.ts", 1, 1, 0.9)],
        }),
        lookup: true,
    });
    let r = retrieve_candidates(&RetrieveOptions {
        bm25_leg: Some(true),
        rerank: Some(false),
        ..opts(SearchMode::Semantic, Some(bm25))
    })
    .unwrap();
    let hit = r
        .candidates
        .iter()
        .find(|c| c.id() == "src/indexed.ts#1")
        .expect("BM25-only chunk survives");
    assert_eq!(hit.hit.ranks.get(Bm25), Some(1));
    assert_eq!(hit.hit.raw_scores.get(Bm25), Some(3.7));
    assert_eq!(
        r.candidates
            .iter()
            .find(|c| c.id() == "src/other.ts#0")
            .unwrap()
            .hit
            .ranks
            .get(Embed),
        Some(1)
    );

    let r = retrieve_candidates(&RetrieveOptions {
        bm25_leg: Some(true),
        rerank: Some(false),
        ..opts(SearchMode::Hybrid, Some(down_service()))
    })
    .unwrap();
    assert_eq!(r.resolved_mode, ResolvedSearchMode::Lexical);
    assert!(r.candidates.iter().all(|c| c.hit.ranks.get(Bm25).is_none()));

    let padded: Arc<dyn EmbsearchService> = Arc::new(StubService {
        available: true,
        state: EmbsearchState::Ready { chunk_count: 3 },
        search: Box::new(|_| {
            vec![
                chunk("src/indexed.ts#0", "src/indexed.ts", 1, 10, 0.9),
                chunk("src/other.ts#0", "src/other.ts", 1, 1, 0.0),
            ]
        }),
        lookup: false,
    });
    let r = retrieve_candidates(&RetrieveOptions {
        query: "zzz_no_lexical_match_zzz",
        ..opts(SearchMode::Semantic, Some(padded))
    })
    .unwrap();
    assert!(ids(&r.candidates).contains(&"src/indexed.ts#0"));
    assert!(!ids(&r.candidates).contains(&"src/other.ts#0"));

    let r = retrieve_candidates(&opts(SearchMode::Lexical, None)).unwrap();
    let fallbacks: Vec<_> = r
        .candidates
        .iter()
        .filter(|c| c.path == "src/unindexed.ts")
        .collect();
    assert_eq!(fallbacks.len(), 1);
    assert_eq!(
        (
            fallbacks[0].id(),
            fallbacks[0].start_line,
            fallbacks[0].end_line
        ),
        ("src/unindexed.ts#L3", 1, 12)
    );

    let r = retrieve_candidates(&opts(SearchMode::Hybrid, Some(down_service()))).unwrap();
    assert_eq!(r.resolved_mode, ResolvedSearchMode::Lexical);
    assert!(r.degraded_reason.unwrap().contains("binary not found"));
    assert_eq!(r.index_phase, "unavailable");
    assert!(!r.candidates.is_empty());
}

#[test]
fn run_search_returns_budgeted_text_and_writes_a_trace() {
    let d = hybrid_repo();
    with_agent_dir(|_| {
        let r = run_search(
            &RetrieveOptions {
                mode: SearchMode::Hybrid,
                service: Some(ready_service()),
                ..RetrieveOptions::new(&d.0, "tokenBudget")
            },
            None,
        )
        .unwrap();
        assert_eq!(r.resolved_mode, ResolvedSearchMode::Hybrid);
        assert!(
            r.text.contains("src/indexed.ts:1-20 [embed+grep]"),
            "{}",
            r.text
        );
        assert!(!r.text.contains("src/indexed.ts:11-"));
        let trace_path = search_trace_path(&d.0);
        let last = std::fs::read_to_string(&trace_path)
            .unwrap()
            .trim()
            .lines()
            .last()
            .unwrap()
            .to_owned();
        let trace: serde_json::Value = serde_json::from_str(&last).unwrap();
        assert_eq!(trace["resolvedMode"], "hybrid");
        assert_eq!(trace["rrfK"], DEFAULT_RRF_K);
        assert!(trace["retrievers"]["grep"]["hitCount"].as_u64().unwrap() > 0);
        assert_eq!(trace["retrievers"]["embed"]["hitCount"], 2);
        assert_eq!(trace["fused"][0]["id"], "src/indexed.ts#1");
        assert!(trace["fused"][0]["mergedFrom"]
            .as_array()
            .unwrap()
            .contains(&"src/indexed.ts#0".into()));
    });
}

// --- the tool ---

#[test]
fn search_tool_lexical_results_and_empty_result_text() {
    let d = hybrid_repo();
    with_agent_dir(|_| {
        let tool = create_search_tool_definition(&d.0, SearchToolOptions::default());
        let result = (tool.execute)(
            "c".into(),
            serde_json::json!({"query": "tokenBudget"}),
            None,
            None,
            None,
        )
        .unwrap();
        let text = match &result.content[0] {
            cortexcode_ai_types::Content::Text(t) => t.text.clone(),
            _ => unreachable!(),
        };
        assert!(text.starts_with("src/indexed.ts:"), "{text}");
        assert_eq!(result.details["resolvedMode"], "lexical");
        let result = (tool.execute)(
            "c".into(),
            serde_json::json!({"query": "zzz_nothing_zzz", "mode": "hybrid"}),
            None,
            None,
            None,
        )
        .unwrap();
        let text = match &result.content[0] {
            cortexcode_ai_types::Content::Text(t) => t.text.clone(),
            _ => unreachable!(),
        };
        assert_eq!(text, "No results for \"zzz_nothing_zzz\" (lexical)\n\n[hybrid requested but semantic index is not enabled]");
        assert_eq!(
            result.details,
            serde_json::json!({"resultCount": 0, "resolvedMode": "lexical"})
        );
    });
}

// --- the lexical leg against rg ---

/// hoocode's rg invocation, parsed the way lexical-retriever.ts does.
fn rg_hits(cwd: &Path, pattern: &str, glob: Option<&str>) -> Option<Vec<(String, usize)>> {
    let mut args = vec![
        "--json",
        "--line-number",
        "--color=never",
        "--hidden",
        "--no-require-git",
        "--ignore-case",
        "--sort",
        "path",
        "--glob",
        "!**/.git/**",
    ];
    if let Some(glob) = glob {
        args.extend(["--glob", glob]);
    }
    let cwd_str = cwd.to_string_lossy().into_owned();
    args.extend(["--", pattern, &cwd_str]);
    let output = std::process::Command::new("rg")
        .args(&args)
        .env_remove("RIPGREP_CONFIG_PATH")
        .output()
        .ok()?;
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|e| e["type"] == "match")
            .map(|e| {
                let path = e["data"]["path"]["text"].as_str().unwrap();
                let rel = Path::new(path)
                    .strip_prefix(cwd)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                (rel, e["data"]["line_number"].as_u64().unwrap() as usize)
            })
            .collect(),
    )
}

#[test]
fn lexical_leg_matches_ripgrep_when_installed() {
    let d = TempDir::new("rg-parity-");
    d.write(".gitignore", "ignored/\n*.log\n");
    d.write("ignored/x.ts", "alpha hidden by gitignore\n");
    d.write("debug.log", "alpha in a log\n");
    d.write(".hidden/h.ts", "Alpha in a hidden dir\n");
    d.write(".git/config", "alpha inside git\n");
    d.write("sub/.ignore", "skip.ts\n");
    d.write("sub/skip.ts", "alpha skipped by .ignore\n");
    d.write("sub/keep.ts", "one\nALPHA two\nthree alpha\n");
    d.write("a-file.ts", "alpha\n");
    d.write("a.ts", "alphabet\n");
    d.write("B.ts", "alpha upper\n");
    d.write("z/deep/n.md", "alpha deep\n");
    std::fs::write(d.0.join("bin.dat"), b"alpha\x00binary\n").unwrap();
    for glob in [None, Some("*.ts"), Some("**/sub/**")] {
        let Some(expected) = rg_hits(&d.0, "alpha", glob) else {
            eprintln!("rg not installed; skipping the parity check");
            return;
        };
        let got: Vec<(String, usize)> = run_lexical_retriever(RunLexicalOptions {
            cwd: &d.0,
            query: "alpha",
            limit: 200,
            glob,
            signal: None,
            paths: None,
        })
        .unwrap()
        .into_iter()
        .map(|h| (h.rel, h.line))
        .collect();
        assert_eq!(got, expected, "glob {glob:?}");
    }
}
