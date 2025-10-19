# codex-backend-proxy 编译、调试与排错（v2）

本文配合 design-v2.md，提供可复制的构建、运行、联调与排错步骤，覆盖 Responses 路由、SSE、指令规避、参数过滤与结构化日志等关键点。

## 0. 前置准备

- 已使用 Codex 登录并写入 `~/.codex/auth.json`（包含 `access_token` 与 `account_id`）。
- 工具建议：`curl`、`jq`（用于构造 JSON 与解析日志）。

## 1. 编译

- Release 构建（推荐）：

```
cargo build -p codex-backend-proxy --release
```

- Debug 运行（开发态）：

```
cargo run -p codex-backend-proxy -- \
  --http-shutdown \
  --port 3456 \
  --bind 0.0.0.0 \
  --verbose
```

说明：Debug 模式便于快速迭代；线上或基准测试建议使用 Release 二进制。

## 2. 启动（Release 二进制）

```
./target/release/codex-backend-proxy \
  --http-shutdown \
  --port 3456 \
  --bind 0.0.0.0 \
  --verbose
```

- 启动日志示例：
  - `codex-backend-proxy listening on 0.0.0.0:3456; base_url=https://chatgpt.com/backend-api/codex`
  - `proxy route: Post /v1/responses -> https://chatgpt.com/backend-api/codex/responses`

安全提示：`--bind 0.0.0.0` 将对局域网暴露端口，调试完成后建议改回默认 `127.0.0.1`。

## 3. 健康检查与关停

- 健康检查：

```
curl -sS http://127.0.0.1:3456/health
```

- 关停（仅在 `--http-shutdown` 下有效）：

```
curl -sS http://127.0.0.1:3456/shutdown
```

## 4. 最小联调用例（Responses）

- 流式（SSE）用例：

```
jq -n \
  --arg model 'gpt-5' \
  --arg txt '流式测试：你好' \
  '{
    model: $model,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -d @-
```

期望：终端打印 `event:`/`data:` SSE 事件；代理日志含 `{"type":"sse_start"}`。

- 非流式对照：去掉 `,"stream":true`，且 `curl` 去掉 `-N` 选项；返回单个 JSON。

## 5. 指令校验规避与参数过滤验证

- 构造一个“带自定义 instructions + 不被 Codex 接受参数”的请求，观察代理改写：

```
jq -n \
  --arg model 'gpt-5' \
  --arg sys 'You are DeepChat, a highly capable AI assistant.' \
  --arg txt '介绍下 macross 动画' \
  '{
    model: $model,
    instructions: $sys,
    input: [ { role: "user", content: [ { type: "input_text", text: $txt } ] } ],
    max_output_tokens: 128000,
    stream: true
  }' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -d @-
```

在代理日志中检查：
- `upstream_request` 体预览：
  - 不再包含 `max_output_tokens`；
  - `instructions` 已替换为官方内置文本；
  - `input[0]` 为 `role=user` 且 content 首元素为 IGNORE 前缀的 `input_text`，第二元素为原 system 文本；
  - `include` 自动补齐包含 `"reasoning.encrypted_content"`；`store` 自动补 `false`；
  - `Accept: text/event-stream` 已设置（由于 `stream: true`）。

## 6. 结构化日志与换行检查

- 关键日志类型：
  - `inbound_request`：入站方法/URL/头与体预览（JSON Pretty，多行保留换行）。
  - `upstream_request`：发往上游的 URL/头与体预览（可见代理改写效果）。
  - `sse_start`：检测到上游 `text/event-stream` 并开始透传。
  - `upstream_response`：上游状态码/头与体预览。

- 脱敏/截断策略：
  - 头值最长 200 字符；体预览最长 4000 字符，超出以 `… (truncated)` 标记。
  - `authorization` 永远 `<redacted>`；`chatgpt-account-id` 仅保留尾 4 位。

## 7. 常见错误与排错步骤

- 400 `Unsupported parameter: ...`
  - 确认入站 JSON 可被解析（否则无法改写，将原样转发）。
  - 确认出站体预览已删除 `max_output_tokens/temperature/top_p/...` 等；若仍存在，检查日志中的 `inbound body (preview)` 是否为合法 JSON。

- 400 `Instructions are not valid`
  - 出站体中 `instructions` 是否为官方文本；`input[0]` 是否为 IGNORE 前缀的 user 消息。
  - 确认未自行添加 `Originator/Session-Id` 等可能触发更严校验的头。

- 401 Unauthorized
  - 日志应出现 `upstream 401: attempting token refresh` 与 retry 的状态行；若仍失败，使用官方登录流程刷新 `~/.codex/auth.json`。
  - 检查 `ChatGPT-Account-Id` 是否存在（日志头中可见尾 4 位）。

- 非预期一次性返回（预期流式）
  - 入站是否 `stream:true`；出站头是否包含 `Accept: text/event-stream`；上游响应 `content-type` 是否 SSE。

- 日志“挤在一行/无换行”
  - 注意日志包含两段：一行结构化 JSON + 多行 body 预览段；在支持换行的终端查看，或用 `less -SR`。

## 8. 端口与 server-info（可选）

- 随机端口 + 导出 server-info：

```
./target/release/codex-backend-proxy \
  --http-shutdown \
  --server-info /tmp/cbp.json \
  --verbose
```

- 读取端口并探测：

```
PORT=$(jq -r .port /tmp/cbp.json)
curl -sS http://127.0.0.1:$PORT/health
```

## 9. 进阶：工具与输出控制（透传验证）

- 工具/并行调用字段原样透传（代理不改写）：

```
jq -n '{
  model: "gpt-5",
  input: [ { role: "user", content: [ { type: "input_text", text: "测试 tools 透传" } ] } ],
  tools: [ { type:"function", name:"search", strict:true, parameters:{type:"object",properties:{q:{type:"string"}},required:["q"]} } ],
  tool_choice: "auto",
  parallel_tool_calls: true,
  store: false,
  stream: true
}' \
| curl -NsS -X POST http://127.0.0.1:3456/v1/responses \
    -H 'Content-Type: application/json' \
    -d @-
```

检查 `upstream_request` 日志体预览，确认相关字段未被代理改写。

## 10. 参数速查（运行时开关）

- `--port <u16>`：监听端口；未设置时使用随机可用端口。
- `--bind <ADDR>`：监听地址，默认 `127.0.0.1`；调试跨设备可用 `0.0.0.0`。
- `--base-url <URL>`：覆盖上游基座（默认 `https://chatgpt.com/backend-api/codex`）。
- `--codex-home <PATH>`：指定 `~/.codex` 目录位置。
- `--server-info <FILE>`：启动成功后输出 `{port,pid}` 单行 JSON。
- `--http-shutdown`：开启 `GET /shutdown`。
- `--verbose`：打印路由、结构化请求/响应日志（含脱敏与截断）。

---

提示：若需要严格对齐官方校验（启用 `Originator/Session-Id` 等），请先确保工具 schema 与官方完全一致，再行调整代理强制头策略（见 design-v2.md“与参考文档的差异与取舍”）。
