# Environment Variables / 环境变量

This document covers all environment variables recognized by HAPIR.

本文档列出 HAPIR 所有支持的环境变量。

---

## Hub (`hapir hub`)

Variables read by the hub server process.

以下变量由 hub 服务进程读取。

### Data & Storage / 数据与存储

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_HOME` | `~/.hapir` | Data directory. Stores settings, runner state, and is used as the base for other paths. / 数据目录，用于存储设置、runner 状态等，并作为其他路径的基准目录。 |
| `DB_PATH` | `{HAPIR_HOME}/hapir.db` | SQLite database file path. / SQLite 数据库文件路径。 |

### Networking / 网络

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_LISTEN_HOST` | `127.0.0.1` | Host address the server binds to. Set to `0.0.0.0` to listen on all interfaces. / 服务监听地址。设为 `0.0.0.0` 可监听所有网卡。 |
| `HAPIR_LISTEN_PORT` | `3006` | TCP port the server listens on. Must be a valid port number (1–65535). / 服务监听端口，需为有效端口号（1–65535）。 |
| `HAPIR_PUBLIC_URL` | `http://localhost:{port}` | Public-facing URL used in the web UI and notifications. Required when accessed from outside localhost. / 公开访问地址，用于 Web UI 和通知推送。从外部访问时必须设置。 |
| `CORS_ORIGINS` | derived from `HAPIR_PUBLIC_URL` | Comma-separated list of allowed CORS origins. Auto-derived from `HAPIR_PUBLIC_URL` if not set. / 逗号分隔的 CORS 允许来源列表。未设置时自动从 `HAPIR_PUBLIC_URL` 推导。 |

### Authentication / 认证

| Variable | Default | Description / 说明 |
|---|---|---|
| `CLI_API_TOKEN` | — | Static token used by the CLI to authenticate against the hub API (`/cli` routes). / CLI 连接 hub API（`/cli` 路由）时使用的静态令牌。 |

### Notifications / 通知

| Variable | Default | Description / 说明 |
|---|---|---|
| `TELEGRAM_BOT_TOKEN` | — | Telegram bot token. Required to enable Telegram bot notifications and Mini App integration. / Telegram 机器人令牌，启用 Telegram 通知和 Mini App 集成时需要。 |
| `TELEGRAM_NOTIFICATION` | `true` | Set to `false` to disable Telegram notification delivery even when a bot token is configured. / 设为 `false` 可在已配置 bot token 的情况下禁用 Telegram 通知推送。 |
| `VAPID_SUBJECT` | `mailto:admin@localhost` | VAPID subject URI for Web Push notifications. Should be a `mailto:` or `https:` URL identifying the app operator. / Web Push 通知的 VAPID subject URI，应为标识应用运营方的 `mailto:` 或 `https:` 地址。 |

### Voice / 语音

| Variable | Default | Description / 说明 |
|---|---|---|
| `ELEVENLABS_API_KEY` | — | ElevenLabs API key. Required for the voice assistant feature. / ElevenLabs API 密钥，语音助手功能必需。 |
| `ELEVENLABS_AGENT_ID` | — | ElevenLabs agent ID to use for voice sessions. / 语音会话使用的 ElevenLabs agent ID。 |

### Tuning / 调节

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_TODO_BACKFILL_LIMIT` | `200` | Maximum number of messages to scan when backfilling todo items for a session. / 回填会话 todo 事项时扫描的最大消息数。 |

---

## CLI / Runner

Variables read by `hapir` subcommands (claude, codex, gemini, opencode, runner, mcp, etc.) and the background runner process.

以下变量由 `hapir` 各子命令（claude、codex、gemini、opencode、runner、mcp 等）及后台 runner 进程读取。

### Connection / 连接

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_API_URL` | `http://localhost:3006` | Hub server URL. The CLI and runner connect to this address. / Hub 服务地址，CLI 和 runner 连接此地址。 |
| `CLI_API_TOKEN` | — | Authentication token for the hub API. Obtained via `hapir auth login`. / Hub API 认证令牌，通过 `hapir auth login` 获取。 |
| `HAPIR_HOME` | `~/.hapir` | CLI data directory. Stores auth state and runner metadata. / CLI 数据目录，存储认证状态和 runner 元数据。 |
| `HAPIR_HTTP_MCP_URL` | — | HTTP backend URL for the MCP stdio bridge (`hapir mcp`). Overridden by the `--url` flag. / MCP stdio 桥接（`hapir mcp`）的 HTTP 后端地址，可被 `--url` 参数覆盖。 |

### Agent Executables / Agent 可执行文件路径

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_CLAUDE_PATH` | `claude` | Claude Code executable. May be a command string with arguments, e.g. `bash /path/to/wrapper.sh`. / Claude Code 可执行文件路径，支持带参数的命令字符串，如 `bash /path/to/wrapper.sh`。 |
| `HAPIR_CODEX_PATH` | `codex` | Codex executable. Supports command strings. / Codex 可执行文件路径，支持命令字符串。 |
| `HAPIR_GEMINI_PATH` | `gemini` | Gemini CLI executable. Supports command strings. / Gemini CLI 可执行文件路径，支持命令字符串。 |
| `HAPIR_OPENCODE_PATH` | `opencode` | OpenCode executable. Supports command strings. / OpenCode 可执行文件路径，支持命令字符串。 |

### Agent Configuration / Agent 配置

| Variable | Default | Description / 说明 |
|---|---|---|
| `CLAUDE_CONFIG_DIR` | `~/.claude` | Claude desktop config directory. Used to locate MCP config and slash command definitions. / Claude 桌面版配置目录，用于定位 MCP 配置和斜杠命令定义。 |
| `CODEX_HOME` | `~/.codex` | Codex home directory. Used for slash commands and skills lookup. / Codex 主目录，用于斜杠命令和技能查找。 |

### Machine Identity / 机器标识

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_HOSTNAME` | system hostname | Override the hostname reported to the hub for machine registration. Useful in containerized environments. / 覆盖上报给 hub 的机器主机名，适合在容器化环境中使用。 |

### Feature Flags / 功能开关

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPIR_EXPERIMENTAL` | — | Set to `true`, `1`, or `yes` to enable experimental features. / 设为 `true`、`1` 或 `yes` 可启用实验性功能。 |

### Runner Internals / Runner 内部参数

| Variable | Default | Description / 说明 |
|---|---|---|
| `HAPI_RUNNER_HEARTBEAT_INTERVAL` | `60000` | Heartbeat interval in milliseconds for the runner process. / runner 进程的心跳间隔（毫秒）。 |
| `HAPI_RUNNER_HTTP_TIMEOUT` | `10000` | HTTP request timeout in milliseconds for runner control API calls. / runner 控制 API 请求的超时时间（毫秒）。 |

---

## System / 系统变量

Standard system variables that HAPIR reads but does not own.

HAPIR 读取但不拥有的标准系统变量。

| Variable | Description / 说明 |
|---|---|
| `RUST_LOG` | Log level filter (e.g. `info`, `debug`, `hapir_hub=trace`). Defaults to `info`. / 日志级别过滤器（如 `info`、`debug`、`hapir_hub=trace`），默认 `info`。 |
| `SHELL` | Default shell for bash handler (Unix). Falls back to `/bin/zsh` on macOS, `/bin/bash` on Linux. / bash 处理器使用的默认 shell（Unix），macOS 回退到 `/bin/zsh`，Linux 回退到 `/bin/bash`。 |
| `COMSPEC` | Default shell on Windows. Falls back to `powershell.exe`. / Windows 上的默认 shell，回退到 `powershell.exe`。 |
| `LC_ALL` / `LANG` | Locale settings. Used to detect Chinese language for i18n of CLI help text. / 区域设置，用于检测中文以国际化 CLI 帮助文本。 |

---

## Security Note / 安全说明

The following variables are **automatically stripped** from the PTY environment before spawning agent subprocesses, to prevent credential leakage into AI tool outputs:

以下变量在启动 agent 子进程前会**自动从 PTY 环境中移除**，防止凭据泄漏到 AI 工具输出中：

- `CLI_API_TOKEN`
- `HAPIR_API_URL`
- `HAPIR_HTTP_MCP_URL`
- `TELEGRAM_BOT_TOKEN`
- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `GEMINI_API_KEY`
- `GOOGLE_API_KEY`