use super::*;

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    for m in crate::schema::migrations() {
        conn.execute_batch(m.up).unwrap();
    }
    conn
}

#[test]
fn enqueue_and_list() {
    let conn = setup();
    let id = enqueue_validation(&conn, Some(1), Some(1), Some(1)).unwrap();
    assert!(id > 0);
    let q = list_queue(&conn).unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!(q[0].status, "pending");
}

#[test]
fn enqueue_idempotent() {
    let conn = setup();
    let id1 = enqueue_validation(&conn, Some(1), None, None).unwrap();
    let id2 = enqueue_validation(&conn, Some(1), None, None).unwrap();
    assert_eq!(id1, id2);
}

#[test]
fn record_verdict_completes_entry() {
    let conn = setup();
    let id = enqueue_validation(&conn, Some(1), None, None).unwrap();
    record_verdict(&conn, id, "pass", Some("ok"), Some("thor")).unwrap();
    let q = list_queue(&conn).unwrap();
    assert_eq!(q[0].status, "completed");
}

#[test]
fn get_verdict_returns_latest() {
    let conn = setup();
    let id = enqueue_validation(&conn, Some(42), None, None).unwrap();
    record_verdict(&conn, id, "pass", None, None).unwrap();
    let v = get_verdict(&conn, 42).unwrap().unwrap();
    assert_eq!(v.verdict, "pass");
}

#[test]
fn get_verdict_none_for_unknown() {
    let conn = setup();
    assert!(get_verdict(&conn, 999).unwrap().is_none());
}
