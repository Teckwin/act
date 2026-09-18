# Agent Core Tools (act) 设计文档

> Rust 实现的跨平台 Agent 常用工具库：统一内核（CommandManager + PermissionVerifier + CommandExecutor）托管注册全部指令，解决 Agent 直接调用系统工具导致的编码错乱、越界访问与不可审计问题。通过 CLI 与 MCP stdio 两种形态对外服务。

## 1. 背景与目标

Agent 使用宿主内置工具（shell + cat/grep/find/curl 等）时的典型问题：

| 问题 | 根因 | act 的解法 |
|---|---|---|
| 中文乱码 | Windows 控制台代码页、GBK/UTF-16 文件误按 UTF-8 读 | 编码引擎（BOM 探测→UTF-8 严格→encoding_rs 回退）+ 控制台 CP 65001 + 全 JSON UTF-8 输出通道 |
| 越界读写 | 工具无沙箱概念 | PathGuard：多根白名单 + 词法归一化 + 逐级 canonicalize（防符号链接逃逸）+ deny-by-default |
| SSRF | 任意 URL 抓取 | UrlGuard：仅 http/https、禁 userinfo、私网/回环/链路本地全封禁、DNS 解析后校验、重定向逐跳校验 |
| 误删误改 | 无保护机制 | ProtectGuard（.git/.env/密钥默认禁写禁删）+ trash 回收站删除 + 原子写 |
| 不可审计 | 调用无痕迹 | AuditLog：每次调用（含拒绝）写 `.act/audit.jsonl` |
| 结果爆炸 | 长内容撑爆上下文 | OutputPolicy：超阈值（默认 32KB）溢出落盘 + 摘要返回 |

## 2. 总体架构

```
入口层   act <Command|alias> --flags …              act Sys_Serve (可选 MCP stdio)
              │                                    │
┌─────────────▼────────────────────────────────────▼──────────────┐
│ Kernel (act-kernel)                                              │
│  CommandManager ── 唯一入口，任何 handler 只能经此调用            │
│   ├─ CommandRegistry   命名强校验 <Domain>_<Action> PascalCase  │
│   ├─ PermissionVerifier 五守卫管线（执行前）                    │
│   │    1 CapabilityGuard  按 cli/mcp 模式开关 read/write/net     │
│   │    2 PathGuard        路径沙箱（多根/链接/UNC/大小写）        │
│   │    3 ProtectGuard     受保护 glob（.git/** .env* *.pem …）   │
│   │    4 UrlGuard         域名黑白名单 + 私网封禁(SSRF)          │
│   │    5 LimitGuard       批量上限/字节上限/深度/超时参数         │
│   ├─ CommandExecutor    超时包裹执行                             │
│   ├─ OutputPolicy       溢出落盘 + 摘要（extractive/LLM）        │
│   └─ AuditLog           JSONL 全量审计                           │
├──────────────────────────────────────────────────────────────────┤
│ Extensions（只能 register() 注册进内核，无旁路）                  │
│  act-fs : 16 条 Fs_* 指令（读/写/追加/编辑/建/删/移动/复制/       │
│           列目录/建目录/移动目录/复制目录/删目录/找文件/搜内容/    │
│           元信息）                                               │
│  act-web: 3 条 Web_* 指令（Fetch/Search/Research）               │
└──────────────────────────────────────────────────────────────────┘
```

### 2.1 防绕过设计（kernel + extension 模式）

1. **可见性隔离**：`act-fs`/`act-web` 的实现细节不对外公开，handler 通过 `CommandDef` 注册；Rust 可见性在编译期保证没有旁路调用点。
2. **Extension 契约**：handler 签名为 `execute(params, &SandboxContext)`，只拿到沙箱上下文（`resolve_path`/`check_url`），拿不到绕过守卫的原始能力。
3. **命名强校验**：`CommandName::parse` 用 `^[A-Z][A-Za-z0-9]{1,20}_[A-Z][A-Za-z0-9]{1,30}$` 强校验；未注册域（除内置 `Fs`/`Web`）注册即报错，MCP/CLI 面上不可能出现野生命令。
4. **唯一执行路径**：MCP `tools/call` 与 CLI `exec` 都只调 `CommandManager::execute`，先过五守卫再执行；守卫失败整次调用拒绝并落审计。

## 3. 参数规范（路径/URL 单独成字段，支持批量）

每条命令在 `CommandDef` 中声明 `path_fields` / `url_fields`（JSON Pointer 通配模式），校验器按声明提取后**批量校验**，内容与路径永不混杂：

```jsonc
{ "paths":  ["a.rs", "docs/b.md"] }                        // Fs_ReadFile
{ "files":  [{ "path": "a.txt", "content": "…" }] }        // Fs_WriteFile
{ "moves":  [{ "from": "d1", "to": "d2" }] }               // Fs_MoveDir
{ "urls":   ["https://…"] }                                // Web_Fetch
{ "queries":["rust mcp"] }                                 // Web_Search
```

声明示例：`Fs_MoveDir.path_fields = ["/moves/*/from", "/moves/*/to"]`。

## 4. 安全守卫细则

### 4.1 PathGuard（路径沙箱）

解析流程（每个进入内核的路径）：

1. 预处理：去空白、拒 NUL；
2. **词法归一化**（不碰文件系统）：消解 `.`/`..`，Windows 处理盘符前缀；
3. **词法包含检查**：归一化路径必须以组件前缀方式落在某个根内（挡住 `..` 穿越，且发生在任何 IO 之前）；
4. **逐级 canonicalize**：存在组件逐个探测；遇到符号链接则 canonicalize 该点并**重新做包含检查**（挡符号链接逃逸），从 canonical 位置继续拼接；不存在的前缀允许（写场景），但剩余组件必须全是 Normal；
5. Windows 适配：剥 `\\?\` / `\\?\UNC\` 前缀、大小写不敏感比较、`C:\a\..\b` 归一化正确。

附加规则：删除/移动拒绝以根为对象或包含根（`ensure_not_root`）；路径深度上限 100。

### 4.2 UrlGuard（SSRF 防护）

- scheme 白名单（默认 http/https）；禁 userinfo；
- 字面 IP：回环/私网/链路本地/组播/保留段全拒（IPv4 + IPv6 + v4-mapped）；
- 域名：先过黑白名单（后缀匹配、punycode 归一），再 **DNS 解析逐 IP 校验**（防域名解析到内网；测试可注入 StubResolver）；
- 重定向：手动跟随，**每一跳重新过 UrlGuard**，上限默认 5 跳；
- `allow_private_ips=true` 仅建议本地测试开启。

### 4.3 ProtectGuard / LimitGuard / CapabilityGuard

- 保护 glob（相对根，`/` 分隔）：默认 `.git/**`、`.env*`、`**/*.pem`、`**/*.key`、`id_rsa*`、`.act/**` 等；写/删一律拒，读默认拒（`allow_read_protected` 可放开）；
- LimitGuard：单批 ≤256 项、读 ≤8MB、写 ≤8MB、fetch ≤2MB、find/grep ≤500 条、depth ≤64、context_lines ≤10 等；命令级参数不能超过配置上限；
- CapabilityGuard：`capabilities.{cli,mcp}.{read,write,net}` 按调用模式禁用整类能力（如 `ACT_DISABLE_NET=1`）。

### 4.4 危险操作降级

- `delete_mode: "trash"`（默认）：删除进 `<主根>/.act/trash/<时间戳>-<id>/<rootIdx>/<原相对路径>`，保留 7 天自动清理；
- 递归删除目录必须显式 `recursive: true`；
- 写文件原子化：同目录临时文件 + rename（Windows 覆盖场景先移走旧文件入 trash）。

## 5. 编码引擎

```
读：BOM 探测(utf-8-bom/utf-16le/utf-16be) → 严格 UTF-8 → encoding_rs 回退(默认 gbk)
    返回 { content, encoding, lossy }，lossy=true 表示不可逆内容
写：默认 UTF-8 无 BOM；可选 utf-8-bom / utf-16le / utf-16be / gbk / …
    EOL 归一化：auto（保持原文件主流风格）| lf | crlf | none
编辑：按探测编码解码 → 替换 → 按原编码写回（GBK 文件编辑后仍是 GBK）
搜索：Fs_GrepFile 逐文件解码后匹配（GBK 中文内容可命中）
输出：CLI/MCP 输出统一 UTF-8 JSON；Windows 启动即设 ConsoleCP 65001
```

## 6. Web 能力

- **Web_Fetch**：并行抓取（并发 ≤8）、逐条隔离失败；HTML→Markdown（htmd，预清洗 script/style/nav/header/footer/aside）；format 支持 markdown/text/json/html；按 Content-Type charset 解码。
- **Web_Search**：引擎注册表 + 并行执行 + **RRF（Reciprocal Rank Fusion）合并去重**（www/尾斜杠/http-https 归一）。
  - 内置免 key：DuckDuckGo（html 端点解析）、Bing（结果页解析）
  - 可选：Google（`SERPAPI_KEY`）、Brave（`BRAVE_API_KEY`）、SearXNG（`engines.searxng_url`）
  - 引擎失败降级：单引擎错误进 `warnings`，不影响其他引擎
- **Web_Research**：topic → 多引擎搜索 → 取前 N 源 → 并行抓取 → 每源抽取式摘要（400 字）→ 综述（800 字）→ 带编号引用的 Markdown 报告。纯启发式，不依赖 LLM。

## 7. 结果溢出 + 摘要压缩（OutputPolicy）

```
result 序列化 > output.max_inline_bytes(默认 32KB)？
├─ 否 → 原样返回
├─ 是 且 compression.enabled=false → 硬截断 + preview
└─ 是 且 compression.enabled=true
   ├─ 全文写 <主根>/.act/overflow/<Command>/<时间戳>-<hash>.json（沙箱内）
   ├─ 摘要：extractive（分句→CJK bigram+词频打分→保序 top-k，零依赖，默认）
   │        或 llm（OpenAI 兼容 /chat/completions；失败自动降级 extractive）
   └─ 返回 { truncated:true, summary, full_content_path, original_bytes }
```

溢出文件保留 7 天自动清理。

## 8. 审计

`.act/audit.jsonl` 每行：`{ts, mode(cli|mcp), command, paths[], urls[], allowed, error_code, error, duration_ms}`。放行与拒绝都记录。

## 9. CLI / MCP 接口

```
act <Command|alias> --flags …      # 唯一入口（parser 归一化 → kernel.exec）
act list [--json]                    # 命令清单
act Sys_Verify -t <Cmd> --<目标flags>   # 权限预检（透传目标命令 flags，不执行）
act mcp                              # MCP stdio 服务端
act install --project [--user] [--exe <path>] [--force]
```

退出码：0 成功 / 2 权限 / 3 未知命令 / 4 参数 / 5 执行 / 6 超时 / 7 配置。

MCP（JSON-RPC 2.0，行分隔 stdio）：`initialize`（协议版本回显 2024-11-05/2025-03-26/2025-06-18）、`ping`、`tools/list`、`tools/call`（错误返回 `isError:true` + 结构化 code/message）。stdout 只走协议，日志全部 stderr。

## 10. 配置（`act.config.json`）

优先级：内置默认 ← 配置文件（cwd 或 `ACT_CONFIG`）← 环境变量（`ACT_EXTRA_ROOTS`、`ACT_DISABLE_NET`、`ACT_DISABLE_WRITE`）。示例见仓库根 `act.config.json`。

## 11. Crate 清单与验收对照

| Crate | 范围 | 验收（测试） |
|---|---|---|
| act-kernel | 注册/守卫/执行/审计/输出策略/配置 | 52 单测：逃逸矩阵、SSRF、命名、deny-default、溢出 |
| act-fs | 16 条 Fs_* + 编码引擎 + trash + 原子写 | 28 测试：安全矩阵、编码 roundtrip、trash、中文 fixture |
| act-web | 3 条 Web_* + 引擎注册表 | 16 测试：本地服务器 fixture、引擎解析、research 端到端 |
| act-cli | CLI + MCP 服务端 + install | 6 e2e：CLI 读写/退出码、MCP 协议往返、install |

## 12. 向 weave 的借鉴

| 借鉴点 | 来源 | 改进 |
|---|---|---|
| Registry + Aspect 管线 | weave-core `tool/` | 原生 async；守卫拆成五个独立 Guard；命名强校验 |
| 路径沙箱 canonicalize+containment | weave-infra `sandbox/fs.rs` | 修复 Windows `\\?\`/大小写；逐级 canonicalize 防链接逃逸；多根支持 |
| EnvSanitizer / PolicyEngine 思想 | weave-infra `sandbox/` | act 零 shell 执行，无需环境清洗 |
