# Dashboard Protocol

## Scope

这份文档定义多策略实时面板的 HTTP 返回结构、字段命名规范和数据边界。

当前目标是一个以实时监控为主的 Web 面板，不是通用开放 API。
当前实现采用“HTTP info 轮询 + HTTP 按需读取”的模式：

- `/api/info`
  提供当前完整 info，前端首屏加载和定时轮询都从这里读取
- `/api/positions`
  按策略返回当前持仓明细
- `/api/open-orders`
  按策略返回当前未成交订单明细
- `/api/positions-page`
  按策略返回已结算历史记录结果

前端表格交互规则：

- 表格视窗固定展示 10 条记录高度
- 持仓和未成交订单在前端本地按 10 条一批滚动展开
- 历史记录使用 `/api/positions-page` 的 `page / page_size` 做滚动到底自动续页

## Design Rules

- 默认返回字段使用 `snake_case`
- 历史已结算记录 payload 直接复用 SQLite `positions` 子集，为兼容现有前端保留 camelCase 字段
- 所有时间字段统一使用毫秒时间戳，命名为 `*_ms`
- 价格、数量、金额统一使用字符串传输
- 枚举值统一使用小写字符串
- 市场统一使用 `market_slug`
- 策略名统一使用 `strategy`
- 订单、成交、资产 ID 保留原始字段语义，不混用

## Info

前端首屏和后续轮询都读取当前完整 info：

```json
{
  "account": {},
  "strategies": []
}
```

### `account`

`account` 是账户级全局运行状态：

```json
{
  "runtime_status": "running",
  "binance_ws_status": "connected",
  "polymarket_ws_status": "connected",
  "server_time_ms": 1710000000000,
  "open_order_count": 4,
  "position_count": 2,
  "trigger_count": 15,
  "closed_count": 6,
  "today_closed_count": 2,
  "closed_win_count": 4,
  "closed_loss_count": 2,
  "missed_count": 9,
  "missed_win_count": 5,
  "missed_loss_count": 4,
  "today_order_count": 15,
  "today_trade_count": 9,
  "today_closed_pnl_usdc": "51.77",
  "settled_pnl_usdc": "3.42",
  "last_error": null
}
```

字段规则：

- 状态类字段统一用 `*_status`
- 计数字段统一用 `*_count`
- 金额字段统一用 `*_usdc`
- 时间字段统一用 `*_ms`

状态枚举：

- `runtime_status`: `starting | running | degraded | stopped`
- `binance_ws_status`: `connecting | connected | reconnecting | disconnected`
- `polymarket_ws_status`: `connecting | connected | reconnecting | disconnected`

### `strategies`

`strategies` 是所有策略的当前摘要：

```json
{
  "strategy": "crypto_reversal",
  "status": "running",
  "outcome": "up",
  "last_scan_ms": 1710000000000,
  "last_signal_ms": 1710000001000,
  "last_order_ms": 1710000002000,
  "last_trade_ms": 1710000003000,
  "open_order_count": 2,
  "position_count": 1,
  "trigger_count": 12,
  "closed_count": 4,
  "today_closed_count": 1,
  "closed_win_count": 3,
  "closed_loss_count": 1,
  "missed_count": 8,
  "missed_win_count": 5,
  "missed_loss_count": 3,
  "today_order_count": 12,
  "today_trade_count": 8,
  "today_closed_pnl_usdc": "43.21",
  "settled_pnl_usdc": "2.18",
  "last_error": null
}
```

当前策略 info 只保留摘要字段和 `latest_signal`；
持仓、未成交订单、历史记录明细统一走独立查询接口，避免把大列表塞进 `/api/info`。

前端通过 `/api/info` 获取当前状态；当前不再维护 dashboard 专用 WebSocket 增量流。
其中 `trigger_count / closed_count / today_closed_count` 由后端在返回 info 时直接查询 SQLite 聚合，不再由前端拼装。

字段定义：

- `strategy`
  策略主键名，例如 `crypto_reversal`
- `status`
  当前策略状态：`starting | running | degraded | stopped`
- `outcome`
  最近一次已知结算方向；没有结果时为空字符串
- `last_scan_ms`
  最近一次扫描时间
- `last_signal_ms`
  最近一次产出候选时间
- `last_order_ms`
  最近一次成功下单时间
- `last_trade_ms`
  最近一次成交时间
- `open_order_count`
  当前该策略关联的活跃挂单数
- `position_count`
  当前该策略关联的持仓数
- `trigger_count`
  策略触发次数；来源于 `strategy` 表
- `closed_count`
  已关联 closed position 的数量；来源于 `positions`
- `today_closed_count`
  今日抓到的 closed position 数量；来源于 `positions.timestamp`
- `closed_win_count`
  已关联 closed position 且 `side = outcome` 的数量
- `closed_loss_count`
  已关联 closed position 且 `side != outcome` 的数量
- `missed_count`
  未关联 closed position 的数量
- `missed_win_count`
  未关联 closed position 且 `side = outcome` 的数量
- `missed_loss_count`
  未关联 closed position 且 `side != outcome` 的数量
- `today_order_count`
  今日订单数
- `today_trade_count`
  今日成交数
- `today_closed_pnl_usdc`
  今日抓到的 closed position 累计盈亏
- `settled_pnl_usdc`
  已结算累计盈亏；优先来源于 `positions.cash_pnl`
- `last_error`
  最近一次错误；没有则为 `null`

## Polling Model

dashboard 当前只对外暴露 `info` 消息形状。
运行时内部仍然按 `signal / order / trade / error` 更新状态，但这些变化由前端下一次 `/api/info` 轮询统一读取，不再单独对外推送增量事件。

## Detail APIs

### `/api/positions`

查询参数：

- `strategy`
  必填，策略名

返回结构：

```json
{
  "strategy": "crypto_reversal",
  "total": 1,
  "rows": []
}
```

`rows` 当前字段：

- `asset_id`
- `market_slug`
- `outcome`
- `size`
- `avg_price`
- `open_cost`
- `realized_pnl`
- `buy_fee_usdc`
- `buy_fee_shares`
- `sell_fee_usdc`
- `last_trade_ms`

### `/api/open-orders`

查询参数：

- `strategy`
  必填，策略名

返回结构：

```json
{
  "strategy": "crypto_reversal",
  "total": 2,
  "rows": []
}
```

`rows` 当前字段：

- `order_id`
- `market_slug`
- `side`
- `order_side`
- `status`
- `price`
- `size`
- `created_at_ms`

### `/api/positions-page`

查询参数：

- `strategy`
  必填，策略名
- `range`
  可选，`all | 1d | 1w | 1m`
- `page`
  可选，默认 `1`
- `page_size`
  可选，默认 `10`

前端当前固定使用 `page_size=10`，首屏请求第 1 页，滚动到底后继续请求后续页并追加到当前列表。

返回结构里的 `rows` 使用当前 SQLite `positions` 表已保存的关键字段：

- `proxyWallet`
- `asset`
- `conditionId`
- `marketSlug`
- `outcome`
- `avgPrice`
- `size`
- `totalBought`
- `currentValue`
- `cashPnl`
- `realizedPnl`
- `curPrice`
- `endDate`
- `timestamp`
  本地抓取该已结算快照的时间戳，单位毫秒

命名规则：

- 统一使用短名，不给事件类型叠额外语义
- 订单状态变化放在 `order.payload.status` 里表达
- 成交事实统一用 `trade`
- 右侧事件流默认由前端本地维护一个滚动窗口，不在 `info` 里补历史事件

### Signal Payload

如果界面需要显示最近一次候选，使用每个策略最近收到的 `signal` 事件：

```json
{
  "symbol": "eth",
  "interval": "5m",
  "market_slug": "eth-updown-march-24-1005",
  "side": "up",
  "signal_time_ms": 1710000000000,
  "score": 0.41,
  "size_factor": 1.5
}
```

字段定义：

- `symbol`
  基础资产，例如 `eth`
- `interval`
  策略周期，例如 `5m | 15m`
- `market_slug`
  Polymarket 市场标识
- `side`
  策略方向：`up | down`
- `signal_time_ms`
  信号时间
- `score`
  信号分数
- `size_factor`
  仓位倍率

## Payload Conventions

### Order Payload

策略视角和交易所视角的方向不能混用。
如果需要同时表达，使用两个字段：

```json
{
  "order_id": "0xabc",
  "market_slug": "eth-updown-march-24-1005",
  "side": "up",
  "order_side": "buy",
  "status": "live",
  "price": "0.51",
  "size": "8",
  "created_at_ms": 1710000002000
}
```

`order` 事件只表达订单事实，具体阶段统一放在 `status`：

- `submitted`
- `live`
- `matched`
- `canceled`

字段规则：

- `side`
  策略方向：`up | down`
- `order_side`
  交易所订单方向：`buy | sell`

### Trade Payload

```json
{
  "trade_id": "0xtrade",
  "order_id": "0xabc",
  "asset_id": "12345",
  "side": "buy",
  "price": "0.51",
  "size": "8",
  "fee_bps": "0",
  "event_time_ms": 1710000003000,
  "created_at_ms": 1710000003001
}
```

这里的 `side` 表示账户视角下的成交方向，使用 `buy | sell`。
`trade` 事件只表示已经发生的成交事实，不再额外拆成别的事件名。

### Position Payload

详情面板里的实时持仓来自进程内 `positions` 内存 info：

```json
{
  "asset_id": "12345",
  "market_slug": "eth-updown-march-24-1005",
  "outcome": "Yes",
  "size": "8",
  "avg_price": "0.43",
  "open_cost": "3.44",
  "realized_pnl": "0",
  "buy_fee_usdc": "0.01",
  "buy_fee_shares": "0.02",
  "sell_fee_usdc": "0",
  "last_trade_ms": 1710000003000
}
```

这部分只表达当前仍在内存里的活跃仓位，不走数据库重放。

### Settlement Payload

详情面板里的已结算结果来自官方 `positions(redeemable=true)` 接口，再按本地 `strategy` 归因过滤：

抓取结果会按当前本地 schema 需要的字段子集写入 SQLite `positions` 表；
当前 dashboard 面板仍然使用运行时内存结果做过滤与展示。

```json
{
  "proxyWallet": "0x0000000000000000000000000000000000000000",
  "asset": "12345",
  "conditionId": "0xabc",
  "marketSlug": "eth-updown-march-24-1005",
  "outcome": "Yes",
  "avgPrice": "0.43",
  "size": "8",
  "totalBought": "8",
  "currentValue": "1",
  "cashPnl": "0.57",
  "realizedPnl": "0.57",
  "curPrice": "0",
  "endDate": "2026-03-25T00:00:00Z",
  "timestamp": 1710003600000
}
```

其中 `timestamp` 表示本地抓取这条 closed position 快照的时间，不表示成交时间或市场结算原始时间。

面板只展示能匹配到本地 `strategy` 记录的 closed position，不展示账户里无归因的其他已结算仓位。

历史记录通过 `/api/positions-page?strategy=...` 按需读取；
`info.strategies[].settlements` 只保留一个小窗口，避免把长列表反复通过实时流传输。

### Strategy Attribution Payload

`strategy` 表中的 `event` 字段推荐固定为：

```json
{
  "signal_time_ms": 1710000000000,
  "score": 0.41,
  "size_factor": 1.5,
  "conditions": {
    "warmup_bars": 100,
    "rsi_period": 14,
    "bb_period": 30,
    "bb_stddev": 2.0,
    "macd_fast": 12,
    "macd_slow": 26,
    "macd_signal": 9,
    "min_width_pct": 0.2,
    "long_rsi_max": 40.0,
    "short_rsi_min": 60.0,
    "band_pad_pct": 0.0,
    "add_score": 0.32,
    "max_score": 0.5
  },
  "trigger": {
    "symbol": "eth",
    "interval": "5m",
    "market_slug": "eth-updown-march-24-1005",
    "side": "up"
  }
}
```

这个 JSON 的目标是回答：

- 这笔单由哪个信号触发
- 当时分数是多少
- 当时策略阈值是什么
- 当时市场和方向是什么

## Naming Constraints

以下命名禁止混用：

- 不使用 `timestamp`、`time`、`ts`
  统一改为 `*_ms`
- 不使用 `market`
  统一改为 `market_slug`
- 不使用 `strategy_name`
  统一改为 `strategy`
- 不使用 `qty`
  统一改为 `size`
- 不使用 camelCase
  全部使用 `snake_case`

## Data Boundaries

运行时数据边界固定如下：

- `orders`
  账户级订单事实表，只记录用户 WS 订单事件
- `trades`
  账户级成交事实表，只记录用户 WS 成交事件
- `strategy`
  策略归因表，记录“为什么下这笔单”

dashboard 视图再额外分成两层：

- `positions`
  进程内实时仓位 info
- `positions`
  官方接口返回的已结算仓位，再按 `strategy` 归因过滤

这几层不能重新混回一张表，也不要让 `orders/trades` 重新承担策略解释语义。
