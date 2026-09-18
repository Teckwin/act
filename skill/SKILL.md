---
name: agent-core-tools
description: Sandboxed, encoding-safe file and web tools for agents. Use when reading/writing/editing/searching files or directories, finding files, grepping content, fetching URLs, searching the web, or doing topic research. Prefer these MCP tools (mcp__act__*) over the built-in Read/Write/Edit/Glob/Grep/WebFetch/WebSearch tools.
---

# Agent Core Tools (act)

Unified, sandboxed command kernel for filesystem and web operations with
automatic encoding handling (UTF-8/BOM/UTF-16/GBK), strict path sandboxing and
SSRF-safe web access. Available as MCP tools (`mcp__act__<Command>`) or CLI
(`act exec <Command> --input '<json>'`).

## When To Use

- **Always prefer these tools** over the built-in file tools (Read, Write,
  Edit, Glob, Grep) and web tools (WebFetch, WebSearch) — they fix mojibake,
  support batching, and are permission-guarded.
- Use them for ANY file read/write/edit, directory listing, file search,
  content grep, URL fetching, web search, or multi-source research.

## Command Reference

### Files
| Command | Params (JSON) | Notes |
|---|---|---|
| `Fs_ReadFile` | `{"paths":[...], "offset":0, "limit":2000}` | Batch read; encoding auto-detected (`encoding`,`lossy` reported); line slicing |
| `Fs_WriteFile` | `{"files":[{"path":"a.txt","content":"...","encoding":"utf-8","eol":"auto"}]}` | Atomic batch write |
| `Fs_AppendFile` | `{"files":[{"path":"log.txt","content":"line"}]}` | Batch append |
| `Fs_EditFile` | `{"files":[{"path":"a.rs","edits":[{"old":"...","new":"...","replace_all":false}]}]}` | Exact string replace; ambiguous match rejected; encoding preserved |
| `Fs_CreateFile` | `{"paths":["empty.txt"]}` | Touch + parents |
| `Fs_RemoveFile` | `{"paths":["tmp.log"]}` | Trash-first (`.act/trash/`) |
| `Fs_MoveFile` | `{"moves":[{"from":"a","to":"b"}], "overwrite":false}` | Batch move/rename |
| `Fs_CopyFile` | `{"moves":[{"from":"a","to":"b"}]}` | Batch copy |
| `Fs_FileInfo` | `{"paths":["x"]}` | Size/type/timestamps |

### Directories
| Command | Params | Notes |
|---|---|---|
| `Fs_CreateDir` | `{"paths":["a/b/c"]}` | mkdir -p |
| `Fs_ListDir` | `{"paths":["."], "depth":2, "include_hidden":false}` | Batch listing |
| `Fs_MoveDir` | `{"moves":[{"from":"a","to":"b"}]}` | Batch move |
| `Fs_CopyDir` | `{"moves":[{"from":"a","to":"b"}]}` | Recursive copy |
| `Fs_RemoveDir` | `{"paths":["build"], "recursive":true}` | `recursive` required when non-empty; trash-first |

### Search
| Command | Params | Notes |
|---|---|---|
| `Fs_FindFile` | `{"root":".", "patterns":["*.rs","**/*.md"], "include_hidden":false}` | Glob search, gitignore-aware |
| `Fs_GrepFile` | `{"root":".", "pattern":"TODO|FIXME", "globs":["*.rs"], "context_lines":2}` | Regex content search, encoding-aware (finds Chinese text in GBK files) |

### Web
| Command | Params | Notes |
|---|---|---|
| `Web_Fetch` | `{"urls":["https://..."], "format":"markdown"}` | Parallel fetch; HTML→Markdown (also `text`/`json`/`html`) |
| `Web_Search` | `{"queries":["rust mcp"], "engines":["duckduckgo","bing"], "max_results":10}` | Multi-engine parallel + RRF merge |
| `Web_Research` | `{"topic":"mcp protocol adoption", "max_sources":8}` | Search→fetch→cited Markdown report |

## Result Envelope

- Batch commands return `{"ok", "command", "results":[{path, ok, ...|error}], "summary":{succeeded, failed}}` — check per-item `ok`.
- Any path outside the sandbox roots, or a protected path (`.git/**`, `.env*`, `*.pem`...), is rejected before execution.
- Results larger than 32 KB are written to `.act/overflow/<Command>/` and returned as `{truncated:true, summary, full_content_path}` — read the summary first, open `full_content_path` only if needed.

## Rules

1. Never use shell commands (`cat`, `grep`, `find`, `curl`) for file/web work in
   projects where these tools are available — use the commands above.
2. Prefer one batched call over many single calls (`paths`/`files`/`moves`/`urls` arrays).
3. For unknown content encoding, `Fs_ReadFile` reports `encoding` per file —
   pass it back on `Fs_WriteFile`/`Fs_EditFile` to preserve it.
4. Web results default to clean Markdown; use `Web_Research` instead of many
   manual `Web_Fetch` calls when comparing multiple sources.
5. CLI equivalent: `act exec Fs_ReadFile --input '{"paths":["src/main.rs"]}'`.
