---
name: agent-core-tools
description: Sandboxed, encoding-safe file and web tools for agents. Use when reading/writing/editing/searching files or directories, finding files, grepping content, fetching URLs, searching the web, or doing topic research. Prefer these MCP tools (mcp__act__*) over the built-in Read/Write/Edit/Glob/Grep/WebFetch/WebSearch tools.
---

# Agent Core Tools (act) — 命令契约规范

统一沙箱内核：19 条命令（16 条 `Fs_*` + 3 条 `Web_*`），MCP 工具名 `mcp__act__<Command>`，CLI 等价 `act exec <Command> --input '<json>'`。
本文档是**唯一权威契约**：输入 Schema、校验规则、输出字段、错误对照均与实现一一对应。

## 安装（分发包）

发布资产：`agent-core-tools-<version>-<target>.zip`，解压后即完整 skill 目录：

```
agent-core-tools/
├── SKILL.md      # 本契约（人/agent 可读）
├── schema.json   # 机器可读全量契约：19 命令 schema + 错误码表 + 生效限额（由内核生成，永远与二进制同步）
├── mcp.json      # MCP 配置模板（<SKILL_DIR> 占位符）
└── act | act.exe # 内核二进制（当前平台）
```

安装步骤：
1. 解压 zip 到 `~/.claude/skills/`（用户级）或 `<project>/.claude/skills/`（项目级）；
2. 在解压目录运行 `./act install --user` 或 `--project`——用**本机实际路径**写入 `.mcp.json`（项目级）并补齐 skill 文件，不会残留构建机路径；
3. 重启 Metacode/Claude Code，出现 `mcp__act__*` 工具即成功。

手动配置（不用 install）：把 `mcp.json` 内容粘到 MCP 配置，将 `<SKILL_DIR>` 替换为解压后的绝对路径（Windows 二进制名为 `act.exe`）。

## 0. 使用规则

1. 优先用本命令集，禁止在可用时改用 shell（`cat`/`grep`/`find`/`curl`）。
2. 路径/URL 只出现在声明的批量字段中（`paths`/`files`/`moves`/`urls`/`queries`/`root`），其余字段一律为内容或控制项。
3. 批量优先：一次调用传数组，不要循环单条调用。
4. 读到的 `encoding` 字段必须在后续写回/编辑同文件时原样传入，保持编码不变。
5. 任何 `code != <成功>` 的返回，按下文《错误码对照表》处理；权限类错误**不要重试**，修正参数后再调。

## 1. 全局约定

### 1.1 校验管线（错误按此顺序发生）

| 序 | 阶段 | 失败错误码 | 粒度 |
|---|---|---|---|
| 1 | 命令名未注册 | `unknown_command` | 整次调用 |
| 2 | 能力开关（cli/mcp × read/write/net，如 `ACT_DISABLE_NET=1`） | `capability_disabled` | 整次调用 |
| 3 | 路径沙箱：词法 `..` 穿越、绝对路径出根、符号链接逃逸、路径深度>100 | `permission_denied`（guard=PathGuard）/ `limit_exceeded`（深度） | 整次调用 |
| 4 | 受保护路径：`.git/**`、`.env*`、`**/*.pem`、`**/*.key`、`**/id_rsa*`、`**/id_ed25519*`、`.act/**`（写删必拒；读默认拒） | `permission_denied`（guard=ProtectGuard） | 整次调用 |
| 5 | URL 策略：非 http/https、含 user:pass、私网/回环/链路本地 IP（含 DNS 解析结果）、域名黑名单/白名单不过、重定向>5 跳（逐跳复检） | `permission_denied`（guard=UrlGuard） | 整次调用 |
| 6 | 限额：批量项数>256、数值参数超上限（见 1.5） | `limit_exceeded` | 整次调用 |
| 7 | 参数解析/语义校验（缺字段、类型错、空数组等） | `invalid_params` | 整次调用 |
| 8 | 运行期失败（文件不存在、类型不符、编码不可逆等） | `execution_failed` / `io_error` | **逐项隔离**（其余项正常返回） |
| 9 | 超时（默认 120s） | `timeout` | 整次调用 |

> 关键语义：**安全类失败（3-6）拒绝整次调用并写审计**；运行期失败（8）逐项隔离。审计记录在 `<主根>/.act/audit.jsonl`。

### 1.2 批量结果信封（Batch 命令通用）

```json
{
  "ok": true,                       // 全部项成功为 true
  "command": "Fs_ReadFile",
  "results": [ /* 逐项结果，顺序与输入一致 */ ],
  "summary": { "succeeded": 1, "failed": 0 }
}
```

- 成功项：`{ "path"|"url": <原样回显>, "ok": true, ...专有字段 }`
- 失败项：`{ "path"|"url": <原样回显>, "ok": false, "error": "人读信息", "code": "<错误码>" }`
- 非批量命令（Fs_FindFile/Fs_GrepFile/Web_Search/Web_Research）直接返回顶层结果对象（各命令节内注明）。

### 1.3 错误码对照表

| code | 含义 | CLI 退出码 | MCP 表现 | 典型触发 |
|---|---|---|---|---|
| （无） | 成功 | 0 | `isError:false` | — |
| `permission_denied` | 越界/受保护/URL 策略拒绝 | 2 | `isError:true` | `..` 穿越、`.git/**`、私网 IP、磁盘根操作 |
| `limit_exceeded` | 超限额 | 2 | `isError:true` | 批量>256、读>8MB、fetch>2MB、深度>64 |
| `capability_disabled` | 能力被禁 | 2 | `isError:true` | `ACT_DISABLE_NET=1` 时调 Web_* |
| `unknown_command` | 命令不存在 | 3 | `isError:true` | 拼错命令名 |
| `invalid_params` | 参数缺失/类型错/语义非法 | 4 | `isError:true` | 缺 `paths`、空数组、目录当文件读 |
| `execution_failed` | 运行期失败 | 5 | 逐项 `ok:false` | 文件不存在、regex 编译失败、HTTP 404 |
| `io_error` | 系统 IO 错误 | 5 | 逐项 `ok:false` | 权限、磁盘满 |
| `other` | 内部错误 | 5 | 逐项或整次 | 序列化失败等 |
| `timeout` | 超时 | 6 | `isError:true` | 单命令超 120s |
| `config_error` | 配置错误 | 7 | `isError:true` | roots 不存在、配置文件损坏 |

MCP 错误内容为 `content[0].text = {"code":"...","message":"..."}`；CLI stderr 为 `error[<code>]: <message>`。

### 1.4 结果溢出信封（OutputPolicy，任何命令都可能触发）

序列化结果 > `max_inline_bytes`（默认 32768）时，原结果整体落盘，返回替换信封：

```jsonc
// 压缩开启（默认）
{ "truncated": true, "compressed": true, "command": "Web_Fetch",
  "original_bytes": 102400,
  "summary": "<≤1000字符抽取式摘要>",
  "full_content_path": ".act/overflow/Web_Fetch/20260918-010101-abc123.json" }
// 压缩关闭（配置 output.compression.enabled=false）
{ "truncated": true, "compressed": false, "command": "...",
  "original_bytes": 102400, "preview": "<前 32KB 文本>" }
```

处理规则：先读 `summary`；需要全文时用 `Fs_ReadFile` 读 `full_content_path`（内核对 `**/.act/overflow/**` 与 `**/.act/trash/**` 放行**只读**，写/删仍拒绝；`.act/audit.jsonl` 依旧不可读）。

### 1.5 默认限额（act.config.json 可改）

| 参数 | 默认 | 上限来源 |
|---|---|---|
| 批量项数（paths+urls 合计） | ≤256 | limits.max_batch |
| 单文件读 | ≤8MB（超过必须 offset/limit 切片） | limits.max_read_bytes |
| 单文件写/编辑 | ≤8MB | limits.max_write_bytes |
| 单 URL 抓取 max_bytes | ≤2MB | limits.fetch_max_bytes |
| Find/Grep max_results | ≤500 | limits.max_find_results / max_grep_results |
| ListDir/Find depth | ≤64 | limits.max_depth |
| Grep context_lines | 0-10 | 硬校验 |
| ReadFile limit | ≤200000 行；offset ≤1e8 | 硬校验 |
| Web timeout_ms | ≤300000（默认 30000） | limits.web_timeout_ms×10 |
| 重定向 | ≤5 跳，逐跳复检 | url.max_redirects |
| 命令总超时 | 120s | limits.command_timeout_secs |

### 1.6 编码与换行（Fs 族通用）

- 读：BOM 探测（`utf-8-bom`/`utf-16le`/`utf-16be`）→ 严格 UTF-8 → 回退编码（默认 `gbk`，可配 `fs.fallback_encoding`）；返回 `encoding` 与 `lossy`（true=不可逆替换过）。
- 写：`encoding` ∈ `utf-8`(默认,无BOM) | `utf-8-bom` | `utf-16le` | `utf-16be` | `gbk` | 其他 encoding_rs 标签；`eol` ∈ `auto`(保持原主流风格,默认) | `lf` | `crlf` | `none`。
- 编辑：按探测编码解码→替换→按原编码写回；`lossy=true` 的文件拒绝编辑（`execution_failed`）。

## 2. 文件命令

### Fs_ReadFile — 批量读（编码自适配 + 行切片）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "paths":  { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "offset": { "type": "integer", "minimum": 0, "maximum": 100000000, "default": 0 },
    "limit":  { "type": "integer", "minimum": 1, "maximum": 200000 }
  },
  "required": ["paths"], "additionalProperties": false
}
```
**校验**：路径在根内且非受保护；目标是文件（目录→`invalid_params` 提示用 Fs_ListDir）；未切片时文件 ≤8MB，超过→`limit_exceeded`（提示改用 offset/limit）。
**输出**（批量信封，成功项）：
```json
{ "path": "a.rs", "ok": true, "encoding": "utf-8", "lossy": false,
  "size": 1234, "total_lines": 100, "line_start": 0, "line_end": 99,
  "content": "..." }
```
**错误对照**：`permission_denied`＝出根/受保护；`invalid_params`＝空 paths/是目录；`limit_exceeded`＝>8MB 未切片；`execution_failed`＝不存在/IO。

### Fs_WriteFile — 批量原子写（临时文件+rename）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "files": {
      "type": "array", "minItems": 1,
      "items": {
        "type": "object",
        "properties": {
          "path":    { "type": "string" },
          "content": { "type": "string" },
          "encoding": { "type": "string", "default": "utf-8" },
          "eol":     { "type": "string", "enum": ["auto", "lf", "crlf", "none"], "default": "auto" }
        },
        "required": ["path", "content"], "additionalProperties": false
      }
    }
  },
  "required": ["files"]
}
```
**校验**：写能力开启；目标路径在根内、非受保护、非沙箱根本身；编码后字节 ≤8MB。
**输出**：`{ "path", "ok", "bytes": <写入字节数>, "encoding" }`
**错误对照**：`permission_denied`＝受保护/出根；`limit_exceeded`＝内容>8MB；`execution_failed`＝编码不可表示（如中文→ascii）/IO；`invalid_params`＝缺 path/content。

### Fs_AppendFile — 批量追加（不存在则创建）

输入同 Fs_WriteFile（eol 同样适用）。输出：`{ "path", "ok", "appended_bytes", "encoding" }`。错误对照同 Fs_WriteFile。

### Fs_EditFile — 精确字符串替换（保持原编码）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "files": {
      "type": "array", "minItems": 1,
      "items": {
        "type": "object",
        "properties": {
          "path": { "type": "string" },
          "edits": {
            "type": "array", "minItems": 1,
            "items": {
              "type": "object",
              "properties": {
                "old": { "type": "string", "minLength": 1 },
                "new": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
              },
              "required": ["old", "new"], "additionalProperties": false
            }
          }
        },
        "required": ["path", "edits"], "additionalProperties": false
      }
    }
  },
  "required": ["files"]
}
```
**校验（全部通过才落盘，任一失败则该文件不动）**：`old != new`；`old` 必须存在（0 次→`execution_failed` pattern not found）；命中>1 且未 `replace_all`→`execution_failed`（错误信息含次数，要求补长模式）；文件 ≤8MB 且解码 `lossy=false`。
**输出**：`{ "path", "ok", "replacements": <总替换次数>, "encoding" }`
**错误对照**：`execution_failed`＝not found/歧义/lossy 文件；其余同写。

### Fs_CreateFile — 批量 touch（建父目录，存在则 no-op）

**输入**：`{ "paths": string[] (minItems 1) }`
**输出**：`{ "path", "ok", "created": true|false }`

### Fs_RemoveFile — 删除文件（trash 优先）

**输入**：`{ "paths": string[] (minItems 1) }`
**校验**：目标是文件或符号链接（目录→`invalid_params` 提示用 Fs_RemoveDir）；路径非受保护、非根。
**输出**：
```json
{ "path": "t.log", "ok": true, "deleted": true,
  "mode": "trash",                                  // trash(默认) | permanent
  "trash_path": ".act/trash/20260918-010846-4badbe29/0/t.log" }  // mode=trash 时才有
```
**错误对照**：`permission_denied`＝受保护（.env、密钥等）/根；`invalid_params`＝是目录/空数组。

### Fs_MoveFile / Fs_CopyFile — 批量移动/复制（文件）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "moves": {
      "type": "array", "minItems": 1,
      "items": {
        "type": "object",
        "properties": { "from": { "type": "string" }, "to": { "type": "string" } },
        "required": ["from", "to"], "additionalProperties": false
      }
    },
    "overwrite": { "type": "boolean", "default": false }
  },
  "required": ["moves"]
}
```
**校验**：from/to 都在根内且非受保护；from 存在且是文件；from≠to；to 已存在时需 `overwrite:true`（旧目标先进 trash），否则 `execution_failed` destination exists；跨卷自动降级 copy+delete。
**输出**：`{ "path": <from>, "ok", "to": <to>, "op": "moved" }`（Copy 为 `"copied"`）

### Fs_FileInfo — 批量元信息

**输入**：`{ "paths": string[] }`
**输出**：
```json
{ "path": "x", "ok": true, "abs_path": "D:/…/x", "exists": true,
  "is_file": true, "is_dir": false, "is_symlink": false,
  "size": 5, "modified": "RFC3339|null", "created": "RFC3339|null", "readonly": false }
```

## 3. 目录命令

### Fs_CreateDir — 批量 mkdir -p

**输入**：`{ "paths": string[] }`　**输出**：`{ "path", "ok", "created": true|false }`

### Fs_ListDir — 批量列目录

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "paths":          { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "depth":          { "type": "integer", "minimum": 1, "maximum": 64, "default": 1 },
    "include_hidden": { "type": "boolean", "default": false },
    "max_results":    { "type": "integer", "minimum": 1, "maximum": 500 }
  },
  "required": ["paths"]
}
```
**输出**（批量信封，成功项）：
```json
{ "path": "src", "ok": true,
  "entries": [ { "path": "src/main.rs", "name": "main.rs", "is_dir": false, "size": 1024 } ],
  "count": 1, "truncated": false }
```
`entries[].path` 为「输入路径 + 相对子路径」，可直接回传其他命令。`invalid_params`＝非目录。

### Fs_MoveDir / Fs_CopyDir — 批量移动/复制目录

输入同 Fs_MoveFile（moves+overwrite）。**校验**：from 是目录；**禁止 to 位于 from 内部**（`invalid_params` destination inside source）；拒绝操作沙箱根本身（`permission_denied` guard=SandboxRoot）；非空目标需 overwrite（旧目标进 trash）。
**输出**：同 Fs_MoveFile（`op: "moved"|"copied"`）。

### Fs_RemoveDir — 删除目录（trash 优先）

**输入 Schema**
```json
{ "type": "object",
  "properties": {
    "paths":     { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "recursive": { "type": "boolean", "default": false }
  },
  "required": ["paths"] }
```
**校验**：必须是目录；**非空目录必须 `recursive:true`**（否则 `invalid_params`，错误信息含 "recursive"）；沙箱根本身或包含根的路径一律拒绝（`permission_denied` guard=SandboxRoot）。
**输出**：同 Fs_RemoveFile（`deleted/mode/trash_path`）。

## 4. 搜索命令

### Fs_FindFile — glob 文件查找（gitignore 感知）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "root":              { "type": "string" },
    "patterns":          { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "max_depth":         { "type": "integer", "minimum": 1, "maximum": 64 },
    "include_hidden":    { "type": "boolean", "default": false },
    "respect_gitignore": { "type": "boolean", "default": true },
    "max_results":       { "type": "integer", "minimum": 1, "maximum": 500 }
  },
  "required": ["root", "patterns"]
}
```
**校验**：root 在根内且是目录（否则 `invalid_params`）；patterns 为 globset 语法（`*.rs` 同时匹配文件名与相对路径；多模式 OR）；glob 编译失败→`invalid_params`。
**输出（非批量信封）**：
```json
{ "ok": true, "command": "Fs_FindFile", "root": ".",
  "matches": [ { "path": "src/工具类.rs", "is_dir": false, "size": 12 } ],
  "count": 2, "truncated": false }
```
**错误对照**：`permission_denied`＝root 出根；`invalid_params`＝root 非目录/bad glob；`limit_exceeded`＝max_results>500。

### Fs_GrepFile — regex 内容搜索（按文件编码解码后匹配）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "root":              { "type": "string" },
    "pattern":           { "type": "string", "description": "Rust regex 语法（不支持 lookahead/backreference）" },
    "globs":             { "type": "array", "items": { "type": "string" } },
    "include_hidden":    { "type": "boolean", "default": false },
    "respect_gitignore": { "type": "boolean", "default": true },
    "max_results":       { "type": "integer", "description": "匹配行总数上限，≤500" },
    "context_lines":     { "type": "integer", "minimum": 0, "maximum": 10, "default": 0 }
  },
  "required": ["root", "pattern"]
}
```
**校验**：pattern 必须可编译（失败→`invalid_params` bad regex）；单文件 >8MB 或解码 lossy（二进制样）**静默跳过**；context_lines>10→`limit_exceeded`。
**输出（非批量信封）**：
```json
{ "ok": true, "command": "Fs_GrepFile", "root": ".", "pattern": "TODO",
  "files": [ { "path": "src/a.rs", "encoding": "utf-8", "match_count": 2,
    "matches": [ { "line": 10, "text": "// TODO …", "context": "     9 | …\n>   10 | // TODO …\n    11 | …" } ] } ],
  "file_count": 1, "match_lines": 2, "truncated": false }
```
`context` 仅在 `context_lines>0` 时非 null（`>` 标记命中行）。GBK 文件中的中文可直接命中。

## 5. Web 命令

### Web_Fetch — 并行抓取 + HTML→Markdown

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "urls":       { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "format":     { "type": "string", "enum": ["markdown", "text", "json", "html"], "default": "markdown" },
    "max_bytes":  { "type": "integer", "minimum": 1, "maximum": 2097152 },
    "timeout_ms": { "type": "integer", "minimum": 1, "maximum": 300000 }
  },
  "required": ["urls"]
}
```
**校验**：每个 URL 过 UrlGuard（见 1.1-5，含每跳重定向复检）；`format:json` 时响应体非法 JSON→`execution_failed`。
**输出（批量信封，成功项）**：
```json
{ "url": "https://…", "ok": true, "final_url": "https://…",
  "status": 200, "content_type": "text/html; charset=utf-8",
  "bytes": 12345, "truncated": false, "redirects": ["https://hop1"],
  "content": "# Markdown …" }
```
**错误对照**：`permission_denied`＝私网 IP/坏 scheme/带凭据/域名被拒；`execution_failed`＝HTTP ≥400（错误串含状态码）/超时/DNS；`limit_exceeded`＝max_bytes>2MB/重定向>5。

### Web_Search — 多引擎并行 + RRF 合并去重

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "queries":     { "type": "array", "items": { "type": "string" }, "minItems": 1 },
    "engines":     { "type": "array", "items": { "type": "string", "enum": ["duckduckgo", "bing", "google", "brave", "searxng"] } },
    "max_results": { "type": "integer", "minimum": 1, "maximum": 500, "default": 10 }
  },
  "required": ["queries"]
}
```
**校验**：未配置凭据的引擎（google 需 `SERPAPI_KEY`、brave 需 `BRAVE_API_KEY`、searxng 需 `engines.searxng_url`）不可选；显式点名不可用引擎且无可用→`invalid_params`（信息含 available 列表）。
**输出（非批量信封）**：
```json
{ "ok": true, "command": "Web_Search",
  "queries": [ {
    "query": "rust mcp",
    "results": [ { "rank": 1, "title": "…", "url": "https://…",
                   "snippet": "…", "engines": ["bing", "duckduckgo"], "score": 0.033 } ],
    "count": 10,
    "engines_used": ["bing", "duckduckgo"],
    "warnings": [ { "engine": "bing", "error": "…" } ] } ] }
```
**错误对照**：单引擎失败**不失败整次调用**，进 `warnings`；全部引擎失败时对应 query 的 results 为空（顶层仍 `ok:true`）；`invalid_params`＝空 queries/点名不可用引擎。

### Web_Research — 深度调研（搜索→选优→并行抓取→带引用报告）

**输入 Schema**
```json
{
  "type": "object",
  "properties": {
    "topic":       { "type": "string", "minLength": 1 },
    "max_sources": { "type": "integer", "minimum": 1, "maximum": 20, "default": 8 },
    "max_results": { "type": "integer", "minimum": 1, "default": 15 },
    "engines":     { "type": "array", "items": { "type": "string" } }
  },
  "required": ["topic"]
}
```
**输出（非批量信封）**：
```json
{ "ok": true, "command": "Web_Research", "topic": "…",
  "report": "# Research: …\n## Overview\n…\n## Sources\n### [1] …",
  "sources": [ { "n": 1, "title": "…", "url": "https://…", "final_url": "https://…",
                 "bytes": 45678, "summary": "≤400字符抽取式摘要" } ],
  "failed": [ { "url": "https://…", "error": "…" } ] }
```
**错误对照**：`invalid_params`＝空 topic/无可用引擎；`execution_failed`＝搜索零结果 或 **全部**源抓取失败（部分失败进 `failed` 数组，报告照常生成）。

## 6. CLI 与 MCP

```bash
act exec <Command> --input '<json>'    # 执行；--input - 读 stdin
act verify <Command> --input '<json>'  # 权限预检（不执行）→ {allowed, paths[], urls[]} | {allowed:false, error, code}
act schema                            # 输出机器可读契约（即 schema.json 内容，限额为当前配置生效值）
act list [--json]                      # 命令清单+schema
act package [--out dist]              # 本地打包 skill 分发 zip（SKILL.md+schema.json+mcp.json+本二进制）
act install --project|--user [--force] # 用本机实际路径写 .mcp.json + 补齐 skill 目录（SKILL.md/schema.json/mcp.json）
```

- MCP：`tools/list` 返回 `name/description/inputSchema`（与本文件 Schema 一致）；`tools/call` 失败时 `isError:true`，text 为 `{"code","message"}`。
- CLI 退出码见 1.3；Windows 控制台自动切 UTF-8（CP 65001），输出永远 UTF-8 JSON。
