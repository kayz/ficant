mod support;

use ficant_domain::primitives::{ContentHash, Ulid};
use ficant_storage::lease_queue::{
    EnqueueTask, LeaseQueueError, LeaseTaskState, PostgresLeaseQueue,
};

const ULID_PREFIX: &str = "01ARZ3NDEKTSV4RRFFQ69G5FA";

fn id(suffix: char) -> Ulid {
    Ulid::new(format!("{ULID_PREFIX}{suffix}")).unwrap()
}

fn task(suffix: char, key: &str) -> EnqueueTask {
    EnqueueTask {
        tenant_id: id('T'),
        task_id: id(suffix),
        run_id: id('R'),
        node_id: id(suffix),
        graph_digest: ContentHash::digest(b"graph-v1"),
        execution_identity_digest: ContentHash::digest(b"execution-v1"),
        planned_artifact_id: id(suffix),
        task_key: key.to_owned(),
    }
}

async fn insert_run(pool: &sqlx::PgPool) {
    sqlx::query(
        "INSERT INTO research.experiment_runs
         (tenant_id, experiment_run_id, owner_id, state, revision,
          idempotency_key, fingerprint, payload)
         VALUES ($1, $2, $3, 'RUNNING', 2, $4, $5, $6)",
    )
    .bind(id('T').as_str())
    .bind(id('R').as_str())
    .bind(id('W').as_str())
    .bind("lease-queue-run")
    .bind(vec![7_u8; 32])
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.research_graphs
         (tenant_id,graph_id,version,owner_id,graph_digest,payload)
         VALUES ($1,$2,1,$3,$4,$5)",
    )
    .bind(id('T').as_str())
    .bind(id('Q').as_str())
    .bind(id('W').as_str())
    .bind(hash_hex(&ContentHash::digest(b"graph-v1")))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO research.execution_identities
         (tenant_id,run_id,owner_id,graph_id,graph_version,graph_digest,
          reproducibility_digest,execution_identity_digest,payload)
         VALUES ($1,$2,$3,$4,1,$5,$6,$7,$8)",
    )
    .bind(id('T').as_str())
    .bind(id('R').as_str())
    .bind(id('W').as_str())
    .bind(id('Q').as_str())
    .bind(hash_hex(&ContentHash::digest(b"graph-v1")))
    .bind(hash_hex(&ContentHash::digest(b"repro-v1")))
    .bind(hash_hex(&ContentHash::digest(b"execution-v1")))
    .bind(vec![1_u8])
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn postgres_queue_is_atomic_idempotent_and_recovers_expired_leases() {
    let pool = support::postgres_pool().await;
    support::reset_postgres(&pool).await;
    support::migrate(&pool).await;
    insert_run(&pool).await;
    let queue = PostgresLeaseQueue::new(pool.clone());

    verify_idempotent_lifecycle(&queue).await;
    verify_concurrent_claims(&queue).await;
    verify_expired_recovery(&queue, &pool).await;
}

async fn verify_idempotent_lifecycle(queue: &PostgresLeaseQueue) {
    let first_input = task('C', "run-r/node-a/attempt-1");
    let first = queue.enqueue(first_input.clone()).await.unwrap();
    assert!(first.inserted());
    assert_eq!(first.task().state(), LeaseTaskState::Pending);
    let replay = queue.enqueue(first_input.clone()).await.unwrap();
    assert!(!replay.inserted());
    assert_eq!(replay.task(), first.task());
    let mut changed = first_input;
    changed.task_id = id('D');
    assert_eq!(queue.enqueue(changed).await, Err(LeaseQueueError::Conflict));

    let worker_x = id('X');
    let lease_g = id('G');
    let claimed = queue
        .claim(&id('T'), &worker_x, &lease_g, 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.task_id(), &id('C'));
    assert_eq!(claimed.claim_count(), 1);
    assert_eq!(claimed.lease_owner(), Some(&worker_x));
    assert_eq!(
        queue
            .renew(&id('T'), &id('C'), &id('Y'), &lease_g, 60)
            .await,
        Err(LeaseQueueError::Conflict)
    );
    assert_eq!(
        queue
            .renew(&id('T'), &id('C'), &worker_x, &lease_g, 60)
            .await
            .unwrap()
            .state(),
        LeaseTaskState::Leased
    );

    let completion = ContentHash::digest(b"node-output");
    let completed = queue
        .complete(
            &id('T'),
            &id('C'),
            &worker_x,
            &lease_g,
            &completion,
            &id('C'),
        )
        .await
        .unwrap();
    assert!(completed.completed());
    assert_eq!(completed.task().state(), LeaseTaskState::Completed);
    assert_eq!(completed.task().completion_hash(), Some(&completion));
    assert!(
        !queue
            .complete(
                &id('T'),
                &id('C'),
                &worker_x,
                &lease_g,
                &completion,
                &id('C'),
            )
            .await
            .unwrap()
            .completed()
    );
    assert_eq!(
        queue
            .complete(
                &id('T'),
                &id('C'),
                &worker_x,
                &lease_g,
                &ContentHash::digest(b"changed"),
                &id('C'),
            )
            .await,
        Err(LeaseQueueError::Conflict)
    );
}

async fn verify_concurrent_claims(queue: &PostgresLeaseQueue) {
    let worker_x = id('X');
    queue.enqueue(task('D', "task-d")).await.unwrap();
    queue.enqueue(task('E', "task-e")).await.unwrap();
    let queue_clone = queue.clone();
    let tenant = id('T');
    let worker_y = id('Y');
    let lease_h = id('H');
    let lease_j = id('J');
    let (left, right) = tokio::join!(
        queue.claim(&tenant, &worker_x, &lease_h, 60),
        queue_clone.claim(&tenant, &worker_y, &lease_j, 60)
    );
    let left = left.unwrap().unwrap();
    let right = right.unwrap().unwrap();
    assert_ne!(left.task_id(), right.task_id());

    let left_hash = ContentHash::digest(b"left");
    let right_hash = ContentHash::digest(b"right");
    queue
        .complete(
            &id('T'),
            left.task_id(),
            left.lease_owner().unwrap(),
            left.lease_id().unwrap(),
            &left_hash,
            left.planned_artifact_id(),
        )
        .await
        .unwrap();
    queue
        .complete(
            &id('T'),
            right.task_id(),
            right.lease_owner().unwrap(),
            right.lease_id().unwrap(),
            &right_hash,
            right.planned_artifact_id(),
        )
        .await
        .unwrap();
}

fn hash_hex(value: &ContentHash) -> String {
    use std::fmt::Write as _;

    value
        .as_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut output, byte| {
            write!(output, "{byte:02x}").unwrap();
            output
        })
}

async fn verify_expired_recovery(queue: &PostgresLeaseQueue, pool: &sqlx::PgPool) {
    let worker_x = id('X');
    queue
        .enqueue(task('F', "recover-after-expiry"))
        .await
        .unwrap();
    let abandoned = queue
        .claim(&id('T'), &worker_x, &id('K'), 60)
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE research.execution_tasks
         SET lease_expires_at = CURRENT_TIMESTAMP - INTERVAL '1 second'
         WHERE tenant_id = $1 AND task_id = $2",
    )
    .bind(id('T').as_str())
    .bind(abandoned.task_id().as_str())
    .execute(pool)
    .await
    .unwrap();
    let recovered = queue
        .claim(&id('T'), &id('Y'), &id('M'), 60)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.task_id(), abandoned.task_id());
    assert_eq!(recovered.lease_owner(), Some(&id('Y')));
    assert_eq!(recovered.lease_id(), Some(&id('M')));
    assert_eq!(recovered.claim_count(), 2);

    assert_eq!(
        queue
            .claim(&id('A'), &worker_x, &id('P'), 60)
            .await
            .unwrap(),
        None
    );
}
