use super::support::*;
use meibo_core::{Engine, OpenOptions, types::*};
use std::{
    io::Read,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

struct StopOnDrop<'a>(&'a AtomicBool);
impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn read_once(db: &Engine, deep: &ListQuery) {
    for term in ["株式", "株式会社"] {
        let page = db.search_name(term, &search_options(50)).unwrap();
        assert!(page.n.is_some());
        assert!(
            !page.rows.is_empty(),
            "search must exercise matching rows: {term}"
        );
    }
    assert_eq!(db.list(&query()).unwrap().rows.len(), 200);
    assert_eq!(db.list(deep).unwrap().rows.len(), 200);
}

fn deep_query(db: &Engine) -> ListQuery {
    let mut q = query();
    q.after = Some(
        db.cursor_after_offset(&q, 19_000)
            .unwrap()
            .expect("deep cursor"),
    );
    q
}

fn write_all(db: &mut Engine) {
    for index in 0..2_000 {
        db.record_activity(&memo(CompanyId(index + 1), 2_000_000_000 + index))
            .unwrap();
    }
}

fn check_final(db: &Engine, baseline: u64) {
    verify(db);
    assert_eq!(
        u64::try_from(activities(db, 20_000).len()).unwrap(),
        baseline + 2_000
    );
}

// REQ 5.6
#[test]
fn threads_read_while_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("threads.db");
    let baseline = seed(&path, 20_000, false);
    let stop = AtomicBool::new(false);
    let (tx, rx) = mpsc::channel();
    thread::scope(|scope| {
        let _stop = StopOnDrop(&stop);
        let mut readers = Vec::new();
        for _ in 0..2 {
            let (path, stop, tx) = (&path, &stop, tx.clone());
            readers.push(scope.spawn(move || {
                let db = Engine::open(path, &OpenOptions::default()).unwrap();
                let deep = deep_query(&db);
                let mut iterations = 0;
                loop {
                    read_once(&db, &deep);
                    iterations += 1;
                    if iterations == 1 {
                        tx.send(()).unwrap();
                    }
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                }
                iterations
            }));
        }
        drop(tx);
        for _ in 0..2 {
            rx.recv_timeout(TIMEOUT).expect("reader readiness");
        }
        let writer = scope.spawn(|| {
            let _stop = StopOnDrop(&stop);
            let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
            write_all(&mut db);
        });
        writer.join().unwrap();
        for reader in readers {
            assert!(reader.join().unwrap() >= 1);
        }
    });
    check_final(
        &Engine::open(&path, &OpenOptions::default()).unwrap(),
        baseline,
    );
}

// REQ 5.6
#[test]
fn reader_child() {
    let Some(path) = std::env::var_os(DB_ENV) else {
        return;
    };
    let db = Engine::open(Path::new(&path), &OpenOptions::default()).unwrap();
    let deep = deep_query(&db);
    let stop = Arc::new(AtomicBool::new(false));
    let input_stop = Arc::clone(&stop);
    let input = thread::spawn(move || {
        let mut buf = [0; 1];
        assert_eq!(std::io::stdin().read(&mut buf).unwrap(), 0);
        input_stop.store(true, Ordering::Release);
    });
    let mut iterations = 0;
    loop {
        read_once(&db, &deep);
        iterations += 1;
        if iterations == 1 {
            emit("READY");
        }
        if stop.load(Ordering::Acquire) {
            break;
        }
    }
    input.join().unwrap();
    emit(&format!("DONE {iterations}"));
}

// REQ 5.6
#[test]
fn process_reads_while_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("process.db");
    let baseline = seed(&path, 20_000, false);
    let mut child = ChildGuard::spawn("concurrent::reader_child", &path, &[]);
    child.receive("READY");
    let mut db = Engine::open(&path, &OpenOptions::default()).unwrap();
    write_all(&mut db);
    drop(child.child.stdin.take());
    let iterations: u64 = child.receive("DONE ").parse().unwrap();
    assert!(iterations >= 1);
    assert!(child.finish().success());
    child.drain();
    check_final(&db, baseline);
    eprintln!("CONCURRENT child_iterations={iterations}");
}
