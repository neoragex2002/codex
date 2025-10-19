# ChatGPT 后端 API 要求

本文档详细说明了使用 OAuth 身份验证向 ChatGPT 后端 Codex API (`https://chatgpt.com/backend-api/codex/responses`) 发出成功请求的确切要求。

**注意：** 本文档源自github issues。其中所提供的信息，表明ChatGPT 后端 Codex API对其API接口调用参数，从Instruction到Tools声明都施加了严厉的限制。其中有若干限制是经过确认了的，其中若干额外限制是未经确认的。请在使用该文档时，结合实际官方codex cli源码信息和实测后端返回的错误信息情况加之甄别确认。

> Reference: https://github.com/sst/opencode/issues/1686

## 概述

ChatGPT 后端 API 执行严格的验证。它期望请求与官方 Rust Codex 客户端发送的内容完全匹配，包括特定的工具、指令和模型。

## 1. 身份验证要求

### OAuth 令牌 (Token)

* 位置: `~/.codex/auth.json`
* 类型: JWT 不记名令牌 (bearer token)

### 账户 ID 提取

从 JWT 负载 (payload) 中提取 `chatgpt_account_id`：

```javascript
const tokenParts = oauth_token.split('.');
const payload = JSON.parse(Buffer.from(tokenParts[1], 'base64url').toString());
const chatgptAccountId = payload['[https://api.openai.com/auth](https://api.openai.com/auth)'].chatgpt_account_id;
````

## 2. 必需的请求头 (Headers)

```json
{
  "Content-Type": "application/json",
  "Authorization": "Bearer ${oauth_jwt_token}",
  "Accept": "text/event-stream",
  "OpenAI-Beta": "responses=experimental",
  "session_id": "crypto.randomUUID()",
  "originator": "codex_cli_rs",
  "chatgpt-account-id": "chatgpt_account_id"
}
```

  * `session_id`: 生成唯一的 UUID v4
  * `originator`: 必须**完全**是这个值
  * `chatgpt-account-id`: 从 JWT 中提取的 ID

**重要提示**：不要包含 `User-Agent` 请求头。

## 3. 模型要求

  * **必需模型**: `gpt-5`、`gpt-5-codex`
  * 其他模型 (例如 `gpt-4o`) 将返回 `{"detail":"Unsupported model"}` 错误

## 4. 指令 (Instructions) 要求

必须使用来自 Rust Codex 提示文件的**确切**指令：

  * 文件: `/codex/main/codex-rs/core/prompt.md`
  * 该指令引用了 `apply_patch`，并且必须与所发送的工具相匹配
  * 任何偏差都会导致 `{"detail":"Instructions are not valid"}` 错误

## 5. 必需的工具模式 (Tools Schema)

必须**完全**按照以下特定结构发送这两个工具：

### Shell 工具

```json
{
  "type": "function",
  "name": "shell",
  "description": "运行一个 shell 命令并返回其输出",
  "strict": false,
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "array",
        "items": { "type": "string" }
      },
      "workdir": {
        "type": "string"
      },
      "timeout": {
        "type": "number"
      }
    },
    "required": ["command"],
    "additionalProperties": false
  }
}
```

### Update Plan (更新计划) 工具

```json
{
  "type": "function",
  "name": "update_plan",
  "description": "使用 update_plan 工具来让用户了解当前任务的最新计划。\n在理解用户任务后，使用初始计划调用 update_plan 工具。计划示例：\n1. 探索代码库以查找相关文件 (状态: in_progress)\n2. 在 XYZ 组件中实现该功能 (状态: pending)\n3. 提交更改并发起一个 pull request (状态: pending)\n每一步都应该是一个简短的、一句话的描述。\n在所有步骤完成之前，计划中应该始终只有一个 in_progress 状态的步骤。\n每当你完成一个步骤时，调用 update_plan 工具，将完成的步骤标记为 `completed`，并将下一步标记为 `in_progress`。\n在运行命令之前，请考虑你是否已完成上一个步骤，并确保在进入下一步之前将其标记为已完成。\n有时，你可能需要在任务中途更改计划：使用更新后的计划调用 `update_plan`，并确保在这样做时提供一个 `explanation` (解释)来说明理由。\n当所有步骤都完成后，最后一次调用 update_plan，并将所有步骤标记为 `completed`。",
  "strict": false,
  "parameters": {
    "type": "object",
    "properties": {
      "explanation": {
        "type": "string"
      },
      "plan": {
        "type": "array",
        "description": "步骤列表",
        "items": {
          "type": "object",
          "properties": {
            "step": { "type": "string" },
            "status": { "type": "string" }
          },
          "required": ["step", "status"],
          "additionalProperties": false
        }
      }
    },
    "required": ["plan"],
    "additionalProperties": false
  }
}
```

## 6. 请求体 (Request Body) 结构

```json
{
  "model": "gpt-5",
  "instructions": "instructions",
  "input": [
    {
      "type": "message",
      "role": "user",
      "content": [
        {
          "type": "input_text",
          "text": "用户的消息内容"
        }
      ]
    }
  ],
  "store": false,
  "stream": true,
  "include": ["reasoning.encrypted_content"],
  "tools": "[shell_tool, update_plan_tool]",
  "tool_choice": "auto",
  "parallel_tool_calls": false
}
```

  * `instructions`: 来自 `prompt.md` 的完整 Codex 指令
  * `tools`: `shell_tool` 和 `update_plan_tool` 对象
  * **注意**: **不要**包含 `temperature` 参数 - 这会导致错误

## 7. 验证规则

ChatGPT 后端 API 强制执行以下严格的验证规则：

### Originator (来源) 匹配

当发送 `originator: codex_cli_rs` 时：

  * 指令 (Instructions) **必须**与 Rust Codex 的指令完全匹配
  * 工具 (Tools) **必须**是 `shell` 和 `update_plan`，且其 schema 必须与上面显示的完全一致
  * 模型 (Model) **必须**是 `gpt-5`

### 工具与指令的一致性

  * 如果指令中提到了 "apply patches" (应用补丁)，则 `shell` 工具必须存在 (因为 `apply_patch` 在 Codex 中是一个 shell 命令)
  * 指令中引用的工具名称必须与实际提供的工具相匹配

### 参数限制

  * 不支持 `temperature` 参数 (会导致 `{"detail":"Unsupported parameter: temperature"}`)
  * 工具结构必须是扁平化的 (属性直接在 tool 对象上，而不是嵌套在 `function` 下)

## 8. 常见错误信息

| 错误信息 | 原因 | 解决方案 |
| :--- | :--- | :--- |
| `"Instructions are not valid"` | 指令与该 originator 预期的格式不匹配 | 使用 `prompt.md` 中确切的 Codex 指令 |
| `"Unsupported model"` | originator 使用了错误的模型 | 使用 `gpt-5` |
| `"Unsupported parameter: temperature"` | 请求中包含了 temperature 参数 | 从请求中移除 temperature |
| `"Missing required parameter: 'tools[0].name'"` | 工具结构错误 | 使用扁平化的工具结构 (非嵌套) |

## 9. 测试

测试脚本位于 `test-mirror-tools.ts`，它演示了一个包含所有正确参数的工作请求。

## 10. 重要说明

1.  **需要完全匹配**: 后端执行复杂的验证。每个方面都必须与 Rust Codex 客户端发送的内容完全匹配。
2.  **工具名称不兼容**: 如果你的应用程序使用不同的工具名称 (例如 `read_file`, `edit_file` 而不是 `shell`)，你就不能声明 `originator: codex_cli_rs`，否则会导致验证错误。
3.  **没有部分合规**: 你不能混合搭配——要么发送与 Codex 完全一致的内容，要么就不要使用 `originator: codex_cli_rs` 头部。
4.  **流式响应 (Streaming Response)**: 该 API 以 `text/event-stream` 内容类型返回流式响应。你需要解析 SSE (Server-Sent Events) 格式来提取响应内容。

## 示例工作请求

请参阅 `test-mirror-tools.ts` 以获取一个完整的、能成功通过身份验证并从 ChatGPT 后端 API 接收响应的工作示例。
