use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use polymarket_client_sdk_v2::types::U256;
use polymarket_ltf::polymarket::types::orderbook::{BinaryOrderBook, Level, OrderBooks, Side};
use rust_decimal::Decimal;
use std::sync::RwLock;

const UP_ASSET_ID: u64 = 1;
const DOWN_ASSET_ID: u64 = 2;

#[derive(Clone, Copy)]
struct RawLevelUpdate {
    asset_id: U256,
    side: Side,
    price: Decimal,
    size: Decimal,
}

fn asset(id: u64) -> U256 {
    U256::from(id)
}

fn bid_price(index: usize) -> Decimal {
    Decimal::new(4_990 - index as i64, 4)
}

fn ask_price(index: usize) -> Decimal {
    Decimal::new(5_010 + index as i64, 4)
}

fn build_books(levels_per_side: usize) -> OrderBooks {
    let mut books = OrderBooks::new();
    books
        .insert(BinaryOrderBook::new(asset(UP_ASSET_ID), asset(DOWN_ASSET_ID)).unwrap())
        .unwrap();

    let bids = (0..levels_per_side)
        .map(|index| Level::new(bid_price(index), Decimal::new(1_000 + index as i64, 0)))
        .collect::<Vec<_>>();
    let asks = (0..levels_per_side)
        .map(|index| Level::new(ask_price(index), Decimal::new(2_000 + index as i64, 0)))
        .collect::<Vec<_>>();

    books.replace(&asset(UP_ASSET_ID), bids, asks).unwrap();
    books
}

fn build_price_change_updates(levels_per_side: usize) -> Vec<RawLevelUpdate> {
    let mut updates = Vec::with_capacity(levels_per_side * 4);

    for index in 0..levels_per_side {
        let bid = bid_price(index);
        let ask = ask_price(index);
        let bid_size = Decimal::new(3_000 + index as i64, 0);
        let ask_size = Decimal::new(4_000 + index as i64, 0);

        updates.push(RawLevelUpdate {
            asset_id: asset(UP_ASSET_ID),
            side: Side::Buy,
            price: bid,
            size: bid_size,
        });
        updates.push(RawLevelUpdate {
            asset_id: asset(DOWN_ASSET_ID),
            side: Side::Sell,
            price: Decimal::ONE - bid,
            size: ask_size,
        });
        updates.push(RawLevelUpdate {
            asset_id: asset(UP_ASSET_ID),
            side: Side::Sell,
            price: ask,
            size: ask_size,
        });
        updates.push(RawLevelUpdate {
            asset_id: asset(DOWN_ASSET_ID),
            side: Side::Buy,
            price: Decimal::ONE - ask,
            size: bid_size,
        });
    }

    updates
}

fn build_canonical_updates(levels_per_side: usize) -> Vec<RawLevelUpdate> {
    let mut updates = Vec::with_capacity(levels_per_side * 2);

    for index in 0..levels_per_side {
        updates.push(RawLevelUpdate {
            asset_id: asset(UP_ASSET_ID),
            side: Side::Buy,
            price: bid_price(index),
            size: Decimal::new(3_000 + index as i64, 0),
        });
        updates.push(RawLevelUpdate {
            asset_id: asset(UP_ASSET_ID),
            side: Side::Sell,
            price: ask_price(index),
            size: Decimal::new(4_000 + index as i64, 0),
        });
    }

    updates
}

fn build_mirrored_pair_updates() -> Vec<RawLevelUpdate> {
    vec![
        RawLevelUpdate {
            asset_id: asset(UP_ASSET_ID),
            side: Side::Buy,
            price: Decimal::new(77, 2),
            size: Decimal::new(212, 0),
        },
        RawLevelUpdate {
            asset_id: asset(DOWN_ASSET_ID),
            side: Side::Sell,
            price: Decimal::new(23, 2),
            size: Decimal::new(212, 0),
        },
    ]
}

fn build_mirrored_updates(levels_per_side: usize) -> Vec<RawLevelUpdate> {
    let mut updates = Vec::with_capacity(levels_per_side * 2);

    for index in 0..levels_per_side {
        updates.push(RawLevelUpdate {
            asset_id: asset(DOWN_ASSET_ID),
            side: Side::Sell,
            price: Decimal::ONE - bid_price(index),
            size: Decimal::new(3_000 + index as i64, 0),
        });
        updates.push(RawLevelUpdate {
            asset_id: asset(DOWN_ASSET_ID),
            side: Side::Buy,
            price: Decimal::ONE - ask_price(index),
            size: Decimal::new(4_000 + index as i64, 0),
        });
    }

    updates
}

fn apply_price_change_batch_equivalent(books: &mut OrderBooks, updates: &[RawLevelUpdate]) {
    if try_apply_two_entry_fast_path_equivalent(books, updates) {
        return;
    }

    for update in updates {
        books
            .set_level(&update.asset_id, update.side, update.price, update.size)
            .unwrap();
    }
}

fn apply_price_change_batch_sequential_fallback_equivalent(
    books: &mut OrderBooks,
    updates: &[RawLevelUpdate],
) {
    for update in updates {
        books
            .set_level(&update.asset_id, update.side, update.price, update.size)
            .unwrap();
    }
}

fn try_apply_two_entry_fast_path_equivalent(
    books: &mut OrderBooks,
    updates: &[RawLevelUpdate],
) -> bool {
    let [first, second] = updates else {
        return false;
    };

    let first_update = books
        .normalize_level(&first.asset_id, first.side, first.price, first.size)
        .unwrap();
    let second_update = books
        .normalize_level(&second.asset_id, second.side, second.price, second.size)
        .unwrap();

    if first_update != second_update {
        return false;
    }

    books
        .set_level(
            &first_update.asset_id,
            first_update.side,
            first_update.price,
            first_update.size,
        )
        .unwrap();
    true
}

fn bench_ws_price_change_normalize_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_price_change_normalize_only");

    for levels_per_side in [1usize, 8, 32] {
        let books = build_books(levels_per_side);
        let updates = build_price_change_updates(levels_per_side);

        group.throughput(Throughput::Elements(updates.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_per_side),
            &levels_per_side,
            |b, _| {
                b.iter(|| {
                    for update in black_box(updates.as_slice()) {
                        let canonical = books
                            .normalize_level(
                                &update.asset_id,
                                update.side,
                                update.price,
                                update.size,
                            )
                            .unwrap();
                        black_box(canonical);
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_ws_set_level_canonical(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_set_level_canonical");

    for levels_per_side in [1usize, 8, 32] {
        let mut books = build_books(levels_per_side);
        let updates = build_canonical_updates(levels_per_side);

        group.throughput(Throughput::Elements(updates.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_per_side),
            &levels_per_side,
            |b, _| {
                b.iter(|| {
                    for update in black_box(updates.as_slice()) {
                        books
                            .set_level(&update.asset_id, update.side, update.price, update.size)
                            .unwrap();
                    }
                    black_box(books.best_bid(&asset(UP_ASSET_ID)));
                });
            },
        );
    }

    group.finish();
}

fn bench_ws_set_level_mirrored(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_set_level_mirrored");

    for levels_per_side in [1usize, 8, 32] {
        let mut books = build_books(levels_per_side);
        let updates = build_mirrored_updates(levels_per_side);

        group.throughput(Throughput::Elements(updates.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_per_side),
            &levels_per_side,
            |b, _| {
                b.iter(|| {
                    for update in black_box(updates.as_slice()) {
                        books
                            .set_level(&update.asset_id, update.side, update.price, update.size)
                            .unwrap();
                    }
                    black_box(books.best_bid(&asset(UP_ASSET_ID)));
                });
            },
        );
    }

    group.finish();
}

fn bench_ws_price_change_apply_no_lock(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_price_change_apply_no_lock");

    for levels_per_side in [1usize, 8, 32] {
        let mut books = build_books(levels_per_side);
        let updates = build_price_change_updates(levels_per_side);

        group.throughput(Throughput::Elements(updates.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_per_side),
            &levels_per_side,
            |b, _| {
                b.iter(|| {
                    apply_price_change_batch_equivalent(&mut books, black_box(updates.as_slice()));
                    black_box(books.best_bid(&asset(UP_ASSET_ID)));
                });
            },
        );
    }

    group.finish();
}

fn bench_ws_price_change_pair_apply(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_price_change_pair_apply");
    let updates = build_mirrored_pair_updates();

    group.throughput(Throughput::Elements(updates.len() as u64));

    let mut fast_books = build_books(8);
    group.bench_function("fast_path", |b| {
        b.iter(|| {
            apply_price_change_batch_equivalent(&mut fast_books, black_box(updates.as_slice()));
            black_box(fast_books.best_bid(&asset(UP_ASSET_ID)));
        });
    });

    let mut generic_books = build_books(8);
    group.bench_function("sequential_fallback", |b| {
        b.iter(|| {
            apply_price_change_batch_sequential_fallback_equivalent(
                &mut generic_books,
                black_box(updates.as_slice()),
            );
            black_box(generic_books.best_bid(&asset(UP_ASSET_ID)));
        });
    });

    group.finish();
}

fn bench_ws_price_change_apply_with_lock(c: &mut Criterion) {
    let mut group = c.benchmark_group("ws_price_change_apply_with_lock");

    for levels_per_side in [1usize, 8, 32] {
        let books = RwLock::new(build_books(levels_per_side));
        let updates = build_price_change_updates(levels_per_side);

        group.throughput(Throughput::Elements(updates.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(levels_per_side),
            &levels_per_side,
            |b, _| {
                b.iter(|| {
                    let mut guard = books.write().unwrap();
                    apply_price_change_batch_equivalent(&mut guard, black_box(updates.as_slice()));
                    black_box(guard.best_bid(&asset(UP_ASSET_ID)));
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_ws_price_change_normalize_only,
    bench_ws_set_level_canonical,
    bench_ws_set_level_mirrored,
    bench_ws_price_change_pair_apply,
    bench_ws_price_change_apply_no_lock,
    bench_ws_price_change_apply_with_lock
);
criterion_main!(benches);
