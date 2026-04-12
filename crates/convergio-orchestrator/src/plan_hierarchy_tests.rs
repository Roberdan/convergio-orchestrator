use super::*;

fn setup() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    let pool = convergio_db::pool::create_memory_pool().unwrap();
    let pc = pool.get().unwrap();
    convergio_db::migration::ensure_registry(&pc).unwrap();
    convergio_db::migration::apply_migrations(&pc, "orchestrator", &crate::schema::migrations())
        .unwrap();
    for m in crate::schema::migrations() {
        conn.execute_batch(m.up).unwrap();
    }
    conn
}

#[test]
fn dependencies_met_no_deps() {
    let conn = setup();
    conn.execute(
        "INSERT INTO plans (id, project_id, name) VALUES (1, 'p1', 'plan-a')",
        [],
    )
    .unwrap();
    assert!(dependencies_met(&conn, 1).unwrap());
}

#[test]
fn dependencies_met_blocked() {
    let conn = setup();
    conn.execute_batch(
        "INSERT INTO plans (id, project_id, name, status) VALUES (1, 'p1', 'dep', 'doing');
         INSERT INTO plans (id, project_id, name, depends_on) VALUES (2, 'p1', 'child', '1');",
    )
    .unwrap();
    assert!(!dependencies_met(&conn, 2).unwrap());
}

#[test]
fn dependencies_met_satisfied() {
    let conn = setup();
    conn.execute_batch(
        "INSERT INTO plans (id, project_id, name, status) VALUES (1, 'p1', 'dep', 'done');
         INSERT INTO plans (id, project_id, name, depends_on) VALUES (2, 'p1', 'child', '1');",
    )
    .unwrap();
    assert!(dependencies_met(&conn, 2).unwrap());
}

#[test]
fn master_rollup_empty() {
    let conn = setup();
    let (done, total, status) = master_rollup(&conn, 999).unwrap();
    assert_eq!((done, total), (0, 0));
    assert_eq!(status, "todo");
}
