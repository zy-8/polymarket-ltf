//! Polymarket 高效二元市场订单簿模型。
//!
//! 目标：
//! - 只按 `asset_id` 检索；
//! - 利用二元市场互补关系，只存一份规范化订单簿；
//! - 高频读取时尽量避免整本克隆；
//! - 最常用的 top-of-book 查询尽量走缓存。
//!
//! 当前实现的核心思路：
//! 1. 一个二元市场只保存 `up_asset_id` 视角下的一份 canonical book：
//!    - `up_bids`
//!    - `up_asks`
//! 2. 查询 `down_asset_id` 时，不复制存储，只在读取时做镜像：
//!    - `down_bid = 1 - up_ask`
//!    - `down_ask = 1 - up_bid`
//! 3. `OrderBooks` 内部维护 `asset_id -> (anchor, slot)` 索引，
//!    所以可以直接按 asset 找到所属二元市场与视角。
//! 4. `BinaryOrderBook` 缓存 canonical 视角的 best bid / best ask，
//!    这样 top-of-book 读取不需要每次遍历整本簿。
//!
//! 这个模型刻意保留最小但高频友好的接口：
//! - `BinaryOrderBook`：单个二元市场
//! - `OrderBooks`：多个二元市场集合
//! - `get(asset_id)`：返回零拷贝只读视图 `BookView`
//! - `replace(asset_id, bids, asks)`：整本替换
//! - `set_level(asset_id, side, price, size)`：单档更新

use crate::errors::{PolyfillError, Result};
pub use polymarket_client_sdk_v2::clob::types::Side;
use polymarket_client_sdk_v2::types::U256;
use rust_decimal::Decimal;
use std::collections::{BTreeMap, HashMap};

pub type AssetId = U256;

/// 单个价格档位。
///
/// 这里直接用 `(price, size)` 作为最小表达。
/// `Decimal` 便于无损表达 Polymarket 的概率价格和数量。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Level {
    pub price: Decimal,
    pub size: Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CanonicalLevelUpdate {
    pub asset_id: AssetId,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
}

impl Level {
    pub fn new(price: Decimal, size: Decimal) -> Self {
        Self { price, size }
    }
}

/// 二元市场内部的 asset 槽位。
///
/// 这里只是实现细节，不暴露 `yes/no` 语义，也不要求调用方关心哪边是 `up/down`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AssetSlot {
    Up,
    Down,
}

/// asset 直达索引。
///
/// `book_idx` 指向该 asset 所属二元市场在集合中的位置。
/// `slot` 指出这个 asset 在该市场里是 `up` 还是 `down`。
#[derive(Debug, Clone, Copy)]
struct AssetLookup {
    book_idx: usize,
    slot: AssetSlot,
}

/// 单个二元市场的高效订单簿。
///
/// 内部只保存 `up_asset_id` 视角下的 canonical 数据：
/// - 买盘：`up_bids`
/// - 卖盘：`up_asks`
///
/// `down_asset_id` 视角下的订单簿永远通过镜像推导，不额外存第二份。
#[derive(Debug, Clone)]
pub struct BinaryOrderBook {
    up_asset_id: AssetId,
    down_asset_id: AssetId,
    up_bids: BTreeMap<Decimal, Decimal>,
    up_asks: BTreeMap<Decimal, Decimal>,
    best_up_bid: Option<Level>,
    best_up_ask: Option<Level>,
}

impl BinaryOrderBook {
    pub fn new(up_asset_id: U256, down_asset_id: U256) -> Result<Self> {
        if up_asset_id == down_asset_id {
            return Err(PolyfillError::validation(
                "二元市场的两个 asset_id 不能相同",
            ));
        }

        Ok(Self {
            up_asset_id,
            down_asset_id,
            up_bids: BTreeMap::new(),
            up_asks: BTreeMap::new(),
            best_up_bid: None,
            best_up_ask: None,
        })
    }

    pub fn asset_ids(&self) -> (&U256, &U256) {
        (&self.up_asset_id, &self.down_asset_id)
    }

    pub fn contains(&self, asset_id: &U256) -> bool {
        *asset_id == self.up_asset_id || *asset_id == self.down_asset_id
    }

    /// 返回指定 asset 的只读视图。
    ///
    /// 这个视图是零拷贝的，不会把整本簿 materialize 成一个新的 owned 结构。
    pub fn get(&self, asset_id: &U256) -> Option<BookView<'_>> {
        let slot = self.slot(asset_id)?;
        Some(BookView { book: self, slot })
    }

    /// 按某个 asset 的视角整本替换订单簿。
    ///
    /// 如果传入的是 `down_asset_id` 视角的 bids / asks，内部会先镜像回 `up_asset_id`
    /// 视角再落盘。
    pub fn replace(&mut self, asset_id: &U256, bids: Vec<Level>, asks: Vec<Level>) -> Result<()> {
        self.replace_from_iters(asset_id, bids, asks)
    }

    /// 按某个 asset 的视角更新单个价格档位。
    ///
    /// 约定：
    /// - `size = 0` 表示删除档位
    /// - `size > 0` 表示插入或覆盖档位
    ///
    /// 这里直接复用官方 SDK 的 `Side`：
    /// - `Side::Buy` 对应订单簿买盘
    /// - `Side::Sell` 对应订单簿卖盘
    pub fn set_level(
        &mut self,
        asset_id: &U256,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> Result<()> {
        let side = validate_side(side)?;

        // 如果更新来自 down_asset_id，需要把方向和价格都镜像回 canonical 视角。
        let (canonical_side, canonical_price) = match self.resolve(asset_id)? {
            AssetSlot::Up => (side, price),
            AssetSlot::Down => (mirror_side(side)?, mirror_price(price)),
        };

        self.set_canonical(canonical_side, canonical_price, size)
    }

    fn replace_from_iters<I, J>(&mut self, asset_id: &U256, bids: I, asks: J) -> Result<()>
    where
        I: IntoIterator<Item = Level>,
        J: IntoIterator<Item = Level>,
    {
        match self.resolve(asset_id)? {
            AssetSlot::Up => {
                self.up_bids = levels_to_map(bids);
                self.up_asks = levels_to_map(asks);
            }
            AssetSlot::Down => {
                self.up_bids = mirrored_asks_to_bids(asks);
                self.up_asks = mirrored_bids_to_asks(bids);
            }
        }

        self.refresh_best_quotes();
        Ok(())
    }

    // 这里直接写 canonical `up_asset_id` 视角，不再做镜像转换。
    // 调用方如果来自 `down_asset_id`，应先在外层完成归一化。
    fn set_canonical(&mut self, side: Side, price: Decimal, size: Decimal) -> Result<()> {
        let side = validate_side(side)?;
        let levels = match side {
            Side::Buy => &mut self.up_bids,
            Side::Sell => &mut self.up_asks,
            Side::Unknown => unreachable!("unknown side already validated"),
            _ => unreachable!("future sdk side variant already rejected"),
        };

        if size.is_zero() {
            levels.remove(&price);
        } else {
            levels.insert(price, size);
        }

        self.refresh_best_quote(side);
        Ok(())
    }

    fn refresh_best_quotes(&mut self) {
        self.best_up_bid = top_bid(&self.up_bids);
        self.best_up_ask = top_ask(&self.up_asks);
    }

    fn refresh_best_quote(&mut self, side: Side) {
        match side {
            Side::Buy => self.best_up_bid = top_bid(&self.up_bids),
            Side::Sell => self.best_up_ask = top_ask(&self.up_asks),
            Side::Unknown => unreachable!("unknown side already validated"),
            _ => unreachable!("future sdk side variant already rejected"),
        }
    }

    fn resolve(&self, asset_id: &U256) -> Result<AssetSlot> {
        self.slot(asset_id)
            .ok_or_else(|| PolyfillError::validation(format!("未找到 asset_id {}", asset_id)))
    }

    fn slot(&self, asset_id: &U256) -> Option<AssetSlot> {
        if *asset_id == self.up_asset_id {
            Some(AssetSlot::Up)
        } else if *asset_id == self.down_asset_id {
            Some(AssetSlot::Down)
        } else {
            None
        }
    }
}

/// 指定 asset 视角下的只读订单簿。
///
/// 这是高频读取的关键结构：
/// - 不复制整本簿
/// - best bid / ask / mid / spread 都可以直接基于缓存或镜像计算
/// - 只有在调用 `bids(limit)` / `asks(limit)` 时，才会按需生成一小段 owned 数据
pub struct BookView<'a> {
    book: &'a BinaryOrderBook,
    slot: AssetSlot,
}

impl<'a> BookView<'a> {
    pub fn asset_id(&self) -> &'a U256 {
        match self.slot {
            AssetSlot::Up => &self.book.up_asset_id,
            AssetSlot::Down => &self.book.down_asset_id,
        }
    }

    pub fn other_asset_id(&self) -> &'a U256 {
        match self.slot {
            AssetSlot::Up => &self.book.down_asset_id,
            AssetSlot::Down => &self.book.up_asset_id,
        }
    }

    pub fn best_bid(&self) -> Option<Level> {
        match self.slot {
            AssetSlot::Up => self.book.best_up_bid,
            AssetSlot::Down => self.book.best_up_ask.map(mirror_level),
        }
    }

    pub fn best_ask(&self) -> Option<Level> {
        match self.slot {
            AssetSlot::Up => self.book.best_up_ask,
            AssetSlot::Down => self.book.best_up_bid.map(mirror_level),
        }
    }

    pub fn mid(&self) -> Option<Decimal> {
        Some((self.best_bid()?.price + self.best_ask()?.price) / Decimal::from(2u32))
    }

    pub fn spread(&self) -> Option<Decimal> {
        Some(self.best_ask()?.price - self.best_bid()?.price)
    }

    /// 返回前 `limit` 档买盘。
    ///
    /// 这里会按需分配一个小 `Vec`，但只复制需要的前几档，而不是整本克隆。
    pub fn bids(&self, limit: usize) -> Vec<Level> {
        match self.slot {
            AssetSlot::Up => collect_bids(&self.book.up_bids, limit),
            AssetSlot::Down => collect_mirrored_bids(&self.book.up_asks, limit),
        }
    }

    /// 返回前 `limit` 档卖盘。
    pub fn asks(&self, limit: usize) -> Vec<Level> {
        match self.slot {
            AssetSlot::Up => collect_asks(&self.book.up_asks, limit),
            AssetSlot::Down => collect_mirrored_asks(&self.book.up_bids, limit),
        }
    }

    /// 无分配地遍历前 `limit` 档买盘。
    ///
    /// 如果调用方只是做统计或打印，这个接口比 `bids(limit)` 更省。
    pub fn for_each_bid(&self, limit: usize, mut f: impl FnMut(Level)) {
        match self.slot {
            AssetSlot::Up => {
                for (price, size) in self.book.up_bids.iter().rev().take(limit) {
                    f(Level::new(*price, *size));
                }
            }
            AssetSlot::Down => {
                for (price, size) in self.book.up_asks.iter().take(limit) {
                    f(Level::new(mirror_price(*price), *size));
                }
            }
        }
    }

    /// 无分配地遍历前 `limit` 档卖盘。
    pub fn for_each_ask(&self, limit: usize, mut f: impl FnMut(Level)) {
        match self.slot {
            AssetSlot::Up => {
                for (price, size) in self.book.up_asks.iter().take(limit) {
                    f(Level::new(*price, *size));
                }
            }
            AssetSlot::Down => {
                for (price, size) in self.book.up_bids.iter().rev().take(limit) {
                    f(Level::new(mirror_price(*price), *size));
                }
            }
        }
    }
}

/// 多个二元市场的高效集合。
///
/// 内部有两个索引：
/// - `books`: 顺序存储每个二元市场
/// - `asset_index`: 任意 asset_id 直达所属市场和槽位
///
/// 外部只按 asset 维度操作。
#[derive(Debug, Default, Clone)]
pub struct OrderBooks {
    books: Vec<BinaryOrderBook>,
    asset_index: HashMap<AssetId, AssetLookup>,
}

impl OrderBooks {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个新的二元市场。
    ///
    /// 内部顺序追加到 `books`，并让两个 asset 都索引到同一个 `book_idx`。
    pub fn insert(&mut self, book: BinaryOrderBook) -> Result<()> {
        let up_asset_id = book.up_asset_id;
        let down_asset_id = book.down_asset_id;

        if let Some(existing) = self.asset_index.get(&up_asset_id) {
            return Err(PolyfillError::validation(format!(
                "asset_id {} 已绑定到二元市场索引 {}",
                up_asset_id, existing.book_idx
            )));
        }

        if let Some(existing) = self.asset_index.get(&down_asset_id) {
            return Err(PolyfillError::validation(format!(
                "asset_id {} 已绑定到二元市场索引 {}",
                down_asset_id, existing.book_idx
            )));
        }

        let book_idx = self.books.len();

        self.asset_index.insert(
            up_asset_id,
            AssetLookup {
                book_idx,
                slot: AssetSlot::Up,
            },
        );
        self.asset_index.insert(
            down_asset_id,
            AssetLookup {
                book_idx,
                slot: AssetSlot::Down,
            },
        );
        self.books.push(book);

        Ok(())
    }

    /// 返回指定 asset 的只读视图。
    ///
    /// 这是零拷贝查询入口，适合高频读取。
    pub fn get(&self, asset_id: &U256) -> Option<BookView<'_>> {
        let lookup = self.asset_index.get(asset_id)?;
        let book = self.books.get(lookup.book_idx)?;
        Some(BookView {
            book,
            slot: lookup.slot,
        })
    }

    pub fn replace(&mut self, asset_id: &U256, bids: Vec<Level>, asks: Vec<Level>) -> Result<()> {
        self.replace_from_iters(asset_id, bids, asks)
    }

    pub(crate) fn replace_from_iters<I, J>(
        &mut self,
        asset_id: &U256,
        bids: I,
        asks: J,
    ) -> Result<()>
    where
        I: IntoIterator<Item = Level>,
        J: IntoIterator<Item = Level>,
    {
        let book_idx = self
            .asset_index
            .get(asset_id)
            .copied()
            .map(|lookup| lookup.book_idx)
            .ok_or_else(|| PolyfillError::validation(format!("未找到 asset_id {}", asset_id)))?;

        let book = self.books.get_mut(book_idx).ok_or_else(|| {
            PolyfillError::internal_simple(format!("未找到 asset_id {} 对应的订单簿", asset_id))
        })?;

        book.replace_from_iters(asset_id, bids, asks)
    }

    pub fn set_level(
        &mut self,
        asset_id: &U256,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> Result<()> {
        let book_idx = self
            .asset_index
            .get(asset_id)
            .copied()
            .map(|lookup| lookup.book_idx)
            .ok_or_else(|| PolyfillError::validation(format!("未找到 asset_id {}", asset_id)))?;

        let book = self.books.get_mut(book_idx).ok_or_else(|| {
            PolyfillError::internal_simple(format!("未找到 asset_id {} 对应的订单簿", asset_id))
        })?;

        book.set_level(asset_id, side, price, size)
    }

    /// 直接应用一条 canonical 视角的档位更新。
    ///
    /// 调用方必须保证 `asset_id` 已经是 `up_asset_id`，这里不再做镜像归一化。
    pub(crate) fn apply_canonical(
        &mut self,
        asset_id: &U256,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> Result<()> {
        let lookup =
            self.asset_index.get(asset_id).copied().ok_or_else(|| {
                PolyfillError::validation(format!("未找到 asset_id {}", asset_id))
            })?;

        if lookup.slot != AssetSlot::Up {
            return Err(PolyfillError::validation(format!(
                "canonical 更新必须指向 up_asset_id: {}",
                asset_id
            )));
        }

        let book = self.books.get_mut(lookup.book_idx).ok_or_else(|| {
            PolyfillError::internal_simple(format!("未找到 asset_id {} 对应的订单簿", asset_id))
        })?;

        book.set_canonical(side, price, size)
    }

    /// 把任意 asset 视角下的一条档位更新归一化到 `up_asset_id` 视角。
    ///
    /// 这个方法只做坐标变换，不修改订单簿，适合在 fast path 里先判断两条更新
    /// 是否实际上指向同一个 canonical level。
    pub fn normalize_level(
        &self,
        asset_id: &U256,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> Result<CanonicalLevelUpdate> {
        let lookup =
            self.asset_index.get(asset_id).copied().ok_or_else(|| {
                PolyfillError::validation(format!("未找到 asset_id {}", asset_id))
            })?;

        let book = self.books.get(lookup.book_idx).ok_or_else(|| {
            PolyfillError::internal_simple(format!("未找到 asset_id {} 对应的订单簿", asset_id))
        })?;
        let side = validate_side(side)?;

        let (side, price) = match lookup.slot {
            AssetSlot::Up => (side, price),
            AssetSlot::Down => (mirror_side(side)?, mirror_price(price)),
        };

        Ok(CanonicalLevelUpdate {
            asset_id: book.up_asset_id,
            side,
            price,
            size,
        })
    }

    /// 高频场景下的便捷方法：直接返回 best bid。
    pub fn best_bid(&self, asset_id: &U256) -> Option<Level> {
        self.get(asset_id)?.best_bid()
    }

    pub fn best_ask(&self, asset_id: &U256) -> Option<Level> {
        self.get(asset_id)?.best_ask()
    }

    pub fn mid(&self, asset_id: &U256) -> Option<Decimal> {
        self.get(asset_id)?.mid()
    }

    pub fn spread(&self, asset_id: &U256) -> Option<Decimal> {
        self.get(asset_id)?.spread()
    }

    pub fn len(&self) -> usize {
        self.books.len()
    }

    pub fn is_empty(&self) -> bool {
        self.books.is_empty()
    }

    pub fn remove(&mut self, asset_id: &U256) -> Option<[U256; 2]> {
        let lookup = self.asset_index.get(asset_id).copied()?;
        let removed = self.books.swap_remove(lookup.book_idx);
        let removed_pair = [removed.up_asset_id, removed.down_asset_id];

        self.asset_index.remove(&removed_pair[0]);
        self.asset_index.remove(&removed_pair[1]);

        if let Some(moved) = self.books.get(lookup.book_idx) {
            self.asset_index.insert(
                moved.up_asset_id,
                AssetLookup {
                    book_idx: lookup.book_idx,
                    slot: AssetSlot::Up,
                },
            );
            self.asset_index.insert(
                moved.down_asset_id,
                AssetLookup {
                    book_idx: lookup.book_idx,
                    slot: AssetSlot::Down,
                },
            );
        }

        Some(removed_pair)
    }
}

fn top_bid(levels: &BTreeMap<Decimal, Decimal>) -> Option<Level> {
    levels
        .iter()
        .next_back()
        .map(|(price, size)| Level::new(*price, *size))
}

fn top_ask(levels: &BTreeMap<Decimal, Decimal>) -> Option<Level> {
    levels
        .iter()
        .next()
        .map(|(price, size)| Level::new(*price, *size))
}

fn levels_to_map(levels: impl IntoIterator<Item = Level>) -> BTreeMap<Decimal, Decimal> {
    let mut map = BTreeMap::new();

    for level in levels {
        if !level.size.is_zero() {
            map.insert(level.price, level.size);
        }
    }

    map
}

fn collect_bids(levels: &BTreeMap<Decimal, Decimal>, limit: usize) -> Vec<Level> {
    levels
        .iter()
        .rev()
        .take(limit)
        .map(|(price, size)| Level::new(*price, *size))
        .collect()
}

fn collect_asks(levels: &BTreeMap<Decimal, Decimal>, limit: usize) -> Vec<Level> {
    levels
        .iter()
        .take(limit)
        .map(|(price, size)| Level::new(*price, *size))
        .collect()
}

fn collect_mirrored_bids(levels: &BTreeMap<Decimal, Decimal>, limit: usize) -> Vec<Level> {
    levels
        .iter()
        .take(limit)
        .map(|(price, size)| Level::new(mirror_price(*price), *size))
        .collect()
}

fn collect_mirrored_asks(levels: &BTreeMap<Decimal, Decimal>, limit: usize) -> Vec<Level> {
    levels
        .iter()
        .rev()
        .take(limit)
        .map(|(price, size)| Level::new(mirror_price(*price), *size))
        .collect()
}

fn mirrored_asks_to_bids(asks: impl IntoIterator<Item = Level>) -> BTreeMap<Decimal, Decimal> {
    let mut map = BTreeMap::new();

    for level in asks {
        if !level.size.is_zero() {
            map.insert(mirror_price(level.price), level.size);
        }
    }

    map
}

fn mirrored_bids_to_asks(bids: impl IntoIterator<Item = Level>) -> BTreeMap<Decimal, Decimal> {
    let mut map = BTreeMap::new();

    for level in bids {
        if !level.size.is_zero() {
            map.insert(mirror_price(level.price), level.size);
        }
    }

    map
}

fn mirror_level(level: Level) -> Level {
    Level::new(mirror_price(level.price), level.size)
}

fn mirror_price(price: Decimal) -> Decimal {
    Decimal::ONE - price
}

fn validate_side(side: Side) -> Result<Side> {
    match side {
        Side::Buy | Side::Sell => Ok(side),
        Side::Unknown => Err(PolyfillError::validation("订单簿更新方向不能是 UNKNOWN")),
        _ => Err(PolyfillError::validation(
            "订单簿更新方向不是受支持的 SDK Side",
        )),
    }
}

fn mirror_side(side: Side) -> Result<Side> {
    match validate_side(side)? {
        Side::Buy => Ok(Side::Sell),
        Side::Sell => Ok(Side::Buy),
        Side::Unknown => unreachable!("unknown side already validated"),
        _ => unreachable!("future sdk side variant already rejected"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: u64) -> U256 {
        U256::from(id)
    }

    #[test]
    fn test_binary_order_book_can_read_up_asset_directly() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        book.replace(
            &asset(1),
            vec![
                Level::new(Decimal::new(45, 2), Decimal::new(100, 0)),
                Level::new(Decimal::new(47, 2), Decimal::new(200, 0)),
            ],
            vec![
                Level::new(Decimal::new(50, 2), Decimal::new(120, 0)),
                Level::new(Decimal::new(52, 2), Decimal::new(150, 0)),
            ],
        )
        .unwrap();

        let view = book.get(&asset(1)).unwrap();

        assert_eq!(
            view.best_bid(),
            Some(Level::new(Decimal::new(47, 2), Decimal::new(200, 0)))
        );
        assert_eq!(
            view.best_ask(),
            Some(Level::new(Decimal::new(50, 2), Decimal::new(120, 0)))
        );
        assert_eq!(view.mid(), Some(Decimal::new(485, 3)));
    }

    #[test]
    fn test_binary_order_book_can_read_down_asset_as_mirror() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        book.replace(
            &asset(1),
            vec![Level::new(Decimal::new(47, 2), Decimal::new(200, 0))],
            vec![Level::new(Decimal::new(50, 2), Decimal::new(120, 0))],
        )
        .unwrap();

        let view = book.get(&asset(2)).unwrap();

        assert_eq!(
            view.best_bid(),
            Some(Level::new(Decimal::new(50, 2), Decimal::new(120, 0)))
        );
        assert_eq!(
            view.best_ask(),
            Some(Level::new(Decimal::new(53, 2), Decimal::new(200, 0)))
        );
        assert_eq!(view.mid(), Some(Decimal::new(515, 3)));
    }

    #[test]
    fn test_replace_from_down_view_is_mirrored_back_to_up() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        book.replace(
            &asset(2),
            vec![Level::new(Decimal::new(48, 2), Decimal::new(70, 0))],
            vec![Level::new(Decimal::new(55, 2), Decimal::new(90, 0))],
        )
        .unwrap();

        let asset_a = book.get(&asset(1)).unwrap();
        let asset_b = book.get(&asset(2)).unwrap();

        assert_eq!(
            asset_a.best_bid(),
            Some(Level::new(Decimal::new(45, 2), Decimal::new(90, 0)))
        );
        assert_eq!(
            asset_a.best_ask(),
            Some(Level::new(Decimal::new(52, 2), Decimal::new(70, 0)))
        );
        assert_eq!(
            asset_b.best_bid(),
            Some(Level::new(Decimal::new(48, 2), Decimal::new(70, 0)))
        );
        assert_eq!(
            asset_b.best_ask(),
            Some(Level::new(Decimal::new(55, 2), Decimal::new(90, 0)))
        );
    }

    #[test]
    fn test_set_level_from_down_side_updates_mirrored_book() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        book.set_level(
            &asset(2),
            Side::Buy,
            Decimal::new(40, 2),
            Decimal::new(100, 0),
        )
        .unwrap();

        let asset_a = book.get(&asset(1)).unwrap();
        let asset_b = book.get(&asset(2)).unwrap();

        assert_eq!(
            asset_a.best_ask(),
            Some(Level::new(Decimal::new(60, 2), Decimal::new(100, 0)))
        );
        assert_eq!(
            asset_b.best_bid(),
            Some(Level::new(Decimal::new(40, 2), Decimal::new(100, 0)))
        );
    }

    #[test]
    fn test_book_view_can_iterate_without_allocating() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        book.replace(
            &asset(1),
            vec![
                Level::new(Decimal::new(45, 2), Decimal::new(10, 0)),
                Level::new(Decimal::new(47, 2), Decimal::new(20, 0)),
            ],
            vec![Level::new(Decimal::new(50, 2), Decimal::new(30, 0))],
        )
        .unwrap();

        let view = book.get(&asset(1)).unwrap();
        let mut bids = Vec::new();

        view.for_each_bid(2, |level| bids.push(level));

        assert_eq!(
            bids,
            vec![
                Level::new(Decimal::new(47, 2), Decimal::new(20, 0)),
                Level::new(Decimal::new(45, 2), Decimal::new(10, 0))
            ]
        );
    }

    #[test]
    fn test_order_books_get_by_asset_only() {
        let mut books = OrderBooks::new();
        books
            .insert(BinaryOrderBook::new(asset(1), asset(2)).unwrap())
            .unwrap();

        books
            .replace(
                &asset(1),
                vec![Level::new(Decimal::new(44, 2), Decimal::new(100, 0))],
                vec![Level::new(Decimal::new(56, 2), Decimal::new(100, 0))],
            )
            .unwrap();

        assert!(books.get(&asset(1)).is_some());
        assert!(books.get(&asset(2)).is_some());
        assert_eq!(books.mid(&asset(1)), Some(Decimal::new(500, 3)));
        assert_eq!(books.len(), 1);
    }

    #[test]
    fn test_order_books_reject_duplicate_asset_binding() {
        let mut books = OrderBooks::new();
        books
            .insert(BinaryOrderBook::new(asset(1), asset(2)).unwrap())
            .unwrap();

        let error = books
            .insert(BinaryOrderBook::new(asset(1), asset(3)).unwrap())
            .unwrap_err();

        assert!(error.to_string().contains("asset_id 1"));
    }

    #[test]
    fn test_unknown_side_is_rejected() {
        let mut book = BinaryOrderBook::new(asset(1), asset(2)).unwrap();

        let error = book
            .set_level(
                &asset(1),
                Side::Unknown,
                Decimal::new(40, 2),
                Decimal::new(100, 0),
            )
            .unwrap_err();

        assert!(error.to_string().contains("UNKNOWN"));
    }

    #[test]
    fn test_normalize_level_can_map_down_back_to_up() {
        let mut books = OrderBooks::new();
        books
            .insert(BinaryOrderBook::new(asset(1), asset(2)).unwrap())
            .unwrap();

        let update = books
            .normalize_level(
                &asset(2),
                Side::Sell,
                Decimal::new(55, 2),
                Decimal::new(90, 0),
            )
            .unwrap();

        assert_eq!(
            update,
            CanonicalLevelUpdate {
                asset_id: asset(1),
                side: Side::Buy,
                price: Decimal::new(45, 2),
                size: Decimal::new(90, 0),
            }
        );
    }

    #[test]
    fn test_order_books_remove_can_cleanup_both_assets() {
        let mut books = OrderBooks::new();
        books
            .insert(BinaryOrderBook::new(asset(1), asset(2)).unwrap())
            .unwrap();
        books
            .insert(BinaryOrderBook::new(asset(3), asset(4)).unwrap())
            .unwrap();

        let removed = books.remove(&asset(1)).unwrap();

        assert_eq!(removed, [asset(1), asset(2)]);
        assert!(books.get(&asset(1)).is_none());
        assert!(books.get(&asset(2)).is_none());
        assert!(books.get(&asset(3)).is_some());
        assert!(books.get(&asset(4)).is_some());
        assert_eq!(books.len(), 1);
    }
}
