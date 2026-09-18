# 故障排查

[English](troubleshooting.md) · [**中文**](troubleshooting-zh.md)

## 诊断选路

先看控制台策略页的**候选顺序预览**，或运行 `ar-ocg-router --plan`，
它会打印当前时刻的决策而不发送流量。`GET /router/stats` 会显示每个端点被跳过的原因。

用 `--log-level debug` 启动，可以看到每个请求的完整候选列表与跳过原因。

## 客户端收到 503 `no_account_available`

没有任何端点能服务这个请求。检查 debug 日志里的 `plan.reason` 与 `plan.skipped`：

| 原因 | 处理 |
| --- | --- |
| `no <mode> support` | 该端点的 `mode` 不覆盖正在使用的协议（`/v1/messages` 需要 `anthropic-messages`） |
| `cooldown Ns` | 最近失败过；等待或修掉根因 |
| `quota exhausted` | 套餐达到 `exhaust_at_pct`；提高上限或等窗口重置 |
| `rule not matched` | `fallback` 端点的 `rule` 当前不满足；加 `always` |
| `disabled` | 该端点是 `enabled: false` |

## 客户端收到 502 `all_accounts_failed`

所有候选都失败了，响应里会逐个列出状态。常见原因：

- **401 / 403** —— 密钥不对，或产品前缀错了。OpenCode Go 必须是
  `https://opencode.ai/zen/go/v1`，不是 `/zen/v1`。
- **400 MissingSessionID** —— OpenCode Go 端点没开 `inject_session: true`。
- **429 或额度类措辞** —— 套餐已耗尽；该端点进入冷却。
- **模型错误** —— 端点的 `model` 不是该商家提供的。各家模型 id 不同，
  见[模型库](model-library-zh.md)。

## `--selftest` 报传输错误

`os error 10013` / `WSAEACCES` / "forbidden by its access permissions" 表示出网被策略拦截，
不是上游的问题。给该可执行文件放行（见[部署](deployment-zh.md)）。session 0 里的服务
不会弹出交互提示，所以必须显式放行。部分机器按二进制产物生效，因此每次重新编译都可能
要重新加规则。

## 请求比预期慢

重试链会掩盖配置错误：如果第一个端点服务不了，客户端只会看到响应变慢。日志里找
`endpoint <名> disabled for Ns after M consecutive failures`。路由器会整段跳过这类端点，
而不是每个请求都白白付一次注定失败的尝试，并在 `skip_secs` 后恢复。

## 额度显示 `source=local` 或读数陈旧

厂商用量接口不可用、或未公开且已变更。此时改用本地记账（按 token 数累计的费用），
只影响闲时充裕判断的精度。`quota.last_error` 记录上次探测失败的原因。

## 配置改动没有生效

文件每 3 秒被检测一次，正常情况下会自动重载。如果没有，说明解析失败了：`--check`
会打印错误，`GET /router/stats` 的 warnings 数组也有。顶层段落名写错
（例如把 `plans:` 写成 `opencodego:`）会报 `unknown top-level section`，其余照常忽略。

## 仪表盘数字看起来不对

- `savings.opencodego_saved_usd` 是「走预付端点的流量」按当时忙/闲单价**本应花掉的现金**——
  估算值，不是账单。
- `fallback_spent_usd` 是现金账号**实际被扣掉**的金额。
- `cold_starts` 统计成功请求中 prompt 未命中上游缓存的次数；当会话在端点之间跳动时，
  这是最该盯的一个数。
- 这些计数是**累计值**，不是「本次运行」的值：它们会写进状态文件，重启服务不会清零。
  清零只有一个入口——总览页的**重置数据**：它清空全局计数、每个端点的统计和端点失败记忆，
  并立即写入状态文件。本地额度记账**不受重置影响**：它参与选路，清掉会让套餐看起来没用过，
  于是被优先消耗。
- 如果一个计数完全不动，先确认标签页没有被浏览器挂起，再看这次请求是否真的被端点服务
  （响应头 `x-router-account`）。

## 卸载

Windows 用 `service.ps1 uninstall`，Linux 用 `install.sh uninstall`：会移除服务与二进制，
保留配置与日志。状态文件 `ar-ocg-router.state.json` 只是辅助信息，可以安全删除——
但删掉统计也会一起归零，因为数据就存在那里。
不可读或版本不符的文件会被改名隔离为 `.corrupt` 并重建。
