use rclaude::{
    auth::validate_authorization_signature,
    protocol::{ErrorCode, decode_request},
};
#[test]
fn authorization_requires_a_stable_certificate_identity() {
    let certificate =
        "Identifier=io.github.typiqally.rclaude\nAuthority=rclaude Local Development\n";
    assert!(validate_authorization_signature(certificate).is_ok());
    assert_eq!(
        validate_authorization_signature(
            "Identifier=io.github.typiqally.rclaude\nSignature=adhoc\n"
        )
        .unwrap_err(),
        ErrorCode::AuthorizationSetupRequired
    );
    assert!(
        validate_authorization_signature("Identifier=another.app\nAuthority=Local signer\n")
            .is_err()
    );
}
#[test]
fn a_renderer_or_broker_frame_cannot_enable_interactive_authorization() {
    assert!(decode_request(br#"{"version":1,"id":1,"type":"authorize"}"#).is_err());
}
#[test]
fn explicit_authorization_refuses_an_unsigned_test_build_before_accessing_keychain() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_rclaude"))
        .arg("authorize")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("certificate-signed"),
        "unexpected fixed error: {stderr}"
    );
}
