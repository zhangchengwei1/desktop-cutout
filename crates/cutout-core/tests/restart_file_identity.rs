use cutout_core::{
    jobs::{TaskState, Workspace},
    session::SessionStore,
};
use std::{
    env, fs,
    path::Path,
    process::{Command, Output},
};

const STAGE: &str = "CLEARFRAME_RESTART_PROBE_STAGE";
const ROOT: &str = "CLEARFRAME_RESTART_PROBE_ROOT";

#[test]
fn file_identity_survives_a_process_restart_and_rejects_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/images/hard-edge-opaque.png");
    let source = root.join("source.png");
    fs::copy(&fixture, &source).unwrap();

    run_child(root, "seed");
    run_child(root, "unchanged");

    let replacement = root.join("replacement.png");
    fs::copy(&source, &replacement).unwrap();
    assert_eq!(fs::read(&source).unwrap(), fs::read(&replacement).unwrap());
    fs::remove_file(&source).unwrap();
    fs::rename(&replacement, &source).unwrap();
    run_child(root, "replaced");
}

fn run_child(root: &Path, stage: &str) {
    let output = Command::new(env::current_exe().unwrap())
        .args(["--exact", "restart_helper", "--nocapture"])
        .env(STAGE, stage)
        .env(ROOT, root)
        .output()
        .unwrap();
    assert_child_success(stage, output);
}

fn assert_child_success(stage: &str, output: Output) {
    assert!(
        output.status.success(),
        "{stage} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn restart_helper() {
    let Ok(stage) = env::var(STAGE) else {
        return;
    };
    let root = env::var(ROOT).unwrap();
    let root = Path::new(&root);
    let source = root.join("source.png");
    let store = SessionStore::new(root.join("session"));
    let workspace = Workspace::new(|_| {}).unwrap();
    match stage.as_str() {
        "seed" => {
            let batch_id = workspace.create_paused_batch(root, &[source]).unwrap();
            let item = workspace
                .batch_items(batch_id, 0, 1)
                .unwrap()
                .items
                .remove(0);
            assert_eq!(item.state, TaskState::Queued);
            workspace.checkpoint(&store).unwrap();
        }
        "unchanged" | "replaced" => {
            assert!(workspace.restore(&store).unwrap());
            let batch = workspace.snapshot().batches.remove(0);
            assert!(batch.paused);
            let item = workspace
                .batch_items(batch.batch_id, 0, 1)
                .unwrap()
                .items
                .remove(0);
            if stage == "unchanged" {
                assert_eq!(item.state, TaskState::Queued);
            } else {
                assert_eq!(item.state, TaskState::Failed);
                assert!(item.error.unwrap().contains("替换"));
            }
            assert!(!root.join(item.output_name).exists());
        }
        _ => panic!("unknown restart probe stage: {stage}"),
    }
}
