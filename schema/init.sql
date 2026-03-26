-- `orders` 是账户级订单事件事实表，只接收用户 WS 推送的真实订单事件。
-- 这里不写本地下单请求结果，也不承担策略归因。
CREATE TABLE IF NOT EXISTS orders (
    -- Polymarket 订单 ID。
    order_id TEXT PRIMARY KEY,
    -- 交易所订单方向，使用 buy / sell。
    side TEXT NOT NULL,
    -- 订单价格。
    price TEXT NOT NULL,
    -- 订单原始数量。
    size TEXT NOT NULL,
    -- 当前订单状态，例如 live / matched / canceled。
    status TEXT NOT NULL,
    -- 本地最后一次写入时间，不代表交易所事件原始时间。
    created_at INTEGER NOT NULL
);

-- `trades` 是账户级成交事实表，只接收用户 WS 推送的真实成交。
-- 这里同样不直接承担策略归因，策略侧归因单独放在 `strategy` 表。
CREATE TABLE IF NOT EXISTS trades (
    -- 本地成交事件主键。当前直接使用 fill id，保证同一成交不重复写入。
    id TEXT PRIMARY KEY,
    -- 关联订单 ID；某些成交场景可能拿不到，所以允许为空。
    order_id TEXT,
    -- Polymarket 成交 ID，唯一。
    trade_id TEXT NOT NULL UNIQUE,
    -- 成交 token 的 asset id。
    asset_id TEXT NOT NULL,
    -- 账户视角下的成交方向，使用 buy / sell。
    side TEXT NOT NULL,
    -- 成交价格。
    price TEXT NOT NULL,
    -- 成交数量。
    size TEXT NOT NULL,
    -- 手续费 bps；上游没有时为空。
    fee_bps TEXT,
    -- 交易所成交时间；来源于 WS trade/fill 时间戳。
    event_time INTEGER,
    -- 本地写入时间。
    created_at INTEGER NOT NULL
);

-- `strategy` 是策略归因表，一行对应一笔由策略主动提交成功的订单。
-- 它负责回答“这笔单为什么下”，而不是回答“账户后来发生了什么”。
CREATE TABLE IF NOT EXISTS strategy (
    -- Polymarket 订单 ID；也是和 `orders/trades` 做关联的主键。
    order_id TEXT PRIMARY KEY,
    -- 策略名，例如 `crypto_reversal`。
    strategy TEXT NOT NULL,
    -- 基础资产，例如 btc / eth。
    symbol TEXT NOT NULL,
    -- 触发这笔单的策略周期，例如 5m / 15m。
    interval TEXT NOT NULL,
    -- 下单对应的 Polymarket market slug。
    market_slug TEXT NOT NULL,
    -- 策略方向，使用 up / down。
    side TEXT NOT NULL,
    -- 本地归因记录写入时间。
    created_at INTEGER NOT NULL,
    -- JSON 事件快照，保存当时的触发内容和策略条件。
    event TEXT NOT NULL
);

-- 订单事实表最常见的查询入口：按订单 ID 回查该订单的所有状态变化。
CREATE INDEX IF NOT EXISTS idx_orders_order_id
    ON orders(order_id);

-- 成交事实表常按订单归集，便于和 `strategy.order_id` 对齐。
CREATE INDEX IF NOT EXISTS idx_trades_order_id
    ON trades(order_id);

-- 成交 ID 是天然唯一键，这里额外建索引是为了显式表达常见查询路径。
CREATE INDEX IF NOT EXISTS idx_trades_trade_id
    ON trades(trade_id);
