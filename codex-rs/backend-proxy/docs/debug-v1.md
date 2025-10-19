# codex-backend-proxy 调试与排错指南

本文帮助你用最少时间验证代理可用性、快速定位“Bad Request/Instructions 错误”等常见问题。代理是透明透传：不修改请求/响应负载（包括 SSE）。若看到 4xx/5xx，多半是请求体或模型侧校验导致。

## 快速起步

- 启动（WSL2 bash，建议开启详细日志）

```
cargo run -p codex-backend-proxy -- \
  --http-shutdown \
  --port 3456 \
  --verbose
```

- 健康检查（WSL2 bash）

```
curl -sS \
  http://127.0.0.1:3456/health
```

- 关停（WSL2 bash）

```
curl -sS \
  http://127.0.0.1:3456/shutdown
```

日志说明（开启 `--verbose`）
- 路由映射：`proxy route: Post /v1/responses -> https://.../responses`
- 401 刷新：`upstream 401: attempting token refresh`
- 错误链：`proxy error: {e:#}`（会包含出错环节与目标 URL）

前置要求
- 已用 Codex 登录，写入 `~/.codex/auth.json`（包含 `access_token` 和 `account_id`）。
- 代理会自动注入 `Authorization: Bearer <token>` 与 `ChatGPT-Account-Id: <account_id>`；401 时自动刷新一次。

## Responses API 必要项（常见坑）

- 必须头部：`OpenAI-Beta: responses=experimental`
- 流式头部：`Accept: text/event-stream`
- 必须字段：
  - `model`: `"gpt-5"` 或 `"gpt-5-codex"`（UI 中的 `gpt-5-medium/high` 属预设，实际仍用 `gpt-5`）
  - `instructions`: 使用官方内置系统提示，而不是随意一句话
  - `input`: 列表；用户消息项的 `content[*].type` 应为 `"input_text"`
  - `store`: `false`
  - `stream`: 可选（流式为 `true`）

说明：若请求携带工具等字段，代理会原样透传，不做改写。

## 多行示例（WSL2 bash）

- gpt-5 流式（使用 `core/prompt.md`）

```
instructions=$(jq -Rs . codex-rs/core/prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5' \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

- gpt-5-codex 流式（使用 `core/gpt_5_codex_prompt.md`）

```
instructions=$(jq -Rs . codex-rs/core/gpt_5_codex_prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5-codex' \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

- 非流式对照（去掉 `Accept` 头与 `stream` 字段）：
  - 将上述 `jq` 里对象的 `,stream:true` 删除，`curl` 命令去掉 `-N` 与 `-H 'Accept: text/event-stream'`。

- Chat Completions 对照（不需要 `instructions`/`store`）

```
curl -NsS -X POST http://127.0.0.1:3456/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
        "model":"gpt-5",
        "messages":[{"role":"user","content":"流式测试：你好"}],
        "stream":true
      }'
```

非流式对照：

```
curl -sS -X POST http://127.0.0.1:3456/v1/chat/completions \
  -H 'Content-Type: application/json' \
  -d '{
        "model":"gpt-5",
        "messages":[{"role":"user","content":"非流式测试：你好"}]
      }'
```

## 常见错误与排查

- `{"detail":"Store must be set to false"}`
  - 在请求体顶层添加 `"store": false`。
- `{"detail":"Instructions are required"}` 或 `{"detail":"Instructions are not valid"}`
  - 使用官方内置系统提示：
    - gpt-5：`codex-rs/core/prompt.md`（如无工具，通常还需拼接 `apply-patch` 工具说明；简单连通性测试可仅用 prompt.md）
    - gpt-5-codex：`codex-rs/core/gpt_5_codex_prompt.md`
  - 不要使用任意一句话代替 `instructions`。
- `{"detail":"Bad Request"}`
  - 多半是 JSON 被 shell 破坏或字段结构不对。建议用 `jq -n` 生产 JSON 并 `-d @-` 传给 curl（参考“可复制命令”）。
- `invalid_request_error: input[0].content[0].type = 'text'`
  - 类型应为 `"input_text"`（用户输入），而非 `"text"`。
- 401 Unauthorized
  - 代理会自动刷新一次；若仍失败，重新登录刷新 `auth.json`。
- 网络/TLS/DNS 问题
  - 连通性验证（WSL2 bash）

```
curl -I \
  https://chatgpt.com
```

  - 如失败，检查公司代理/证书策略。

补充：若 `account_id` 缺失，代理会尝试从 `id_token` 解析；若仍失败，请重新登录以写入完整 `auth.json`。

## SSE 使用小贴士

- `curl` 流式输出推荐 `-N` 以禁用缓冲。
- 终端中断用 `Ctrl+C`；非流式避免携带 `Accept: text/event-stream` 与 `"stream": true`。

## 可选参数与进阶

- 运行时开关：
  - `--base-url <URL>`：覆盖默认上游（默认 `https://chatgpt.com/backend-api/codex`）
  - `--codex-home <PATH>`：指定 `~/.codex` 目录位置
  - `--server-info <FILE>`：启动后写入 `{port,pid}` 单行 JSON（用于外部脚本发现端口）
  - `--verbose`：打印路由、401 和错误链信息（不含敏感令牌）
  - `--bind <ADDR>`：监听地址（默认 `127.0.0.1`；若需局域网/镜像网络访问可设为 `0.0.0.0`，注意安全）
- gpt-5 文本冗长度
  - 可在请求中添加 `"text":{"verbosity":"low|medium|high"}`。
- 工具与并行调用
  - `tools`/`tool_choice`/`parallel_tool_calls` 字段原样透传；代理不感知或改写。

### 工具、并行调用与输出控制（进阶示例）

- 携带函数工具（Responses），并允许并行：

```
instructions=$(jq -Rs . codex-rs/core/prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg txt '搜索 readme' \
  '{
    model: "gpt-5",
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    tools: [ {
      type: "function",
      name: "search",
      description: "search files",
      strict: true,
      parameters: { type: "object", properties: { query: { type: "string" } }, required: ["query"] }
    } ],
    tool_choice: "auto",
    parallel_tool_calls: true,
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

- 输出格式控制（提供 JSON Schema 并开启 strict）：

```
instructions=$(jq -Rs . codex-rs/core/prompt.md)
SCHEMA='{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}'
jq -n \
  --argjson instructions "$instructions" \
  --arg schema "$SCHEMA" \
  --arg txt '请简短回答：你好' \
  '{
    model: "gpt-5",
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    text: { format: { type: "json_schema", strict: true, name: "codex_output_schema", schema: ($schema|fromjson) } },
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

## 端口发现与 server-info

- 随机端口 + 输出 server-info：

```
codex-backend-proxy --http-shutdown --server-info /tmp/cbp.json
```

- 提取端口并测试：

```
PORT=$(jq -r .port /tmp/cbp.json)
curl -sS \
  http://127.0.0.1:$PORT/health
```

## 模型选择与“预设”说明（gpt-5 low/medium/high）

代码中区分“模型”与“预设”：

- 模型（用于 `model` 字段）
  - `gpt-5`（通用）
  - `gpt-5-codex`（更适合代码/工具场景）
- 预设（UI 层便捷项）
  - `gpt-5-minimal` / `gpt-5-low` / `gpt-5-medium` / `gpt-5-high`
  - `gpt-5-codex-low` / `gpt-5-codex-medium` / `gpt-5-codex-high`

预设的本质：在仍使用对应“模型”的前提下，设置“推理强度”。在 Responses API 里可用 `reasoning.effort` 来表达（必要时再结合 `text.verbosity` 控制输出详略）。序列化值均为小写。

- 预设 → 请求映射（示例）
  - gpt-5-low: `model: "gpt-5"`，并添加 `"reasoning": {"effort": "low"}`
  - gpt-5-medium: `model: "gpt-5"`，并添加 `"reasoning": {"effort": "medium"}`
  - gpt-5-high: `model: "gpt-5"`，并添加 `"reasoning": {"effort": "high"}`
  - gpt-5-minimal: `model: "gpt-5"`，并添加 `"reasoning": {"effort": "minimal"}`
  - gpt-5-codex-low/medium/high: 同理，将 `model` 设为 `"gpt-5-codex"`，并设置相应 `reasoning.effort`

可选：输出详略（非必须）可通过 `text.verbosity` 调整（`low|medium|high`）。

- 多行示例（gpt-5-medium，流式）

```
instructions=$(jq -Rs . codex-rs/core/prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5' \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    reasoning: { effort: "medium" },
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

- 多行示例（gpt-5-codex-high，流式）

```
instructions=$(jq -Rs . codex-rs/core/gpt_5_codex_prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5-codex' \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    reasoning: { effort: "high" },
    text: { verbosity: "high" },
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

### 预设对照清单与示例

预设名称 → `model` + `reasoning.effort`：

- gpt-5-minimal → model: gpt-5, effort: minimal
- gpt-5-low → model: gpt-5, effort: low
- gpt-5-medium → model: gpt-5, effort: medium
- gpt-5-high → model: gpt-5, effort: high
- gpt-5-codex-low → model: gpt-5-codex, effort: low
- gpt-5-codex-medium → model: gpt-5-codex, effort: medium
- gpt-5-codex-high → model: gpt-5-codex, effort: high

可选：`text.verbosity`（low|medium|high）用于控制输出详略；不设置则省略该字段。

- gpt-5 系列（将 `EFFORT` 换成 minimal/low/medium/high）

```
EFFORT=medium
MSG='流式测试：你好'
instructions=$(jq -Rs . codex-rs/core/prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5' \
  --arg effort "$EFFORT" \
  --arg txt "$MSG" \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    reasoning: { effort: $effort },
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

- gpt-5-codex 系列（将 `EFFORT` 换成 low/medium/high，可选 `VERBOSITY`）

```
EFFORT=high
VERBOSITY=high
MSG='流式测试：你好'
instructions=$(jq -Rs . codex-rs/core/gpt_5_codex_prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model 'gpt-5-codex' \
  --arg effort "$EFFORT" \
  --arg verbosity "$VERBOSITY" \
  --arg txt "$MSG" \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    reasoning: { effort: $effort },
    text: { verbosity: $verbosity },
    store: false,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -H 'OpenAI-Beta: responses=experimental' \
    -H 'Accept: text/event-stream' \
    -d @-
```

## FAQ

- 为什么同样 payload 在 Chat Completions 可以，在 Responses 报错？
  - 两者协议不同，字段/形状要求不同。Responses 对 `instructions`/`store`/`input.content[*].type` 等更严格。
- 代理会不会改 headers/内容？
  - 仅注入鉴权相关头（`Authorization` 与 `ChatGPT-Account-Id`）及修正 `Host`；其余尽可能原样转发，并过滤 hop-by-hop 头。

- Chat Completions 404（Not Found）如何处理？
  - 在 ChatGPT 专有后端（`https://chatgpt.com/backend-api/...`）下，`/chat/completions` 常常不对外提供，导致 404。
  - 已验证：无论基座是否带 `codex`（`.../backend-api/codex/chat/completions` 或 `.../backend-api/chat/completions`），都可能返回 404。
  - 建议：改用 `/v1/responses`（本代理的主要目标，已验证可用）。
  - 如必须使用 Chat Completions：请直接调用 OpenAI 公共 API `https://api.openai.com/v1/chat/completions`（需 API Key）；该路径不在本代理南向范围内，代理不会自动路由到 `api.openai.com`。

---
如需更细日志，将运行命令加 `--verbose` 并把包含 `proxy route:`/`proxy error:` 的行贴给我们，便于进一步定位。

## Windows/WSL2（Mirrored Networking）联调

你的环境为 Windows 11 + WSL2 Mirrored Networking。该模式下 Windows 与 WSL2 共享网络栈：
- 在 WSL2 内监听 `127.0.0.1:<port>` 的服务，可直接从 Windows 侧通过 `http://localhost:<port>` 访问。
- 需要向局域网暴露时，在 WSL2 端使用 `--bind 0.0.0.0` 并在 Windows 防火墙放行端口；其他设备用 Windows 主机的局域网 IP 访问。

- 在 WSL2 内启动代理（建议加 `--verbose`）

```
cargo run -p codex-backend-proxy -- \
  --http-shutdown \
  --port 3456 \
  --verbose
```

  - 观察日志：`listening on 127.0.0.1:3456`

- 在 Windows PowerShell 健康检查

```
curl.exe -sS `
  http://localhost:3456/health
```

  - 预期：`{"status":"ok", ...}`

说明：
- PowerShell 的 `curl` 是 `Invoke-WebRequest` 别名，建议用 `curl.exe`。
- Chat Completions 在 ChatGPT backend 下常为 404（见上文 FAQ）；建议优先验证 `/v1/responses`。
- Mirrored Networking 无需 `.wslconfig` 中的 `localhostForwarding` 配置；若 Windows 无法访问，优先检查防火墙策略。

### PowerShell 5 无卡顿方案（不使用 ConvertTo-Json）

在 Windows PowerShell 5 下，`ConvertTo-Json` 对包含大字符串字段（如较大 `instructions`）会卡死，且ctrl+c不可中止。推荐改为：

1) 在 WSL2 中用 `jq` 先生成 payload.json（写到 C 盘路径），避免 PS 侧序列化。
2) 在 Windows 用 `curl.exe --data-binary @...` 直接发送文件。

- WSL2 生成（流式；SSE 持续输出）

```
instructions=$(jq -Rs . /mnt/c/dev/codex/codex-rs/core/prompt.md)
jq -n \
  --argjson instructions "$instructions" \
  --arg model gpt-5 \
  --arg txt '你好,请给我讲十个笑话' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    store: false,
    stream: true
  }' \
  > /mnt/c/dev/codex/stream_payload.json
```

- Windows PowerShell 发送（流式；可加 `--max-time 20` 控制时长）

```
curl.exe -N -sS `
  -X POST http://localhost:3456/v1/responses `
  -H "Content-Type: application/json" `
  -H "OpenAI-Beta: responses=experimental" `
  -H "Accept: text/event-stream" `
  --data-binary @C:\dev\codex\stream_payload.json
```

- 验证“完全覆盖 instructions”（在官方 prompt 末尾追加一行）

```
instructions=$( \
  { \
    cat /mnt/c/dev/codex/codex-rs/core/prompt.md; \
    printf '\nOVERRIDE TEST: hello-from-wsl'; \
  } | jq -Rs . \
)
jq -n \
  --argjson instructions "$instructions" \
  --arg model gpt-5 \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    instructions: $instructions,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    store: false,
    stream: true
  }' \
  > /mnt/c/dev/codex/override_payload.json
```

```
curl.exe -N -sS `
  -X POST http://localhost:3456/v1/responses `
  -H "Content-Type: application/json" `
  -H "OpenAI-Beta: responses=experimental" `
  -H "Accept: text/event-stream" `
  --data-binary @C:\dev\codex\override_payload.json
```
