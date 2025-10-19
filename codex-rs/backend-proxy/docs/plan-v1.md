# codex-backend-proxy 实施计划（详案）

本计划用于在 `codex-rs` 工作区新增 `codex-backend-proxy` 代理服务，北向提供 OpenAI 兼容的 `/v1/responses` 与 `/v1/chat/completions` 两个端点，南向固定对接 ChatGPT 专有后端（`/backend-api/codex`），全程对 MCP 工具语义进行无状态、透明化透传。该计划确保不侵入或修改现有 Codex 代码，仅以“库”的方式复用其公开 API。

## 0. 目标与约束

- 北向：仅允许以下路由（无查询串）：
  - `POST /v1/responses`
  - `POST /v1/chat/completions`
  - `GET  /health`（健康检查）
- 南向：固定指向官方 ChatGPT 后端基座（默认）：
  - `https://chatgpt.com/backend-api/codex`
  - 可选回退域名：`https://chat.openai.com/backend-api/codex`
  - 通过 `--base-url` 可覆盖（默认上述基座）。
- 鉴权：仅南向鉴权（北向不做 OAuth 校验）。复用 `~/.codex/auth.json` 中的令牌与账号信息：
  - `Authorization: Bearer <access_token>`
  - `ChatGPT-Account-Id: <account_id>`（强制要求）
- 工具透传：不解析、不改写 `tools/toolConfig/functionResponse` 等字段，响应中 `functionCall`/文本等以语义透传方式原样转发。
- 并发模型：`tiny_http + reqwest::blocking + 每请求新线程`，支持少量并发与 SSE 长连接。
- 不改动现有 Codex 源代码：仅以库方式使用（如 `codex_core::auth`）。

## 1. 目录结构与工程

在 `codex-rs/` 下新增目录 `backend-proxy/`，形成独立二进制 crate：

- `codex-rs/backend-proxy/Cargo.toml`
  - `package.name = "codex-backend-proxy"`
  - `lib.name = "codex_backend_proxy"`
  - `[[bin]] name = "codex-backend-proxy"`
  - 依赖：
    - `tiny_http`（入站 HTTP 服务器）
    - `reqwest`（blocking 特性，用于上游请求，`.timeout(None)` 支持 SSE）
    - `clap`（CLI 参数解析）
    - `serde`、`serde_json`（轻量 JSON 处理）
    - `anyhow`（错误包装）
    - `codex-core`（复用 `auth` 能力）
- `codex-rs/backend-proxy/src/`
  - `main.rs`：解析 CLI，调用 `lib::run_main(args)`
  - `lib.rs`：实现 `run_main`，组织服务器启动、server-info 输出
  - `args.rs`：CLI 参数定义
  - `server.rs`：监听、接入循环、路由分发（/health、/shutdown、两条代理路由）
  - `router.rs`：路径到上游 URL 的严格映射
  - `auth_loader.rs`：加载 `auth.json`、获取 `access_token` 与 `account_id`、按需刷新
  - `headers.rs`：请求/响应头的构造与过滤策略
  - `proxy.rs`：请求体读取、上游请求发送、SSE 流式回传
  - `refresh.rs`：401 场景按需刷新（仅重试一次）
  - `utils.rs`：`server-info` 写入等小工具

工作区集成：在 `codex-rs/Cargo.toml` 的 `[workspace]` 中追加成员 `backend-proxy`（仅改工作区配置，不改动任何已有 crate 源码）。

## 2. CLI 设计（args.rs）

- `--port <u16>`：监听端口（可选，默认 0，表示随机可用端口）
- `--server-info <FILE>`：启动成功后写入单行 JSON `{ "port": <u16>, "pid": <u32> }`
- `--http-shutdown`：启用 `GET /shutdown`（返回 200 后立即进程退出）
- `--codex-home <PATH>`：覆盖默认 `~/.codex`，用于读取 `auth.json`
- `--base-url <URL>`：覆盖南向基座（默认 `https://chatgpt.com/backend-api/codex`）

示例：
- 默认：`codex-backend-proxy --http-shutdown --server-info /tmp/codex-backend-proxy.json`
- 指定基座：`codex-backend-proxy --base-url https://chat.openai.com/backend-api/codex`

## 3. 服务器与路由（server.rs, router.rs）

- 仅绑定 `127.0.0.1:<port>`（降低误暴露风险）
- 路由白名单（无查询串）：
  - `GET  /health`：返回 `200`，`{"status":"ok","version":"<semver or git>"}`
  - `GET  /shutdown`：当 `--http-shutdown` 开启时返回 `200` 并退出
  - `POST /v1/responses`：代理到 `{base-url}/responses`
  - `POST /v1/chat/completions`：代理到 `{base-url}/chat/completions`
- 其他路径：直接 `403 Forbidden`
- 服务器主循环：对每个请求 `spawn` 新线程处理（少量并发可行，SSE 将占用线程周期）

## 4. 鉴权加载与按需刷新（auth_loader.rs, refresh.rs）

- 加载 `auth.json`：
  - 使用 `codex_core::auth::from_codex_home(codex_home)` 获取 `Option<CodexAuth>`
  - 获取 `access_token`：`auth.get_token().await`（ChatGPT 模式返回 access_token）
  - 获取 `account_id`：`auth.get_account_id()`（`Option<String>`）
- `ChatGPT-Account-Id` 强制要求：
  - 若 `get_account_id()` 为 `None`，尝试从 `id_token` claims 派生：
    1. `auth.get_token_data().await?` 获取 `TokenData`
    2. 读取 `TokenData.id_token.raw_jwt`，解析其 `payload`（Base64URL，无 padding），反序列化 JSON
    3. 从 `"https://api.openai.com/auth"` 对象中读取 `"chatgpt_account_id"`
  - 若仍无，则返回 500，提示“请使用官方 codex 登录以刷新 auth.json，确保 account_id 写入”
- 按需刷新（401 时）：
  - 第一次上游响应为 `401 Unauthorized`：调用 `auth.refresh_token().await` 刷新 access_token（该调用会持久化更新 `auth.json`）
  - 更新 `Authorization` 头后，重试原请求一次
  - 若仍失败（401 或其他错误）：不再重试，原样透传上游响应

说明：按需刷新仅在“收到 401”时触发，不做定时刷新或预检查，保持实现简洁与非侵入性。

## 5. 头部策略（headers.rs）

- 入站请求 → 上游：
  - 丢弃：`authorization`、`host` 以及所有 hop-by-hop 头：
    - `connection`、`keep-alive`、`proxy-authenticate`、`proxy-authorization`、`te`、`trailer`、`transfer-encoding`、`upgrade`
  - 保留：其余可解析头（如 `accept: text/event-stream` 等）
  - 注入：
    - `Authorization: Bearer <access_token>`
    - `ChatGPT-Account-Id: <account_id>`（必需）
    - `Host: <上游域名>`（如 `chatgpt.com`）
- 上游响应 → 出站：
  - 复制状态码
  - 过滤 `tiny_http` 自管头与 hop-by-hop 头（同上）
  - 语义透传一致性：若 `reqwest` 自动解压（默认可能会），需保证不回传失真的 `content-encoding`
  - Body 直接以 `reqwest::blocking::Response` 作为 `tiny_http::Response` 的 body（流式）

安全：严禁日志记录任何令牌或敏感头；报错信息不包含令牌。

## 6. 代理与流式（proxy.rs）

- 读取请求体：一次性读入 `Vec<u8>`（默认不设上限；如后续需要可新增 `--max-body-bytes`）
- 上游请求：`reqwest::blocking::Client`
  - `timeout(None)` 支持 SSE 长连接
  - 发送时附带构造好的头与 body
- 响应构造：
  - 从上游响应复制状态与过滤后的头
  - `content_length`：若上游长度可安全转换为 `usize`，则设置；否则为 `None`
  - 将上游响应对象作为 body（实现 `Read`），流式写回客户端

错误处理：网络错误或上游异常均以 `tiny_http::Response` 合理返回；401 触发一次按需刷新后重试。

## 7. 健康检查与关停（server.rs, utils.rs）

- `/health`：`200 OK`，JSON：`{"status":"ok","version":"<semver or git>"}`
- `/shutdown`：当 `--http-shutdown` 开启时，返回 `200 OK` 并 `process::exit(0)`
- `--server-info <FILE>`：写入单行 JSON：`{"port":<u16>,"pid":<u32>}`，便于外部脚本/启动器获知实际端口与 PID（特别是端口为 0 的场景）

## 8. 日志策略

- 启动日志：监听地址、端口、基座 URL（不含敏感信息）
- 请求日志（可选 debug 级）：方法、路径、上游状态码、耗时；不记录头/体
- 错误日志：描述性错误，不包含令牌

## 9. 验收与测试计划

功能验收（手工）：
- 登录：使用官方 codex 完成一次登录，`~/.codex/auth.json` 包含 `tokens.access_token` 与 `tokens.account_id`
- 启动：运行 `codex-backend-proxy --http-shutdown --server-info /tmp/cbp.json`
  - 验证 `/health` 返回 200
  - 检查 `cbp.json` 输出包含实际端口与 PID
- 代理：
  - 用 Chatbot 指向 `http://127.0.0.1:<port>/v1/responses` 与 `/v1/chat/completions`
  - 正常交互：文本与 `functionCall` 在 SSE 流中透明透传
  - 头部：上游应收到 `Authorization`、`ChatGPT-Account-Id`、`Host`，且不含 hop-by-hop 头
- 刷新：
  - 人为使用过期 `access_token` 验证：首次 401 触发刷新并重试一次，若刷新成功应恢复；否则将 401 透传
- 关停：访问 `/shutdown`（启用了 `--http-shutdown`）应优雅退出

自动化测试（后续补充）：
- 单元：头部过滤（请求/响应）、路由白名单、账户头必需性校验
- 集成：
  - 模拟上游 SSE 响应，验证文本与 `functionCall` 块的透明透传
  - 模拟上游 401，验证按需刷新重试一次
  - 验证 `/health` 与 `/shutdown`

## 10. 风险与回退策略

- 风险：上游域名或路径变动（`/backend-api/codex`）；
  - 对策：`--base-url` 可覆盖；默认仍指向官方域名
- 风险：`auth.json` 缺少 `account_id`；
  - 对策：尝试从 `id_token` claims 解析；仍无则清晰报错提示重新登录
- 风险：高并发下线程数过多；
  - 对策：当前满足“少量并发”需求；未来可加并发上限或迁移 async

## 11. 非侵入性保证

- 仅以库方式复用 Codex 代码：`codex_core::auth` 等；
- 不修改现有 crate 任一源文件与行为；
- 本代理作为独立新增 crate，不影响现有功能与测试。

## 12. 时间与里程碑

- T+0.5 天：脚手架 + /health + /shutdown + server-info + 路由白名单
- T+1 天：鉴权加载 + 头部策略 + 基本代理与 SSE 流式透传
- T+1.5 天：401 按需刷新与重试一次；完善错误处理
- T+2 天：手工验收 + 基础自动化测试（若需要）+ 文档整理

## 13. 成功判定标准（DoD）

- 在本机成功启动并通过 `/health`；
- `/v1/responses` 与 `/v1/chat/completions` 可稳定透传 SSE（文本与 `functionCall` 语义不变）；
- 上游请求包含正确的 `Authorization`、`ChatGPT-Account-Id` 与 `Host`，不存在 hop-by-hop 头；
- 当 `access_token` 过期时，能自动刷新并仅重试一次；
- 不记录敏感信息；
- 不修改既有 Codex 源码；
- 有清晰的错误提示与可读日志，`server-info` 输出可被外部脚本使用。

