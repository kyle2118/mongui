//! Offline stand-in for the Mongo worker, used only to capture screenshots of every screen
//! without a server. Compiled only with `--features demo`; release builds never contain it.
//!
//! Run: `MONGUI_DEMO=1 MONGUI_HOME=/tmp/mongui-demo cargo run --features demo`
//! Optional: `MONGUI_DEMO_OPEN=shop.orders`, `MONGUI_DEMO_MODAL=edit|delete|compass|export|remove`.

use crate::worker::{self, Cmd, CollStats, DbInfo, DbStats, Event};
use mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};
use std::{sync::mpsc, time::Duration};

pub fn enabled() -> bool {
    std::env::var_os("MONGUI_DEMO").is_some()
}

fn collections(db: &str) -> Vec<String> {
    let names: &[&str] = match db {
        "shop" => &["customers", "inventory", "orders", "returns", "shipments"],
        "notifications" => &["deliveries", "templates", "rollouts", "subscriptions"],
        "analytics" => &["events_2026_10", "events_2026_11", "sessions"],
        "audit" => &[],
        _ => &["system.version"],
    };
    names.iter().map(|s| s.to_string()).collect()
}

fn order(i: u64) -> Document {
    let statuses = ["READY", "SHIPPED", "PENDING", "CANCELLED", "DELIVERED"];
    let cities = ["Austin", "Berlin", "Toronto", "Lisbon", "Osaka", "Denver"];
    let ts = 1_763_000_000_000i64 + i as i64 * 3_600_000;
    let mut oid = [0u8; 12];
    oid[..4].copy_from_slice(&(1_763_000_000u32 + i as u32 * 97).to_be_bytes());
    oid[8..].copy_from_slice(&(0x5a3c_0000u32 + i as u32).to_be_bytes());
    doc! {
        "_id": ObjectId::from_bytes(oid),
        "orderNo": format!("ORD-{:06}", 104_220 + i),
        "status": statuses[(i % 5) as usize],
        "total": 18.5 + (i as f64 * 7.25) % 240.0,
        "items": (i % 4 + 1) as i32,
        "express": i % 3 == 0,
        "createdAt": DateTime::from_millis(ts),
        "customer": {
            "name": format!("Customer {}", 4_100 + i),
            "email": format!("customer{}@example.com", 4_100 + i),
            "address": { "city": cities[(i % 6) as usize], "zip": format!("{:05}", 10_000 + i * 37) },
        },
        "tags": ["web", if i % 2 == 0 { "promo" } else { "repeat" }],
        "coupon": Bson::Null,
    }
}

pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> (worker::Handle, mpsc::Receiver<Event>) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    let (etx, erx) = mpsc::channel::<Event>();
    std::thread::spawn(move || {
        let total: u64 = 231_847;
        while let Some(cmd) = rx.blocking_recv() {
            std::thread::sleep(Duration::from_millis(60));
            let evs = match cmd {
                Cmd::Connect(p) => {
                    if p.name.contains("PROD") {
                        std::thread::sleep(Duration::from_millis(400));
                        vec![Event::ConnectFailed(
                            "Could not reach any server within 30 seconds.\n\nmongo-prod-1.internal:27017: TLS handshake failed: \
                             invalid peer certificate: UnknownIssuer\n\nThe server's certificate is not signed by the CA you chose. \
                             Try \"cacerts / trust store\" with the JDK cacerts file."
                                .into(),
                        )]
                    } else {
                        vec![
                            Event::Connected { profile_id: p.id.clone() },
                            Event::Databases(
                                ["admin", "analytics", "audit", "notifications", "shop"]
                                    .iter()
                                    .enumerate()
                                    .map(|(i, n)| DbInfo { name: n.to_string(), size_on_disk: 40_960 * (i as u64 + 1) * 1_000 })
                                    .collect(),
                            ),
                        ]
                    }
                }
                Cmd::CancelConnect => vec![],
                Cmd::Disconnect => vec![Event::Disconnected],
                Cmd::ListDatabases => vec![],
                Cmd::ListCollections { db } => vec![Event::Collections { names: collections(&db), db }],
                Cmd::DbStats { db } => {
                    let n = collections(&db).len() as i64;
                    vec![Event::DbStats {
                        db,
                        stats: DbStats { collections: n, objects: n * 48_211, data_size: n * 61_000_000, storage_size: n * 22_000_000, indexes: n * 2, index_size: n * 4_100_000 },
                    }]
                }
                Cmd::Query { req, filter, skip, limit, .. } => {
                    if filter.contains_key("$fail") {
                        vec![Event::QueryFailed { req, error: "unknown top level operator: $fail. If you have a field name that starts with a '$' symbol, consider using $getField or $setField.".into() }]
                    } else if filter.contains_key("none") {
                        vec![Event::Docs { req, docs: vec![], elapsed: Duration::from_millis(3) }]
                    } else {
                        let docs = (skip..(skip + limit as u64).min(total)).map(order).collect();
                        vec![Event::Docs { req, docs, elapsed: Duration::from_millis(14) }]
                    }
                }
                Cmd::Count { req, filter, .. } => {
                    let total = if filter.is_empty() { total } else { 0 };
                    vec![Event::Count { req, total: Some(total), estimated: filter.is_empty() }]
                }
                Cmd::CollStats { db, coll } => vec![Event::CollStats {
                    db,
                    coll,
                    stats: Ok(CollStats { count: total as i64, size: 96_400_000, avg_obj_size: 415, storage_size: 31_800_000, index_count: 4, total_index_size: 9_300_000 }),
                }],
                Cmd::Insert { docs, .. } => vec![Event::Written(format!("{} document(s) inserted", docs.len()))],
                Cmd::Replace { .. } => vec![Event::Written("Document updated".into())],
                Cmd::Delete { .. } => vec![Event::Written("Document deleted".into())],
            };
            for ev in evs {
                let _ = etx.send(ev);
            }
            wake();
        }
    });
    (worker::Handle::from_sender(tx), erx)
}
