# HTTP 接口

[English](api.md) · [**中文**](api-zh.md)

所有内容由同一个进程在同一端口提供。代理与管理接口是两个命名空间：`/v1/*` 走代理，
`/api/*` 属于控制台，其余路径由控制台静态文件接管。

## 代理

| 接口 | 说明 |
| --- | --- |
| `POST /v1/chat/completions` | OpenAI chat |
| `POST /v1/responses` | OpenAI Responses |
| `POST /v1/messages` | Anthropic Messages |
| `GET /v1/models` | 只返回一个 id：`ar-ocg-router` |

裸 `http://host:port` 同样可用，并接受 `/openai/v1` 与 `/api/v1` 前缀。

每个代理响应都带：

| 头 | 值 |
| --- | --- |
| `x-router-account` | 实际服务的端点名 |
| `x-router-endpoint` | 同值，显式别名 |
| `x-router-kind` | `plans` 或 `fallback` |
| `x-router-model` | 实际发给上游的模型 id |
| `x-router-peak` | `peak` 或 `offpeak` |
| `x-router-cache` | `hit`、`cold` 或 `unknown`（仅非流式；流式响应头已发出） |
| `x-router-request-id` | 与日志对应的关联 id |

## 管理接口

| 接口 | 用途 |
| --- | --- |
| `GET /` | Web 控制台（未内嵌时回落为纯文本横幅） |
| `GET /router/banner` | 纯文本横幅，供脚本使用 |
| `GET /health` | 存活探针，永不需要密钥 |
| `GET /router/stats` | 全量 JSON：忙闲状态、计数器、各端点健康、额度三窗口、余额、token 与费用 |
| `GET /router/schedule` | 未来若干次忙闲切换 |
| `POST /router/reload` | 立即重载配置（同时每 3 秒自动检测） |
| `GET /api/config` | 供控制台编辑的配置（密钥已打码） |
| `PUT /api/config` | 校验、备份、写入、热加载 |
| `GET /api/config/raw` · `PUT /api/config/raw` | 原始 YAML 文本 |
| `GET /api/config/export` | 序列化后的配置，供下载 |
| `POST /api/config/import` | 导入 YAML 或 JSON |
| `POST /api/plan/preview` | 按当前时刻模拟候选顺序 |
| `POST /api/test/<名>` | 端点实时探测（密钥、/models、额度、一次请求） |
| `POST /api/endpoints/<名>/state` | 启用或禁用 |
| `POST /api/endpoints/<名>/duplicate` | 以空闲名字复制端点 |
| `GET /api/endpoints/<名>/models` | 该端点的实时模型清单 |
| `GET /api/library` · `PUT /api/library` | 模型库 |
| `GET /api/backups` · `POST /api/backups` | 列出与创建配置备份 |
| `GET` / `DELETE` `/api/backups/<名>` · `POST .../restore` | 下载、删除、恢复 |
| `POST /api/stats/reset` | 清零统计、端点健康与计数器（额度账本保留）；校准记录中会被拒绝 |
| `POST /api/endpoints/<名>/calibrate` | 额度校准：`stage` 为 `start`、`finish`、`verify`、`anchors`、`cancel`；见[配置文档](configuration-zh.md#校准用两次读数推算真实额度) |

设置了 `server.client_keys` 后，除 `/health` 外的所有管理路由都要求与代理相同的 bearer
token。控制台只在回环地址默认未鉴权——这也是应该保持默认的原因。

## 错误格式

错误是 OpenAI 形状的 JSON：

```json
{"error": {"message": "...", "type": "all_accounts_failed", "code": 502,
           "router": "ar-ocg-router 0.0.1"}}
```

`type` 取值包括 `unauthorized`、`no_account_available`、`all_accounts_failed`、
`reload_failed`、`not_found`、`admin_error`。
