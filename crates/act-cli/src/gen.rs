//! Documentation generators: SKILL.md is compiled from CommandDef metadata
//! (param schema, aliases, examples, output schema) plus the shared error
//! table and live config limits — no hand-written command docs, so the
//! installed skill can never drift from the binary.

use act_kernel::CommandManager;
use serde_json::Value;

const FRONTMATTER: &str = r#"---
name: agent-core-tools
description: Sandboxed, encoding-safe file and web tools for agents. Use when reading/writing/editing/searching files or directories, finding files, grepping content, fetching URLs, searching the web, or doing topic research. Prefer these CLI commands over the built-in Read/Write/Edit/Glob/Grep/WebFetch/WebSearch tools.
---

# Agent Core Tools (act)

统一沙箱内核，CLI 化调用（本 skill 目录自带二进制，无需任何注册）：

```bash
<BIN> <Command> --flags ...        # 主用法（schema 驱动参数）
<BIN> exec <Command> --input '<json>'   # JSON 兜底通道（复杂值/对象数组）
```

> `<BIN>` = 本 skill 目录下的可执行文件：`~/.claude/skills/agent-core-tools/act`（Windows 为 `act.exe`）。
> 完整机器可读契约见同目录 `schema.json`（19 命令 schema + 错误码 + 限额，由本二进制生成）。
"#;

const RULES: &str = r#"
## 使用规则

1. 优先用本命令集完成文件/网页操作，禁止在可用时改用 shell（`cat`/`grep`/`find`/`curl`）。
2. 命令默认**单目标**（单文件/单目录/单 from→to）；仅 `Fs_CreateFile`、`Fs_CreateDir`（批量新建）与 `Web_Fetch`、`Web_Search`（并行抓取/多查询合并）支持批量。
3. 读返回的 `encoding` 字段必须在写回/编辑同一文件时用 `--encoding` 原样传回，保持编码不变。
4. 权限类错误（`permission_denied`）不要重试——修正路径/参数后再调；错误语义见下方错误码表。
5. 单次调用输出超过 32KB 时返回溢出信封 `{truncated:true, summary, full_content_path}`：先读摘要，需要全文再用 `Fs_ReadFile --path <full_content_path>`（overflow/trash 只读放行）。
"#;

const CONVENTIONS_HEAD: &str = r#"
## 全局约定

### 校验管线（错误按此顺序发生）

| 序 | 阶段 | 失败错误码 | 粒度 |
|---|---|---|---|
| 1 | 命令名未注册 | `unknown_command` | 整次调用 |
| 2 | 能力开关（read/write/net） | `capability_disabled` | 整次调用 |
| 3 | 路径沙箱：`..` 穿越/出根/符号链接逃逸/深度>100 | `permission_denied` | 整次调用 |
| 4 | 受保护路径（`.git/**`、`.env*`、`*.pem`、`*.key`、`id_rsa*`、`.act/**`；overflow/trash 只读放行） | `permission_denied` | 整次调用 |
| 5 | URL 策略：非 http(s)/带凭据/私网 IP/DNS 解析到内网/域名黑白名单/重定向>5 跳 | `permission_denied` | 整次调用 |
| 6 | 限额（批量项数/字节/深度/行数等） | `limit_exceeded` | 整次调用 |
| 7 | 参数缺失/类型错/未知 flag/语义非法 | `invalid_params` | 整次调用 |
| 8 | 运行期失败（不存在/HTTP 4xx 5xx/编码不可表示等） | `execution_failed` | 逐项隔离（批量命令） |
| 9 | 超时 | `timeout` | 整次调用 |

安全类失败拒绝整次调用并写审计 `<主根>/.act/audit.jsonl`；批量命令的运行期失败仅影响该项。
"#;

const ENCODING_NOTES: &str = r#"
### 编码与换行

- 读：BOM 探测（utf-8-bom/utf-16le/utf-16be）→ 严格 UTF-8 → 回退编码（默认 `gbk`，配置 `fs.fallback_encoding`）；返回 `encoding` 与 `lossy`。
- 写：`--encoding` ∈ utf-8(默认) | utf-8-bom | utf-16le | utf-16be | gbk …；`--eol` ∈ auto(默认) | lf | crlf | none。
- 编辑按探测编码解码→替换→原编码写回；`lossy=true` 的文件拒绝编辑。
- Windows 控制台自动 UTF-8（CP 65001），输出永远 UTF-8 JSON。
"#;

pub fn skill_markdown(manager: &CommandManager) -> String {
    let mut out = String::with_capacity(24 * 1024);
    out.push_str(FRONTMATTER);
    out.push_str(RULES);
    out.push_str(CONVENTIONS_HEAD);

    // Error table (from the binary's own error contract).
    out.push_str("\n### 错误码对照表\n\n");
    out.push_str(&markdown_table(
        &["code", "含义", "退出码", "MCP", "粒度", "典型触发"],
        crate::schema::error_table()
            .as_array()
            .map(|rows| {
                rows.iter()
                    .map(|r| {
                        vec![
                            format!("`{}`", r["code"].as_str().unwrap_or_default()),
                            r["meaning"].as_str().unwrap_or_default().to_string(),
                            r["exitCode"].to_string(),
                            r["mcp"].as_str().unwrap_or_default().to_string(),
                            r["granularity"].as_str().unwrap_or_default().to_string(),
                            r["guards"]
                                .as_array()
                                .map(|g| {
                                    g.iter()
                                        .filter_map(|x| x.as_str())
                                        .collect::<Vec<_>>()
                                        .join("/")
                                })
                                .unwrap_or_default(),
                        ]
                    })
                    .collect()
            })
            .unwrap_or_default(),
    ));
    out.push('\n');

    // Limits table.
    out.push_str("\n### 默认限额（act.config.json 可改）\n\n");
    let limits = &manager.config().limits;
    let limit_rows = vec![
        vec!["批量项数".into(), format!("≤{}", limits.max_batch)],
        vec![
            "单文件读".into(),
            format!(
                "≤{}B（超过必须 --offset/--limit 切片）",
                limits.max_read_bytes
            ),
        ],
        vec![
            "单文件写/编辑".into(),
            format!("≤{}B", limits.max_write_bytes),
        ],
        vec![
            "单 URL 抓取".into(),
            format!("≤{}B", limits.fetch_max_bytes),
        ],
        vec![
            "Find/Grep 结果".into(),
            format!("≤{}", limits.max_find_results),
        ],
        vec!["目录深度".into(), format!("≤{}", limits.max_depth)],
        vec![
            "命令总超时".into(),
            format!("{}s", limits.command_timeout_secs),
        ],
        vec!["Web 单请求".into(), format!("{}ms", limits.web_timeout_ms)],
        vec![
            "重定向".into(),
            format!("≤{} 跳（逐跳复检）", manager.config().url.max_redirects),
        ],
        vec![
            "内联输出".into(),
            format!(
                "≤{}B，超出溢出+摘要",
                manager.config().output.max_inline_bytes
            ),
        ],
    ];
    out.push_str(&markdown_table(&["项", "默认"], limit_rows));
    out.push('\n');
    out.push_str(ENCODING_NOTES);

    out.push_str("\n## 命令参考（共 ");
    out.push_str(&manager.list().len().to_string());
    out.push_str(" 条）\n");

    for info in manager.list() {
        out.push_str(&command_section(&info));
    }

    out.push_str(CONST_TAIL);
    out
}

fn command_section(info: &act_kernel::CommandInfo) -> String {
    let mut out = String::with_capacity(1200);
    out.push_str(&format!("\n### {}\n\n{}\n\n", info.name, info.description));
    if !info.cmd_aliases.is_empty() {
        out.push_str(&format!(
            "别名：`act {}`（等价 `act {}`）\n\n",
            info.cmd_aliases[0], info.name
        ));
    }

    let example = info.example.as_deref().unwrap_or("<flags>");
    out.push_str(&format!("```bash\nact {} {}\n```\n\n", info.name, example));

    // Parameter table from the input schema.
    let props = info
        .input_schema
        .get("properties")
        .and_then(|v| v.as_object())
        .cloned()
        .unwrap_or_default();
    let required: Vec<String> = info
        .input_schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let alias_of = |prop: &str| -> Vec<String> {
        info.cli_aliases
            .iter()
            .filter(|(_, canonical)| canonical == prop)
            .map(|(alias, _)| {
                if alias.len() == 1 {
                    format!("-{alias}")
                } else {
                    format!("--{}", alias.replace('_', "-"))
                }
            })
            .collect()
    };

    let mut rows: Vec<Vec<String>> = Vec::new();
    for (prop, schema) in &props {
        let canonical_flag = format!("--{}", prop.replace('_', "-"));
        let mut flags = vec![canonical_flag];
        flags.extend(alias_of(prop));
        // boolean negation hint
        if schema.get("type").and_then(|t| t.as_str()) == Some("boolean") {
            flags.push(format!("--no-{}", prop.replace('_', "-")));
        }
        let typ = schema
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string")
            .to_string();
        let enum_vals = schema
            .get("enum")
            .and_then(|e| e.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .unwrap_or_default();
        let typ = if !enum_vals.is_empty() {
            format!("{typ} ({enum_vals})")
        } else {
            typ
        };
        let need = if required.contains(prop) {
            "必填"
        } else {
            "可选"
        }
        .to_string();
        let default = schema
            .get("default")
            .map(|d| d.to_string())
            .unwrap_or_else(|| "-".into());
        let desc = schema
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string();
        rows.push(vec![flags.join(" / "), typ, need, default, desc]);
    }
    out.push_str("**参数**\n\n");
    out.push_str(&markdown_table(
        &["flag", "类型", "必填", "默认", "说明"],
        rows,
    ));
    out.push('\n');

    // Output table from the output schema.
    if let Some(out_props) = info
        .output_schema
        .get("properties")
        .and_then(|v| v.as_object())
    {
        let mut rows: Vec<Vec<String>> = Vec::new();
        for (field, schema) in out_props {
            let typ = match field.as_str() {
                "command" => format!("`{}`", info.name),
                _ => schema
                    .get("type")
                    .and_then(|t| t.as_str())
                    .or_else(|| schema.get("const").and_then(|c| c.as_str()))
                    .unwrap_or("object")
                    .to_string(),
            };
            let desc = schema
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();
            rows.push(vec![format!("`{field}`"), typ, desc]);
        }
        out.push_str("**输出**\n\n");
        out.push_str(&markdown_table(&["字段", "类型", "说明"], rows));
        out.push('\n');
    }
    out
}

fn markdown_table(headers: &[&str], rows: Vec<Vec<String>>) -> String {
    let mut out = String::new();
    out.push('|');
    for h in headers {
        out.push_str(&format!(" {h} |"));
    }
    out.push_str(" |\n|");
    for _ in headers {
        out.push_str(" --- |");
    }
    out.push_str(" |\n");
    for row in rows {
        out.push('|');
        for cell in row {
            out.push_str(&format!(" {} |", cell.replace('|', "\\|")));
        }
        out.push('\n');
    }
    out
}

const CONST_TAIL: &str = r#"
## JSON 兜底通道

复杂值（多键对象数组等）用 exec：

```bash
act exec Fs_EditFile --input '{"path":"a.rs","edits":[{"old":"x","new":"y"}]}'
```

## 可选：MCP 接入

本 skill 纯 CLI 化，无需注册即可用。如需 MCP 工具形态，参考同目录 `mcp.json` 模板把 `act` 以 `mcp` 参数注册为 stdio server。
"#;

/// Validation hook: every command must carry an example that the flag parser
/// accepts and that satisfies required fields (run in tests).
pub fn validate_examples(manager: &CommandManager) -> Vec<String> {
    let mut problems = Vec::new();
    for info in manager.list() {
        let def = manager
            .registry_def(&info.name)
            .expect("listed command must have a def");
        if def.example.is_none() {
            problems.push(format!("{}: missing example", info.name));
            continue;
        }
        if let Err(err) = crate::flags::parse_example(&def, &info) {
            problems.push(format!(
                "{}: example rejected by parser: {}",
                info.name, err
            ));
        }
    }
    problems
}

/// Helper used by schema.rs to keep envelopes consistent.
pub fn batch_commands(manager: &CommandManager) -> Vec<String> {
    manager
        .list()
        .iter()
        .filter(|c| c.output_schema.pointer("/properties/results").is_some())
        .map(|c| c.name.clone())
        .collect()
}

pub fn envelope_summary(manager: &CommandManager) -> Value {
    let batch = batch_commands(manager);
    let all: Vec<String> = manager.list().iter().map(|c| c.name.clone()).collect();
    let flat: Vec<String> = all.iter().filter(|n| !batch.contains(n)).cloned().collect();
    serde_json::json!({
        "flat": { "shape": { "ok": "bool", "command": "string", "...": "专有字段" }, "applies_to": flat },
        "batch": { "shape": { "ok": "bool", "command": "string", "results": "item[]", "summary": { "succeeded": "int", "failed": "int" } }, "applies_to": batch },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_examples_parse_and_every_command_documented() {
        let manager = crate::schema::detached_manager().expect("manager");
        // 1) every example must be accepted by the flag parser
        let problems = validate_examples(&manager);
        assert!(problems.is_empty(), "example drift: {problems:?}");
        // 2) generated SKILL.md must contain every command section and alias
        let skill = skill_markdown(&manager);
        for info in manager.list() {
            assert!(
                skill.contains(&format!("### {}\n", info.name)),
                "missing {}",
                info.name
            );
            if let Some(alias) = info.cmd_aliases.first() {
                assert!(
                    skill.contains(&format!("act {alias}")),
                    "missing alias {alias}"
                );
            }
        }
        assert!(skill.contains("name: agent-core-tools"));
    }

    #[tokio::test]
    async fn output_contract_violation_detected() {
        // A handler that violates its declared done contract must fail with
        // contract_violation (black-box proof of registration-time contracts).
        use act_kernel::error::ActResult;
        use async_trait::async_trait;
        struct Bad;
        #[async_trait]
        impl act_kernel::CommandHandler for Bad {
            async fn execute(
                &self,
                _p: serde_json::Value,
                _c: &act_kernel::SandboxContext,
            ) -> ActResult<serde_json::Value> {
                Ok(serde_json::json!({ "unexpected": true })) // missing required fields
            }
        }
        let cwd = std::env::temp_dir();
        let cfg = act_kernel::ActConfig {
            roots: vec![cwd.clone()],
            ..Default::default()
        };
        let manager = act_kernel::CommandManager::new(cfg).unwrap();
        let def = act_kernel::builder::CommandBuilder::new(
            "Fs_Bad",
            "bad",
            act_kernel::Capability::Read,
            "bad",
        )
        .param(
            act_kernel::builder::Param::string("path")
                .alias("p")
                .required()
                .verify(act_kernel::Verify::PathLike),
        )
        .output_done(serde_json::json!({
            "type": "object", "required": ["ok", "path"],
            "properties": { "ok": { "type": "boolean" }, "path": { "type": "string" } }
        }))
        .bind(std::sync::Arc::new(Bad))
        .unwrap();
        manager.register(def).unwrap();
        let err = manager
            .execute(
                "Fs_Bad",
                serde_json::json!({"path": "x"}),
                act_kernel::InvokeMode::Cli,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code(), "contract_violation");
    }
}
