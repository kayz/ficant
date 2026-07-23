use ficant_application::ports::stable_node_artifact_id;
use ficant_domain::primitives::{ContentHash, Ulid};

fn id(value: &str) -> Ulid {
    Ulid::new(value).unwrap()
}

#[test]
fn planned_artifact_identity_is_run_independent_and_result_identity_bound() {
    let node = id("01ARZ3NDEKTSV4RRFFQ69G5F01");
    let same = ContentHash::digest(b"same-reproducibility");
    let changed = ContentHash::digest(b"changed-reproducibility");

    let first = stable_node_artifact_id(&same, &node);
    let replay = stable_node_artifact_id(&same, &node);
    let changed_identity = stable_node_artifact_id(&changed, &node);
    let changed_node = stable_node_artifact_id(&same, &id("01ARZ3NDEKTSV4RRFFQ69G5F02"));

    assert_eq!(first, replay);
    assert_ne!(first, changed_identity);
    assert_ne!(first, changed_node);
    assert_eq!(first.as_str().len(), 26);
}
