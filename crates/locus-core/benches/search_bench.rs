use criterion::{criterion_group, criterion_main, BatchSize, BenchmarkId, Criterion};
use locus_core::memory::{MemoryType, NewMemory};
use locus_core::search::Query;
use locus_core::store::Store;
use uuid::Uuid;

fn build_store(size: usize) -> Store {
    let db_dir = std::env::temp_dir().join(format!("locus-bench-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&db_dir).expect("create bench dir");
    let db_path = db_dir.join("locus.db");
    let store = Store::open_at(db_path).expect("store init");

    for i in 0..size {
        let kind = if i % 2 == 0 {
            MemoryType::Code
        } else {
            MemoryType::Decision
        };

        let namespace = if i % 3 == 0 {
            "project:auth"
        } else {
            "project:billing"
        };

        store
            .insert_memory(NewMemory {
                namespace: Some(namespace.to_string()),
                memory_type: kind,
                title: format!("Auth middleware decision {i}"),
                content: format!(
                    "Use AuthService::verify_token and route handler verify_token_handler_{i}"
                ),
                entities: vec![
                    "AuthService::verify_token".to_string(),
                    format!("verify_token_handler_{i}"),
                ],
                importance: (i % 100) as u8,
                source: Some("bench".to_string()),
            })
            .expect("insert memory");
    }

    store
}

fn search_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("fts5-search");

    for size in [1_000_usize, 10_000_usize] {
        group.bench_with_input(BenchmarkId::new("exact", size), &size, |b, size| {
            let store = build_store(*size);
            b.iter_batched(
                || {
                    let mut q = Query::new("AuthService::verify_token");
                    q.namespace = Some("project:auth".to_string());
                    q.limit = 10;
                    q
                },
                |q| store.search(q).expect("search"),
                BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("prefix", size), &size, |b, size| {
            let store = build_store(*size);
            b.iter_batched(
                || {
                    let mut q = Query::new("verify*");
                    q.namespace = Some("project:auth".to_string());
                    q.limit = 10;
                    q
                },
                |q| store.search(q).expect("search"),
                BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("partial", size), &size, |b, size| {
            let store = build_store(*size);
            b.iter_batched(
                || {
                    let mut q = Query::new("fy_token_hand");
                    q.namespace = Some("project:auth".to_string());
                    q.limit = 10;
                    q
                },
                |q| store.search(q).expect("search"),
                BatchSize::SmallInput,
            );
        });

        group.bench_with_input(BenchmarkId::new("typo", size), &size, |b, size| {
            let store = build_store(*size);
            b.iter_batched(
                || {
                    let mut q = Query::new("autth middlewaer");
                    q.namespace = Some("project:auth".to_string());
                    q.limit = 10;
                    q
                },
                |q| store.search(q).expect("search"),
                BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, search_bench);
criterion_main!(benches);
