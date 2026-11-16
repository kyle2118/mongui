//! Background thread that owns the MongoDB client. The UI never blocks on the network:
//! it sends a `Cmd` and later receives an `Event`.

use crate::conn::{self, Profile};
use futures_util::TryStreamExt;
use mongodb::{
    Client,
    bson::{Bson, Document, doc},
};
use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, mpsc as tmpsc};

/// Longest a single query may run on the server before it is cancelled.
pub const QUERY_TIME_LIMIT: Duration = Duration::from_secs(30);
/// Counting a filtered result on a huge collection can be slow, so it gets a shorter leash.
pub const COUNT_TIME_LIMIT: Duration = Duration::from_secs(12);

pub enum Cmd {
    Connect(Box<Profile>),
    /// Abandon a connection attempt that is still in progress.
    CancelConnect,
    Disconnect,
    ListDatabases,
    ListCollections { db: String },
    Query { req: u64, db: String, coll: String, filter: Document, sort: Document, skip: u64, limit: i64 },
    Count { req: u64, db: String, coll: String, filter: Document },
    CollStats { db: String, coll: String },
    DbStats { db: String },
    Insert { db: String, coll: String, docs: Vec<Document> },
    Replace { db: String, coll: String, id: Bson, doc: Document },
    Delete { db: String, coll: String, id: Bson },
}

#[derive(Clone, Debug, Default)]
pub struct DbInfo {
    pub name: String,
    pub size_on_disk: u64,
}

#[derive(Clone, Debug, Default)]
pub struct CollStats {
    pub count: i64,
    pub size: i64,
    pub avg_obj_size: i64,
    pub storage_size: i64,
    pub index_count: i64,
    pub total_index_size: i64,
}

#[derive(Clone, Debug, Default)]
pub struct DbStats {
    pub collections: i64,
    pub objects: i64,
    pub data_size: i64,
    pub storage_size: i64,
    pub indexes: i64,
    pub index_size: i64,
}

pub enum Event {
    Connected { profile_id: String },
    ConnectFailed(String),
    Disconnected,
    Databases(Vec<DbInfo>),
    Collections { db: String, names: Vec<String> },
    Docs { req: u64, docs: Vec<Document>, elapsed: Duration },
    QueryFailed { req: u64, error: String },
    /// `None` means the count took too long and was abandoned.
    Count { req: u64, total: Option<u64>, estimated: bool },
    /// `Err` carries the server error, e.g. when the login lacks the collStats privilege.
    CollStats { db: String, coll: String, stats: Result<CollStats, String> },
    DbStats { db: String, stats: DbStats },
    /// A write finished; the UI should reload the current page.
    Written(String),
    Error(String),
}

#[derive(Clone)]
pub struct Handle {
    tx: tmpsc::UnboundedSender<Cmd>,
}

impl Handle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

pub fn spawn(wake: impl Fn() + Send + Sync + 'static) -> (Handle, mpsc::Receiver<Event>) {
    let (tx, mut rx) = tmpsc::unbounded_channel::<Cmd>();
    let (etx, erx) = mpsc::channel::<Event>();
    let wake = Arc::new(wake);
    std::thread::Builder::new()
        .name("mongo-worker".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
                .expect("tokio runtime");
            rt.block_on(async move {
                let client: Arc<Mutex<Option<Client>>> = Arc::new(Mutex::new(None));
                let mut connecting: Option<tokio::task::JoinHandle<()>> = None;
                while let Some(cmd) = rx.recv().await {
                    if matches!(cmd, Cmd::CancelConnect | Cmd::Connect(_)) {
                        // Dropping the attempt also removes its temporary certificate files.
                        if let Some(task) = connecting.take() {
                            task.abort();
                        }
                        if matches!(cmd, Cmd::CancelConnect) {
                            continue;
                        }
                    }
                    let is_connect = matches!(cmd, Cmd::Connect(_));
                    let client = client.clone();
                    let etx = etx.clone();
                    let wake = wake.clone();
                    // Each command runs on its own task so a slow count never delays a page load.
                    let task = tokio::spawn(async move {
                        for ev in handle(cmd, client).await {
                            let _ = etx.send(ev);
                        }
                        wake();
                    });
                    if is_connect {
                        connecting = Some(task);
                    }
                }
            });
        })
        .expect("spawn worker thread");
    (Handle { tx }, erx)
}

fn num(d: &Document, key: &str) -> i64 {
    match d.get(key) {
        Some(Bson::Int32(i)) => *i as i64,
        Some(Bson::Int64(i)) => *i,
        Some(Bson::Double(f)) => *f as i64,
        _ => 0,
    }
}

fn is_timeout(e: &mongodb::error::Error) -> bool {
    let s = e.to_string();
    s.contains("MaxTimeMSExpired") || s.contains("operation exceeded time limit") || s.contains("exceeded time limit")
}

async fn handle(cmd: Cmd, slot: Arc<Mutex<Option<Client>>>) -> Vec<Event> {
    if let Cmd::Connect(profile) = cmd {
        return match conn::connect(&profile).await {
            Ok(c) => {
                let old = slot.lock().await.replace(c.clone());
                if let Some(old) = old {
                    old.shutdown().await;
                }
                let mut evs = vec![Event::Connected { profile_id: profile.id.clone() }];
                evs.push(list_databases(&c, &profile.default_db).await);
                evs
            }
            Err(e) => vec![Event::ConnectFailed(format!("{e:#}"))],
        };
    }
    if let Cmd::Disconnect = cmd {
        if let Some(old) = slot.lock().await.take() {
            old.shutdown().await;
        }
        return vec![Event::Disconnected];
    }
    let Some(client) = slot.lock().await.clone() else {
        return vec![Event::Error("Not connected".into())];
    };
    let ev = match cmd {
        Cmd::Connect(_) | Cmd::CancelConnect | Cmd::Disconnect => unreachable!(),
        Cmd::ListDatabases => list_databases(&client, "").await,
        Cmd::ListCollections { db } => match client.database(&db).list_collection_names().await {
            Ok(mut names) => {
                names.sort_by_key(|n| n.to_lowercase());
                Event::Collections { db, names }
            }
            Err(e) => Event::Error(format!("Could not list collections in {db}: {e}")),
        },
        Cmd::Query { req, db, coll, filter, sort, skip, limit } => {
            let started = Instant::now();
            let c = client.database(&db).collection::<Document>(&coll);
            let mut find = c.find(filter).skip(skip).limit(limit).max_time(QUERY_TIME_LIMIT);
            if !sort.is_empty() {
                find = find.sort(sort);
            }
            let result = async {
                let cursor = find.await?;
                cursor.try_collect::<Vec<Document>>().await
            }
            .await;
            match result {
                Ok(docs) => Event::Docs { req, docs, elapsed: started.elapsed() },
                Err(e) if is_timeout(&e) => Event::QueryFailed {
                    req,
                    error: format!(
                        "The query was stopped after {} seconds so it would not load the server. Narrow the filter, or filter and sort on an indexed field.",
                        QUERY_TIME_LIMIT.as_secs()
                    ),
                },
                Err(e) => Event::QueryFailed { req, error: e.to_string() },
            }
        }
        Cmd::Count { req, db, coll, filter } => {
            let c = client.database(&db).collection::<Document>(&coll);
            if filter.is_empty() {
                // Instant, from collection metadata. Exact enough for paging millions of docs.
                match c.estimated_document_count().await {
                    Ok(n) => Event::Count { req, total: Some(n), estimated: true },
                    Err(_) => Event::Count { req, total: None, estimated: true },
                }
            } else {
                match c.count_documents(filter).max_time(COUNT_TIME_LIMIT).await {
                    Ok(n) => Event::Count { req, total: Some(n), estimated: false },
                    Err(_) => Event::Count { req, total: None, estimated: false },
                }
            }
        }
        Cmd::CollStats { db, coll } => {
            match client.database(&db).run_command(doc! {"collStats": &coll}).await {
                Ok(d) => Event::CollStats {
                    db,
                    coll,
                    stats: Ok(CollStats {
                        count: num(&d, "count"),
                        size: num(&d, "size"),
                        avg_obj_size: num(&d, "avgObjSize"),
                        storage_size: num(&d, "storageSize"),
                        index_count: num(&d, "nindexes"),
                        total_index_size: num(&d, "totalIndexSize"),
                    }),
                },
                Err(e) => Event::CollStats { db, coll, stats: Err(e.to_string()) },
            }
        }
        Cmd::DbStats { db } => match client.database(&db).run_command(doc! {"dbStats": 1}).await {
            Ok(d) => Event::DbStats {
                db,
                stats: DbStats {
                    collections: num(&d, "collections"),
                    objects: num(&d, "objects"),
                    data_size: num(&d, "dataSize"),
                    storage_size: num(&d, "storageSize"),
                    indexes: num(&d, "indexes"),
                    index_size: num(&d, "indexSize"),
                },
            },
            Err(_) => return vec![],
        },
        Cmd::Insert { db, coll, docs } => {
            let n = docs.len();
            match client.database(&db).collection::<Document>(&coll).insert_many(docs).await {
                Ok(_) => Event::Written(if n == 1 { "Document inserted".into() } else { format!("{n} documents inserted") }),
                Err(e) => Event::Error(format!("Insert failed: {e}")),
            }
        }
        Cmd::Replace { db, coll, id, doc } => {
            match client.database(&db).collection::<Document>(&coll).replace_one(doc! {"_id": id}, doc).await {
                Ok(r) if r.matched_count == 0 => {
                    Event::Error("Update failed: the document no longer exists (someone may have deleted it).".into())
                }
                Ok(_) => Event::Written("Document updated".into()),
                Err(e) => Event::Error(format!("Update failed: {e}")),
            }
        }
        Cmd::Delete { db, coll, id } => {
            match client.database(&db).collection::<Document>(&coll).delete_one(doc! {"_id": id}).await {
                Ok(r) if r.deleted_count == 0 => Event::Error("Delete failed: the document no longer exists.".into()),
                Ok(_) => Event::Written("Document deleted".into()),
                Err(e) => Event::Error(format!("Delete failed: {e}")),
            }
        }
    };
    vec![ev]
}

async fn list_databases(client: &Client, default_db: &str) -> Event {
    match client.list_databases().await {
        Ok(specs) => {
            let mut dbs: Vec<DbInfo> =
                specs.into_iter().map(|s| DbInfo { name: s.name, size_on_disk: s.size_on_disk }).collect();
            dbs.sort_by_key(|d| d.name.to_lowercase());
            Event::Databases(dbs)
        }
        Err(e) => {
            // Some logins may not list databases; fall back to the one they named.
            if !default_db.trim().is_empty() {
                Event::Databases(vec![DbInfo { name: default_db.trim().to_string(), size_on_disk: 0 }])
            } else {
                Event::Error(format!("Could not list databases: {e}"))
            }
        }
    }
}
