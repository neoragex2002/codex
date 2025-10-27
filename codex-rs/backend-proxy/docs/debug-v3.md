# codex-backend-proxy 编译、调试与排错（v3）

本文档在 v2 的基础上，加入 API Key 模式的操作步骤，并统一说明两种模式（OAuth 与 API Key）下的请求体规范化、SSE 与日志排查方法。

## 0. 前置准备

- 选择一种鉴权模式：
  - API Key 模式：`~/.codex/auth.json` 写入 `{"OPENAI_API_KEY":"sk-..."}`。
  - OAuth 模式：`~/.codex/auth.json` 含 `tokens.id_token/access_token/refresh_token`（可从 `id_token` 解析出 `chatgpt_account_id`）。
- 可选：在 `~/.codex/config.toml` 配置 Responses provider：

```toml
model_provider = "tabcode"

[model_providers.tabcode]
name = "openai"
base_url = "https://api.tabcode.cc/openai"
wire_api = "responses"
```

- 工具建议：`curl`、`jq`。

## 1. 编译与启动

- Debug 启动（推荐用于开发）：

```
cargo run -p codex-backend-proxy -- \
  --http-shutdown \
  --port 3456 \
  --bind 127.0.0.1 \
  --verbose
```

- 期望：
  - 若为 OAuth：`base_url=https://chatgpt.com/backend-api/codex`
  - 若为 API Key 且未配置 provider：`base_url=https://api.openai.com/v1`
  - 若配置了 provider：`base_url=<provider.base_url>`

## 2. 健康检查与关停

```
curl -sS http://127.0.0.1:3456/health
curl -sS http://127.0.0.1:3456/shutdown   # 仅 --http-shutdown 生效
```

## 3. 最小联调（非流式）

- 发送最小请求（代理会自动规范化请求体）：

```
jq -n '{
  model:"gpt-5",
  input:[{ type:"message", role:"user", content:[{ type:"input_text", text:"你好" }] }],
  stream:false
}' | \
curl -sS -X POST http://127.0.0.1:3456/v1/responses \
  -H 'Content-Type: application/json' \
  -d @-
```

- 期望：返回单个 JSON。verbose 日志中的 upstream_request 体预览看到：
  - `instructions` 为官方内置文本；
  - 如提供了 `instructions/system`，会在 `input` 首位注入一条带 IGNORE 前缀的 `role=user` 消息；
  - `store=false`、`include=["reasoning.encrypted_content"]`；
  - 删除了不支持字段（如果你传了）。

## 4. 流式联调（SSE）

```
jq -n '{
  model:"gpt-5",
  input:[{ type:"message", role:"user", content:[{ type:"input_text", text:"流式测试" }] }],
  stream:true
}' | \
curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
  -H 'Content-Type: application/json' \
  -d @-
```

- 期望：终端看到逐条的 `event:`/`data:`；日志含 `{"type":"sse_start"}` 与上游 `content-type: text/event-stream`。

## 5. OAuth 模式专项验证

- 401 刷新：当 access_token 过期时，日志会打印 `upstream 401: attempting token refresh`，随后进行一次重试；若失败，请重新登录以刷新 `~/.codex/auth.json`。
- 请求头：日志可见 `ChatGPT-Account-Id`（尾 4 位）与 `OpenAI-Beta`、`Accept`（当 `stream=true`）。

## 6. API Key 模式专项验证

- `~/.codex/auth.json` 仅需 `OPENAI_API_KEY` 字段。
- 不会发送 `ChatGPT-Account-Id`，也不会进行 401 刷新重试。
- 若使用自定义 provider，观察 upstream URL 为 `base_url + /responses`。

## 7. 常见错误与排查

- 400 `Instructions are required`：
  - 确认 inbound body 为合法 JSON；代理需要能解析后执行规范化；
  - 查看 upstream_request 体预览是否存在 `instructions` 与首条带 IGNORE 的 user 消息。
- 400 `Unsupported parameter: ...`：
  - 规范化应已删除 `max_output_tokens/temperature/top_p/...` 等不支持字段；若仍报错，检查入站 JSON 是否正确被解析。
- 401 Unauthorized（API Key）：
  - 检查 `OPENAI_API_KEY` 是否有效；API Key 模式不会自动刷新；
  - 观察日志是否存在 `upstream status: 401` 且无 refresh 文案。
- 无法流式：
  - 入站 `stream` 是否为 `true`；上游响应是否为 `text/event-stream`；日志是否打印 `sse_start`。

## 8. 端点优先级与覆盖

- 端点优先级：`--base-url` > `config.toml` provider.base_url（wire_api=responses）> 按鉴权模式默认。
- 可覆盖到代理启动行：

```
cargo run -p codex-backend-proxy -- \
  --http-shutdown \
  --port 3456 \
  --bind 127.0.0.1 \
  --base-url https://api.tabcode.cc/openai \
  --verbose
```

## 9. 日志说明（脱敏/截断）

- `authorization` 永远 `<redacted>`，`chatgpt-account-id` 仅保留尾 4 位。
- 大字段会被截断并标注 `… (truncated)`；SSE 的 `data:` JSON 会做 per-field 截断后 pretty 打印。

---

提示：若未来需要对接 Azure Responses，可在实现层面识别 Azure 端点，并采用 `store=true` 与 input item id 附加的特定策略（参考官方客户端）。

