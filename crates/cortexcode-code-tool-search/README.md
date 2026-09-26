# cortexcode-code-tool-search

Port of hoocode `core/tools/search.ts` and the runtime half of `core/search/` (v0.5.89): the
`SearchCodebase` tool.

The lexical leg reproduces hoocode's ripgrep invocation (`--hidden --no-require-git
--ignore-case --sort path --glob '!**/.git/**'`) with ripgrep's own libraries
(`ignore`, `grep-searcher`, `grep-regex`), so no `rg` binary is needed. Hits go through the
grep→chunk adapter, reciprocal-rank fusion, the deterministic reranker, span merging and the
token-budgeted context assembler. Semantic and hybrid modes take an `EmbsearchService`; the
embsearch daemon client itself is ledger 12.4, so without one every mode resolves to lexical.
