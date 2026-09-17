# 上游事实

[English](upstream.md) · [**中文**](upstream-zh.md)

以下厂商行为均已对线上接口实测。模型清单、额度接口与条款由厂商控制且随时可能变更——
请把它当作起点而非承诺。

## 忙闲时段

| 项目 | 结论 |
| --- | --- |
| 忙时 | 周一~周五 **01:00-04:00** 与 **06:00-10:00 UTC** |
| 闲时 | 其余全部时间（含整个周末），单价为忙时的 50% |
| 模型 | DeepSeek V4.1 Flash、V4 Pro、V4 Flash、V4 Flash Vision (exp) |

DeepSeek 官方与 OpenCode Go 共用同一窗口。

## 接入点与协议

| 上游 | Base URL | 协议 |
| --- | --- | --- |
| DeepSeek 官方 | `https://api.deepseek.com` | `/chat/completions`、`/responses` |
| OpenCode Go | `https://opencode.ai/zen/go/v1` | `/chat/completions`、`/responses` |

Go 的前缀是 `/zen/go/v1`，**不是** `/zen/v1`（后者是 Zen 按量付费产品）。

## OpenCode Go 的两个硬性要求

1. 每个请求都必须带 **`x-opencode-session`**，否则返回
   `400 {"type":"error","error":{"type":"MissingSessionID",...}}`。路由器会从客户端的请求头或
   请求体里找会话 id，找不到就用进程级兜底，保证请求始终可用、会话路由尽量稳定。
2. 客户端必须带自己的 **User-Agent**；Go 按它判断「编码 agent 流量」。

## 额度窗口

OpenCode Go 暴露三个独立窗口（月度上限的占比）：

| 窗口 | 占比 | 额度 $60 的套餐 |
| --- | --- | --- |
| 5 小时滚动 | 20% | $12 |
| 周 | 50% | $30 |
| 月 | 100% | $60 |

`GET https://opencode.ai/zen/go/v1/usage` 返回它们。该接口**未公开**，没有任何兼容性承诺。
`GET .../quota` 是 404，不存在。DeepSeek 官方没有额度接口，只有
`GET https://api.deepseek.com/user/balance`。

## 决定整套策略的经济学

DeepSeek 系模型在 OpenCode Go 与官方 API 上价格完全相同，且忙时同样翻倍：

| 模型 | 闲时（入 / 出 / 缓存） | 忙时 |
| --- | --- | --- |
| DeepSeek V4.1 Flash | $0.15 / $0.60 / $0.003 | $0.30 / $1.20 / $0.006 |
| DeepSeek V4 Pro | $0.66 / $1.98 / $0.022 | $1.32 / $3.96 / $0.044 |

也就是说：一个 token 的额度，任何时段都恰好等于等量现金。因此「忙时用 Go、闲时用官方」
在现金总额上是中性的。真正影响支出的只有一件事：**避免额度在窗口结束时未使用完毕**——这正是
`idle_prefer: surplus_first` 所实现的。

## 模型清单不可作为准入依据

`GET <base>/models` 不能用来做准入控制：

| 上游 | 结果 | 性质 |
| --- | --- | --- |
| OpenCode Go | 200，约 37 个 | 可用目录 |
| DeepSeek 官方 | 200，2 个 | 可信 |
| 部分国内套餐 | 200，上百个 | 整个账号目录；里面有调不通的条目，也有可用却不出现的 id |

同一模型在各家的 id 也不一样。这正是路由器原样发出每个端点的 `model`、以及
[模型库](model-library-zh.md) 存在的原因。

## 合规

部分套餐（尤其是腾讯 Token Plan）明确限定只能在受支持的编程工具内使用，禁止自动化脚本、
自定义应用后端与非交互式批量调用。本路由器是本地代理，是否算「后端」请自行判断；
不要拿这些套餐跑批量任务。
