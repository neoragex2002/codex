# codex-backend-proxy：面向 ChatGPT 后端的无状态 OpenAI 兼容代理（v1）

本文档汇总了需求、思路、架构、模块计划、测试策略、权衡取舍以及关键源码参考，用于构建一个稳健、可维护的 Rust 代理：北向提供 OpenAI 兼容接口，南向仅转发到 ChatGPT 专有后端（`/backend-api/codex`），并保持对 MCP 工具语义的完全透明透传与无状态。

## 1) 需求

- 范围与非目标
  - 南向：仅对接 ChatGPT 专有后端，路径以 `…/backend-api/codex` 为根。
  - 北向：对第三方 Chatbot 暴露两类 OpenAI 兼容端点：
    - `POST /v1/responses`
    - `POST /v1/chat/completions`
  - 认证简化：不实现北向的 OAuth 验证；直接复用此前使用官方 codex 登录写入的 `~/.codex/auth.json`，读取其中的访问令牌与 `account_id`（ChatGPT 账号 ID）用于南向鉴权。`ChatGPT-Account-Id` 视为南向 ChatGPT 后端的必需头，必须设置。
  - 无状态代理：不注册、不解析、不缓存任何 MCP 工具定义或与工具相关的状态。
  - 透传：请求与响应（含 SSE）均不改写工具相关字段。
  - 客户端负责逻辑：Chatbot 端负责工具定义、执行与响应构造；代理仅承载数据转发。
  - 松耦合：将 Codex Rust 代码视为可复用库，避免侵入内部实现。

- 明确不包含
  - `api.openai.com` 南向；基于 API Key 的流程；Azure 端点；代理内实现任何工具业务逻辑；北向 OAuth 或 JWKS 校验。

- 核心透传字段
  - 请求透传：`contents`、`tools`、`toolConfig`、`functionResponse`。
  - 响应透传（SSE 或非流式）：文本块与 `functionCall` 块、各类增量块。

- 运维
  - 代理以独立二进制运行于本机端口，仅允许前述两类端点。
  - 提供 `GET /health` 健康检查端点。
  - 可选 `GET /shutdown` 用于本机优雅停服。
  - 可选 `--server-info <file>` 启动时输出 `{ port, pid }` JSON（单行）。

## 2) 与设计原则对照

- 无状态代理：不感知工具定义或会话状态，不做缓存。
- 请求透传：最小化修改，仅覆盖必要头（授权、Host），不改动工具或内容结构。
- 响应透传：SSE 直接转发，不判别类型，不重建事件。
- Chatbot 负责一切：工具注册、调用与输出拼装均由 Chatbot 端完成。
- 核心库松耦合：复用 `codex_core::auth` 等公开 API 读取 `auth.json` 与令牌，而不修改内部实现。

## 3) 解决方案概览

- 新建 crate：`codex-backend-proxy`（二进制同名），位于 `codex-rs/backend-proxy/`。
- 北向接口（严格白名单）：
  - 仅接受 `POST /v1/responses` 与 `POST /v1/chat/completions`（无查询串）；其他路径一律 `403 Forbidden`。
  - 暴露 `GET /health`，返回 `200` 与简短 JSON（如 `{ "status": "ok", "version": "…" }`）。
- 南向目标（仅官方域名）：
  - `/v1/responses` → `https://chatgpt.com/backend-api/codex/responses`
  - `/v1/chat/completions` → `https://chatgpt.com/backend-api/codex/chat/completions`
  - 必要时可退回 `https://chat.openai.com/backend-api/codex`。当前不计划自定义域名。
- 认证（仅南向）：
  - 通过 `codex_core::auth` 读取 `~/.codex/auth.json`，获取 `access_token` 与 `account_id`。
  - `ChatGPT-Account-Id` 视为必需：若 `auth.json` 未提供，则尝试从 ID Token 的 claims（`https://api.openai.com/auth`）中解析；若仍无，直接返回 500，并提示用户重新登录以写入完整信息。
  - 注入头部：
    - `Authorization: Bearer <access_token>`
    - `ChatGPT-Account-Id: <account_id>`
    - 覆盖 `Host` 以匹配上游域名。
  - 按需刷新：当第一次收到上游 `401 Unauthorized` 时，调用 `codex_core::auth` 刷新令牌（如 `CodexAuth::refresh_token()`），持久化更新后的 `auth.json`，重建头并仅重试一次；若仍失败，则原样透传上游状态与正文。
- 传输与流式：
  - 入站用 `tiny_http`；上游转发用 `reqwest::blocking` 且 `.timeout(None)` 支持长连 SSE。
  - 语义透传（非字节级）：HTTP 客户端可能自动解压上游响应；需确保头与体一致（例如不要转发失真的 `content-encoding`）。
  - 将上游响应体直接作为 `tiny_http::Response` 的 body 流式回传；仅过滤 `tiny_http` 自管头。
- 日志与安全：
  - 不记录令牌或敏感信息。
  - 仅打印最小必要的请求/错误日志。

## 4) 架构

- 入站服务：`tiny_http::Server` 监听 `127.0.0.1:<port>`。
- 工作线程：每个请求派生一线程，校验路径、读取 body、构建上游头、转发至目标 URL。
- 头部策略（入站 → 出站）：
  - 丢弃入站的 `Authorization` 与 `Host`。
  - 丢弃 hop-by-hop 头：`connection`、`keep-alive`、`proxy-authenticate`、`proxy-authorization`、`te`、`trailer`、`transfer-encoding`、`upgrade`。
  - 保留其他可解析的头。
  - 新增 `Authorization`（源自 `auth.json`）、`Host`（由上游 URL 决定）、`ChatGPT-Account-Id`。
- 响应策略：
  - 复制状态码。
  - 复制头，但过滤 `tiny_http` 自管与其他 hop-by-hop 头。
  - 若上游被自动解压，需保证头/体一致（例如不再转发 `content-encoding`）。
  - 流式透传 body。
- 控制端点：
  - `GET /health` 返回 200 与简短 JSON。
  - 可选 `GET /shutdown`（由 `--http-shutdown` 启用）以退出。
  - 可选 `--server-info <file>`：启动写入单行 JSON `{ "port": <u16>, "pid": <u32> }`。

## 5) 模块拆分（建议）

- `args.rs`
  - CLI 解析：`--port`、`--server-info <FILE>`、`--http-shutdown`、`--codex-home <PATH?>`、`--base-url <URL>`（默认 `https://chatgpt.com/backend-api/codex`）。
- `auth_loader.rs`
  - 解析 codex home（默认 `~/.codex`）；使用 `codex_core::auth::try_read_auth_json` 读取 `auth.json`。
  - 提取 `access_token` 与必需的 `account_id`（若无则报错）。
- `router.rs`
  - 校验路径并映射到上游端点。
  - 白名单：`POST /v1/responses`、`POST /v1/chat/completions`、`GET /health`。
- `headers.rs`
  - 构建出站 `HeaderMap`（复制入站可保留头，剔除 `authorization`、`host` 与 hop-by-hop；追加南向所需头）。
- `proxy.rs`
  - 使用 `reqwest::blocking::Client` + `.timeout(None)` 转发。
  - 用上游响应构建 `tiny_http::Response` 并流式回传。
- `server.rs`
  - 绑定监听、接受连接、派生工作线程、实现 `/health` 与可选 `/shutdown`。
- `main.rs`
  - 组装上述各模块；处理 `server_info` 输出与启动日志。

Crate 命名需遵循仓库约定，采用 `codex-` 前缀（如 `codex-backend-proxy`）。

## 6) 测试策略

- 单元测试
  - `headers` 策略：入站头的保留与剔除；剔除 hop-by-hop 头；注入必需的 `ChatGPT-Account-Id`。
  - 路径校验：仅允许白名单端点；其他返回 403。
- 集成测试（模拟上游）
  - Responses：SSE 流透传（文本与 `functionCall` 透明性）。
  - Chat Completions：流式增量不被改写。
  - 错误传播：上游非 2xx 的状态码与正文原样回传（不包裹），在保证头一致性的前提下保留 `content-type`。
  - `server-info` 输出与 `/shutdown` 行为；`/health` 返回 200。

注意：不得新增或修改任何与 `CODEX_SANDBOX_*` 相关的代码。

## 7) 交付计划（里程碑）

1. 脚手架
   - 新建 `codex-backend-proxy`；实现 CLI、HTTP 服务绑定、`/health`、`/shutdown`、`server-info`。
2. 鉴权加载
   - 通过 `codex_core::auth` 读取 `auth.json`；提取 `access_token` 与必需的 `account_id`。
3. `/v1/responses` 转发
   - 严格路径白名单；上游 `${base}/responses`；SSE 流代理。
4. `/v1/chat/completions` 转发
   - 严格路径白名单；上游 `${base}/chat/completions`；SSE 流代理。
5. 头部策略与日志
   - 保留/覆盖策略；剔除 hop-by-hop 头；最小化日志且不含敏感信息。
6. 测试
   - 按上述单测与集成测试覆盖。
7. 文档
   - 更新 README：先用官方 codex 登录一次，再运行代理；Chatbot 指向代理端点即可。

## 8) 权衡与后续增强

- 阻塞 vs 异步
  - 维持与现有 `responses-api-proxy` 一致的 `reqwest::blocking` + `tiny_http` 简洁实现，适合本地或小规模转发。
  - 初期不提供并发配置；如未来需要更高并发或复杂路由，可迁移至 async（hyper/axum）或引入线程池/并发限制。
- 令牌生命周期
  - 初版假定 `auth.json` 中的 `access_token` 有效；如上游返回 401，可引入基于 `codex_core::auth` 的刷新逻辑。
- 北向认证
  - 按需求不实现。
- 配置
  - 默认使用官方域名（chatgpt.com；必要时退回 chat.openai.com），始终含 `/backend-api/codex`；如确有需要，可通过 `--base-url` 覆盖。
- 可观测性
  - 初期最小日志；后续可加指标与结构化日志。

## 9) 源码参考（关键文件与起始行）

- 现有严格 Responses 代理与流式透传
  - `codex-rs/responses-api-proxy/src/lib.rs:34`（CLI 名称/说明）
  - `codex-rs/responses-api-proxy/src/lib.rs:73`（启动日志）
  - `codex-rs/responses-api-proxy/src/lib.rs:118`（forward_request 入口）
  - `codex-rs/responses-api-proxy/src/lib.rs:119`（仅允许 POST /v1/responses）
  - `codex-rs/responses-api-proxy/src/lib.rs:154`（敏感授权头处理）
  - `codex-rs/responses-api-proxy/src/lib.rs:162`（上游 URL）
- Provider URL 组合（Responses vs Chat；ChatGPT 登录默认使用 backend-api/codex）
  - `codex-rs/core/src/model_provider_info.rs:141`（get_full_url 起始）
  - `codex-rs/core/src/model_provider_info.rs:149`（ChatGPT → backend-api/codex 默认基座）
  - `codex-rs/core/src/model_provider_info.rs:160`（Responses 路径后缀）
  - `codex-rs/core/src/model_provider_info.rs:161`（Chat Completions 路径后缀）
- Backend client 注入 ChatGPT 账号头的策略
  - `codex-rs/backend-client/src/client.rs:89`（构造头部）
  - `codex-rs/backend-client/src/client.rs:102`（注入 `ChatGPT-Account-Id`）
- OAuth 登录、令牌交换与持久化
  - `codex-rs/login/src/server.rs:456`（exchange_code_for_tokens 签名）
  - `codex-rs/login/src/server.rs:470`（token endpoint POST）
  - `codex-rs/login/src/server.rs:500`（persist_tokens_async 入口）
  - `codex-rs/login/src/server.rs:619`（通过 token-exchange 获取 API Key 的示例）
- Core 的鉴权帮助
  - `codex-rs/core/src/auth.rs:124`（两种模式下获取 token）
  - `codex-rs/core/src/auth.rs:134`（获取 account_id）
  - `codex-rs/core/src/auth.rs`（refresh_token 以及 token 持久化逻辑）
- Chat Completions 流式处理（说明保持工具与内容透传的上下文理解）
  - `codex-rs/core/src/chat_completions.rs:1`（SSE、tool_calls、finish_reason 处理）

## 10) 使用示例（交付后）

- 一次性：使用官方 Codex 登录（写入 `~/.codex/auth.json`）。
- 启动代理（默认）：
  - `codex-backend-proxy --http-shutdown --server-info /tmp/codex-backend-proxy.json`
  - 可选覆盖基座：`--base-url https://chatgpt.com/backend-api/codex`
- 配置 Chatbot 指向：
  - `http://127.0.0.1:<port>/v1/responses`
  - `http://127.0.0.1:<port>/v1/chat/completions`
- 停止代理：
  - `curl -sS http://127.0.0.1:<port>/shutdown`

server-info 的用途
- 当未显式指定 `--port`（使用临时端口）时，`--server-info` 文件为外部启动器或脚本提供实际监听端口与进程 PID，便于工具自动连接、在 UI 展示状态或执行优雅停服，而无需解析日志。

对 Codex 代码的使用边界
- 代理严格以“库”的方式复用 Codex 现有 crate（如 `codex_core::auth`），绝不修改其内部实现；本代理作为新增 crate 仅依赖其公开 API。

## 11) 代码风格与仓库约定

- Crate 名采用 `codex-` 前缀（如 `codex-backend-proxy`）。
- Rust 编码注意：
  - `format!` 参数内联（如 `format!("value={}", x)`），避免拼接。
  - 可折叠的 if 书写合并，贴合 clippy 要求。
  - 能用方法引用时避免冗余闭包。
- 不要新增或修改任何与 `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` 或 `CODEX_SANDBOX_ENV_VAR` 相关的代码。
