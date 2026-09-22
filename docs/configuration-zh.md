# 配置参考

[English](configuration.md) · [**中文**](configuration-zh.md)

路由器只读一个 YAML 文件。所有字段都有可用默认值，所以最小可用配置就是一个端点。
带完整注释的模板见 `config.example.yaml`。

## 文件查找顺序

1. 给了 `--config <path>` 就用它（相对/绝对均可）；
2. 否则找**二进制同级目录**的 `config.yaml` → `config.yml`；
3. 再找**当前工作目录**下的同名文件。

都没有则退出码 1，并打印搜索过的全部路径。

## 端点

一个端点 = 一个商家 + 一个模型。分两个列表：

| 列表 | 含义 |
| --- | --- |
| `plans` | 预付订阅与额度包。消耗它们不花钱，直到额度用完，所以默认优先使用。 |
| `fallback` | 按 token 计费的现金账号。价格随忙闲时段变化。 |

```yaml
plans:
  - name: go-dsf
    url: https://opencode.ai/zen/go/v1
    provider: opencodego
    key: sk-...
    model: deepseek-flash
    mode: both
    order: 10
    inject_session: true
    quota: { unit: usd, rolling: 12, weekly: 30, monthly: 60, probe: usage }

fallback:
  - name: ds-official-flash
    url: https://api.deepseek.com
    provider: deepseek
    key: sk-...
    model: deepseek-flash
    mode: both
    order: 10
    rule: [offpeak, quota_low]
    quota: { probe: balance }
```

### 端点字段

| 字段 | 说明 |
| --- | --- |
| `name` | 日志、统计与客户端钉端点都用它。显式优先；重名自动加 `-2`、`-3`。不写则自动生成 `<provider>-<模型>-<密钥指纹>`。 |
| `provider` | `opencodego` / `deepseek` / `generic`。默认按 URL 猜测。**走中转或反代时必须显式指定**，否则模型映射、额度与余额探测都会退化为 `generic`。 |
| `url` | 上游前缀，末尾不带 `/`。 |
| `key` | 明文、`env:VAR` 或 `file:/path/key.txt`。 |
| `model` | **必填。** 该端点唯一会发出的模型 id，原样透传。写错会在上游报错并自动换下一个。 |
| `mode` | `openai-completion` / `openai-responses` / `anthropic-messages` / `both` / `any`，也可写列表。 |
| `order` | 同一列表内的消耗顺序，越小越先。由条目位置推导，每次保存都会按位置重写——请在控制台里拖拽调整，不要手改。 |
| `rule` | 仅 `fallback`：允许它**作为首选**的条件。见下。 |
| `inject_session` | 发送 `x-opencode-session`。OpenCode Go 要求必须带。 |
| `headers` | 附加请求头（部分中转要求客户端特征头）。 |
| `drop_params` | 该端点不接受的请求体字段。 |
| `max_output_tokens_limit` | **是上限，不是固定值。** 客户端要的值超过它时，改用它发出去；不超过则一字不动。不写或写 `0` 表示完全不碰这个值。请按实测填、不要猜：填小了会静默缩短每一次长回答。详见下。 |
| `enabled` | `false` 会把该端点从**所有**候选列表移除，包括失败兜底。见 [路由决策](routing-zh.md)。 |

#### `max_output_tokens_limit`

有些上游在 `max_output_tokens` 超过自家上限时会**整个请求**退回，而且理由是一句**不点名参数**
的 "a parameter specified in the request is not valid"——于是故障看起来像「这个端点什么都服务不了」，
而不是「有一个数字太大」。编码类客户端还会雪上加霜：按上下文窗口的整数倍索要（384000 = 128k × 3），
没有任何模型能返回这么多。

这个上限就是为这种情况准备的，且**按端点配置**，因为各上游的上限不同
（DeepSeek 收 200000，方舟拒绝）。两个性质是刻意的：

- **是钳制，不是强设。** 请求里写 4096 就照 4096 发。
- **是你说的事实，不是路由器的猜测。** 请在实测上游一次之后填。路由器**不会**从模型库或任何别处推导它。

每次实际钳制都会记一条 INFO：
`<端点>: max_output_tokens 384000 -> 131072 (endpoint limit)`，所以日志里能查到实际发出去的值。

### `rule` 取值

多值之间是「或」：`always`（默认）、`peak`、`offpeak`、`quota_low`、`quota_exhausted`、
`primary_unavailable`、`never`。另有 `no_error_fallback`，禁止把它用作失败兜底。

## 额度

| 字段 | 说明 |
| --- | --- |
| `quota.unit` | `usd` / `rmb` / `tokens` / `none`。按 token 抵扣的套餐不需要价表。 |
| `quota.probe` | `usage`（厂商用量接口）、`balance`（账户余额）、`none`（只本地记账）。 |
| `quota.rolling` / `weekly` / `monthly` | 三个窗口上限，单位同上。OpenCode Go 的内置默认值是**非促销**额度：3 / 7.5 / 15 美元（即 15 美元/月的 20% / 50% / 100%）。促销提高的是额度总量、不是形状，所以促销数字请写在端点的配置里：编译进二进制的默认值会在促销开始或结束的那天静默报错。 |
| `quota.cycle_day` | 订阅套餐的**重置日**（1–31），即购买日。不填或填 `0` = 不重置，月窗口按 30 天滚动累计（按量计费端点用这个）。见下。 |
| `quota.refresh_secs` | 探测间隔。`usage` 默认 60 秒，`balance` 默认 300 秒。 |

拿不到上游数据时回退到本地记账（按累计金额或 token 换算成额度占比）。这只会影响闲时
「预付额度是否会被用尽」的判断精度，不影响可用性。

账本只存 **token 计数，不存钱**。金额是「读取那一刻生效的单价」的函数；把单价固化进样本，
等于把每条请求当时的配置冻在文件里，之后修正单价也永远修不好它算出的历史。存计数则相反：
修正后的单价一次性作用于全部历史，包括回溯；两个模型单价不同，也只需各自按自己的单价计算
再相加。

### 共用同一个套餐的端点

两个端点若配置为**同一 provider 且同一 key**，那就是同一个套餐下的两个模型，共用一份额度。
它们共享额度状态（读数、校准、token 账本），但各自保留统计、冷却与「处理中」标记——那些
描述的是端点本身而非套餐。单价仍按端点各自配置：厂商对不同模型计费不同，账本按各自单价计价。

因此套餐**只需校准一次**即可覆盖其全部模型；控制台对它们显示同一个已用百分比，而不会把同一份
消耗挂在两个名字下重复显示。

组标识里必须含 key：同一厂商用两个不同 key 就是两个套餐，合并会凭空造出一份双方都没有的额度。
没有 key 的端点无从归组，各自独立记账。

### 订阅周期：为什么需要一个「重置日」

按量计费的账号没有周期，花掉的钱一直累加。订阅套餐不一样：厂商在每个计费周期开始时把额度
重置为满额，而重置日就是**购买日**。官方规则是「购买当天生效，到次月同日 23:59:59 到期」，
例如 1 月 4 日买则 2 月 4 日到期；1 月 31 日买则 2 月 28 日到期（当月没有该日就用当月最后一天）。

30 天滑动窗口**表达不了**这件事：在厂商重置的那天，滑动窗口里仍挂着整整上个月的量，于是本地
百分比从高位起步，要再过一个月才自洽。

```yaml
quota: { unit: rmb, monthly: 200, probe: none, cycle_day: 26 }
```

配上 `cycle_day` 之后，「本周期已用」= 从本周期起点累计到今天；账本各窗口的金额也按同一窗口
计算（用周期起点，而非滚动 30 天），因此金额与百分比在厂商重置当天不会互相矛盾。周期边界由锚点日算出，因此
**本地也能知道重置时刻**——滑动窗口从来给不出这个值，所以控制台现在能显示与厂商一致的重置
倒计时和投影。

### 校准：用两次读数推算真实额度

路由器只能看到**经过它自己转发**的流量。没有用量接口的套餐（如火山 Coding Plan），本地账本
从零开始，而厂商后台早已消耗了一部分——本地百分比必然偏低。控制台的「额度校准」向导用
**两次读数**解决这个问题：

1. 抄下后台三个窗口的当前百分比（第 1 次读数）；
2. 正常使用一段时间，让路由器积累消耗；
3. 再抄一次（第 2 次读数）。

两次读数之间的**百分点差**对应路由器实际转发的消耗量，一比即得「百分点 ↔ 真实金额」的换算
关系；以套餐价格（月窗口上限）为锚，即可推算出各窗口的真实总额，以及输入、缓存、输出三种
token 的真实单价——后者会自动写回 `prices`，让本地记账从「比例」变成「真钱」。

推算结果跨周期复用，直到套餐或系数变更。第三次读数可以验证推算：预测与实际的偏差即为
非路由器流量或系数误差。两次读数必须落在同一个订阅周期内，且月窗口的变动至少 0.3 个百分点
（低于此值会被拒绝——后台百分比的舍入误差在窗口太窄时会被急剧放大）。

「记录起点」到「推算」之间的进度（起点以来已消耗）从本地账本读取，因此**重置数据不会
让它失真**；反过来，重置数据在校准记录中会被拒绝（提示先取消校准），而一旦执行就会丢弃
未完成的起点读数。5 小时或周窗口的读数若跨越了各自的清零时刻（或位移不足 0.3 个百分点），
该窗口会被跳过而不参与推算，响应中会注明。月末「预测」百分比只在本地账本从周期起点起
就完整观察时才显示——只有半程数据的账本无法外推，只报真实百分比。

> **上限**：这个数字只在**这个 key 独占给路由器使用**时才会与后台持续吻合。共用同一个 key 的话，
> 差额会随时间重新累积，任何配置项都无法修复。

### 单价，以及为什么不需要「扣减系数」

端点可以声明自己的计费：

```yaml
prices: { currency: CNY, input: 1, output: 4, cached_input: 0.02, peak_multiplier: 2 }
```

优先级是 **上游自报的金额 > 这里的单价 > 不记账**（宁可为空，也不猜）。`currency` 只是标签：
路由器**不做任何汇率换算**——两家商家的定价不是一组汇率，也没有任何人报过它们之间的汇率。

各厂商对不同 token 类型收费不同，这件事就表达在**这里**：命中缓存的输入 0.02、未命中 1。
同一组单价同时喂给本地账本，所以一个套餐的剩余预算与厂商实际扣的费用用的是同一套钱。
这就是**不需要另做一套「扣减系数」**的原因：一个端点的价目表已经覆盖了缓存命中/未命中/输出的
差异；额度不是钱而是 token 的套餐，用 `quota.unit: tokens` 表达即可。再加一张系数表等于把
同一件事说两遍，而两遍可以互相矛盾。

参考价（provider / model → 单价），以下为标注日期核实的值：

| 厂商 | 模型 | 币种 | 输入 | 缓存 | 输出 | 高峰 |
| --- | --- | --- | --- | --- | --- | --- |
| DeepSeek 官方 | deepseek-flash | CNY | 1 | 0.02 | 4 | ×2 |
| DeepSeek 官方 | deepseek-v4-pro | CNY | 4.5 | 0.15 | 13.5 | ×2 |
| OpenCode Go | deepseek-flash | USD | 0.15 | 0.003 | 0.60 | — |
| OpenCode Go | deepseek-v4-pro | USD | 0.66 | 0.022 | 1.98 | — |
| OpenCode Go | glm-5.3-flash | USD | 0.15 | 0.015 | 0.50 | — |
| OpenCode Go | glm-5.3 | USD | 1.40 | 0.26 | 4.40 | — |
| OpenCode Go | kimi-k3 | USD | 3.00 | 0.30 | 15.00 | — |
| OpenCode Go | qwen3.8-max | USD | 2.00 | 0.25 | 6.00 | — |
| 火山方舟 | 全部 | CNY | 自行实测 | 自行实测 | 自行实测 | — |

DeepSeek 的高峰时段是北京时间周一至周五 09:00–12:00、14:00–18:00，正是默认
`peak_windows` 里的 `01:00-04:00, 06:00-10:00 UTC`。方舟按地域定价、变动频繁，且不返回单次
费用，请从你的控制台取数填入。

## 路由策略

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `mode` | `auto` | `auto` 按忙闲时段与额度余量决策；`plans` 始终优先使用预付额度；`fallback` 始终优先使用按量计费账号。 |
| `peak_windows` | `Mon-Fri 01:00-04:00, 06:00-10:00 UTC` | 语法 `<星期> <HH:MM-HH:MM>[, ...] [时区]`，多段用 `;` 分隔。 |
| `idle_prefer` | `surplus_first` | `surplus_first` 在预付额度预计用不完时继续使用，额度紧张时才改用半价现金账号；`plans` 与 `fallback` 为固定策略。 |
| `surplus_max_pct` | 80 | 任一窗口达到该百分比即视为额度紧张。 |
| `surplus_projection` | true | 按窗口进度线性外推；预计用不完的套餐也算充裕。 |
| `exhaust_at_pct` | 99 | 达到该百分比后不再优先使用该套餐。 |
| `session_affinity` | true | 同一会话固定同一端点，保住上游的会话与缓存语义。 |
| `session_affinity_ttl_secs` | 1800 | 绑定有效期。 |
| `session_fallback` | `process` | 客户端没传会话头时用什么：`process`（缓存更稳）或 `per-request`。 |
| `session_headers` | 见模板 | 按顺序从这些请求头里取会话 id。 |
| `user_agent` | `ar-ocg-router/<版本> (coding-agent)` | OpenCode Go 按 UA 判断流量类型，不要改成通用 SDK 名。 |
| `skip_after_failures` / `skip_secs` | 3 / 300 | 连续失败 N 次后整段跳过该端点，跳过时长为后者。 |
| `attempt_budget_secs` | 120 | 端点间重试的总时间预算。 |
| `cooldown_secs` / `auth_cooldown_secs` / `server_error_cooldown_secs` | 30 / 600 / 20 | 429 / 401-403 / 5xx 后的冷却时长。 |
| `retry_on_model_error` | true | 把「模型不存在」也当作切换信号。各家的模型 id 不同，这一步很关键。 |
| `inject_stream_usage` | false | 给流式 chat 请求加 `stream_options.include_usage`。 |
| `quota_refresh_secs` | 60 | 后台额度轮询间隔（端点自己的配置优先）。 |

## 兼容层

默认零改写：除 `model` 外，请求体原样透传，协议头原样转发。只在某个上游确实需要时才开。

| 字段 | 说明 |
| --- | --- |
| `developer_role_to_system` | 改写 `developer` 角色。DeepSeek 系接口不接受它。 |
| `max_completion_tokens_to_max_tokens` | 仅 chat 协议的重命名。 |
| `drop_params` | 所有端点都要丢弃的字段，例如 `store`、`service_tier`。 |
| `forward_headers` | 额外要原样转发的客户端头。 |

其余参数（`tools`、`tool_choice`、`response_format`、`temperature`、`thinking`、
`reasoning_effort`、各类 beta 头……）全部透传而不做白名单过滤，避免新参数被吞掉。

## 服务与日志

| 字段 | 默认 | 说明 |
| --- | --- | --- |
| `server.host` / `port` | `127.0.0.1` / 8787 | 监听地址。除非同时配了 `client_keys`，否则保持回环地址。 |
| `server.client_keys` | 空 | 填了之后客户端必须带 `Authorization: Bearer <key>`；管理接口同样要求。`/health` 不受限制。 |
| `server.max_connections` | 64 | 超出的连接直接 503。 |
| `server.max_body_bytes` | 64 MiB | 超出返回 413。 |
| `log.level` | `info` | `error` < `warn` < `info` < `debug` < `trace`。排查选路问题时用 `debug`。 |
| `log.file` | 无 | 同时写文件。相对路径解析到二进制所在目录——服务启动时的工作目录是 SCM 目录。 |
