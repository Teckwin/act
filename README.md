# Agent Core Tools (`act`)

[![CI](https://github.com/TeckWin/act/actions/workflows/ci.yml/badge.svg)](https://github.com/TeckWin/act/actions/workflows/ci.yml)
[![Release](https://github.com/TeckWin/act/actions/workflows/release.yml/badge.svg)](https://github.com/TeckWin/act/actions/workflows/release.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

Rust 实现的跨平台 Agent 常用工具库。统一内核（CommandManager + PermissionVerifier + CommandExecutor）托管注册全部指令：**16 条 Fs_\* 文件/目录指令 + 3 条 Web_\* 抓取/搜索/调研指令**，通过 CLI 与 MCP stdio 服务对外，替代 Agent 内置文件/搜索工具，根治编码乱码、越界访问与不可审计问题。

```
act exec Fs_ReadFile --input '{"paths":["src/main.rs","docs/设计.md"]}'
act exec Web_Research --input '{"topic":"mcp protocol adoption"}'
act mcp   # 作为 MCP server 运行
```

## 特性

- **编码安全**：BOM/UTF-16/GBK 自动探测与转换，读返回编码标注，编辑保持原编码写回，Windows 控制台强制 UTF-8（CP 65001），输出全 JSON。
- **绝对不越界**：路径多根白名单 + 符号链接逃逸防护 + `..` 穿越拦截；`.git`/`.env`/密钥默认禁改禁删；删除默认进回收站（`.act/trash/`）。
- **SSRF 防护**：仅 http/https、私网/回环/链路本地 IP 全封禁、DNS 解析后校验、重定向逐跳校验。
- **防绕过内核**：所有实现只经 `CommandManager::execute()` 调用；命令名强校验（`<Domain>_<Action>` PascalCase）；每次调用（含拒绝）写审计日志 `.act/audit.jsonl`。
- **批量 + 逐项隔离**：`paths/files/moves/urls/queries` 数组批量操作，运行期错误逐项返回。
- **结果防爆**：超过 32KB 自动溢出到 `.act/overflow/` 并返回摘要（内置抽取式算法，可选 LLM）。
- **多引擎搜索**：DuckDuckGo + Bing 免 key 并行；Google(SERPAPI)/Brave/SearXNG 可选；RRF 合并去重；`Web_Research` 一键产出带引用的调研报告。

## 安装

### 方式一：下载 skill 分发包（推荐）

从 [Releases](https://github.com/TeckWin/act/releases) 下载对应平台的 `agent-core-tools-<ver>-<target>.zip`（自包含 skill 目录，开箱即用）：

| 产物 | 平台 |
|---|---|
| `agent-core-tools-<ver>-x86_64-pc-windows-msvc.zip` | Windows 10/11 x64 |
| `agent-core-tools-<ver>-x86_64-unknown-linux-gnu.zip` | Linux x64 (glibc) |
| `agent-core-tools-<ver>-aarch64-unknown-linux-gnu.zip` | Linux ARM64 (glibc) |
| `agent-core-tools-<ver>-x86_64-apple-darwin.zip` | macOS Intel |
| `agent-core-tools-<ver>-aarch64-apple-darwin.zip` | macOS Apple Silicon |

每个 zip 内为完整 skill 目录（`SKILL.md` 命令契约 + `schema.json` 机器可读契约 + `mcp.json` 配置模板 + `act`/`act.exe` 二进制）：

```bash
# 用户级安装（~/.claude/skills/）
unzip agent-core-tools-*-x86_64-unknown-linux-gnu.zip -d ~/.claude/skills/
cd ~/.claude/skills/agent-core-tools && ./act install --user

# 或项目级安装（<project>/.claude/skills/ + 写入 .mcp.json）
unzip agent-core-tools-*.zip -d <project>/.claude/skills/
cd <project>/.claude/skills/agent-core-tools && ./act install --project
```

Windows（PowerShell）：

```powershell
Expand-Archive agent-core-tools-*-x86_64-pc-windows-msvc.zip -DestinationPath $HOME\.claude\skills\
& $HOME\.claude\skills\agent-core-tools\act.exe install --user
```

> `act install` 以**本机实际路径**写入 `.mcp.json`（自动检测解压位置，不携带构建机路径），并在 skill 目录补齐 `schema.json`/`mcp.json`。手动配置则把包内 `mcp.json` 的 `<SKILL_DIR>` 替换为解压后的绝对路径。
### 方式二：从源码构建

要求 Rust 1.80+（无 OpenSSL 等本地依赖，rustls 纯 Rust TLS）：

```bash
git clone https://github.com/TeckWin/act.git
cd act
cargo build --release
# 二进制: target/release/act(.exe)
```

## 快速开始

```bash
act list                          # 查看全部 19 条指令
act exec Fs_ReadFile --input '{"paths":["README.md"]}'
act exec Fs_WriteFile --input '{"files":[{"path":"cn.txt","content":"中文内容","encoding":"gbk"}]}'
act exec Web_Research --input '{"topic":"mcp protocol adoption"}'
```

### 接入 Metacode / Claude Code

```bash
# 方式一：项目级（写 .mcp.json + .claude/skills/agent-core-tools）
cd your-project
/path/to/act install --project --force

# 方式二：用户级（装 skill 到 ~/.claude/skills，打印 MCP 配置）
/path/to/act install --user
```

`.mcp.json` 生成内容：

```json
{
  "mcpServers": {
    "act": { "command": "/path/to/act", "args": ["mcp"] }
  }
}
```

重启 Metacode 后即可使用 `mcp__act__Fs_ReadFile`、`mcp__act__Web_Search` 等工具替代内置 Read/Grep/Glob/WebFetch/WebSearch。

## 指令速查（19 条）

| 域 | 指令 |
|---|---|
| 文件 | Fs_ReadFile · Fs_WriteFile · Fs_AppendFile · Fs_EditFile · Fs_CreateFile · Fs_RemoveFile · Fs_MoveFile · Fs_CopyFile · Fs_FileInfo |
| 目录 | Fs_CreateDir · Fs_ListDir · Fs_MoveDir · Fs_CopyDir · Fs_RemoveDir |
| 搜索 | Fs_FindFile（glob）· Fs_GrepFile（regex 内容搜索，编码感知） |
| Web | Web_Fetch（并行抓取转 Markdown）· Web_Search（多引擎并行+RRF）· Web_Research（带引用调研报告） |

参数规范：路径/URL 独立成批量字段，便于统一校验与拦截。完整参数表见 `skill/SKILL.md` 或 `act list --json`。

## 配置（可选）

在项目根放 `act.config.json`（示例见仓库根），常用项：

```jsonc
{
  "roots": ["."],                       // 沙箱根（默认 cwd）
  "protected": ["**/.git/**", ".env*"], // 受保护 glob
  "fs": { "delete_mode": "trash", "fallback_encoding": "gbk" },
  "limits": { "max_batch": 256 },
  "url": { "allow_private_ips": false },
  "output": { "max_inline_bytes": 32768 },
  "engines": { "enabled": ["duckduckgo", "bing"] }
}
```

环境变量：`ACT_CONFIG`（配置路径）、`ACT_EXTRA_ROOTS`（追加根）、`ACT_DISABLE_NET`/`ACT_DISABLE_WRITE`（一键禁网/禁写）、`SERPAPI_KEY`、`BRAVE_API_KEY`。

## 工作区结构

```
crates/act-kernel   内核：注册表/五守卫/执行器/审计/输出策略/配置
crates/act-fs       Fs_* 指令 + 编码引擎 + trash + 原子写
crates/act-web      Web_* 指令 + HTTP管线 + 引擎注册表 + 调研管线
crates/act-cli      二进制 act：CLI + MCP stdio 服务端 + install
packaging/skill/    分发包模板源：SKILL.md（命令契约）+ mcp.json（配置模板）
dist/               本地打包产出（act package，gitignored）
docs/DESIGN.md      完整设计文档（中文）
.github/workflows   CI（三平台测试）+ Release（五目标打包发布）
```

## CI / 发布

发布流程（严格遵循）：

1. **commit / PR → CI 验证**：`ci.yml` 在 ubuntu / windows / macos 三平台跑 `cargo test --workspace` + 中文读写冒烟；
2. **CI 全绿后打 tag 发布**：`release.yml` 先由 prepare 任务用内核二进制生成与实现同步的 `schema.json`，再五目标矩阵打包 `agent-core-tools-<ver>-<target>.zip`（SKILL.md+schema.json+mcp.json+二进制）附到 GitHub Release；本地等价命令：`cargo run --bin act -- package --out dist`。

```bash
git commit -am "fix: ..." && git push          # 1. 先看 CI 绿
gh run watch --repo Teckwin/act $(gh run list --repo Teckwin/act -L1 --json databaseId --jq '.[0].databaseId')
git tag v0.1.1 && git push origin v0.1.1     # 2. 再触发发布
```

手动 `workflow_dispatch` 可仅产 artifacts 不发布。

## 测试

```bash
cargo test --workspace   # 103 个测试：安全矩阵/编码/SSRF/协议 e2e
```
