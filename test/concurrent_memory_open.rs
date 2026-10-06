use memory::MemoryStore;
use std::sync::{Arc, Barrier};
#[test]
fn concurrent_acp_stores_open_and_write_shared_wal_database() {
    let root = std::env::temp_dir().join(format!("ax-concurrent-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let database = root.join("memory.sqlite3");
    drop(MemoryStore::open(&database).unwrap());
    let barrier = Arc::new(Barrier::new(12));
    let threads = (0..12)
        .map(|i| {
            let path = database.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let store = MemoryStore::open(&path).unwrap();
                store
                    .create_session(format!("Concurrent SSH {i}"))
                    .unwrap()
                    .id
            })
        })
        .collect::<Vec<_>>();
    let mut sessions = std::collections::HashSet::new();
    for thread in threads {
        assert!(sessions.insert(thread.join().unwrap()));
    }
    let store = MemoryStore::open(&database).unwrap();
    for id in sessions {
        assert!(store.session(&id).unwrap().is_some());
    }
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
