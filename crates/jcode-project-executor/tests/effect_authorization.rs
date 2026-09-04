use jcode_project_executor::effect::{
    EffectAuthorizationBinding, MaterialAuthorizationClaim, WRITE_PROCESS_EFFECT_CLASS,
};

fn binding() -> EffectAuthorizationBinding {
    EffectAuthorizationBinding {
        work_id: "work-1".to_owned(),
        execution_id: "execution-1".to_owned(),
        resource_identity: "ntwrkfx/example".to_owned(),
        workspace_identity: "/work/example".to_owned(),
        candidate_revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        effect_class: WRITE_PROCESS_EFFECT_CLASS.to_owned(),
        authorization_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_owned(),
    }
}

fn claim() -> MaterialAuthorizationClaim {
    let value = binding();
    MaterialAuthorizationClaim {
        work_id: value.work_id,
        execution_id: value.execution_id,
        resource_identity: value.resource_identity,
        workspace_identity: value.workspace_identity,
        candidate_revision: value.candidate_revision,
        effect_class: value.effect_class,
        authorization_digest: value.authorization_digest,
    }
}

#[test]
fn exact_material_authorization_binding_validates() {
    binding().validate(&claim()).unwrap();
}

#[test]
fn material_authorization_rejects_wrong_scope_effect_or_digest() {
    let expected = binding();
    let mut cases = Vec::new();
    let mut c = claim();
    c.work_id = "work-2".to_owned();
    cases.push(c);
    let mut c = claim();
    c.execution_id = "execution-2".to_owned();
    cases.push(c);
    let mut c = claim();
    c.resource_identity = "ntwrkfx/other".to_owned();
    cases.push(c);
    let mut c = claim();
    c.workspace_identity = "/work/other".to_owned();
    cases.push(c);
    let mut c = claim();
    c.candidate_revision = "cccccccccccccccccccccccccccccccccccccccc".to_owned();
    cases.push(c);
    let mut c = claim();
    c.effect_class = "OTHER_EFFECT".to_owned();
    cases.push(c);
    let mut c = claim();
    c.authorization_digest =
        "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd".to_owned();
    cases.push(c);
    for candidate in cases {
        assert!(expected.validate(&candidate).is_err());
    }
}

#[test]
fn malformed_or_missing_authorization_digest_is_rejected() {
    let mut missing = claim();
    missing.authorization_digest.clear();
    assert!(binding().validate(&missing).is_err());
    let mut malformed = claim();
    malformed.authorization_digest = "not-a-sha256".to_owned();
    assert!(binding().validate(&malformed).is_err());
}
