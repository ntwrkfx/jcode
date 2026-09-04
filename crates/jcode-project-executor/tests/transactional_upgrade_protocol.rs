use jcode_project_executor::{ExecutorCommand, ExecutorRequest, ResumeIntent};

#[test]
fn protocol_projects_only_prepare_inspect_and_resume() {
    let prepare: ExecutorRequest = serde_json::from_value(serde_json::json!({
        "protocol":"project-executor/v1","id":"d-prepare",
        "command":{"op":"upgrade_prepare","execution_id":"11111111-2222-4333-8444-555555555555","attempt_id":"attempt-1","expected_successor_revision":"2222222222222222222222222222222222222222","checkpoint_generation":7}
    })).unwrap();
    assert!(matches!(
        prepare.command,
        ExecutorCommand::UpgradePrepare { .. }
    ));

    let inspect: ExecutorRequest = serde_json::from_value(serde_json::json!({
        "protocol":"project-executor/v1","id":"d-inspect","command":{"op":"upgrade_inspect"}
    }))
    .unwrap();
    assert!(matches!(inspect.command, ExecutorCommand::UpgradeInspect));

    let intent = ResumeIntent::new(
        "work:d",
        "attempt-1",
        "11111111-2222-4333-8444-555555555555",
        7,
        "event-1",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "G2",
        "executor-v2",
        "2222222222222222222222222222222222222222",
        "material-v1",
    )
    .unwrap();
    let resume = ExecutorRequest {
        protocol: "project-executor/v1".into(),
        id: "d-resume".into(),
        command: ExecutorCommand::UpgradeResume {
            intent: intent.clone(),
            external_event_payload: "event-payload".into(),
        },
    };
    let roundtrip: ExecutorRequest =
        serde_json::from_value(serde_json::to_value(resume).unwrap()).unwrap();
    match roundtrip.command {
        ExecutorCommand::UpgradeResume {
            intent: observed,
            external_event_payload,
        } => {
            assert_eq!(observed, intent);
            assert_eq!(external_event_payload, "event-payload");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn protocol_does_not_expose_restart_or_deploy_operation() {
    for op in [
        "upgrade_restart",
        "upgrade_deploy",
        "upgrade_install",
        "upgrade_canary",
    ] {
        let value =
            serde_json::json!({"protocol":"project-executor/v1","id":"bad","command":{"op":op}});
        assert!(
            serde_json::from_value::<ExecutorRequest>(value).is_err(),
            "{op} must remain outside D executor transport"
        );
    }
}
