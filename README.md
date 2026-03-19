# polymarket-ltf

这是一个基于 Rust 的实时数据项目，当前围绕 Polymarket 市场发现、订单簿订阅、RTDS Chainlink 价格、Binance 参考价格以及 CSV 快照写入展开。

## 模块说明

- `src/types/crypto.rs`：跨 Binance / Polymarket / RTDS 共用的 `Symbol` 与 `Interval` 类型。
- `src/binance/websocket.rs`：Binance `bookTicker` 订阅与本地缓存。
- `src/polymarket/market_registry.rs`：活跃市场发现、本地注册表和订阅调度。
- `src/polymarket/orderbook_stream.rs`：Polymarket 订单簿流与本地盘口缓存。
- `src/polymarket/rtds_stream.rs`：Polymarket RTDS Chainlink 价格流。
- `src/snapshot.rs`：snapshot 计算与 CSV 追加写入。

## 开发命令

```bash
cargo check
cargo fmt
cargo test
```

日志通过 `RUST_LOG` 控制，例如：

```bash
RUST_LOG=info cargo run --example book_monitor
RUST_LOG=debug cargo run --example snapshot_write
```

## 示例

```bash
cargo run --example book_monitor
cargo run --example gamma_market_by_slug
cargo run --example snapshot_write
cargo run --example snapshot_write -- btc 5m data/snapshots
cargo run --example clob_ok
```

- `book_monitor`：刷新 market registry、调度 Polymarket 订阅，并打印当前盘口。
- `gamma_market_by_slug`：生成当前和下一个 market slug，并查询对应的 Gamma 市场详情。
- `snapshot_write`：启动 Binance、RTDS、market registry 和 orderbook stream，并持续将 snapshot 追加写入 CSV。
- `clob_ok`：最小 CLOB 健康检查。

## Snapshot CSV

当前 snapshot 列为：

```text
timestamp,binance_mid_price,chainlink_price,spread_binance_chainlink,spread_delta,chainlink_start_delta,up_bid_price,up_bid_size,up_ask_price,up_ask_size,down_bid_price,down_bid_size,down_ask_price,down_ask_size,z_score,vel_spread,up_mid_price_slope,binance_sigma,chainlink_change_30s_pct,chainlink_change_60s_pct,chainlink_run
```

字段说明：

- `timestamp`：当前 snapshot 写入时间，格式到毫秒。
- `binance_mid_price`：Binance `bookTicker` 中间价，公式为 `(best_bid + best_ask) / 2`。
- `chainlink_price`：当前最新的 Chainlink 价格。
- `spread_binance_chainlink`：Binance 中间价与 Chainlink 价格差，公式为 `binance_mid_price - chainlink_price`。
- `spread_delta`：当前 spread 相对上一次 snapshot 的变化量。
- `chainlink_start_delta`：当前 Chainlink 价格相对当前 market 周期起点的变化量。
- `up_bid_price`：当前 market 的 `up_asset_id` 最优买价。
- `up_bid_size`：当前 market 的 `up_asset_id` 最优买量。
- `up_ask_price`：当前 market 的 `up_asset_id` 最优卖价。
- `up_ask_size`：当前 market 的 `up_asset_id` 最优卖量。
- `down_bid_price`：当前 market 的 `down_asset_id` 最优买价。
- `down_bid_size`：当前 market 的 `down_asset_id` 最优买量。
- `down_ask_price`：当前 market 的 `down_asset_id` 最优卖价。
- `down_ask_size`：当前 market 的 `down_asset_id` 最优卖量。
- `z_score`：当前 spread 在滚动窗口内的标准分数，窗口为 `120s`。
- `vel_spread`：spread 的短周期变化速度，当前窗口为 `5s`。
- `up_mid_price_slope`：`up` 方向盘口中间价斜率，当前窗口为 `5s`。
- `binance_sigma`：Binance 中间价在滚动窗口内的标准差，窗口为 `30s`。
- `chainlink_change_30s_pct`：Chainlink 价格相对 30 秒前的百分比变化。
- `chainlink_change_60s_pct`：Chainlink 价格相对 60 秒前的百分比变化。
- `chainlink_run`：Chainlink 价格连续同方向变动次数。

CSV 默认按以下目录结构写入：

```text
data/snapshots/<symbol>/<interval>/<market_slug>.csv
```
